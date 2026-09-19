// Async decoded-thumbnail textures for gallery tiles.
//
// `GtkPicture::set_filename` reads and decodes the JPEG on the GTK thread
// (`g_file_load_bytes` + `gdk_texture_new_from_bytes`), so binding a screenful
// of tiles stalls the scroll frame. This module decodes cached thumbnails on
// worker threads and hands the main thread a ready `GdkMemoryTexture`, with an
// LRU cache so rebinding the same photos is free.
//
// Tune the cache with `PIC_THUMB_TEXTURE_CACHE_MB` (default 64).

use std::cell::{Cell, RefCell};

use gtk::prelude::*;
use gtk4 as gtk;

#[derive(Clone, PartialEq, Eq, Hash)]
struct TextureKey {
    path: String,
    rotation: i32,
    recipe: String,
}

const DEFAULT_CACHE_BUDGET: usize = 64 * 1024 * 1024;
const DECODE_WORKERS: usize = 3;

struct CacheEntry {
    key: TextureKey,
    texture: gtk::gdk::Paintable,
    bytes: usize,
}

type Callback = Box<dyn FnOnce(Option<gtk::gdk::Paintable>)>;

thread_local! {
    static CACHE: RefCell<VecDeque<CacheEntry>> = const { RefCell::new(VecDeque::new()) };
    static CACHE_BYTES: Cell<usize> = const { Cell::new(0) };
    static PENDING: RefCell<HashMap<u64, Vec<Callback>>> = RefCell::new(HashMap::new());
    static TEXTURE_IN_FLIGHT: RefCell<HashMap<TextureKey, u64>> = RefCell::new(HashMap::new());
    static NEXT_REQUEST: Cell<u64> = const { Cell::new(1) };
    static BUDGET: Cell<usize> = const { Cell::new(0) };
}

fn budget() -> usize {
    BUDGET.with(|budget| {
        let value = budget.get();
        if value > 0 {
            return value;
        }
        let resolved = std::env::var("PIC_THUMB_TEXTURE_CACHE_MB")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .map(|mb| mb.saturating_mul(1024 * 1024))
            .unwrap_or(DEFAULT_CACHE_BUDGET);
        budget.set(resolved);
        resolved
    })
}

fn cache_get(key: &TextureKey) -> Option<gtk::gdk::Paintable> {
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        let position = cache.iter().position(|entry| &entry.key == key)?;
        let entry = cache.remove(position)?;
        let texture = entry.texture.clone();
        cache.push_back(entry);
        Some(texture)
    })
}

fn cache_insert(key: TextureKey, texture: gtk::gdk::Paintable, bytes: usize) {
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(position) = cache.iter().position(|entry| entry.key == key) {
            if let Some(removed) = cache.remove(position) {
                CACHE_BYTES.with(|total| total.set(total.get().saturating_sub(removed.bytes)));
            }
        }
        cache.push_back(CacheEntry { key, texture, bytes });
        CACHE_BYTES.with(|total| total.set(total.get().saturating_add(bytes)));
        let limit = budget();
        while CACHE_BYTES.with(|total| total.get()) > limit && cache.len() > 1 {
            if let Some(removed) = cache.pop_front() {
                CACHE_BYTES.with(|total| total.set(total.get().saturating_sub(removed.bytes)));
            }
        }
    });
}

/// Drop every cached texture for `path` so the next request re-decodes it.
pub fn invalidate_texture(path: &str) {
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        let mut removed = 0usize;
        cache.retain(|entry| {
            if entry.key.path == path {
                removed = removed.saturating_add(entry.bytes);
                false
            } else {
                true
            }
        });
        CACHE_BYTES.with(|total| total.set(total.get().saturating_sub(removed)));
    });
}

/// Decode `path` off the GTK thread and invoke `callback` with the texture, or
/// `None` when it cannot be read or decoded. The callback always runs on the
/// thread that called this function (the GTK main thread).
pub fn request_texture(
    path: String,
    rotation: i32,
    recipe: String,
    callback: impl FnOnce(Option<gtk::gdk::Paintable>) + 'static,
) {
    let key = TextureKey {
        path,
        rotation,
        recipe,
    };
    if let Some(texture) = cache_get(&key) {
        callback(Some(texture));
        return;
    }
    if let Some(id) = TEXTURE_IN_FLIGHT.with(|in_flight| in_flight.borrow().get(&key).copied()) {
        PENDING.with(|pending| {
            pending
                .borrow_mut()
                .entry(id)
                .or_default()
                .push(Box::new(callback));
        });
        return;
    }
    let id = NEXT_REQUEST.with(|next| {
        let id = next.get();
        next.set(id.wrapping_add(1));
        id
    });
    PENDING.with(|pending| {
        pending.borrow_mut().insert(id, vec![Box::new(callback)]);
    });
    TEXTURE_IN_FLIGHT.with(|in_flight| {
        in_flight.borrow_mut().insert(key.clone(), id);
    });
    enqueue(key, id);
}

type Job = (TextureKey, u64);
type Queue = (Mutex<VecDeque<Job>>, Condvar);
static QUEUE: OnceLock<Arc<Queue>> = OnceLock::new();

fn enqueue(key: TextureKey, id: u64) {
    let queue = QUEUE.get_or_init(|| {
        let queue = Arc::new((Mutex::new(VecDeque::<Job>::new()), Condvar::new()));
        for _ in 0..DECODE_WORKERS {
            let queue = queue.clone();
            std::thread::spawn(move || worker_loop(queue));
        }
        queue
    });
    let (jobs, wake) = &**queue;
    if let Ok(mut jobs) = jobs.lock() {
        jobs.push_back((key, id));
        wake.notify_one();
    }
}

fn worker_loop(queue: Arc<Queue>) {
    let (jobs, wake) = &*queue;
    loop {
        let job = {
            let mut jobs = match jobs.lock() {
                Ok(jobs) => jobs,
                Err(_) => return,
            };
            while jobs.is_empty() {
                jobs = match wake.wait(jobs) {
                    Ok(jobs) => jobs,
                    Err(_) => return,
                };
            }
            jobs.pop_front()
        };
        let Some((key, id)) = job else {
            continue;
        };
        let decoded = decode_texture(&key);
        glib::MainContext::default().invoke(move || deliver(id, key, decoded));
    }
}

fn decode_texture(key: &TextureKey) -> Option<(u32, u32, Vec<u8>)> {
    let bytes = std::fs::read(&key.path).ok()?;
    let decoded = decode_jpeg_turbo(&bytes).ok()?;
    let image = image::DynamicImage::ImageRgb8(decoded.image).to_rgba8();
    let rotation = key.rotation.rem_euclid(360);
    let image = if rotation == 0 {
        image
    } else {
        crate::edit::render::apply_library_rotation(image, rotation)
    };
    let recipe = crate::edit::EditRecipe::decode(&key.recipe);
    let image = if recipe.is_default() {
        image
    } else {
        crate::edit::render::apply_recipe(image, &recipe)
    };
    let (width, height) = image.dimensions();
    Some((width, height, image.into_raw()))
}

fn deliver(id: u64, key: TextureKey, decoded: Option<(u32, u32, Vec<u8>)>) {
    let texture = decoded.and_then(|(width, height, bytes)| {
        if width == 0 || height == 0 {
            return None;
        }
        let byte_len = bytes.len();
        let bytes = glib::Bytes::from_owned(bytes);
        let memory = gtk::gdk::MemoryTexture::new(
            width as i32,
            height as i32,
            gtk::gdk::MemoryFormat::R8g8b8a8,
            &bytes,
            width as usize * 4,
        );
        let paintable: gtk::gdk::Paintable = memory.upcast();
        cache_insert(key.clone(), paintable.clone(), byte_len);
        Some(paintable)
    });
    TEXTURE_IN_FLIGHT.with(|in_flight| {
        in_flight.borrow_mut().remove(&key);
    });
    let callbacks = PENDING.with(|pending| pending.borrow_mut().remove(&id));
    if let Some(callbacks) = callbacks {
        for callback in callbacks {
            callback(texture.clone());
        }
    }
}
