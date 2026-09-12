use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use chrono::{Local, TimeZone};

use gio::prelude::*;
use glib::subclass::prelude::*;
use gtk::prelude::*;
use gtk4 as gtk;

use crate::db::Photo;
use crate::photo_object::PhotoObject;

// EDIT THESE TWO VALUES to set your default thumbnail width and height.
// They are independent. Example: 180 x 120, 200 x 140, 220 x 160.
const DEFAULT_TILE_WIDTH: i32 = 180;
const DEFAULT_TILE_HEIGHT: i32 = 120;

// Existing +/- zoom remains enabled. Width changes by this step and height
// scales by the same factor, preserving the custom shape above.
const MIN_TILE_WIDTH: i32 = 100;
const MAX_TILE_WIDTH: i32 = 300;
const ZOOM_STEP_WIDTH: i32 = 24;

// Pointer travel (px) required before a folder press becomes a rubberband drag
// instead of a click. Keeps double-click detection intact.
const DRAG_CLAIM_THRESHOLD: f64 = 6.0;

const RAW_THUMBNAIL_CACHE_CAPACITY: usize = 256;
// Folder scrolling must never decode or stat thumbnails from the ListView
// bind callback. Keep a modest RAM LRU of paintables that were loaded by the
// viewport loaders so recycled rows can still show an instant thumbnail while
// the user scrolls through nearby photos. Motion prefetch fills this cache off
// the GTK thread; the ListView bind itself remains RAM-only.
const FOLDER_THUMBNAIL_CACHE_CAPACITY: usize = 512;
const FOLDER_THUMBNAIL_ASYNC_PREFETCH_MAX_IN_FLIGHT: usize = 8;
const FOLDER_THUMBNAIL_ASYNC_VISIBLE_MAX_IN_FLIGHT: usize = 16;

static FOLDER_THUMBNAIL_ASYNC_IN_FLIGHT: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

thread_local! {
    static RAW_THUMBNAIL_CACHE: RefCell<VecDeque<(String, i32, String, gtk::gdk::Paintable)>> =
        const { RefCell::new(VecDeque::new()) };
    static FOLDER_THUMBNAIL_CACHE: RefCell<VecDeque<(String, gtk::gdk::Paintable)>> =
        const { RefCell::new(VecDeque::new()) };
    static SELECTION_POSITION_CALLS: Cell<u64> = const { Cell::new(0) };
    static SCROLL_PROBE_PICK_CALLS: Cell<u64> = const { Cell::new(0) };
    static SCROLL_PROBE_PICK_NS: Cell<u128> = const { Cell::new(0) };
    static SCROLL_PROBE_SCAN_NS: Cell<u128> = const { Cell::new(0) };
    static THUMB_LOAD_COUNT: Cell<u64> = const { Cell::new(0) };
    static THUMB_LOAD_RELOADS: Cell<u64> = const { Cell::new(0) };
    static THUMB_LOAD_MAX_MS: Cell<u128> = const { Cell::new(0) };
    static THUMB_LOAD_FS_MS: Cell<u128> = const { Cell::new(0) };
    static THUMB_LOAD_APPLY_MS: Cell<u128> = const { Cell::new(0) };
    static THUMB_LOAD_SEEN: RefCell<HashSet<i64>> = RefCell::new(HashSet::new());
}

fn folder_thumbnail_cache_get(path: &str) -> Option<gtk::gdk::Paintable> {
    FOLDER_THUMBNAIL_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        let index = cache
            .iter()
            .position(|(cached_path, _)| cached_path == path)?;
        let entry = cache.remove(index)?;
        let paintable = entry.1.clone();
        cache.push_back(entry);
        Some(paintable)
    })
}

fn folder_thumbnail_cache_insert(path: String, paintable: gtk::gdk::Paintable) {
    FOLDER_THUMBNAIL_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        cache.retain(|(cached_path, _)| cached_path != &path);
        cache.push_back((path, paintable));
        while cache.len() > FOLDER_THUMBNAIL_CACHE_CAPACITY {
            cache.pop_front();
        }
    });
}

fn claim_folder_thumbnail_async(path: &str, visible_priority: bool) -> bool {
    let in_flight = FOLDER_THUMBNAIL_ASYNC_IN_FLIGHT.get_or_init(|| Mutex::new(HashSet::new()));
    let Ok(mut in_flight) = in_flight.lock() else {
        return false;
    };
    let limit = if visible_priority {
        FOLDER_THUMBNAIL_ASYNC_VISIBLE_MAX_IN_FLIGHT
    } else {
        FOLDER_THUMBNAIL_ASYNC_PREFETCH_MAX_IN_FLIGHT
    };
    if in_flight.contains(path) || in_flight.len() >= limit {
        return false;
    }
    in_flight.insert(path.to_string());
    true
}

fn release_folder_thumbnail_async(path: &str) {
    if let Some(in_flight) = FOLDER_THUMBNAIL_ASYNC_IN_FLIGHT.get() {
        if let Ok(mut in_flight) = in_flight.lock() {
            in_flight.remove(path);
        }
    }
}

enum FolderThumbnailAsyncResult {
    Loaded { width: i32, height: i32, pixels: Vec<u8> },
    Missing,
    Failed,
}

pub(crate) fn take_scroll_probe_stats() -> (u64, u128, u128) {
    SCROLL_PROBE_PICK_CALLS.with(|calls| {
        SCROLL_PROBE_PICK_NS.with(|pick| {
            SCROLL_PROBE_SCAN_NS.with(|scan| {
                let stats = (calls.get(), pick.get(), scan.get());
                calls.set(0);
                pick.set(0);
                scan.set(0);
                stats
            })
        })
    })
}

fn record_thumb_load(id: i64, total_ms: u128, fs_ms: u128, apply_ms: u128) {
    THUMB_LOAD_COUNT.with(|count| count.set(count.get().wrapping_add(1)));
    THUMB_LOAD_MAX_MS.with(|max| max.set(max.get().max(total_ms)));
    THUMB_LOAD_FS_MS.with(|sum| sum.set(sum.get().wrapping_add(fs_ms)));
    THUMB_LOAD_APPLY_MS.with(|sum| sum.set(sum.get().wrapping_add(apply_ms)));
    THUMB_LOAD_SEEN.with(|seen| {
        if !seen.borrow_mut().insert(id) {
            THUMB_LOAD_RELOADS.with(|count| count.set(count.get().wrapping_add(1)));
        }
    });
}

pub(crate) fn take_thumb_load_stats() -> (u64, u64, u128, u128, u128) {
    let loads = THUMB_LOAD_COUNT.with(|cell| cell.replace(0));
    let reloads = THUMB_LOAD_RELOADS.with(|cell| cell.replace(0));
    let max_ms = THUMB_LOAD_MAX_MS.with(|cell| cell.replace(0));
    let fs_ms = THUMB_LOAD_FS_MS.with(|cell| cell.replace(0));
    let apply_ms = THUMB_LOAD_APPLY_MS.with(|cell| cell.replace(0));
    THUMB_LOAD_SEEN.with(|seen| seen.borrow_mut().clear());
    (loads, reloads, max_ms, fs_ms, apply_ms)
}

mod square_tile {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use glib::subclass::prelude::*;
    use gtk::prelude::*;
    use gtk::subclass::prelude::*;
    use gtk4 as gtk;

    use crate::photo_object::PhotoObject;

    #[derive(Default)]
    pub struct SquareTile {
        pub width: Cell<i32>,
        pub height: Cell<i32>,
        pub favorite_indicators_visible: Cell<bool>,
        pub photo: RefCell<Option<PhotoObject>>,
        pub visual_loaded: Cell<bool>,
        // Stored so the offline badge can be created lazily on first use
        // (folder tiles no longer build it eagerly).
        pub unavailable: RefCell<Option<Rc<dyn Fn(PhotoObject, gtk::Widget)>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SquareTile {
        const NAME: &'static str = "PicasaSquareTile";
        type Type = super::SquareTile;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for SquareTile {
        fn dispose(&self) {
            self.photo.take();
            while let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for SquareTile {
        fn measure(&self, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            let requested = match orientation {
                gtk::Orientation::Horizontal => self.width.get().max(1),
                gtk::Orientation::Vertical => self.height.get().max(1),
                _ => self.width.get().max(1),
            };

            (requested, requested, -1, -1)
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            let child_width = self.width.get().min(width).max(1);
            let child_height = self.height.get().min(height).max(1);

            if let Some(child) = self.obj().first_child() {
                let x = ((width - child_width) / 2).max(0) as f32;
                let y = ((height - child_height) / 2).max(0) as f32;

                let transform =
                    gtk::gsk::Transform::new().translate(&gtk::graphene::Point::new(x, y));

                child.allocate(child_width, child_height, baseline, Some(transform));
            }
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            if let Some(child) = self.obj().first_child() {
                self.obj().snapshot_child(&child, snapshot);
            }
        }
    }
}

glib::wrapper! {
    pub struct SquareTile(ObjectSubclass<square_tile::SquareTile>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl SquareTile {
    pub(crate) fn new(width: i32, height: i32, child: &impl IsA<gtk::Widget>) -> Self {
        let tile: Self = glib::Object::new();
        tile.imp().width.set(width.max(1));
        tile.imp().height.set(height.max(1));
        tile.imp().favorite_indicators_visible.set(true);
        child.as_ref().set_parent(&tile);
        tile
    }

    fn set_tile_size(&self, width: i32, height: i32) {
        let width = width.max(1);
        let height = height.max(1);
        if self.imp().width.get() == width && self.imp().height.get() == height {
            return;
        }
        self.imp().width.set(width);
        self.imp().height.set(height);
        self.queue_resize();
    }

    /// Remember how to open the unavailable dialog so the offline badge can be
    /// created lazily instead of on every folder tile.
    fn set_unavailable_handler(&self, handler: Rc<dyn Fn(PhotoObject, gtk::Widget)>) {
        self.imp().unavailable.replace(Some(handler));
    }

    fn ensure_offline_badge(&self, frame: &gtk::Overlay) -> gtk::Button {
        if let Some(existing) = find_overlay_child(frame, "offline-badge") {
            if let Ok(button) = existing.downcast::<gtk::Button>() {
                return button;
            }
        }
        let badge = gtk::Button::with_label("!");
        badge.set_halign(gtk::Align::Start);
        badge.set_valign(gtk::Align::Start);
        badge.set_margin_top(8);
        badge.set_margin_start(8);
        badge.add_css_class("offline-badge");
        badge.set_tooltip_text(Some("Original photo unavailable"));
        badge.set_visible(false);
        frame.add_overlay(&badge);
        if let Some(handler) = self.imp().unavailable.borrow().clone() {
            let tile = self.downgrade();
            badge.connect_clicked(move |badge| {
                let Some(tile) = tile.upgrade() else {
                    return;
                };
                let Some(photo) = tile.imp().photo.borrow().as_ref().cloned() else {
                    return;
                };
                handler(photo, badge.clone().upcast());
            });
        }
        badge
    }

    fn set_favorite_indicator_visible(&self, visible: bool) {
        self.imp().favorite_indicators_visible.set(visible);
        self.refresh_favorite_indicator();
    }

    fn refresh_favorite_indicator(&self) {
        let Some(photo) = self.imp().photo.borrow().clone() else {
            return;
        };
        let Some(frame) = self.first_child().and_downcast::<gtk::Overlay>() else {
            return;
        };
        let visible = self.imp().favorite_indicators_visible.get() && photo.favorite();
        if visible {
            ensure_favorite_badge(&frame).set_visible(true);
        } else if let Some(child) = find_overlay_child(&frame, "favorite-badge") {
            child.set_visible(false);
        }
    }

    fn set_photo_deferred(&self, photo: &PhotoObject) {
        let same_photo = self
            .imp()
            .photo
            .borrow()
            .as_ref()
            .is_some_and(|current| current.id() == photo.id());
        if same_photo {
            return;
        }
        self.unload_visual();
        self.imp().photo.replace(Some(photo.clone()));
    }

    fn load_visual(&self) {
        if self.imp().visual_loaded.get() {
            return;
        }
        let trace = std::env::var_os("PICASA_TRACE").is_some();
        let load_started = trace.then(Instant::now);
        let Some(photo) = self.imp().photo.borrow().as_ref().cloned() else {
            return;
        };
        let fs_started = trace.then(Instant::now);
        // The original may live on a spun-down or disconnected drive. Stat it
        // off the GTK thread; the offline badge updates when the async probe
        // completes. Measured: a synchronous stat here froze a scroll frame for
        // 4484 ms (load_visual fs_ms=4484, scroll-baseline3.log).
        schedule_availability_probe(self, &photo);
        let mut cache_hit = false;
        let mut request_priority = false;
        if let Some(path) = photo.cached_thumbnail_path() {
            let available = std::path::Path::new(&path).is_file();
            photo.set_thumbnail_available(available);
            cache_hit = available;
            if !available {
                request_priority = true;
                if std::env::var_os("PICASA_TRACE_VERBOSE").is_some() {
                    eprintln!(
                        "THUMB PRIORITY visible_missing thread=main id={} path={} cache={}",
                        photo.id(),
                        photo.path(),
                        path
                    );
                }
            }
        }
        let fs_ms = fs_started
            .map(|started| started.elapsed().as_millis())
            .unwrap_or(0);
        let priority_started = trace.then(Instant::now);
        if request_priority {
            crate::thumbnail::request_priority(
                photo.path(),
                Some(photo.mtime()),
                Some(photo.size_bytes()),
            );
        }
        let priority_ms = priority_started
            .map(|started| started.elapsed().as_millis())
            .unwrap_or(0);
        crate::diagnostics::visible_thumbnail(photo.thumbnail_available());
        let apply_started = trace.then(Instant::now);
        self.refresh_thumbnail_with_probe(false);
        let apply_ms = apply_started
            .map(|started| started.elapsed().as_millis())
            .unwrap_or(0);
        self.imp().visual_loaded.set(true);
        if let Some(started) = load_started {
            let total_ms = started.elapsed().as_millis();
            record_thumb_load(photo.id(), total_ms, fs_ms, apply_ms);
            // Per-tile tracing can dominate the GTK main thread during a large
            // ListView jump. Keep the aggregate counters above, but emit an
            // individual line only when this bind is itself slow enough to be
            // diagnostically useful.
            if total_ms >= 12 {
                let raw =
                    crate::image_format::uses(&photo.path(), crate::image_format::DecoderKind::Raw);
                let rotation = photo.rotation().rem_euclid(360);
                let edited = !crate::edit::EditRecipe::decode(&photo.edit_recipe()).is_default();
                let apply_kind = if raw && rotation == 0 && !edited {
                    "filename"
                } else if raw {
                    "raw_transform"
                } else if rotation != 0 || edited {
                    "edited_transform"
                } else {
                    "filename"
                };
                eprintln!(
                    "UI PERF load_visual_slow id={} fs_ms={} priority_ms={} apply_ms={} total_ms={} cache={} kind={} raw={} rotation={} edited={} path={}",
                    photo.id(),
                    fs_ms,
                    priority_ms,
                    apply_ms,
                    total_ms,
                    if cache_hit { "hit" } else { "miss" },
                    apply_kind,
                    raw,
                    rotation,
                    edited,
                    photo.path()
                );
            }
        }
    }

    fn unload_visual(&self) {
        self.imp().visual_loaded.set(false);
        if let Some(frame) = self.first_child().and_downcast::<gtk::Overlay>() {
            if let Some(picture) = frame.child().and_downcast::<gtk::Picture>() {
                picture.set_paintable(gtk::gdk::Paintable::NONE);
            }
        }
        // Deliberately no per-tile trace here. GtkListView can unbind
        // thousands of tiles during a scrollbar jump; synchronous stderr
        // output would become part of the scroll workload.
    }

    fn bind_photo(&self, photo: &PhotoObject) {
        let trace = std::env::var_os("PICASA_TRACE").is_some();
        let started = trace.then(Instant::now);
        self.set_photo_deferred(photo);
        self.load_visual();
        if let Some(started) = started {
            let elapsed_ms = started.elapsed().as_millis();
            if elapsed_ms >= 12 {
                eprintln!(
                    "UI PERF grid_bind_slow id={} elapsed_ms={}",
                    photo.id(),
                    elapsed_ms
                );
            }
        }
    }

    /// Folder ListView bind must stay strictly presentation-only.
    ///
    /// GtkListView can recycle hundreds or thousands of rows during a large
    /// scrollbar jump. Do not stat files, probe originals, request thumbnail
    /// generation, or render RAW/edit transforms here. Those operations turn
    /// harmless widget recycling into synchronous main-thread work.
    fn bind_photo_folder_fast(&self, photo: &PhotoObject) {
        // Folder bind immediately paints the correct paintable, so replace the
        // photo in place instead of running the full unload path. The old path
        // cleared the paintable and toggled CSS on every recycled tile, which
        // invalidated GTK's style tree for the whole realized list on each
        // scroll frame (measured: gtk_css_node_validate_internal dominated).
        let same_photo = self
            .imp()
            .photo
            .borrow()
            .as_ref()
            .is_some_and(|current| current.id() == photo.id());
        if !same_photo {
            self.imp().photo.replace(Some(photo.clone()));
        }

        let Some(bound) = self.imp().photo.borrow().as_ref().cloned() else {
            return;
        };
        let Some(frame) = self.first_child().and_downcast::<gtk::Overlay>() else {
            return;
        };
        let Some(picture) = frame.child().and_downcast::<gtk::Picture>() else {
            return;
        };

        // RAM-only fast path. Never touch the filesystem from ListView bind,
        // but reuse a paintable that the settled-viewport loader has already
        // decoded. This keeps nearby thumbnails visible during wheel scrolling
        // without restoring the old per-bind I/O/RAW/edit work.
        let memory_hit = bound
            .cached_thumbnail_path()
            .as_deref()
            .and_then(folder_thumbnail_cache_get);
        if let Some(paintable) = memory_hit.as_ref() {
            picture.set_paintable(Some(paintable));
            if picture.has_css_class("missing-thumbnail") {
                picture.remove_css_class("missing-thumbnail");
            }
        } else {
            picture.set_paintable(gtk::gdk::Paintable::NONE);
            if !picture.has_css_class("missing-thumbnail") {
                picture.add_css_class("missing-thumbnail");
            }
        }

        let unavailable = !bound.original_available();
        if unavailable {
            let badge = self.ensure_offline_badge(&frame);
            if !badge.is_visible() {
                badge.set_visible(true);
            }
            if badge.tooltip_text().as_deref() != Some("Original photo unavailable") {
                badge.set_tooltip_text(Some("Original photo unavailable"));
            }
        } else if let Some(child) = find_overlay_child(&frame, "offline-badge") {
            if child.is_visible() {
                child.set_visible(false);
            }
        }
        let favorite = self.imp().favorite_indicators_visible.get() && bound.favorite();
        if favorite {
            ensure_favorite_badge(&frame).set_visible(true);
        } else if let Some(child) = find_overlay_child(&frame, "favorite-badge") {
            child.set_visible(false);
        }
        let edited = !crate::edit::EditRecipe::decode(&bound.edit_recipe()).is_default();
        if edited {
            let badge = ensure_edited_badge(&frame);
            badge.set_visible(true);
            if badge.tooltip_text().as_deref() != Some("Edited") {
                badge.set_tooltip_text(Some("Edited"));
            }
        } else if let Some(child) = find_overlay_child(&frame, "edited-badge") {
            child.set_visible(false);
        }
        if let Some(placeholder) = picture.next_sibling().and_downcast::<gtk::Image>() {
            let visible = memory_hit.is_none();
            if placeholder.is_visible() != visible {
                placeholder.set_visible(visible);
            }
        }
        self.imp().visual_loaded.set(memory_hit.is_some());
    }

    /// Queue a cached folder thumbnail without blocking the GTK thread.
    ///
    /// Fast Folder scrolling can recycle rows faster than the settled viewport
    /// loader runs. The bind path remains RAM-only; cache-file probing, JPEG
    /// reading, and decoding happen on a bounded worker set here. The result is
    /// applied only if this recycled tile still represents the same photo.
    fn queue_folder_cached_visual_async(&self, visible_priority: bool) -> bool {
        let Some(photo) = self.imp().photo.borrow().as_ref().cloned() else {
            return false;
        };
        if self.imp().visual_loaded.get() {
            return false;
        }
        let Some(cached_path) = photo.cached_thumbnail_path() else {
            return false;
        };

        if let Some(paintable) = folder_thumbnail_cache_get(&cached_path) {
            let Some(frame) = self.first_child().and_downcast::<gtk::Overlay>() else {
                return false;
            };
            let Some(picture) = frame.child().and_downcast::<gtk::Picture>() else {
                return false;
            };
            picture.set_paintable(Some(&paintable));
            picture.remove_css_class("missing-thumbnail");
            if let Some(placeholder) = picture.next_sibling().and_downcast::<gtk::Image>() {
                placeholder.set_visible(false);
            }
            self.imp().visual_loaded.set(true);
            return true;
        }

        if !claim_folder_thumbnail_async(&cached_path, visible_priority) {
            return false;
        }

        let photo_id = photo.id();
        let source_path = photo.path();
        let mtime = photo.mtime();
        let size_bytes = photo.size_bytes();
        let worker_path = cached_path.clone();
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let result = if !std::path::Path::new(&worker_path).is_file() {
                FolderThumbnailAsyncResult::Missing
            } else {
                match image::open(&worker_path) {
                    Ok(image) => {
                        let image = image.to_rgba8();
                        FolderThumbnailAsyncResult::Loaded {
                            width: image.width() as i32,
                            height: image.height() as i32,
                            pixels: image.into_raw(),
                        }
                    }
                    Err(_) => FolderThumbnailAsyncResult::Failed,
                }
            };
            release_folder_thumbnail_async(&worker_path);
            let _ = sender.send(result);
        });

        let tile = self.clone();
        glib::timeout_add_local(Duration::from_millis(8), move || {
            let Ok(result) = receiver.try_recv() else {
                return glib::ControlFlow::Continue;
            };

            let still_bound = tile
                .imp()
                .photo
                .borrow()
                .as_ref()
                .is_some_and(|current| current.id() == photo_id);

            match result {
                FolderThumbnailAsyncResult::Loaded { width, height, pixels } => {
                    let bytes = glib::Bytes::from_owned(pixels);
                    let texture = gtk::gdk::MemoryTexture::new(
                        width,
                        height,
                        gtk::gdk::MemoryFormat::R8g8b8a8,
                        &bytes,
                        width as usize * 4,
                    );
                    let paintable: gtk::gdk::Paintable = texture.upcast();
                    folder_thumbnail_cache_insert(cached_path.clone(), paintable.clone());
                    if still_bound {
                        if let Some(bound) = tile.imp().photo.borrow().as_ref() {
                            bound.set_thumbnail_available(true);
                        }
                        if let Some(frame) = tile.first_child().and_downcast::<gtk::Overlay>() {
                            if let Some(picture) = frame.child().and_downcast::<gtk::Picture>() {
                                picture.set_paintable(Some(&paintable));
                                picture.remove_css_class("missing-thumbnail");
                                if let Some(placeholder) =
                                    picture.next_sibling().and_downcast::<gtk::Image>()
                                {
                                    placeholder.set_visible(false);
                                }
                            }
                        }
                        tile.imp().visual_loaded.set(true);
                    }
                }
                FolderThumbnailAsyncResult::Missing => {
                    if still_bound {
                        if let Some(bound) = tile.imp().photo.borrow().as_ref() {
                            bound.set_thumbnail_available(false);
                        }
                        tile.imp().visual_loaded.set(true);
                    }
                    crate::thumbnail::request_priority(
                        source_path.clone(),
                        Some(mtime),
                        Some(size_bytes),
                    );
                }
                FolderThumbnailAsyncResult::Failed => {
                    if still_bound {
                        tile.imp().visual_loaded.set(true);
                    }
                }
            }
            glib::ControlFlow::Break
        });

        true
    }

    /// Load the final viewport thumbnail after Folder scrolling settles.
    ///
    /// Cache probing and priority generation are allowed here because this is
    /// called for only the small near-viewport set, never for every ListView
    /// bind. Pixel transforms remain forbidden on the GTK thread.
    fn load_folder_cached_visual(&self) {
        let trace = std::env::var_os("PICASA_TRACE").is_some();
        let started = trace.then(Instant::now);
        let Some(photo) = self.imp().photo.borrow().as_ref().cloned() else {
            return;
        };
        let Some(frame) = self.first_child().and_downcast::<gtk::Overlay>() else {
            return;
        };
        let Some(picture) = frame.child().and_downcast::<gtk::Picture>() else {
            return;
        };
        // Tooltip setup is deceptively expensive in GTK (it can trigger widget
        // picking and CSS work), so it is deliberately kept out of the
        // per-scroll bind path and applied once the viewport settles. Tooltips
        // are only useful when the pointer can hover, which means scrolling has
        // stopped.
        let filename = photo.filename();
        if picture.tooltip_text().as_deref() != Some(filename.as_str()) {
            picture.set_tooltip_text(Some(&filename));
        }
        if self.imp().visual_loaded.get() {
            return;
        }

        if let Some(path) = photo.cached_thumbnail_path() {
            if let Some(paintable) = folder_thumbnail_cache_get(&path) {
                picture.set_paintable(Some(&paintable));
                picture.remove_css_class("missing-thumbnail");
                if let Some(placeholder) = picture.next_sibling().and_downcast::<gtk::Image>() {
                    placeholder.set_visible(false);
                }
                self.imp().visual_loaded.set(true);
                return;
            }
        }

        schedule_availability_probe(self, &photo);
        let fs_started = trace.then(Instant::now);
        let cached = photo.cached_thumbnail_path();
        let existing = cached
            .as_deref()
            .filter(|path| std::path::Path::new(path).is_file());
        photo.set_thumbnail_available(existing.is_some());
        let fs_ms = fs_started
            .map(|value| value.elapsed().as_millis())
            .unwrap_or(0);

        if let Some(path) = existing {
            // The cached JPEG is already EXIF-oriented. Folder scrolling uses
            // the base cache immediately; RAW/edit refinements must never run
            // from ListView recycling.
            picture.set_filename(Some(path));
            picture.remove_css_class("missing-thumbnail");
            if let Some(paintable) = picture.paintable() {
                folder_thumbnail_cache_insert(path.to_string(), paintable);
            }
        } else {
            picture.set_paintable(gtk::gdk::Paintable::NONE);
            picture.add_css_class("missing-thumbnail");
            crate::thumbnail::request_priority(
                photo.path(),
                Some(photo.mtime()),
                Some(photo.size_bytes()),
            );
        }

        if let Some(placeholder) = picture.next_sibling().and_downcast::<gtk::Image>() {
            placeholder.set_visible(existing.is_none());
        }
        self.imp().visual_loaded.set(true);

        if let Some(started) = started {
            let total_ms = started.elapsed().as_millis();
            record_thumb_load(photo.id(), total_ms, fs_ms, total_ms.saturating_sub(fs_ms));
            if total_ms >= 12 {
                eprintln!(
                    "UI PERF folder_cached_load_slow id={} total_ms={} fs_ms={} cache={} path={}",
                    photo.id(),
                    total_ms,
                    fs_ms,
                    if existing.is_some() { "hit" } else { "miss" },
                    photo.path()
                );
            }
        }
    }

    fn clear_photo(&self) {
        self.unload_visual();
        self.imp().photo.take();
        if let Some(frame) = self.first_child().and_downcast::<gtk::Overlay>() {
            if let Some(picture) = frame.child().and_downcast::<gtk::Picture>() {
                picture.set_paintable(gtk::gdk::Paintable::NONE);
                picture.set_tooltip_text(None);
            }
            for class_name in ["manual-selected", "folder-photo-selected"] {
                frame.remove_css_class(class_name);
            }
            let mut child = frame.first_child();
            while let Some(current) = child {
                if let Some(image) = current.downcast_ref::<gtk::Image>() {
                    if image.has_css_class("favorite-badge")
                        || image.has_css_class("edited-badge")
                        || image.has_css_class("selection-badge")
                    {
                        image.set_visible(false);
                    }
                }
                if let Some(button) = current.downcast_ref::<gtk::Button>() {
                    if button.has_css_class("offline-badge") {
                        button.set_visible(false);
                    }
                }
                child = current.next_sibling();
            }
        }
    }

    fn set_manual_selected(&self, selected: bool) {
        let Some(frame) = self.first_child().and_downcast::<gtk::Overlay>() else {
            return;
        };
        // `clear_photo` hides the selection badge when a tile is recycled.
        // Opacity is the real on/off switch via CSS, so make the badge visible
        // again here or recycled tiles lose their indicator.
        if selected {
            let badge = ensure_selection_badge(&frame);
            if !badge.is_visible() {
                badge.set_visible(true);
            }
        }
        let has_class = frame.has_css_class("folder-photo-selected");
        if selected && !has_class {
            frame.add_css_class("folder-photo-selected");
        } else if !selected && has_class {
            frame.remove_css_class("folder-photo-selected");
        }
    }

    fn refresh_thumbnail(&self) {
        self.refresh_thumbnail_with_probe(true);
    }

    fn refresh_availability(&self) {
        let Some(photo) = self.imp().photo.borrow().clone() else {
            return;
        };
        // Read the value the async probe or the availability worker already
        // stored; never stat the original on the GTK thread here.
        let available = photo.original_available();

        if let Some(frame) = self.first_child().and_downcast::<gtk::Overlay>() {
            if !available {
                self.ensure_offline_badge(&frame).set_visible(true);
            } else if let Some(child) = find_overlay_child(&frame, "offline-badge") {
                child.set_visible(false);
            }
        }
    }

    fn refresh_thumbnail_with_probe(&self, probe_thumbnail: bool) {
        let Some(photo) = self.imp().photo.borrow().clone() else {
            return;
        };
        let Some(frame) = self.first_child().and_downcast::<gtk::Overlay>() else {
            return;
        };
        let Some(picture) = frame.child().and_downcast::<gtk::Picture>() else {
            return;
        };
        let placeholder = picture.next_sibling().and_downcast::<gtk::Image>();
        let cached = photo.cached_thumbnail_path();
        let thumbnail_available = if probe_thumbnail {
            // This is an explicit refresh (for example after thumbnail
            // generation or clearing the cache), not a per-bind probe.
            let available = cached
                .as_deref()
                .map(|path| std::path::Path::new(path).is_file())
                .unwrap_or(false);
            photo.set_thumbnail_available(available);
            available
        } else {
            photo.thumbnail_available()
        };
        let existing = cached.as_deref().filter(|_| thumbnail_available);

        if let Some(path) = existing {
            // RAW correction requires decoding and cropping pixels. Do not do
            // that synchronously while GtkGridView binds a tile; the cached
            // file is already a usable thumbnail and keeps startup/scrolling
            // responsive.
            if crate::image_format::uses(&photo.path(), crate::image_format::DecoderKind::Raw)
                && photo.rotation().rem_euclid(360) == 0
                && crate::edit::EditRecipe::decode(&photo.edit_recipe()).is_default()
            {
                picture.set_filename(Some(path));
            } else if let Some(cropped) = raw_cached_thumbnail(&photo, path) {
                picture.set_paintable(Some(&cropped));
            } else if let Some(rotated) =
                crate::photo_texture::edited_thumbnail(path, photo.rotation(), &photo.edit_recipe())
            {
                picture.set_paintable(Some(&rotated));
            } else {
                picture.set_filename(Some(path));
            }
        } else {
            picture.set_paintable(gtk::gdk::Paintable::NONE);
        }
        picture.set_tooltip_text(Some(&photo.filename()));
        let unavailable = !photo.original_available();
        if unavailable {
            let badge = self.ensure_offline_badge(&frame);
            badge.set_visible(true);
            badge.set_tooltip_text(Some("Original photo unavailable"));
        } else if let Some(child) = find_overlay_child(&frame, "offline-badge") {
            child.set_visible(false);
        }
        if self.imp().favorite_indicators_visible.get() && photo.favorite() {
            ensure_favorite_badge(&frame).set_visible(true);
        } else if let Some(child) = find_overlay_child(&frame, "favorite-badge") {
            child.set_visible(false);
        }
        let edited = !crate::edit::EditRecipe::decode(&photo.edit_recipe()).is_default();
        if edited {
            let badge = ensure_edited_badge(&frame);
            badge.set_visible(true);
            badge.set_tooltip_text(Some("Edited"));
        } else if let Some(child) = find_overlay_child(&frame, "edited-badge") {
            child.set_visible(false);
        }
        if let Some(placeholder) = placeholder {
            placeholder.set_visible(existing.is_none());
        }
        if existing.is_some() {
            picture.remove_css_class("missing-thumbnail");
        } else {
            picture.add_css_class("missing-thumbnail");
        }
    }
}

/// Probe an original file's availability without blocking the GTK main thread.
///
/// `gio` runs the stat on a worker thread and delivers the callback back to the
/// thread-default main context. The result is stored on the shared
/// `PhotoObject`, so other tiles bound to the same photo see it immediately.
fn schedule_availability_probe(tile: &SquareTile, photo: &PhotoObject) {
    // Re-probe on rebind once the previous result is older than the TTL, so a
    // drive/file that goes offline without a mount event still updates its
    // badge. The probe itself stays off the GTK thread.
    const AVAILABILITY_REPROBE_TTL: std::time::Duration = std::time::Duration::from_secs(10);
    let now = Instant::now();
    if let Some(checked_at) = photo.original_checked_at() {
        if now.duration_since(checked_at) < AVAILABILITY_REPROBE_TTL {
            return;
        }
    }
    photo.set_original_checked_at(Some(now));
    let file = crate::source::file(&photo.path());
    let photo = photo.clone();
    let tile = tile.clone();
    file.query_info_async(
        gio::FILE_ATTRIBUTE_STANDARD_TYPE,
        gio::FileQueryInfoFlags::NONE,
        glib::Priority::DEFAULT_IDLE,
        gio::Cancellable::NONE,
        move |result| {
            photo.set_original_available(result.is_ok());
            tile.refresh_availability();
        },
    );
}

fn find_overlay_child(frame: &gtk::Overlay, css_class: &str) -> Option<gtk::Widget> {
    let mut child = frame.first_child();
    while let Some(current) = child {
        if current.has_css_class(css_class) {
            return Some(current);
        }
        child = current.next_sibling();
    }
    None
}

/// Folder tiles are built without badges; almost no photo needs one, and
/// creating four extra widgets per tile was the dominant cost of rebuilding the
/// Folder rows (~250-520 ms per column change). These helpers create a badge the
/// first time it is actually needed.
fn ensure_favorite_badge(frame: &gtk::Overlay) -> gtk::Image {
    if let Some(existing) = find_overlay_child(frame, "favorite-badge") {
        if let Ok(image) = existing.downcast::<gtk::Image>() {
            return image;
        }
    }
    let badge = gtk::Image::from_icon_name("emote-love-symbolic");
    badge.set_pixel_size(18);
    badge.set_halign(gtk::Align::End);
    badge.set_valign(gtk::Align::End);
    badge.set_margin_bottom(8);
    badge.set_margin_end(8);
    badge.add_css_class("favorite-badge");
    badge.set_visible(false);
    frame.add_overlay(&badge);
    badge
}

fn ensure_edited_badge(frame: &gtk::Overlay) -> gtk::Image {
    if let Some(existing) = find_overlay_child(frame, "edited-badge") {
        if let Ok(image) = existing.downcast::<gtk::Image>() {
            return image;
        }
    }
    let badge = gtk::Image::from_icon_name("document-edit-symbolic");
    badge.set_pixel_size(18);
    badge.set_halign(gtk::Align::Start);
    badge.set_valign(gtk::Align::End);
    badge.set_margin_bottom(8);
    badge.set_margin_start(8);
    badge.add_css_class("edited-badge");
    badge.set_tooltip_text(Some("Edited"));
    badge.set_visible(false);
    frame.add_overlay(&badge);
    badge
}

fn ensure_selection_badge(frame: &gtk::Overlay) -> gtk::Image {
    if let Some(existing) = find_overlay_child(frame, "selection-badge") {
        if let Ok(image) = existing.downcast::<gtk::Image>() {
            return image;
        }
    }
    let badge = gtk::Image::from_icon_name("object-select-symbolic");
    badge.set_pixel_size(18);
    badge.set_halign(gtk::Align::End);
    badge.set_valign(gtk::Align::Start);
    badge.set_margin_top(8);
    badge.set_margin_end(8);
    badge.add_css_class("selection-badge");
    badge.set_visible(false);
    frame.add_overlay(&badge);
    badge
}

/// Nikon RAW thumbnails can contain a letterboxed embedded preview whose
/// pixel aspect ratio does not match the camera image dimensions. GTK cannot
/// crop bars that are already part of the cached pixels, so crop the small
/// cached image before handing it to the normal Cover-rendered Picture.
pub(crate) fn raw_cached_thumbnail(photo: &PhotoObject, path: &str) -> Option<gtk::gdk::Paintable> {
    let source_path = photo.path();
    if !crate::image_format::uses(&source_path, crate::image_format::DecoderKind::Raw) {
        return None;
    }

    let source_width = photo.width();
    let source_height = photo.height();
    if source_width <= 0 || source_height <= 0 {
        return None;
    }

    let rotation = photo.rotation().rem_euclid(360);
    let edit_recipe = photo.edit_recipe();
    let recipe = crate::edit::EditRecipe::decode(&edit_recipe);
    if let Some(paintable) = raw_thumbnail_cache_get(&source_path, rotation, &edit_recipe) {
        return Some(paintable);
    }

    let mut image = image::open(path).ok()?.to_rgba8();
    let image_width = image.width();
    let image_height = image.height();
    if image_width == 0 || image_height == 0 {
        return None;
    }

    // Cached thumbnails have already had EXIF orientation applied during
    // generation. Raw dimensions are reported in sensor orientation, so
    // orientations 5-8 require the target axes to be swapped as well.
    let orientation = crate::thumbnail::exif_orientation(&source_path);
    let (display_width, display_height) = if matches!(orientation, 5..=8) {
        (source_height, source_width)
    } else {
        (source_width, source_height)
    };
    let target_ratio = display_width as f64 / display_height as f64;
    let image_ratio = image_width as f64 / image_height as f64;
    if image_ratio > target_ratio {
        let crop_width =
            ((image_height as f64 * target_ratio).round() as u32).clamp(1, image_width);
        let left = (image_width - crop_width) / 2;
        image = image::imageops::crop_imm(&image, left, 0, crop_width, image_height).to_image();
    } else if image_ratio < target_ratio {
        let crop_height =
            ((image_width as f64 / target_ratio).round() as u32).clamp(1, image_height);
        let top = (image_height - crop_height) / 2;
        image = image::imageops::crop_imm(&image, 0, top, image_width, crop_height).to_image();
    }

    image = match rotation {
        90 => image::imageops::rotate90(&image),
        180 => image::imageops::rotate180(&image),
        270 => image::imageops::rotate270(&image),
        _ => image,
    };
    if !recipe.is_default() {
        image = crate::edit::render::apply_recipe(image, &recipe);
    }

    let width = image.width() as i32;
    let height = image.height() as i32;
    let bytes = glib::Bytes::from_owned(image.into_raw());
    let texture = gtk::gdk::MemoryTexture::new(
        width,
        height,
        gtk::gdk::MemoryFormat::R8g8b8a8,
        &bytes,
        width as usize * 4,
    );
    let paintable: gtk::gdk::Paintable = texture.upcast();
    raw_thumbnail_cache_insert(source_path, rotation, edit_recipe, paintable.clone());
    Some(paintable)
}

fn raw_thumbnail_cache_get(path: &str, rotation: i32, recipe: &str) -> Option<gtk::gdk::Paintable> {
    RAW_THUMBNAIL_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        let index = cache
            .iter()
            .position(|(cached_path, cached_rotation, cached_recipe, _)| {
                cached_path == path && *cached_rotation == rotation && cached_recipe == recipe
            })?;
        let entry = cache.remove(index)?;
        let paintable = entry.3.clone();
        cache.push_back(entry);
        Some(paintable)
    })
}

fn raw_thumbnail_cache_insert(
    path: String,
    rotation: i32,
    recipe: String,
    paintable: gtk::gdk::Paintable,
) {
    RAW_THUMBNAIL_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        cache.retain(|(cached_path, cached_rotation, cached_recipe, _)| {
            cached_path != &path || *cached_rotation != rotation || cached_recipe != &recipe
        });
        cache.push_back((path, rotation, recipe, paintable));
        while cache.len() > RAW_THUMBNAIL_CACHE_CAPACITY {
            cache.pop_front();
        }
    });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupMode {
    None,
    Day,
    Month,
    /// Internal grouping used by Folder mode. This is not a user-selectable
    /// Library grouping preference; it divides the continuous Picasa-style
    /// Folder stream into real, non-sticky scrolling sections.
    Folder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupDate {
    Taken,
    Added,
}

#[derive(Debug, Clone)]
struct GroupRange {
    start: usize,
    end: usize,
    label: String,
    // Folder groups also carry their id so equal basenames do not merge.
    folder_id: i64,
}

/// A fully built Folder stream kept alive between view switches.
///
/// The Folder virtual rows in `folder_store` reference photos by index and the
/// shared selection model reads them through `store`, so keeping the exact
/// PhotoObjects plus their group ranges lets re-entering Folder mode reuse an
/// already populated model instead of rebuilding ~12k rows with GTK.
#[derive(Clone)]
struct FolderStreamCache {
    photos: Vec<PhotoObject>,
    ranges: Vec<GroupRange>,
    columns: u32,
    order: Vec<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum FolderRowKind {
    #[default]
    Header,
    Photos,
}

/// One Folder model photo row is exactly one visual line. This keeps row
/// geometry stable and lets GtkListView own virtualization without a nested
/// FlowBox wrapping a variable number of internal rows.
const FOLDER_HEADER_HEIGHT: i32 = 58;

fn folder_chunk_size(columns: u32) -> usize {
    columns.max(1) as usize
}

/// Exact vertical offset for a virtual Folder row. Folder mode deliberately
/// gives every model row a fixed height, so we do not need GtkListView's
/// estimated far-row position when restoring an anchor after a column change.
fn folder_row_offset(rows: &[FolderVirtualRow], target_row: usize, tile_height: i32) -> f64 {
    rows.iter()
        .take(target_row)
        .map(|row| match row.kind {
            FolderRowKind::Header => FOLDER_HEADER_HEIGHT,
            FolderRowKind::Photos => folder_line_height(tile_height),
        } as f64)
        .sum()
}

#[derive(Clone, Default)]
pub(crate) struct FolderRowData {
    kind: FolderRowKind,
    folder_id: i64,
    folder_path: String,
    label: String,
    count: usize,
    start: usize,
    end: usize,
    // Lightweight identity for the photos represented by this visual line.
    // Range geometry alone is not enough: a refresh can replace photo ids in
    // place while leaving folder/count/start/end unchanged.
    photo_ids: Vec<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FolderVirtualRow {
    kind: FolderRowKind,
    start: usize,
    end: usize,
}

/// Folder mode uses a flat virtualized model: one lightweight header row plus
/// fixed photo lines containing at most `columns` photos. GtkListView therefore
/// virtualizes one stable-height visual line at a time.
fn folder_virtual_rows(ranges: &[GroupRange], chunk_size: usize) -> Vec<FolderVirtualRow> {
    let chunk_size = chunk_size.max(1);
    let mut rows = Vec::new();
    for range in ranges {
        rows.push(FolderVirtualRow {
            kind: FolderRowKind::Header,
            start: range.start,
            end: range.start,
        });
        let mut start = range.start;
        while start < range.end {
            let end = (start + chunk_size).min(range.end);
            rows.push(FolderVirtualRow {
                kind: FolderRowKind::Photos,
                start,
                end,
            });
            start = end;
        }
    }
    rows
}

mod folder_row_object {
    use std::cell::RefCell;

    use glib::subclass::prelude::*;

    use super::FolderRowData;

    #[derive(Default)]
    pub(crate) struct FolderRowObject {
        pub(crate) data: RefCell<FolderRowData>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for FolderRowObject {
        const NAME: &'static str = "PicasaFolderRowObject";
        type Type = super::FolderRowObject;
    }

    impl ObjectImpl for FolderRowObject {}
}

glib::wrapper! {
    pub(crate) struct FolderRowObject(ObjectSubclass<folder_row_object::FolderRowObject>);
}

impl FolderRowObject {
    fn new(data: FolderRowData) -> Self {
        let object: Self = glib::Object::new();
        object.imp().data.replace(data);
        object
    }

    fn data(&self) -> FolderRowData {
        self.imp().data.borrow().clone()
    }

    /// Cheap membership test that does not clone the row data. This runs over
    /// tens of thousands of rows when a zoom/column change re-anchors the
    /// viewport, so it must not allocate.
    fn contains_photo(&self, photo_id: i64) -> bool {
        let data = self.imp().data.borrow();
        data.kind == FolderRowKind::Photos && data.photo_ids.contains(&photo_id)
    }
}

fn make_folder_tile(
    tile_width: i32,
    tile_height: i32,
    unavailable: &Rc<dyn Fn(PhotoObject, gtk::Widget)>,
) -> SquareTile {
    let frame = gtk::Overlay::new();
    frame.set_overflow(gtk::Overflow::Hidden);
    frame.add_css_class("photo-frame");
    frame.add_css_class("photo-tile");

    let picture = gtk::Picture::new();
    picture.set_content_fit(gtk::ContentFit::Cover);
    picture.set_can_shrink(true);
    picture.set_size_request(1, 1);
    picture.set_hexpand(true);
    picture.set_vexpand(true);
    picture.set_halign(gtk::Align::Fill);
    picture.set_valign(gtk::Align::Fill);
    picture.add_css_class("thumbnail");
    frame.set_child(Some(&picture));

    let placeholder = gtk::Image::from_icon_name("image-x-generic-symbolic");
    placeholder.set_pixel_size(32);
    placeholder.add_css_class("dim-label");
    placeholder.set_visible(false);
    frame.add_overlay(&placeholder);

    // Badges (selection / favourite / edited / offline) are deliberately NOT
    // built here. GtkListView destroys and re-creates every realized Folder row
    // on a column change, so four extra widgets per tile dominated the rebuild
    // (~250-520 ms) and made the sidebar animation jerk. Each badge is created
    // lazily by ensure_*_badge() only when a photo actually needs it.
    let tile = SquareTile::new(tile_width, tile_height, &frame);
    tile.set_unavailable_handler(unavailable.clone());
    // The old FlowBoxChild supplied 6 px padding around each thumbnail. Keep
    // the same geometry directly on the tile now that the nested FlowBox is
    // gone, and let the horizontal line distribute spare width evenly.
    tile.set_hexpand(true);
    tile.set_vexpand(false);
    tile.set_valign(gtk::Align::Start);
    tile.set_margin_start(6);
    tile.set_margin_end(6);
    tile.set_margin_top(6);
    tile.set_margin_bottom(6);
    tile.set_focusable(true);

    tile
}

pub struct Gallery {
    // GtkGridView must remain the direct GtkScrolledWindow child. GTK's list
    // widgets are GtkScrollable and rely on that relationship for correct
    // visible-item allocation and virtualization. Do not wrap this GridView
    // in a Box/Viewport to implement grouping.
    pub root: gtk::GridView,
    pub folder_root: gtk::ListView,
    pub group_header: gtk::Box,
    group_title: gtk::Label,
    group_count: gtk::Label,
    folder_store: gio::ListStore,
    // Reusable Folder stream. Populated whenever the Folder rows are rebuilt
    // and restored when re-entering Folder mode so Open in Folder is instant.
    folder_cache: Rc<RefCell<Option<FolderStreamCache>>>,
    // Presentation-only Folder section order. This must never reorder the
    // shared Library photo model. Empty means use the natural range order.
    folder_order: Rc<RefCell<Vec<i64>>>,
    folder_view_changed: Rc<RefCell<Option<Rc<dyn Fn(bool)>>>>,
    selected: Rc<dyn Fn(Option<PhotoObject>)>,
    store: gio::ListStore,
    selection: gtk::MultiSelection,
    collage_selection_mode: Rc<Cell<bool>>,
    collage_selected_ids: Rc<RefCell<HashSet<i64>>>,
    current_columns: Rc<Cell<u32>>,
    last_layout_width: Rc<Cell<i32>>,
    tile_width: Rc<Cell<i32>>,
    tile_height: Rc<Cell<i32>>,
    current_photos: Rc<RefCell<Vec<PhotoObject>>>,
    replace_generation: Rc<Cell<u64>>,
    // True while a progressive gallery replacement is still building batches.
    // Folder navigation relies on this to keep retrying until the virtualized
    // Folder rows actually exist, instead of stopping as soon as the backing
    // photo store contains the target id.
    stream_building: Rc<Cell<bool>>,
    group_mode: Rc<Cell<GroupMode>>,
    group_date: Rc<Cell<GroupDate>>,
    group_ranges: Rc<RefCell<Vec<GroupRange>>>,
    last_scroll_y: Rc<Cell<f64>>,
    // Photo at the viewport top captured before a zoom resizes the tiles, so
    // the row reshape can restore the same viewport after the relayout.
    zoom_anchor: Rc<Cell<Option<i64>>>,
    folder_scroll_generation: Rc<Cell<u64>>,
    folder_anchor_photo: Cell<Option<i64>>,
    // True while a column change is still positioning the viewport. A second
    // change before it settles must reuse the original anchor instead of
    // reading GTK's transient (often zeroed) adjustment.
    folder_pending_reframe: Rc<Cell<bool>>,
    folder_reframe_photo: Rc<Cell<Option<i64>>>,
    // Coalesces rapid Ctrl+wheel zoom input: the visual reflow is deferred while
    // the user is still spinning so crossing several column boundaries triggers
    // one Folder row rebuild instead of one per notch.
    pending_zoom_width: Rc<Cell<Option<i32>>>,
    zoom_reflow_source: Rc<RefCell<Option<glib::SourceId>>>,
    on_zoom_changed: Rc<dyn Fn(i32)>,
}

impl Gallery {
    pub fn new(
        photos: &[Photo],
        initial_tile_width: i32,
        selected: impl Fn(Option<PhotoObject>) + 'static,
        activate: impl Fn(Vec<PhotoObject>, usize) + 'static,
        context_menu: impl Fn(PhotoObject, gtk::Widget, f64, f64) + 'static,
        unavailable: impl Fn(PhotoObject, gtk::Widget) + 'static,
        on_zoom_changed: impl Fn(i32) + 'static,
    ) -> Self {
        let selected: Rc<dyn Fn(Option<PhotoObject>)> = Rc::new(selected);
        let activate: Rc<dyn Fn(Vec<PhotoObject>, usize)> = Rc::new(activate);
        let context_menu: Rc<dyn Fn(PhotoObject, gtk::Widget, f64, f64)> = Rc::new(context_menu);
        let unavailable: Rc<dyn Fn(PhotoObject, gtk::Widget)> = Rc::new(unavailable);
        let on_zoom_changed: Rc<dyn Fn(i32)> = Rc::new(on_zoom_changed);
        let store = gio::ListStore::new::<PhotoObject>();
        let selection = gtk::MultiSelection::new(Some(store.clone()));
        let collage_selection_mode = Rc::new(Cell::new(false));
        let collage_selected_ids = Rc::new(RefCell::new(HashSet::new()));
        let current_columns = Rc::new(Cell::new(5u32));
        // Independent thumbnail width/height. Change DEFAULT_TILE_WIDTH and
        // DEFAULT_TILE_HEIGHT above to choose your preferred starting size.
        let tile_width = Rc::new(Cell::new(
            initial_tile_width.clamp(MIN_TILE_WIDTH, MAX_TILE_WIDTH),
        ));
        let tile_height = Rc::new(Cell::new(
            ((DEFAULT_TILE_HEIGHT as f64) * tile_width.get() as f64 / DEFAULT_TILE_WIDTH as f64)
                .round()
                .max(1.0) as i32,
        ));

        // Grouping is presented as a sticky heading outside the scrolled
        // GridView. This deliberately avoids nesting multiple GtkGridViews in
        // a GtkViewport, which broke row allocation and virtualization.
        let group_header = gtk::Box::new(gtk::Orientation::Horizontal, 7);
        group_header.set_hexpand(true);
        group_header.set_visible(false);
        group_header.set_margin_start(20);
        group_header.set_margin_end(20);
        group_header.set_margin_top(10);
        group_header.set_margin_bottom(4);
        group_header.add_css_class("group-heading-bar");

        let group_title = gtk::Label::new(None);
        group_title.set_xalign(0.0);
        group_title.add_css_class("section-heading");
        group_header.append(&group_title);

        let group_count = gtk::Label::new(None);
        group_count.set_xalign(0.0);
        group_count.add_css_class("dim-label");
        group_count.add_css_class("section-count");
        group_header.append(&group_count);

        let current_photos = Rc::new(RefCell::new(Vec::<PhotoObject>::new()));

        let factory = gtk::SignalListItemFactory::new();
        let tile_width_for_setup = tile_width.clone();
        let tile_height_for_setup = tile_height.clone();
        let unavailable_for_setup = unavailable.clone();
        factory.connect_setup(move |_, object| {
            let Some(list_item) = object.downcast_ref::<gtk::ListItem>() else {
                return;
            };

            let frame = gtk::Overlay::new();
            frame.set_overflow(gtk::Overflow::Hidden);
            frame.add_css_class("photo-frame");
            frame.add_css_class("photo-tile");

            let picture = gtk::Picture::new();
            picture.set_content_fit(gtk::ContentFit::Cover);
            picture.set_can_shrink(true);
            picture.set_size_request(1, 1);
            picture.set_hexpand(true);
            picture.set_vexpand(true);
            picture.set_halign(gtk::Align::Fill);
            picture.set_valign(gtk::Align::Fill);
            picture.add_css_class("thumbnail");
            frame.set_child(Some(&picture));

            let placeholder = gtk::Image::from_icon_name("image-x-generic-symbolic");
            placeholder.set_pixel_size(32);
            placeholder.add_css_class("dim-label");
            placeholder.set_visible(false);
            frame.add_overlay(&placeholder);

            let checkmark = gtk::Image::from_icon_name("object-select-symbolic");
            checkmark.set_pixel_size(18);
            checkmark.set_halign(gtk::Align::End);
            checkmark.set_valign(gtk::Align::Start);
            checkmark.set_margin_top(8);
            checkmark.set_margin_end(8);
            checkmark.add_css_class("selection-badge");
            frame.add_overlay(&checkmark);

            let favorite_badge = gtk::Image::from_icon_name("emote-love-symbolic");
            favorite_badge.set_pixel_size(18);
            favorite_badge.set_halign(gtk::Align::End);
            favorite_badge.set_valign(gtk::Align::End);
            favorite_badge.set_margin_bottom(8);
            favorite_badge.set_margin_end(8);
            favorite_badge.add_css_class("favorite-badge");
            favorite_badge.set_visible(false);
            frame.add_overlay(&favorite_badge);

            let edited_badge = gtk::Image::from_icon_name("document-edit-symbolic");
            edited_badge.set_pixel_size(18);
            edited_badge.set_halign(gtk::Align::Start);
            edited_badge.set_valign(gtk::Align::End);
            edited_badge.set_margin_bottom(8);
            edited_badge.set_margin_start(8);
            edited_badge.add_css_class("edited-badge");
            edited_badge.set_tooltip_text(Some("Edited"));
            edited_badge.set_visible(false);
            frame.add_overlay(&edited_badge);

            let unavailable_badge = gtk::Button::with_label("!");
            unavailable_badge.set_halign(gtk::Align::Start);
            unavailable_badge.set_valign(gtk::Align::Start);
            unavailable_badge.set_margin_top(8);
            unavailable_badge.set_margin_start(8);
            unavailable_badge.add_css_class("offline-badge");
            unavailable_badge.set_tooltip_text(Some("Original photo unavailable"));
            unavailable_badge.set_visible(false);
            frame.add_overlay(&unavailable_badge);

            let list_item_for_unavailable = list_item.clone();
            let unavailable_for_click = unavailable_for_setup.clone();
            let badge_for_click = unavailable_badge.clone();
            unavailable_badge.connect_clicked(move |_| {
                if let Some(photo) = list_item_for_unavailable
                    .item()
                    .and_downcast::<PhotoObject>()
                {
                    (unavailable_for_click)(photo, badge_for_click.clone().upcast());
                }
            });

            let tile = SquareTile::new(
                tile_width_for_setup.get(),
                tile_height_for_setup.get(),
                &frame,
            );
            tile.set_hexpand(true);
            tile.set_vexpand(false);
            tile.set_valign(gtk::Align::Start);
            list_item.set_child(Some(&tile));
        });

        factory.connect_bind(|_, object| {
            let Some(list_item) = object.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let Some(photo) = list_item.item().and_downcast::<PhotoObject>() else {
                return;
            };
            let Some(tile) = list_item.child().and_downcast::<SquareTile>() else {
                return;
            };
            tile.bind_photo(&photo);
        });

        let root = gtk::GridView::new(Some(selection.clone()), Some(factory));
        root.set_min_columns(5);
        root.set_max_columns(5);
        root.set_single_click_activate(false);
        root.set_enable_rubberband(true);
        root.set_hexpand(true);
        root.set_vexpand(true);
        root.set_halign(gtk::Align::Fill);
        root.set_valign(gtk::Align::Fill);
        root.add_css_class("section-grid");

        // GridView children are recycled and the pointer may land on any
        // descendant of a tile. Use one stable controller on GridView, pick
        // the tile under the pointer, and only claim the event after a bound
        // photo has been identified. Select an unselected clicked photo so
        // context-menu actions and focus restoration target that thumbnail.
        // Right-clicking within an existing multi-selection preserves it.
        let right_click = gtk::GestureClick::new();
        right_click.set_button(3);
        right_click.set_propagation_phase(gtk::PropagationPhase::Capture);
        let context_menu_for_grid = context_menu.clone();
        let root_for_context = root.clone();
        let selection_for_context = selection.clone();
        right_click.connect_pressed(move |gesture, _, x, y| {
            let Some(picked) = root_for_context.pick(x, y, gtk::PickFlags::DEFAULT) else {
                return;
            };
            let Some(tile) = picked
                .ancestor(SquareTile::static_type())
                .and_downcast::<SquareTile>()
            else {
                return;
            };
            let Some(photo) = tile.imp().photo.borrow().as_ref().cloned() else {
                return;
            };
            let Some(position) = (0..selection_for_context.n_items()).find(|position| {
                selection_for_context
                    .item(*position)
                    .and_downcast::<PhotoObject>()
                    .is_some_and(|item| item.id() == photo.id())
            }) else {
                return;
            };
            let Some(frame) = tile.first_child().and_downcast::<gtk::Overlay>() else {
                return;
            };

            let frame_widget = frame.clone().upcast::<gtk::Widget>();
            let local = root_for_context
                .compute_point(
                    &frame_widget,
                    &gtk::graphene::Point::new(x as f32, y as f32),
                )
                .unwrap_or_else(|| gtk::graphene::Point::new(0.0, 0.0));

            gesture.set_state(gtk::EventSequenceState::Claimed);
            if !selection_for_context.is_selected(position) {
                selection_for_context.select_item(position, true);
            }
            root_for_context.grab_focus();
            (context_menu_for_grid)(photo, frame_widget, local.x() as f64, local.y() as f64);
        });
        root.add_controller(right_click);

        // Handle Add Photos clicks on the GridView itself, before the
        // built-in GridView selection controller sees them. This makes the
        // mode behave like a checklist: each click toggles one item and does
        // not collapse the other selected photos.
        let collage_click = gtk::GestureClick::new();
        collage_click.set_button(1);
        collage_click.set_propagation_phase(gtk::PropagationPhase::Capture);
        let mode = collage_selection_mode.clone();
        let selection_for_click = selection.clone();
        let collage_selected_ids_for_click = collage_selected_ids.clone();
        let root_for_click = root.clone();
        collage_click.connect_pressed(move |gesture, _, x, y| {
            if !mode.get() {
                return;
            }
            let Some(picked) = root_for_click.pick(x, y, gtk::PickFlags::DEFAULT) else {
                return;
            };
            let Some(tile) = picked
                .ancestor(SquareTile::static_type())
                .and_downcast::<SquareTile>()
            else {
                return;
            };
            let Some(photo_id) = tile.imp().photo.borrow().as_ref().map(|photo| photo.id()) else {
                return;
            };
            let Some(position) = (0..selection_for_click.n_items()).find(|position| {
                selection_for_click
                    .item(*position)
                    .and_downcast::<PhotoObject>()
                    .is_some_and(|photo| photo.id() == photo_id)
            }) else {
                return;
            };
            gesture.set_state(gtk::EventSequenceState::Claimed);
            let mut selected_ids = collage_selected_ids_for_click.borrow_mut();
            if selected_ids.remove(&photo_id) {
                selection_for_click.unselect_item(position);
            } else {
                selected_ids.insert(photo_id);
                selection_for_click.select_item(position, false);
            }
        });
        root.add_controller(collage_click);

        let selected_for_signal = selected.clone();
        selection.connect_selection_changed(move |selection, _, _| {
            let selected = selection.selection();
            let photo = gtk::BitsetIter::init_first(&selected)
                .and_then(|(_, position)| selection.item(position))
                .and_downcast::<PhotoObject>();
            (selected_for_signal)(photo);
        });

        let current_photos_for_activate = current_photos.clone();
        let selection_for_activate = selection.clone();
        let activate_for_grid = activate.clone();
        root.connect_activate(move |_, position| {
            let Some(activated) = selection_for_activate
                .model()
                .and_then(|model| model.item(position))
                .and_downcast::<PhotoObject>()
            else {
                return;
            };
            let photos = current_photos_for_activate.borrow().clone();
            let index = photos
                .iter()
                .position(|photo| photo.id() == activated.id())
                .unwrap_or(position as usize);
            (activate_for_grid)(photos, index);
        });

        // Folder mode is a flat virtualized ListView. Each Photos model item
        // represents exactly one visual line (at most `current_columns`
        // photos). There is no nested FlowBox and no second viewport/prefetch
        // system: GtkListView owns row lifetime, while bind/unbind owns the
        // SquareTile thumbnail lifetime exactly like the working GridView.
        let folder_store = gio::ListStore::new::<FolderRowObject>();
        let folder_selection = gtk::NoSelection::new(Some(folder_store.clone()));
        let folder_factory = gtk::SignalListItemFactory::new();
        let folder_selected_ids: Rc<RefCell<HashSet<i64>>> = Rc::new(RefCell::new(HashSet::new()));

        let setup_tile_height = tile_height.clone();
        folder_factory.connect_setup(move |_, object| {
            let Some(list_item) = object.downcast_ref::<gtk::ListItem>() else {
                return;
            };

            let row_root = gtk::Box::new(gtk::Orientation::Vertical, 0);
            row_root.set_hexpand(true);
            row_root.set_vexpand(false);
            row_root.add_css_class("folder-stream-row");
            row_root.set_height_request(folder_line_height(setup_tile_height.get()));

            let header_outer = gtk::Box::new(gtk::Orientation::Vertical, 0);
            header_outer.set_widget_name("picasa-folder-section-header");
            header_outer.set_hexpand(true);
            header_outer.set_margin_top(26);
            header_outer.set_margin_bottom(8);
            header_outer.set_margin_start(6);
            header_outer.set_margin_end(6);
            header_outer.add_css_class("folder-section-header");

            let header_line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            header_line.set_hexpand(true);
            let folder_icon = gtk::Image::from_icon_name("folder-symbolic");
            folder_icon.set_pixel_size(16);
            folder_icon.add_css_class("folder-section-icon");
            header_line.append(&folder_icon);

            let title = gtk::Label::new(None);
            title.set_widget_name("picasa-folder-section-title");
            title.set_xalign(0.0);
            title.set_ellipsize(gtk::pango::EllipsizeMode::End);
            title.add_css_class("folder-section-title");
            header_line.append(&title);

            let count = gtk::Label::new(None);
            count.set_widget_name("picasa-folder-section-count");
            count.set_xalign(0.0);
            count.add_css_class("dim-label");
            count.add_css_class("folder-section-count");
            header_line.append(&count);

            let header_spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            header_spacer.set_hexpand(true);
            header_line.append(&header_spacer);

            let actions = gtk::Box::new(gtk::Orientation::Horizontal, 4);
            actions.set_widget_name("picasa-folder-section-actions");
            actions.add_css_class("folder-section-actions");
            header_line.append(&actions);

            header_outer.append(&header_line);
            let separator = gtk::Separator::new(gtk::Orientation::Horizontal);
            separator.add_css_class("folder-section-separator");
            header_outer.append(&separator);
            row_root.append(&header_outer);

            let photo_line = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            photo_line.set_widget_name("picasa-folder-photo-line");
            photo_line.set_hexpand(true);
            photo_line.set_vexpand(false);
            photo_line.set_halign(gtk::Align::Fill);
            photo_line.set_valign(gtk::Align::Start);
            photo_line.add_css_class("folder-photo-line");
            row_root.append(&photo_line);

            list_item.set_child(Some(&row_root));
        });

        let tile_width_for_folder_bind = tile_width.clone();
        let tile_height_for_folder_bind = tile_height.clone();
        let current_columns_for_folder_bind = current_columns.clone();
        let current_photos_for_folder_bind = current_photos.clone();
        let unavailable_for_folder_bind = unavailable.clone();
        let selected_ids_for_folder_bind = folder_selected_ids.clone();

        folder_factory.connect_bind(move |_, object| {
            let trace = std::env::var_os("PICASA_TRACE").is_some();
            let bind_started = trace.then(Instant::now);
            let Some(list_item) = object.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let Some(row) = list_item.item().and_downcast::<FolderRowObject>() else {
                return;
            };
            let Some(row_root) = list_item.child().and_downcast::<gtk::Box>() else {
                return;
            };
            let data = row.data();

            let row_name = format!("picasa-folder-row-{}", data.folder_id);
            if row_root.widget_name() != row_name {
                row_root.set_widget_name(&row_name);
            }
            let Some(header) = row_root.first_child().and_downcast::<gtk::Box>() else {
                return;
            };
            let Some(photo_line) = row_root.last_child().and_downcast::<gtk::Box>() else {
                return;
            };

            match data.kind {
                FolderRowKind::Header => {
                    let header_height = FOLDER_HEADER_HEIGHT;
                    if row_root.height_request() != header_height {
                        row_root.set_height_request(header_height);
                    }
                    if !header.is_visible() {
                        header.set_visible(true);
                    }
                    if photo_line.is_visible() {
                        photo_line.set_visible(false);
                    }
                    for tile in box_tiles(&photo_line) {
                        if tile.imp().photo.borrow().is_some() {
                            tile.clear_photo();
                        }
                        if tile.opacity() != 1.0 {
                            tile.set_opacity(1.0);
                        }
                        if tile.can_target() {
                            tile.set_can_target(false);
                        }
                        if tile.is_visible() {
                            tile.set_visible(false);
                        }
                    }
                    if let Some(title) = find_named_label(header.upcast_ref(), "picasa-folder-section-title") {
                        if title.text().as_str() != data.label {
                            title.set_text(&data.label);
                        }
                        if title.tooltip_text().as_deref() != Some(data.folder_path.as_str()) {
                            title.set_tooltip_text(Some(&data.folder_path));
                        }
                    }
                    if let Some(count) = find_named_label(header.upcast_ref(), "picasa-folder-section-count") {
                        let count_text = format!(
                            "{} {}",
                            format_count(data.count),
                            if data.count == 1 { "photo" } else { "photos" }
                        );
                        if count.text().as_str() != count_text {
                            count.set_text(&count_text);
                        }
                    }
                    let is_first = list_item.position() == 0;
                    if header.has_css_class("first-folder-section-header") != is_first {
                        if is_first {
                            header.add_css_class("first-folder-section-header");
                        } else {
                            header.remove_css_class("first-folder-section-header");
                        }
                    }
                    let first_margin = if is_first { 10 } else { 26 };
                    if header.margin_top() != first_margin {
                        header.set_margin_top(first_margin);
                    }
                }
                FolderRowKind::Photos => {
                    if header.is_visible() {
                        header.set_visible(false);
                    }
                    if !photo_line.is_visible() {
                        photo_line.set_visible(true);
                    }
                    let line_height = folder_line_height(tile_height_for_folder_bind.get());
                    if row_root.height_request() != line_height {
                        row_root.set_height_request(line_height);
                    }

                    let row_photos = {
                        let photos = current_photos_for_folder_bind.borrow();
                        photos
                            .get(data.start..data.end)
                            .map(|slice| slice.to_vec())
                            .unwrap_or_default()
                    };
                    let slot_count = current_columns_for_folder_bind.get().max(1) as usize;
                    let mut tiles = box_tiles(&photo_line);
                    while tiles.len() < slot_count {
                        let tile = make_folder_tile(
                            tile_width_for_folder_bind.get(),
                            tile_height_for_folder_bind.get(),
                            &unavailable_for_folder_bind,
                        );
                        photo_line.append(&tile);
                        tiles.push(tile);
                    }

                    for (slot, tile) in tiles.iter().enumerate() {
                        if slot >= slot_count {
                            if tile.imp().photo.borrow().is_some() {
                                tile.clear_photo();
                            }
                            if tile.opacity() != 1.0 {
                                tile.set_opacity(1.0);
                            }
                            if tile.can_target() {
                                tile.set_can_target(false);
                            }
                            if tile.is_visible() {
                                tile.set_visible(false);
                            }
                            continue;
                        }

                        if !tile.is_visible() {
                            tile.set_visible(true);
                        }
                        tile.set_tile_size(
                            tile_width_for_folder_bind.get(),
                            tile_height_for_folder_bind.get(),
                        );
                        if let Some(photo) = row_photos.get(slot) {
                            if tile.opacity() != 1.0 {
                                tile.set_opacity(1.0);
                            }
                            if !tile.can_target() {
                                tile.set_can_target(true);
                            }
                            tile.bind_photo_folder_fast(photo);
                            tile.set_manual_selected(
                                selected_ids_for_folder_bind.borrow().contains(&photo.id()),
                            );
                        } else {
                            // Hidden widgets do not participate in GtkBox layout.
                            // Keep an allocated transparent slot so a short final
                            // line preserves the exact same column geometry.
                            if tile.imp().photo.borrow().is_some() {
                                tile.clear_photo();
                            }
                            if tile.opacity() != 0.0 {
                                tile.set_opacity(0.0);
                            }
                            if tile.can_target() {
                                tile.set_can_target(false);
                            }
                        }
                    }
                }
            }

            if trace {
                let elapsed_ms = bind_started
                    .map(|started| started.elapsed().as_millis())
                    .unwrap_or(0);
                if elapsed_ms >= 12 {
                    eprintln!(
                        "UI PERF folder_line_bind_slow row={} kind={:?} folder_id={} photos={} elapsed_ms={}",
                        list_item.position(),
                        data.kind,
                        data.folder_id,
                        data.end.saturating_sub(data.start),
                        elapsed_ms
                    );
                }
            }
        });

        folder_factory.connect_unbind(move |_, object| {
            let Some(list_item) = object.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let Some(row_root) = list_item.child().and_downcast::<gtk::Box>() else {
                return;
            };
            let Some(photo_line) = row_root.last_child().and_downcast::<gtk::Box>() else {
                return;
            };
            for tile in box_tiles(&photo_line) {
                tile.clear_photo();
                tile.set_opacity(1.0);
                tile.set_can_target(false);
                tile.set_visible(false);
            }
        });

        let folder_root = gtk::ListView::new(Some(folder_selection.clone()), Some(folder_factory));
        folder_root.set_single_click_activate(false);
        folder_root.set_show_separators(false);
        folder_root.set_hexpand(true);
        folder_root.set_vexpand(true);
        folder_root.set_halign(gtk::Align::Fill);
        folder_root.set_valign(gtk::Align::Fill);
        folder_root.add_css_class("folder-stream");
        folder_root.add_css_class("photo-grid");

        install_folder_root_input(
            &folder_root,
            &selection,
            &current_photos,
            &activate,
            &context_menu,
            &collage_selection_mode,
            &collage_selected_ids,
        );

        let folder_root_for_selection = folder_root.clone();
        let selection_for_folder_style = selection.clone();
        let selected_ids_for_selection = folder_selected_ids.clone();
        let styles_refresh_scheduled = Rc::new(Cell::new(false));
        selection.connect_selection_changed(move |selection, _, _| {
            selected_ids_for_selection.replace(selected_photo_id_set(selection));
            if styles_refresh_scheduled.replace(true) {
                return;
            }
            let root = folder_root_for_selection.clone();
            let selection = selection_for_folder_style.clone();
            let scheduled = styles_refresh_scheduled.clone();
            glib::idle_add_local_once(move || {
                scheduled.set(false);
                refresh_folder_selection_styles(&root, &selection);
            });
        });

        let gallery = Self {
            root,
            folder_root,
            group_header,
            group_title,
            group_count,
            folder_store,
            folder_cache: Rc::new(RefCell::new(None)),
            folder_order: Rc::new(RefCell::new(Vec::new())),
            folder_view_changed: Rc::new(RefCell::new(None)),
            selected,
            store,
            selection,
            collage_selection_mode,
            collage_selected_ids,
            current_columns,
            last_layout_width: Rc::new(Cell::new(0)),
            tile_width,
            tile_height,
            current_photos,
            replace_generation: Rc::new(Cell::new(0)),
            stream_building: Rc::new(Cell::new(false)),
            group_mode: Rc::new(Cell::new(GroupMode::None)),
            group_date: Rc::new(Cell::new(GroupDate::Taken)),
            group_ranges: Rc::new(RefCell::new(Vec::new())),
            last_scroll_y: Rc::new(Cell::new(0.0)),
            zoom_anchor: Rc::new(Cell::new(None)),
            folder_scroll_generation: Rc::new(Cell::new(0)),
            folder_anchor_photo: Cell::new(None),
            folder_pending_reframe: Rc::new(Cell::new(false)),
            folder_reframe_photo: Rc::new(Cell::new(None)),
            pending_zoom_width: Rc::new(Cell::new(None)),
            zoom_reflow_source: Rc::new(RefCell::new(None)),
            on_zoom_changed,
        };
        gallery.replace(photos);
        gallery
    }

    /// Column count a content width produces for the current tile size.
    fn columns_for_width(&self, width: i32) -> u32 {
        let available = (width - 48).max(200);
        ((available as f64) / (self.tile_width.get() as f64 + 30.0))
            .floor()
            .clamp(1.0, 8.0) as u32
    }

    pub fn update_width(&self, width: i32) {
        self.update_layout(width, false);
    }

    fn update_layout(&self, width: i32, tile_size_changed: bool) {
        let trace = std::env::var_os("PICASA_TRACE").is_some();
        let started = trace.then(Instant::now);
        let old_columns = self.current_columns.get();
        let old_width = self.last_layout_width.get();
        let columns = self.columns_for_width(width);
        if width == old_width && columns == old_columns && !tile_size_changed {
            return;
        }
        if trace {
            eprintln!(
                "UI PERF update_width_begin mode={:?} width={} old_layout_width={} tile={}x{} old_columns={} new_columns={} photos={} folder_ranges={} folder_rows={}",
                self.group_mode.get(),
                width,
                old_width,
                self.tile_width.get(),
                self.tile_height.get(),
                old_columns,
                columns,
                self.current_photos.borrow().len(),
                self.group_ranges.borrow().len(),
                self.folder_store.n_items()
            );
        }
        self.last_layout_width.set(width);
        let folder_mode = self.group_mode.get() == GroupMode::Folder;
        if columns == old_columns {
            // Zooming within the same column count only changes tile geometry.
            // Replacing the Folder ListStore here used to invalidate every
            // realized row and cost ~0.8-1.1s for a 4.5k-photo library.
            if folder_mode && tile_size_changed {
                // Tile size changed within the same columns: the rows keep
                // their photos but their heights change, so re-anchor the
                // viewport to the photo that was at the top.
                let anchor = self.take_reframe_anchor();
                update_folder_realized_rows(
                    self.folder_root.upcast_ref(),
                    self.tile_width.get(),
                    self.tile_height.get(),
                );
                self.folder_root.queue_resize();
                if let Some(anchor) = anchor {
                    self.scroll_folder_to_photo(anchor);
                }
            } else if !folder_mode {
                self.root.queue_resize();
                self.update_group_header_for_scroll(self.last_scroll_y.get());
            }
            // Width-only changes with the same columns need no vertical layout
            // work. GTK stretches the row boxes itself. Re-anchoring here made
            // every sidebar animation frame issue competing scroll requests.
            if trace {
                eprintln!(
                    "UI PERF update_width_skip_reflow mode={:?} width={} columns={} reason=columns_unchanged scroll_y={:.0}",
                    self.group_mode.get(),
                    width,
                    columns,
                    self.folder_root.vadjustment().map(|a| a.value()).unwrap_or(0.0)
                );
            }
            return;
        }

        self.current_columns.set(columns);
        self.root.set_min_columns(columns);
        self.root.set_max_columns(columns);
        self.root.queue_resize();
        if folder_mode {
            // Each model item is one visual photo line. A column change must
            // rebuild those lines to keep the layout gapless.
            let anchor = self.take_reframe_anchor();
            let before = self
                .folder_root
                .vadjustment()
                .map(|adjustment| adjustment.value())
                .unwrap_or(0.0);
            self.rebuild_folder_rows();
            if let Some(anchor) = anchor {
                self.scroll_folder_to_photo(anchor);
            }
            if trace {
                eprintln!(
                    "UI PERF folder_zoom_anchor id={:?} before={:.0}",
                    anchor, before
                );
            }
        } else {
            self.update_group_header_for_scroll(self.last_scroll_y.get());
        }
        if trace {
            eprintln!(
                "UI PERF update_width_end mode={:?} width={} columns={} rebuild_ms=0 total_ms={} folder_rows={} strategy={}",
                self.group_mode.get(),
                width,
                columns,
                started.map(|value| value.elapsed().as_millis()).unwrap_or(0),
                self.folder_store.n_items(),
                if folder_mode { "virtual_chunks_rebuilt" } else { "grid_columns" }
            );
        }
    }

    /// Photo that must stay at the top across a reshape. While a previous
    /// reframe is still settling, keep its photo: the viewport is mid-flight
    /// and no longer describes the user's position. Otherwise capture the
    /// current photo (or a zoom's saved anchor) and mark the reframe pending.
    fn take_reframe_anchor(&self) -> Option<i64> {
        if self.folder_pending_reframe.get() {
            return self.folder_reframe_photo.get();
        }
        let captured = self.zoom_anchor.take().or_else(|| {
            self.photo_for_scroll_position(self.last_scroll_y.get())
                .map(|photo| photo.id())
        });
        if let Some(photo_id) = captured {
            self.folder_reframe_photo.set(Some(photo_id));
            self.folder_pending_reframe.set(true);
        }
        captured
    }

    /// Keep the Folder viewport on `photo_id` after the rows were reshaped by a
    /// zoom/column change. Realizes the target row, then aligns its tile to the
    /// top edge using the tile's computed bounds (independent of the variable
    /// row heights). Waits for allocation and bounds the settling corrections.
    fn scroll_folder_to_photo(&self, photo_id: i64) {
        self.folder_anchor_photo.set(Some(photo_id));
        let Some(row) = self.folder_row_index_for_photo(photo_id) else {
            return;
        };
        let exact_offset = {
            let ranges = self.group_ranges.borrow();
            let rows = folder_virtual_rows(&ranges, folder_chunk_size(self.current_columns.get()));
            folder_row_offset(&rows, row as usize, self.tile_height.get())
        };
        let root = self.folder_root.clone();
        let generation = self.folder_scroll_generation.clone();
        let request = generation.get().wrapping_add(1);
        generation.set(request);
        let model_generation = self.replace_generation.clone();
        let model_request = model_generation.get();
        let mode = self.group_mode.clone();
        let pending = self.folder_pending_reframe.clone();
        let reframe_photo = self.folder_reframe_photo.clone();
        let trace = std::env::var_os("PICASA_TRACE").is_some();
        glib::idle_add_local_once(move || {
            if generation.get() != request
                || model_generation.get() != model_request
                || mode.get() != GroupMode::Folder
            {
                return;
            }
            // Every Folder row has a fixed model height. Position the viewport
            // directly instead of asking GtkListView to estimate the offset of
            // a far, unrealized row. The old scroll_to() path visibly landed
            // about a row away first and then snapped into place on later
            // allocation frames.
            if let Some(adjustment) = root.vadjustment() {
                let upper = (adjustment.upper() - adjustment.page_size()).max(adjustment.lower());
                adjustment.set_value(exact_offset.clamp(adjustment.lower(), upper));
                if trace {
                    eprintln!(
                        "UI PERF folder_zoom_anchor_exact row={row} target={:.0} after={:.0}",
                        exact_offset,
                        adjustment.value()
                    );
                }

                // Exact virtual-row positioning succeeded. Do not run the old
                // bounds-based settle loop afterwards: GTK can report transient
                // tile bounds for a few frames after a rebuild, which causes the
                // small visible "rebound" even though the exact anchor is already
                // correct.
                pending.set(false);
                reframe_photo.set(None);
                return;
            }
            // Exact positioning was unavailable. Fall back to the older
            // allocation/bounds correction path below.
            // An idle after another idle does NOT guarantee a GTK allocation.
            // Wait for a frame to lay out the rebuilt/rebound rows before using
            // their bounds; otherwise the old coordinates move us to another row.
            let frames = Cell::new(0);
            let stable_frames = Cell::new(0);
            root.add_tick_callback(move |root_for_align, _| {
                if generation.get() != request
                    || model_generation.get() != model_request
                    || mode.get() != GroupMode::Folder
                {
                    return glib::ControlFlow::Break;
                }
                frames.set(frames.get() + 1);
                if frames.get() < 2 {
                    return glib::ControlFlow::Continue;
                }
                let mut tiles = Vec::new();
                collect_tiles(root_for_align.upcast_ref(), &mut tiles);
                let tile = tiles.into_iter().find(|tile| {
                    tile.imp()
                        .photo
                        .borrow()
                        .as_ref()
                        .is_some_and(|photo| photo.id() == photo_id)
                });
                let Some(tile) = tile else {
                    pending.set(false);
                    return glib::ControlFlow::Break;
                };
                let Some(bounds) = tile.compute_bounds(root_for_align) else {
                    pending.set(false);
                    return glib::ControlFlow::Break;
                };
                let Some(adjustment) = root_for_align.vadjustment() else {
                    pending.set(false);
                    return glib::ControlFlow::Break;
                };
                // Align the row, not the tile's six-pixel top margin. GTK may
                // refine its estimated row heights for another frame after a
                // splice; allow a bounded correction until the anchor settles.
                let target = adjustment.value() + bounds.y() as f64 - tile.margin_top() as f64;
                let upper = (adjustment.upper() - adjustment.page_size()).max(adjustment.lower());
                let target = target.clamp(adjustment.lower(), upper);
                if (target - adjustment.value()).abs() <= 1.0 {
                    stable_frames.set(stable_frames.get() + 1);
                } else {
                    stable_frames.set(0);
                    adjustment.set_value(target);
                }
                if trace {
                    eprintln!(
                        "UI PERF folder_zoom_anchor row={row} bounds_y={:.0} after={:.0}",
                        bounds.y(),
                        adjustment.value()
                    );
                }
                if stable_frames.get() >= 2 || frames.get() >= 8 {
                    // Selecting a fresh anchor is allowed again once this
                    // reshape has been positioned.
                    pending.set(false);
                    reframe_photo.set(None);
                    glib::ControlFlow::Break
                } else {
                    glib::ControlFlow::Continue
                }
            });
        });
    }

    pub fn set_grouping(&self, mode: GroupMode, date: GroupDate) {
        let old_mode = self.group_mode.replace(mode);
        let old_date = self.group_date.replace(date);
        let changed = old_mode != mode || old_date != date;
        if changed || mode != GroupMode::None {
            self.rebuild_group_ranges();
        }

        if mode == GroupMode::Folder {
            // Folder headers live inside the scrolling ListView. The old
            // external heading would be sticky, which is deliberately not
            // Picasa-style.
            self.group_header.set_visible(false);
            self.group_title.set_text("");
            self.group_count.set_text("");
            if old_mode != GroupMode::Folder && !self.restore_folder_cache() {
                // The current model may still contain All Photos/Favourites in
                // a global date order. Do not briefly render that as hundreds
                // of false folder sections while the correctly ordered Folder
                // stream is loading. The subsequent photo replacement builds
                // the Folder rows once; reapplying Folder grouping must not
                // build the same store a second time.
                self.folder_store.remove_all();
            }
        } else {
            let visible = mode != GroupMode::None && !self.group_ranges.borrow().is_empty();
            self.group_header.set_visible(visible);
            if visible {
                self.update_group_header_for_scroll(self.last_scroll_y.get());
            } else {
                self.group_title.set_text("");
                self.group_count.set_text("");
            }
        }

        if let Some(handler) = self.folder_view_changed.borrow().as_ref() {
            handler(mode == GroupMode::Folder);
        }
    }

    pub fn set_folder_view_changed_handler(&self, handler: impl Fn(bool) + 'static) {
        let handler: Rc<dyn Fn(bool)> = Rc::new(handler);
        handler(self.group_mode.get() == GroupMode::Folder);
        self.folder_view_changed.replace(Some(handler));
    }

    pub fn update_group_header_for_scroll(&self, scroll_y: f64) {
        self.last_scroll_y.set(scroll_y.max(0.0));
        let mode = self.group_mode.get();
        if mode == GroupMode::Folder {
            return;
        }
        if mode == GroupMode::None {
            return;
        }

        self.update_group_header_for_index(self.index_for_scroll_position(scroll_y));
    }

    /// Return the photo represented by the leading visible content. Folder
    /// mode reads the virtualized ListView row at the viewport edge; other
    /// modes keep the existing GridView geometry.
    pub fn photo_for_scroll_position(&self, scroll_y: f64) -> Option<PhotoObject> {
        if self.group_mode.get() == GroupMode::Folder {
            return self.photo_for_visible_folder_row();
        }
        self.current_photos
            .borrow()
            .get(self.index_for_scroll_position(scroll_y))
            .cloned()
    }

    fn index_for_scroll_position(&self, scroll_y: f64) -> usize {
        // Grid item padding is 6px on each edge in window.rs. The top grid
        // margin is 20px. Keep this calculation shared with the sticky heading.
        const ITEM_PADDING: f64 = 6.0;
        const GRID_TOP_MARGIN: f64 = 20.0;
        let tile_height = self.tile_height.get().max(1) as f64;
        let row_pitch = tile_height + ITEM_PADDING * 2.0;
        let row = ((scroll_y - GRID_TOP_MARGIN).max(0.0) / row_pitch).floor() as usize;
        row.saturating_mul(self.current_columns.get().max(1) as usize)
    }

    fn rebuild_group_ranges(&self) {
        let mode = self.group_mode.get();
        let date = self.group_date.get();
        let photos = self.current_photos.borrow();

        let mut ranges = Vec::<GroupRange>::new();
        if mode != GroupMode::None {
            for (index, photo) in photos.iter().enumerate() {
                let label = group_label(photo, mode, date);
                let folder_id = if mode == GroupMode::Folder {
                    photo.folder_id()
                } else {
                    0
                };
                match ranges.last_mut() {
                    Some(last) if last.label == label && last.folder_id == folder_id => {
                        last.end = index + 1
                    }
                    _ => ranges.push(GroupRange {
                        start: index,
                        end: index + 1,
                        label,
                        folder_id,
                    }),
                }
            }
        }
        self.group_ranges.replace(ranges);
    }

    fn rebuild_folder_rows(&self) {
        rebuild_folder_rows_for(
            &self.current_photos,
            &self.group_ranges,
            &self.current_columns,
            &self.folder_order,
            &self.folder_store,
        );
        self.save_folder_cache();
    }

    /// Remember the current Folder stream so re-entering Folder mode can reuse
    /// the already built PhotoObjects and virtual rows.
    fn save_folder_cache(&self) {
        if self.group_mode.get() != GroupMode::Folder {
            return;
        }
        save_folder_cache_for(
            &self.folder_cache,
            &self.current_photos,
            &self.group_ranges,
            &self.current_columns,
            &self.folder_order,
        );
    }

    /// Restore a previously built Folder stream when re-entering Folder mode.
    ///
    /// Returns true when the cached rows can be reused as-is (tile geometry and
    /// section order unchanged), in which case `folder_store` must not be
    /// cleared. The Folder rows reference the restored PhotoObjects by index and
    /// `selection` reads them through `store`, so both must be replaced.
    fn restore_folder_cache(&self) -> bool {
        let Some(cache) = self.folder_cache.borrow().clone() else {
            return false;
        };
        if cache.columns != self.current_columns.get() || cache.order != *self.folder_order.borrow()
        {
            return false;
        }
        self.current_photos.replace(cache.photos.clone());
        self.group_ranges.replace(cache.ranges.clone());
        self.store.splice(0, self.store.n_items(), &cache.photos);
        if std::env::var_os("PICASA_TRACE").is_some() {
            eprintln!(
                "UI PERF folder_cache_restore photos={} rows={}",
                cache.photos.len(),
                self.folder_store.n_items()
            );
        }
        true
    }

    /// Reorder only the Folder presentation to match a new sidebar tree mode.
    /// The shared Library photo model and GtkMultiSelection stay untouched;
    /// Folder rows retain their source positions and are merely presented in a
    /// different section order.
    pub fn reorder_folder_stream(&self, folder_order: &[i64]) {
        self.folder_order.replace(folder_order.to_vec());
        if self.group_mode.get() == GroupMode::Folder {
            self.rebuild_folder_rows();
        }
    }

    /// Folder id at the leading visible row. This is intentionally cheaper
    /// than `photo_for_visible_folder_row`: passive sidebar follow needs only
    /// the row's folder identity, not a PhotoObject or a group-range scan.
    pub fn visible_folder_id(&self) -> Option<i64> {
        if self.group_mode.get() != GroupMode::Folder {
            return None;
        }
        let trace = std::env::var_os("PICASA_TRACE").is_some();
        let width = self.folder_root.width().max(1) as f64;
        for y in [4.0_f64, 20.0, 40.0, 64.0] {
            let pick_started = trace.then(Instant::now);
            let picked = self
                .folder_root
                .pick(width * 0.5, y, gtk::PickFlags::DEFAULT);
            if let Some(started) = pick_started {
                SCROLL_PROBE_PICK_CALLS.with(|calls| calls.set(calls.get().wrapping_add(1)));
                SCROLL_PROBE_PICK_NS
                    .with(|ns| ns.set(ns.get().wrapping_add(started.elapsed().as_nanos())));
            }
            if let Some(folder_id) = picked
                .as_ref()
                .and_then(folder_id_from_named_ancestor)
                .filter(|folder_id| *folder_id > 0)
            {
                return Some(folder_id);
            }
        }
        None
    }

    fn photo_for_visible_folder_row(&self) -> Option<PhotoObject> {
        // A hit-test at the viewport centre can land between columns (especially
        // with an even column count). Falling back to the folder's first photo
        // then jumps thousands of rows. Use actual visible tile bounds instead.
        let mut tiles = Vec::new();
        collect_tiles(self.folder_root.upcast_ref(), &mut tiles);
        let height = self.folder_root.height() as f32;
        let preferred = self.folder_anchor_photo.get();
        tiles
            .into_iter()
            .filter_map(|tile| {
                if !tile.is_mapped() || !tile.is_visible() {
                    return None;
                }
                let photo = tile.imp().photo.borrow().clone()?;
                let bounds = tile.compute_bounds(&self.folder_root)?;
                if bounds.y() + bounds.height() <= 0.0 || bounds.y() >= height {
                    return None;
                }
                Some((bounds.y(), bounds.x(), photo))
            })
            // Keep the same logical photo while it remains in the leading row.
            // Choosing the leftmost slot anew on every 6↔7-column transition
            // rounds backwards each time and gradually scrolls up the folder.
            .min_by(|a, b| {
                a.0.total_cmp(&b.0)
                    .then_with(|| (Some(b.2.id()) == preferred).cmp(&(Some(a.2.id()) == preferred)))
                    .then_with(|| a.1.total_cmp(&b.1))
            })
            .map(|(_, _, photo)| photo)
    }

    fn update_group_header_for_index(&self, index: usize) {
        let ranges = self.group_ranges.borrow();
        let Some(range) = ranges
            .iter()
            .find(|range| index >= range.start && index < range.end)
            .or_else(|| ranges.last())
        else {
            self.group_header.set_visible(false);
            return;
        };

        self.group_header.set_visible(true);
        self.group_title.set_text(&range.label);
        let count = range.end.saturating_sub(range.start);
        self.group_count.set_text(&format!(
            "•  {} {}",
            format_count(count),
            if count == 1 { "photo" } else { "photos" }
        ));
    }

    pub fn zoom_in(self: &Rc<Self>) {
        let base = self
            .pending_zoom_width
            .get()
            .unwrap_or_else(|| self.tile_width.get());
        self.request_zoom(base + ZOOM_STEP_WIDTH);
    }

    pub fn zoom_out(self: &Rc<Self>) {
        let base = self
            .pending_zoom_width
            .get()
            .unwrap_or_else(|| self.tile_width.get());
        self.request_zoom(base - ZOOM_STEP_WIDTH);
    }

    /// Record a zoom request. Isolated clicks apply immediately; a rapid
    /// Ctrl+wheel spin coalesces its extra notches into one trailing reflow so
    /// crossing several column boundaries does not rebuild the Folder rows per
    /// notch.
    pub fn request_zoom(self: &Rc<Self>, width: i32) {
        let width = width.clamp(MIN_TILE_WIDTH, MAX_TILE_WIDTH);
        let base = self
            .pending_zoom_width
            .get()
            .unwrap_or_else(|| self.tile_width.get());
        if width == base {
            return;
        }
        // A burst is already in progress if a trailing source exists.
        let leading = self.zoom_reflow_source.borrow().is_none();
        self.pending_zoom_width.set(Some(width));
        if leading {
            let this = self.clone();
            glib::idle_add_local_once(move || {
                if let Some(width) = this.pending_zoom_width.take() {
                    this.apply_zoom(width);
                }
            });
        }
        if let Some(source) = self.zoom_reflow_source.borrow_mut().take() {
            source.remove();
        }
        let this = self.clone();
        let source = glib::timeout_add_local(std::time::Duration::from_millis(150), move || {
            this.zoom_reflow_source.borrow_mut().take();
            if let Some(width) = this.pending_zoom_width.take() {
                this.apply_zoom(width);
            }
            glib::ControlFlow::Break
        });
        self.zoom_reflow_source.replace(Some(source));
    }

    /// Zoom is driven by width. Height scales by the same factor, preserving
    /// the custom width/height shape configured above.
    fn apply_zoom(&self, width: i32) {
        let trace = std::env::var_os("PICASA_TRACE").is_some();
        let zoom_started = trace.then(Instant::now);
        let old_width = self.tile_width.get().max(1);
        let old_height = self.tile_height.get().max(1);
        let width = width.clamp(MIN_TILE_WIDTH, MAX_TILE_WIDTH);
        if width == old_width {
            return;
        }

        // Capture the visible photo before the tile resize disturbs the layout.
        if self.group_mode.get() == GroupMode::Folder {
            self.zoom_anchor.set(
                self.photo_for_scroll_position(self.last_scroll_y.get())
                    .map(|photo| photo.id()),
            );
        } else {
            self.zoom_anchor.set(None);
        }

        let scale = width as f64 / old_width as f64;
        let height = ((old_height as f64) * scale).round().max(1.0) as i32;
        if trace {
            eprintln!(
                "UI PERF folder_zoom_begin mode={:?} old_tile={}x{} new_tile={}x{} photos={} folders={} columns={} folder_rows={}",
                self.group_mode.get(),
                old_width,
                old_height,
                width,
                height,
                self.current_photos.borrow().len(),
                self.group_ranges.borrow().len(),
                self.current_columns.get(),
                self.folder_store.n_items()
            );
        }

        self.tile_width.set(width);
        self.tile_height.set(height);
        let callback_started = trace.then(Instant::now);
        (self.on_zoom_changed)(width);
        if trace {
            eprintln!(
                "UI PERF folder_zoom_setting_callback ms={}",
                callback_started
                    .map(|value| value.elapsed().as_millis())
                    .unwrap_or(0)
            );
        }

        let collect_started = trace.then(Instant::now);
        let mut tiles = Vec::new();
        collect_tiles(self.root.upcast_ref(), &mut tiles);
        let grid_tiles = tiles.len();
        collect_tiles(self.folder_root.upcast_ref(), &mut tiles);
        let total_tiles = tiles.len();
        if trace {
            eprintln!(
                "UI PERF folder_zoom_collect_tiles grid={} folder={} total={} ms={}",
                grid_tiles,
                total_tiles.saturating_sub(grid_tiles),
                total_tiles,
                collect_started
                    .map(|value| value.elapsed().as_millis())
                    .unwrap_or(0)
            );
        }
        let resize_started = trace.then(Instant::now);
        for tile in tiles {
            tile.set_tile_size(width, height);
        }
        if trace {
            eprintln!(
                "UI PERF folder_zoom_resize_tiles count={} ms={}",
                total_tiles,
                resize_started
                    .map(|value| value.elapsed().as_millis())
                    .unwrap_or(0)
            );
        }

        let root_width = if self.group_mode.get() == GroupMode::Folder {
            self.folder_root.width()
        } else {
            self.root.width()
        };
        let layout_started = trace.then(Instant::now);
        if root_width > 100 {
            self.update_layout(root_width, true);
        } else {
            self.update_group_header_for_scroll(self.last_scroll_y.get());
        }
        self.zoom_anchor.set(None);
        if trace {
            eprintln!(
                "UI PERF folder_zoom_end root_width={} columns={} folder_rows={} layout_ms={} total_ms={}",
                root_width,
                self.current_columns.get(),
                self.folder_store.n_items(),
                layout_started.map(|value| value.elapsed().as_millis()).unwrap_or(0),
                zoom_started.map(|value| value.elapsed().as_millis()).unwrap_or(0)
            );
        }
    }

    /// After Folder scrolling settles, load only cached thumbnails for tiles
    /// close to the actual viewport. GtkListView keeps a much larger recycled
    /// widget pool than the visible rows, so loading every bound tile causes
    /// thousands of unnecessary thumbnail operations during scrollbar jumps.
    pub fn refresh_visible_folder_tiles(&self) {
        if self.group_mode.get() != GroupMode::Folder || self.folder_root.height() <= 0 {
            return;
        }

        let trace = std::env::var_os("PICASA_TRACE").is_some();
        let started = trace.then(Instant::now);
        let viewport = self.folder_root.height() as f32;
        let mut tiles = Vec::new();
        collect_tiles(self.folder_root.upcast_ref(), &mut tiles);
        let realized = tiles.len();
        let mut near = 0usize;
        let mut loaded = 0usize;

        for tile in tiles {
            if !tile.is_mapped() || tile.height() <= 0 {
                tile.unload_visual();
                continue;
            }
            let is_near = tile
                .compute_bounds(&self.folder_root)
                .is_some_and(|bounds| {
                    bounds.y() + bounds.height() >= -viewport * 0.25
                        && bounds.y() <= viewport * 1.25
                });
            if is_near {
                near += 1;
                if !tile.imp().visual_loaded.get() {
                    loaded += 1;
                }
                tile.load_folder_cached_visual();
            } else {
                tile.unload_visual();
            }
        }

        if let Some(started) = started {
            let elapsed_ms = started.elapsed().as_millis();
            if elapsed_ms >= 16 {
                eprintln!(
                    "UI PERF folder_visible_refresh_slow realized={} near={} loaded={} elapsed_ms={}",
                    realized, near, loaded, elapsed_ms
                );
            }
        }
    }

    /// Queue cached thumbnails for the tiles that are actually visible now.
    ///
    /// This is the high-priority fast-scroll path. It may use the reserved
    /// visible-thumbnail worker capacity even when background prefetch already
    /// occupies its smaller quota, so a scrollbar jump cannot be blocked by
    /// thumbnails for rows the user has already passed.
    pub fn queue_visible_folder_cached_tiles_async(&self, budget: usize) -> usize {
        if budget == 0
            || self.group_mode.get() != GroupMode::Folder
            || self.folder_root.height() <= 0
        {
            return 0;
        }

        let viewport = self.folder_root.height() as f32;
        let mut tiles = Vec::new();
        collect_tiles(self.folder_root.upcast_ref(), &mut tiles);
        let mut candidates: Vec<(f32, SquareTile)> = Vec::new();

        for tile in tiles {
            if tile.height() <= 0 || tile.imp().visual_loaded.get() {
                continue;
            }
            let Some(bounds) = tile.compute_bounds(&self.folder_root) else {
                continue;
            };
            let center = bounds.y() + bounds.height() * 0.5;
            if bounds.y() + bounds.height() < 0.0 || bounds.y() > viewport {
                continue;
            }
            // Start near the viewport centre, then fan out. This makes a large
            // scrollbar jump paint the part the user is looking at first.
            candidates.push(((center - viewport * 0.5).abs(), tile));
        }

        candidates.sort_by(|left, right| left.0.total_cmp(&right.0));
        let mut queued = 0usize;
        for (_, tile) in candidates.into_iter().take(budget) {
            if tile.queue_folder_cached_visual_async(true) {
                queued += 1;
            }
        }
        queued
    }

    /// Warm a RAM thumbnail buffer around the Folder viewport.
    ///
    /// `direction` is the current scroll direction (negative = up, positive =
    /// down, 0 = unknown). Tiles ahead of the viewport in that direction are
    /// prioritised so they are already in RAM when they scroll into view, which
    /// is what stops the "blank then pop in" flicker during fast scrolling.
    ///
    /// Cache-file I/O and JPEG decode are queued on bounded worker threads;
    /// only texture creation/application returns to GTK. The hot ListView bind
    /// path stays strictly RAM-only.
    pub fn prefetch_folder_cached_tiles(&self, budget: usize, direction: f64) -> usize {
        if budget == 0
            || self.group_mode.get() != GroupMode::Folder
            || self.folder_root.height() <= 0
        {
            return 0;
        }

        let viewport = self.folder_root.height() as f32;
        let trace = std::env::var_os("PICASA_TRACE").is_some();
        let started = trace.then(Instant::now);
        let mut tiles = Vec::new();
        collect_tiles(self.folder_root.upcast_ref(), &mut tiles);
        let realized = tiles.len();
        let mut candidates: Vec<(f32, SquareTile)> = Vec::new();

        for tile in tiles {
            // Deliberately do NOT require is_mapped(): GtkListView recycles a
            // large pool of realized-but-offscreen rows, and those are exactly
            // the ones about to scroll into view. Warm them ahead of time so the
            // bind finds a RAM hit instead of showing a blank tile. Bounds are
            // still checked against a bounded window below.
            if tile.height() <= 0 || tile.imp().visual_loaded.get() {
                continue;
            }
            let Some(bounds) = tile.compute_bounds(&self.folder_root) else {
                continue;
            };
            let below = bounds.y();
            let above_end = bounds.y() + bounds.height();
            // Keep a generous rolling buffer around the viewport. Warming is
            // bounded per slice, so a wide window is cheap and gives fast
            // scrolling more time to reach the warmed tiles.
            if above_end < -viewport * 4.0 || below > viewport * 5.0 {
                continue;
            }

            let base = if above_end < 0.0 {
                -above_end
            } else if below > viewport {
                below - viewport
            } else {
                0.0
            };
            // Penalise tiles behind the direction of travel so request order
            // follows the user rather than filling both sides equally.
            let behind =
                (direction >= 0.0 && above_end < 0.0) || (direction < 0.0 && below > viewport);
            let distance = if behind { base + viewport * 4.0 } else { base };
            candidates.push((distance, tile));
        }

        candidates.sort_by(|left, right| left.0.total_cmp(&right.0));
        let candidate_count = candidates.len();
        let mut loaded = 0usize;
        for (_, tile) in candidates.into_iter().take(budget) {
            if tile.queue_folder_cached_visual_async(false) {
                loaded += 1;
            }
        }
        if let Some(started) = started {
            let ms = started.elapsed().as_millis();
            if ms >= 8 {
                eprintln!(
                    "UI PERF folder_prefetch_slow realized={} candidates={} loaded={} ms={}",
                    realized, candidate_count, loaded, ms
                );
            }
        }
        loaded
    }

    pub fn refresh_thumbnails(&self) {
        let trace = std::env::var_os("PICASA_TRACE").is_some();
        let started = trace.then(Instant::now);
        let mut tiles = Vec::new();
        collect_tiles(self.root.upcast_ref(), &mut tiles);
        collect_tiles(self.folder_root.upcast_ref(), &mut tiles);
        let count = tiles.len();
        for tile in tiles {
            tile.refresh_thumbnail();
        }
        if let Some(started) = started {
            eprintln!(
                "UI PERF refresh_thumbnails tiles={} ms={}",
                count,
                started.elapsed().as_millis()
            );
        }
    }

    pub fn set_favorite_indicators_visible(&self, visible: bool) {
        let mut tiles = Vec::new();
        collect_tiles(self.root.upcast_ref(), &mut tiles);
        collect_tiles(self.folder_root.upcast_ref(), &mut tiles);
        for tile in tiles {
            tile.set_favorite_indicator_visible(visible);
        }
    }

    pub fn refresh_favorite_indicators(&self) {
        let mut tiles = Vec::new();
        collect_tiles(self.root.upcast_ref(), &mut tiles);
        collect_tiles(self.folder_root.upcast_ref(), &mut tiles);
        for tile in tiles {
            tile.refresh_favorite_indicator();
        }
    }

    pub fn update_favorites(&self, ids: &[i64], favorite: bool) {
        let ids = ids.iter().copied().collect::<HashSet<_>>();
        for photo in self.current_photos.borrow().iter() {
            if ids.contains(&photo.id()) {
                photo.set_favorite(favorite);
            }
        }
        self.refresh_favorite_indicators();
    }

    /// Remove visible photos from the current grid without rebuilding the model.
    /// This is used when a photo stops belonging to the active virtual view
    /// (Favourites or an Album). Preserve the viewport and move selection to
    /// the nearest remaining thumbnail instead of jumping back to item 0.
    pub fn remove_photos(&self, ids: &[i64]) {
        if ids.is_empty() {
            return;
        }

        let scroll_y = self.scroll_position();
        let ids = ids.iter().copied().collect::<HashSet<_>>();
        let positions = self
            .current_photos
            .borrow()
            .iter()
            .enumerate()
            .filter_map(|(position, photo)| ids.contains(&photo.id()).then_some(position))
            .collect::<Vec<_>>();
        if positions.is_empty() {
            return;
        }

        let next_position = positions.iter().copied().min().unwrap_or(0);

        self.current_photos
            .borrow_mut()
            .retain(|photo| !ids.contains(&photo.id()));
        for position in positions.into_iter().rev() {
            self.store.remove(position as u32);
        }

        if self.collage_selection_mode.get() {
            self.restore_collage_selection();
        } else {
            self.selection.unselect_all();
            let len = self.store.n_items() as usize;
            if len == 0 {
                (self.selected)(None);
            } else {
                self.selection
                    .select_item(next_position.min(len - 1) as u32, true);
            }
        }

        if self.group_mode.get() != GroupMode::None {
            self.rebuild_group_ranges();
            if self.group_mode.get() == GroupMode::Folder {
                self.rebuild_folder_rows();
            } else {
                self.update_group_header_for_scroll(scroll_y);
            }
        }

        // Invalidate any pending progressive replacement before restoring the
        // old adjustment after the active list widget has processed removals.
        let generation = self.replace_generation.get().wrapping_add(1);
        self.replace_generation.set(generation);
        let folder_mode = self.group_mode.get() == GroupMode::Folder;
        let adjustment = if folder_mode {
            self.folder_root.vadjustment()
        } else {
            self.root.vadjustment()
        };
        schedule_scroll_restore(
            adjustment,
            scroll_y,
            self.replace_generation.clone(),
            generation,
        );
        if folder_mode {
            self.folder_root.grab_focus();
        } else {
            self.root.grab_focus();
        }
    }

    pub fn refresh_thumbnails_for_paths(&self, paths: &[std::path::PathBuf]) {
        if paths.is_empty() {
            return;
        }
        let paths = paths
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect::<HashSet<_>>();
        let mut tiles = Vec::new();
        collect_tiles(self.root.upcast_ref(), &mut tiles);
        collect_tiles(self.folder_root.upcast_ref(), &mut tiles);
        for tile in tiles {
            let matches = tile
                .imp()
                .photo
                .borrow()
                .as_ref()
                .is_some_and(|photo| paths.contains(&photo.path()));
            if matches {
                tile.refresh_thumbnail();
            }
        }
    }

    pub fn refresh_availability(&self) {
        let updates = self
            .current_photos
            .borrow()
            .iter()
            .map(|photo| {
                (
                    photo.id(),
                    crate::source::cached_file_available(&photo.path()),
                )
            })
            .collect::<Vec<_>>();
        self.apply_availability(&updates);
    }

    pub fn availability_snapshot(&self) -> Vec<(i64, String)> {
        self.current_photos
            .borrow()
            .iter()
            .map(|photo| (photo.id(), photo.path()))
            .collect()
    }

    pub fn apply_availability(&self, updates: &[(i64, bool)]) {
        for (id, available) in updates {
            if let Some(photo) = self
                .current_photos
                .borrow()
                .iter()
                .find(|photo| photo.id() == *id)
            {
                photo.set_original_available(*available);
                photo.set_original_checked_at(Some(Instant::now()));
            }
        }

        let mut tiles = Vec::new();
        collect_tiles(self.root.upcast_ref(), &mut tiles);
        collect_tiles(self.folder_root.upcast_ref(), &mut tiles);
        for tile in tiles {
            tile.refresh_availability();
        }
    }

    pub fn replace(&self, photos: &[Photo]) {
        let trace = std::env::var_os("PICASA_TRACE").is_some();
        let replace_started = trace.then(Instant::now);
        let profile_started = crate::diagnostics::refresh_started(photos.len());
        let generation = self.replace_generation.get().wrapping_add(1);
        self.replace_generation.set(generation);
        // Assume a build is in progress until each completion path clears it.
        // Callers such as folder navigation wait on this so they do not give
        // up while the virtualized Folder rows are still being constructed.
        self.stream_building.set(true);
        let unchanged = {
            let current = self.current_photos.borrow();
            current.len() == photos.len()
                && current
                    .iter()
                    .zip(photos)
                    .all(|(object, photo)| object.id() == photo.id)
        };
        if unchanged {
            // Entering Folder mode can intentionally clear the transient
            // Folder ListView while the correctly ordered stream is prepared.
            // If the DB result happens to have the same id order (for example
            // a library containing only one folder), rebuild those rows rather
            // than leaving the Folder view blank.
            if self.group_mode.get() == GroupMode::Folder && self.folder_store.n_items() == 0 {
                self.rebuild_group_ranges();
                self.rebuild_folder_rows();
            }
            if let Some(started) = replace_started {
                eprintln!(
                    "UI PERF gallery_replace photos={} unchanged=true ms={}",
                    photos.len(),
                    started.elapsed().as_millis()
                );
            }
            self.stream_building.set(false);
            return;
        }

        // Same photo set in a different order (for example Folder <-> All
        // Photos): reorder the existing PhotoObjects instead of reconstructing
        // tens of thousands of them. Measured 658-1363 ms to rebuild all 66k.
        let same_set = {
            let current = self.current_photos.borrow();
            current.len() == photos.len() && !current.is_empty() && {
                let ids = current
                    .iter()
                    .map(|object| object.id())
                    .collect::<std::collections::HashSet<_>>();
                photos.iter().all(|photo| ids.contains(&photo.id))
            }
        };
        if same_set {
            let current = self.current_photos.borrow().clone();
            let mut by_id = current
                .into_iter()
                .map(|object| (object.id(), object))
                .collect::<std::collections::HashMap<_, _>>();
            let reordered = photos
                .iter()
                .filter_map(|photo| by_id.remove(&photo.id))
                .collect::<Vec<_>>();
            if !self.collage_selection_mode.get() {
                (self.selected)(None);
            }
            crate::diagnostics::refresh_first_batch(profile_started, reordered.len());
            self.current_photos.replace(reordered.clone());
            self.store.splice(0, self.store.n_items(), &reordered);
            if self.collage_selection_mode.get() {
                self.restore_collage_selection();
            } else if reordered.is_empty() {
                self.selection.unselect_all();
            } else {
                self.selection.select_item(0, true);
            }
            if self.group_mode.get() != GroupMode::None {
                self.rebuild_group_ranges();
                if self.group_mode.get() == GroupMode::Folder {
                    self.rebuild_folder_rows();
                } else {
                    self.update_group_header_for_scroll(self.last_scroll_y.get());
                }
            }
            if let Some(started) = replace_started {
                eprintln!(
                    "UI PERF gallery_replace photos={} same_set_reorder ms={}",
                    reordered.len(),
                    started.elapsed().as_millis()
                );
            }
            crate::diagnostics::refresh_finished(profile_started, reordered.len());
            self.stream_building.set(false);
            return;
        }

        // Constructing tens of thousands of GObjects synchronously blocks
        // GTK for several seconds. Keep the existing model semantics for
        // normal refreshes, but let the main loop make progress between small
        // batches for library-sized replacements.
        const PROGRESSIVE_REPLACE_THRESHOLD: usize = 1_000;
        if photos.len() > PROGRESSIVE_REPLACE_THRESHOLD {
            if let Some(started) = replace_started {
                eprintln!(
                    "UI PERF gallery_replace photos={} progressive=true ms={}",
                    photos.len(),
                    started.elapsed().as_millis()
                );
            }
            self.replace_progressive(photos.to_vec(), generation, profile_started);
            return;
        }

        if !self.collage_selection_mode.get() {
            (self.selected)(None);
        }
        let objects: Vec<PhotoObject> = photos.iter().map(PhotoObject::from_photo).collect();
        crate::diagnostics::refresh_first_batch(profile_started, objects.len());
        self.current_photos.replace(objects.clone());
        self.store.splice(0, self.store.n_items(), &objects);
        if self.collage_selection_mode.get() {
            self.restore_collage_selection();
        } else if objects.is_empty() {
            self.selection.unselect_all();
        } else {
            self.selection.select_item(0, true);
        }
        if self.group_mode.get() != GroupMode::None {
            self.rebuild_group_ranges();
            if self.group_mode.get() == GroupMode::Folder {
                self.rebuild_folder_rows();
            } else {
                self.update_group_header_for_scroll(self.last_scroll_y.get());
            }
        }
        if let Some(started) = replace_started {
            eprintln!(
                "UI PERF gallery_replace photos={} unchanged=false ms={}",
                objects.len(),
                started.elapsed().as_millis()
            );
        }
        crate::diagnostics::refresh_finished(profile_started, objects.len());
        self.stream_building.set(false);
    }

    fn replace_progressive(
        &self,
        photos: Vec<Photo>,
        generation: u64,
        profile_started: Option<Instant>,
    ) {
        // Larger batches finish the model build in far fewer main-loop hops.
        // Each hop is scheduled at idle priority, so with 500-photo batches a
        // 66k stream needed 133 hops and could take >20 s of wall time even
        // though the actual construction work was under a second.
        const BATCH_SIZE: usize = 2_000;

        let photos = Rc::new(photos);
        let offset = Rc::new(Cell::new(0usize));
        let initialized = Rc::new(Cell::new(false));
        let store = self.store.clone();
        let selected = self.selected.clone();
        let current_photos = self.current_photos.clone();
        let selection = self.selection.clone();
        let collage_selection_mode = self.collage_selection_mode.clone();
        let collage_selected_ids = self.collage_selected_ids.clone();
        let group_mode = self.group_mode.clone();
        let group_date = self.group_date.clone();
        let group_ranges = self.group_ranges.clone();
        let group_header = self.group_header.clone();
        let group_title = self.group_title.clone();
        let group_count = self.group_count.clone();
        let last_scroll_y = self.last_scroll_y.clone();
        let current_columns = self.current_columns.clone();
        let tile_height = self.tile_height.clone();
        let folder_store = self.folder_store.clone();
        let folder_order = self.folder_order.clone();
        let folder_cache = self.folder_cache.clone();
        let folder_root = self.folder_root.clone();
        let replace_generation = self.replace_generation.clone();
        let stream_building = self.stream_building.clone();

        let trace = std::env::var_os("PICASA_TRACE").is_some();
        let build_started = trace.then(Instant::now);
        glib::idle_add_local(move || {
            if replace_generation.get() != generation {
                return glib::ControlFlow::Break;
            }

            let batch_started = trace.then(Instant::now);
            let start = offset.get();
            let end = (start + BATCH_SIZE).min(photos.len());
            let create_started = trace.then(Instant::now);
            let objects: Vec<PhotoObject> = photos[start..end]
                .iter()
                .map(PhotoObject::from_photo)
                .collect();
            let create_ms = create_started
                .map(|value| value.elapsed().as_millis())
                .unwrap_or(0);
            let splice_started = trace.then(Instant::now);
            offset.set(end);

            if !initialized.replace(true) {
                if !collage_selection_mode.get() {
                    selected(None);
                }
                current_photos.replace(objects.clone());
                store.splice(0, store.n_items(), &objects);
                crate::diagnostics::refresh_first_batch(profile_started, objects.len());
            } else {
                current_photos.borrow_mut().extend(objects.iter().cloned());
                store.splice(store.n_items(), 0, &objects);
            }

            if let (Some(batch_started), Some(splice_started)) = (batch_started, splice_started) {
                let splice_ms = splice_started.elapsed().as_millis();
                let total_ms = batch_started.elapsed().as_millis();
                // Log sparsely so the trace itself does not dominate the build.
                if end == photos.len() || end % 10000 == 0 {
                    eprintln!(
                        "UI PERF progressive offset={} size={} create_ms={} splice_ms={} batch_ms={} wall_ms={}",
                        end,
                        photos.len(),
                        create_ms,
                        splice_ms,
                        total_ms,
                        build_started
                            .map(|value| value.elapsed().as_millis())
                            .unwrap_or(0)
                    );
                }
            }

            if end >= photos.len() {
                let ranges_started = trace.then(Instant::now);
                rebuild_group_ranges_for(&current_photos, &group_mode, &group_date, &group_ranges);
                let ranges_ms = ranges_started
                    .map(|value| value.elapsed().as_millis())
                    .unwrap_or(0);
                if group_mode.get() == GroupMode::Folder {
                    let rows_started = trace.then(Instant::now);
                    rebuild_folder_rows_for(
                        &current_photos,
                        &group_ranges,
                        &current_columns,
                        &folder_order,
                        &folder_store,
                    );
                    if group_mode.get() == GroupMode::Folder {
                        save_folder_cache_for(
                            &folder_cache,
                            &current_photos,
                            &group_ranges,
                            &current_columns,
                            &folder_order,
                        );
                    }
                    if let Some(value) = rows_started {
                        eprintln!(
                            "UI PERF folder_rows_build ranges_ms={} rows_ms={}",
                            ranges_ms,
                            value.elapsed().as_millis()
                        );
                    }
                    group_header.set_visible(false);
                    group_title.set_text("");
                    group_count.set_text("");
                } else {
                    update_group_header_for_index_for(
                        &group_mode,
                        &group_ranges,
                        &group_header,
                        &group_title,
                        &group_count,
                        (((last_scroll_y.get() - 20.0).max(0.0)
                            / (tile_height.get().max(1) as f64 + 12.0))
                            .floor() as usize)
                            * current_columns.get().max(1) as usize,
                    );
                }
            }
            if end >= photos.len() && collage_selection_mode.get() {
                selection.unselect_all();
                let wanted = collage_selected_ids.borrow().clone();
                for position in 0..store.n_items() {
                    if store
                        .item(position)
                        .and_downcast::<PhotoObject>()
                        .is_some_and(|photo| wanted.contains(&photo.id()))
                    {
                        selection.select_item(position, false);
                    }
                }
            } else if end >= photos.len() && !objects.is_empty() {
                selection.select_item(0, true);
            }
            if end < photos.len() {
                glib::ControlFlow::Continue
            } else {
                crate::diagnostics::refresh_finished(
                    profile_started,
                    current_photos.borrow().len(),
                );
                if let Some(value) = build_started {
                    eprintln!(
                        "UI PERF progressive_done photos={} wall_ms={} folder_rows={}",
                        current_photos.borrow().len(),
                        value.elapsed().as_millis(),
                        folder_store.n_items()
                    );
                }
                // Folder rows (and therefore folder navigation targets) only
                // exist once every batch has been applied.
                stream_building.set(false);
                glib::ControlFlow::Break
            }
        });
    }

    pub fn append_photos(&self, photos: &[Photo]) {
        if photos.is_empty() {
            return;
        }
        let trace = std::env::var_os("PICASA_TRACE").is_some();
        let append_started = trace.then(Instant::now);
        let objects: Vec<PhotoObject> = photos.iter().map(PhotoObject::from_photo).collect();
        self.current_photos
            .borrow_mut()
            .extend(objects.iter().cloned());
        self.store.splice(self.store.n_items(), 0, &objects);
        if self.group_mode.get() != GroupMode::None {
            self.rebuild_group_ranges();
            if self.group_mode.get() == GroupMode::Folder {
                self.rebuild_folder_rows();
            } else {
                self.update_group_header_for_scroll(self.last_scroll_y.get());
            }
        }
        if let Some(started) = append_started {
            eprintln!(
                "UI PERF gallery_append photos={} total={} ms={}",
                photos.len(),
                self.current_photos.borrow().len(),
                started.elapsed().as_millis()
            );
        }
    }

    /// Returns the currently selected thumbnail position when the grid has a
    /// single active selection. Keyboard navigation uses this to decide when
    /// Up/Down should cross into the adjacent folder.
    pub fn selected_position(&self) -> Option<usize> {
        let selected = self.selection.selection();
        gtk::BitsetIter::init_first(&selected).map(|(_, position)| position as usize)
    }

    pub fn scroll_position(&self) -> f64 {
        let adjustment = if self.group_mode.get() == GroupMode::Folder {
            self.folder_root.vadjustment()
        } else {
            self.root.vadjustment()
        };
        adjustment
            .map(|adjustment| adjustment.value())
            .unwrap_or_else(|| self.last_scroll_y.get())
    }

    /// Return keyboard focus to the realized `SquareTile` for `photo_id` in the
    /// folder stream. A `GtkListView` row may hold several photos, so focusing
    /// the list alone leaves arrow keys without a thumbnail to start from.
    /// Falls back to the `ListView` when the tile is not realized yet.
    fn focus_folder_tile(&self, photo_id: i64) {
        let root = self.folder_root.clone();
        glib::idle_add_local_once(move || {
            let root_for_find = root.clone();
            glib::idle_add_local_once(move || {
                let mut tiles = Vec::new();
                collect_tiles(root_for_find.upcast_ref(), &mut tiles);
                let tile = tiles.into_iter().find(|tile| {
                    tile.imp()
                        .photo
                        .borrow()
                        .as_ref()
                        .is_some_and(|photo| photo.id() == photo_id)
                });
                match tile {
                    Some(tile) => {
                        tile.grab_focus();
                    }
                    None => {
                        root_for_find.grab_focus();
                    }
                }
            });
        });
    }

    pub fn restore_view(&self, photo_id: i64, scroll_y: f64) {
        let Some(position) = self
            .current_photos
            .borrow()
            .iter()
            .position(|photo| photo.id() == photo_id)
        else {
            return;
        };
        let folder_mode = self.group_mode.get() == GroupMode::Folder;
        let folder_row = folder_mode
            .then(|| self.folder_row_index_for_photo(photo_id))
            .flatten();
        let root = self.root.clone();
        let folder_root = self.folder_root.clone();
        let selection = self.selection.clone();
        glib::idle_add_local_once(move || {
            selection.select_item(position as u32, true);
            let scroll = gtk::ScrollInfo::new();
            scroll.set_enable_horizontal(false);
            scroll.set_enable_vertical(false);
            if folder_mode {
                if let Some(row) = folder_row {
                    folder_root.scroll_to(row, gtk::ListScrollFlags::FOCUS, Some(scroll));
                }
                if let Some(adjustment) = folder_root.vadjustment() {
                    let upper =
                        (adjustment.upper() - adjustment.page_size()).max(adjustment.lower());
                    adjustment.set_value(scroll_y.clamp(adjustment.lower(), upper));
                }
            } else {
                root.scroll_to(
                    position as u32,
                    gtk::ListScrollFlags::SELECT | gtk::ListScrollFlags::FOCUS,
                    Some(scroll),
                );
                if let Some(adjustment) = root.vadjustment() {
                    let upper =
                        (adjustment.upper() - adjustment.page_size()).max(adjustment.lower());
                    adjustment.set_value(scroll_y.clamp(adjustment.lower(), upper));
                }
            }
        });
        if folder_mode {
            self.focus_folder_tile(photo_id);
        }
    }

    pub fn restore_context_view(&self, photo_id: i64, scroll_y: f64) {
        let Some(position) = self
            .current_photos
            .borrow()
            .iter()
            .position(|photo| photo.id() == photo_id)
        else {
            return;
        };
        let folder_mode = self.group_mode.get() == GroupMode::Folder;
        let folder_row = folder_mode
            .then(|| self.folder_row_index_for_photo(photo_id))
            .flatten();
        let root = self.root.clone();
        let folder_root = self.folder_root.clone();
        glib::idle_add_local_once(move || {
            let scroll = gtk::ScrollInfo::new();
            scroll.set_enable_horizontal(false);
            scroll.set_enable_vertical(false);
            if folder_mode {
                if let Some(row) = folder_row {
                    folder_root.scroll_to(row, gtk::ListScrollFlags::FOCUS, Some(scroll));
                }
                if let Some(adjustment) = folder_root.vadjustment() {
                    let upper =
                        (adjustment.upper() - adjustment.page_size()).max(adjustment.lower());
                    adjustment.set_value(scroll_y.clamp(adjustment.lower(), upper));
                }
            } else {
                root.scroll_to(position as u32, gtk::ListScrollFlags::FOCUS, Some(scroll));
                if let Some(adjustment) = root.vadjustment() {
                    let upper =
                        (adjustment.upper() - adjustment.page_size()).max(adjustment.lower());
                    adjustment.set_value(scroll_y.clamp(adjustment.lower(), upper));
                }
            }
        });
        if folder_mode {
            self.focus_folder_tile(photo_id);
        }
    }

    pub fn is_at_vertical_boundary(&self, direction: i32) -> bool {
        let Some(position) = self.selected_position() else {
            return false;
        };
        let item_count = self.store.n_items() as usize;
        let columns = self.current_columns.get().max(1) as usize;
        if item_count == 0 {
            return false;
        }

        if direction < 0 {
            position < columns
        } else {
            position.saturating_add(columns) >= item_count
        }
    }

    pub fn update_edit_recipe(&self, id: i64, recipe: &str) {
        if let Some(photo) = self
            .current_photos
            .borrow()
            .iter()
            .find(|photo| photo.id() == id)
        {
            photo.set_edit_recipe(recipe.to_string());
        }
        let mut tiles = Vec::new();
        collect_tiles(self.root.upcast_ref(), &mut tiles);
        collect_tiles(self.folder_root.upcast_ref(), &mut tiles);
        for tile in tiles {
            let matches = tile
                .imp()
                .photo
                .borrow()
                .as_ref()
                .is_some_and(|photo| photo.id() == id);
            if matches {
                tile.refresh_thumbnail();
            }
        }
    }

    pub fn update_dimensions(&self, id: i64, width: Option<i64>, height: Option<i64>) {
        if let Some(photo) = self
            .current_photos
            .borrow()
            .iter()
            .find(|photo| photo.id() == id)
        {
            photo.set_width(width.unwrap_or_default());
            photo.set_height(height.unwrap_or_default());
        }
    }

    pub fn selected_photo_ids(&self, fallback_id: Option<i64>) -> Vec<i64> {
        if self.collage_selection_mode.get() {
            return self.collage_selected_ids.borrow().iter().copied().collect();
        }
        let selected = self.selection.selection();
        let mut ids = Vec::new();
        if let Some((mut iter, first)) = gtk::BitsetIter::init_first(&selected) {
            ids.extend(
                std::iter::once(first)
                    .chain(&mut iter)
                    .filter_map(|position| {
                        self.current_photos.borrow().get(position as usize).cloned()
                    })
                    .map(|photo| photo.id()),
            );
        }
        match fallback_id {
            Some(fallback) if !ids.contains(&fallback) => vec![fallback],
            _ => ids,
        }
    }

    pub fn set_collage_selection_mode(&self, active: bool) {
        self.collage_selection_mode.set(active);
        if !active {
            self.collage_selected_ids.borrow_mut().clear();
        }
    }

    pub fn set_selected_photo_ids(&self, ids: &[i64]) {
        self.collage_selected_ids
            .borrow_mut()
            .extend(ids.iter().copied());
        self.collage_selected_ids
            .borrow_mut()
            .retain(|id| ids.contains(id));
        self.selection.unselect_all();
        let wanted = ids.iter().copied().collect::<HashSet<_>>();
        for position in 0..self.store.n_items() {
            let Some(photo) = self.store.item(position).and_downcast::<PhotoObject>() else {
                continue;
            };
            if wanted.contains(&photo.id()) {
                self.selection.select_item(position, false);
            }
        }
    }

    fn restore_collage_selection(&self) {
        self.selection.unselect_all();
        let wanted = self.collage_selected_ids.borrow().clone();
        for position in 0..self.store.n_items() {
            if self
                .store
                .item(position)
                .and_downcast::<PhotoObject>()
                .is_some_and(|photo| wanted.contains(&photo.id()))
            {
                self.selection.select_item(position, false);
            }
        }
    }

    fn folder_row_index_for_photo(&self, photo_id: i64) -> Option<u32> {
        for position in 0..self.folder_store.n_items() {
            let Some(row) = self
                .folder_store
                .item(position)
                .and_downcast::<FolderRowObject>()
            else {
                continue;
            };
            if row.contains_photo(photo_id) {
                return Some(position);
            }
        }
        None
    }

    fn folder_header_row_for_target(&self, folder_id: i64, folder_path: &str) -> Option<u32> {
        let target_path = std::path::Path::new(folder_path);
        let mut descendant = None;
        for position in 0..self.folder_store.n_items() {
            let Some(row) = self
                .folder_store
                .item(position)
                .and_downcast::<FolderRowObject>()
            else {
                continue;
            };
            let data = row.data();
            if data.kind != FolderRowKind::Header {
                continue;
            }
            if data.folder_id == folder_id {
                return Some(position);
            }
            if descendant.is_none()
                && !data.folder_path.is_empty()
                && std::path::Path::new(&data.folder_path).starts_with(target_path)
            {
                descendant = Some(position);
            }
        }
        descendant
    }

    pub fn grab_focus(&self) {
        if self.group_mode.get() == GroupMode::Folder {
            self.folder_root.grab_focus();
        } else {
            self.root.grab_focus();
        }
    }

    /// True while the gallery is still constructing a progressive replacement.
    /// Folder navigation uses this to keep retrying folder/photo reveal until
    /// the virtualized Folder rows exist.
    pub fn stream_building(&self) -> bool {
        self.stream_building.get()
    }

    pub fn select_photo(&self, photo_id: i64) -> bool {
        let Some(model) = self.selection.model() else {
            return false;
        };
        for position in 0..model.n_items() {
            let matches = model
                .item(position)
                .and_downcast::<PhotoObject>()
                .is_some_and(|photo| photo.id() == photo_id);
            if !matches {
                continue;
            }
            if self.group_mode.get() == GroupMode::Folder {
                // The backing store is filled progressively, but the Folder
                // ListView rows are only built once the whole stream is ready.
                // Selecting the photo is safe early, but report success only
                // when the row exists so callers keep retrying instead of
                // leaving the user parked at the folder header/wrong row.
                self.selection.select_item(position, true);
                let Some(row) = self.folder_row_index_for_photo(photo_id) else {
                    return false;
                };
                self.folder_root
                    .scroll_to(row, gtk::ListScrollFlags::FOCUS, None);
                self.focus_folder_tile(photo_id);
                return true;
            }
            self.selection.select_item(position, true);
            self.root.scroll_to(
                position,
                gtk::ListScrollFlags::SELECT | gtk::ListScrollFlags::FOCUS,
                None,
            );
            return true;
        }
        false
    }

    /// Scroll the continuous Folder stream to the folder's real header row
    /// without rebuilding the gallery. Imported-root rows may not own photos
    /// directly, so the first descendant folder header is a valid target.
    pub fn scroll_to_folder(&self, folder_id: i64, folder_path: &str) -> bool {
        let target_path = std::path::Path::new(folder_path);
        let Some(photo_position) = self.current_photos.borrow().iter().position(|photo| {
            if photo.folder_id() == folder_id {
                return true;
            }
            photo
                .folder_path()
                .as_deref()
                .is_some_and(|path| std::path::Path::new(path).starts_with(target_path))
        }) else {
            return false;
        };

        self.selection.select_item(photo_position as u32, true);
        if self.group_mode.get() == GroupMode::Folder {
            let Some(row) = self.folder_header_row_for_target(folder_id, folder_path) else {
                return false;
            };
            self.folder_root
                .scroll_to(row, gtk::ListScrollFlags::FOCUS, None);
            self.folder_root.grab_focus();
        } else {
            self.root.scroll_to(
                photo_position as u32,
                gtk::ListScrollFlags::SELECT | gtk::ListScrollFlags::FOCUS,
                None,
            );
        }
        true
    }

    pub fn select_last_photo(&self) {
        let count = self.store.n_items();
        if count == 0 {
            return;
        }
        let position = count - 1;
        self.selection.select_item(position, true);
        if self.group_mode.get() == GroupMode::Folder {
            if let Some(row) = self.folder_row_index_for_photo(
                self.store
                    .item(position)
                    .and_downcast::<PhotoObject>()
                    .map(|photo| photo.id())
                    .unwrap_or_default(),
            ) {
                self.folder_root
                    .scroll_to(row, gtk::ListScrollFlags::FOCUS, None);
            }
        } else {
            self.root.scroll_to(
                position,
                gtk::ListScrollFlags::SELECT | gtk::ListScrollFlags::FOCUS,
                None,
            );
        }
    }

    pub fn photo_objects(&self) -> Vec<PhotoObject> {
        self.current_photos.borrow().clone()
    }
}

fn rebuild_group_ranges_for(
    current_photos: &Rc<RefCell<Vec<PhotoObject>>>,
    group_mode: &Rc<Cell<GroupMode>>,
    group_date: &Rc<Cell<GroupDate>>,
    group_ranges: &Rc<RefCell<Vec<GroupRange>>>,
) {
    let mode = group_mode.get();
    let date = group_date.get();
    let photos = current_photos.borrow();
    let mut ranges: Vec<GroupRange> = Vec::new();
    if mode != GroupMode::None {
        for (index, photo) in photos.iter().enumerate() {
            let label = group_label(photo, mode, date);
            let folder_id = if mode == GroupMode::Folder {
                photo.folder_id()
            } else {
                0
            };
            match ranges.last_mut() {
                Some(last) if last.label == label && last.folder_id == folder_id => {
                    last.end = index + 1
                }
                _ => ranges.push(GroupRange {
                    start: index,
                    end: index + 1,
                    label,
                    folder_id,
                }),
            }
        }
    }
    group_ranges.replace(ranges);
}

fn build_folder_virtual_objects(
    ranges: &[GroupRange],
    photos: &[PhotoObject],
    line_size: usize,
) -> Vec<FolderRowObject> {
    let line_size = line_size.max(1);
    let estimated_rows = ranges.iter().fold(0usize, |total, range| {
        let photos_in_range = range.end.saturating_sub(range.start);
        total + 1 + photos_in_range.div_ceil(line_size)
    });
    let mut rows = Vec::with_capacity(estimated_rows);

    for range in ranges {
        rows.push(FolderRowObject::new(FolderRowData {
            kind: FolderRowKind::Header,
            folder_id: range.folder_id,
            folder_path: photos
                .get(range.start)
                .and_then(|photo| photo.folder_path())
                .unwrap_or_default(),
            label: range.label.clone(),
            count: range.end.saturating_sub(range.start),
            start: range.start,
            end: range.start,
            photo_ids: Vec::new(),
        }));

        let mut start = range.start;
        while start < range.end {
            let end = (start + line_size).min(range.end);
            let photo_ids = photos
                .get(start..end)
                .map(|slice| slice.iter().map(|photo| photo.id()).collect())
                .unwrap_or_default();
            rows.push(FolderRowObject::new(FolderRowData {
                kind: FolderRowKind::Photos,
                folder_id: range.folder_id,
                // Photo lines need only folder/range/photo identity. Avoid
                // cloning heading strings into thousands of rows.
                folder_path: String::new(),
                label: String::new(),
                count: 0,
                start,
                end,
                photo_ids,
            }));
            start = end;
        }
    }

    rows
}

fn folder_virtual_row_matches(old: &FolderRowData, new: &FolderRowData) -> bool {
    old.kind == new.kind
        && old.folder_id == new.folder_id
        && old.folder_path == new.folder_path
        && old.label == new.label
        && old.count == new.count
        && old.start == new.start
        && old.end == new.end
        && old.photo_ids == new.photo_ids
}

fn ordered_folder_ranges(ranges: &[GroupRange], folder_order: &[i64]) -> Vec<GroupRange> {
    if folder_order.is_empty() {
        return ranges.to_vec();
    }

    let rank = folder_order
        .iter()
        .enumerate()
        .map(|(index, folder_id)| (*folder_id, index))
        .collect::<std::collections::HashMap<_, _>>();
    let mut ordered = ranges.to_vec();
    ordered.sort_by_key(|range| rank.get(&range.folder_id).copied().unwrap_or(usize::MAX));
    ordered
}

fn save_folder_cache_for(
    cache: &Rc<RefCell<Option<FolderStreamCache>>>,
    current_photos: &Rc<RefCell<Vec<PhotoObject>>>,
    group_ranges: &Rc<RefCell<Vec<GroupRange>>>,
    current_columns: &Rc<Cell<u32>>,
    folder_order: &Rc<RefCell<Vec<i64>>>,
) {
    cache.replace(Some(FolderStreamCache {
        photos: current_photos.borrow().clone(),
        ranges: group_ranges.borrow().clone(),
        columns: current_columns.get(),
        order: folder_order.borrow().clone(),
    }));
}

fn rebuild_folder_rows_for(
    current_photos: &Rc<RefCell<Vec<PhotoObject>>>,
    group_ranges: &Rc<RefCell<Vec<GroupRange>>>,
    current_columns: &Rc<Cell<u32>>,
    folder_order: &Rc<RefCell<Vec<i64>>>,
    folder_store: &gio::ListStore,
) {
    let trace = std::env::var_os("PICASA_TRACE_VERBOSE").is_some();
    let started = trace.then(Instant::now);
    let ranges = group_ranges.borrow();
    let photos = current_photos.borrow();
    let old_rows = folder_store.n_items();
    let line_size = folder_chunk_size(current_columns.get());
    let ordered_ranges = ordered_folder_ranges(&ranges, &folder_order.borrow());
    let new_rows = build_folder_virtual_objects(&ordered_ranges, &photos, line_size);

    if trace {
        eprintln!(
            "UI PERF folder_line_plan photos={} folders={} line_size={} columns={} model_rows={} old_store_rows={} ms={}",
            photos.len(),
            ordered_ranges.len(),
            line_size,
            current_columns.get().max(1),
            new_rows.len(),
            old_rows,
            started.map(|value| value.elapsed().as_millis()).unwrap_or(0)
        );
    }

    let old_len = old_rows as usize;
    let new_len = new_rows.len();
    let mut prefix = 0usize;
    while prefix < old_len
        && prefix < new_len
        && folder_store
            .item(prefix as u32)
            .and_downcast::<FolderRowObject>()
            .is_some_and(|old_row| {
                folder_virtual_row_matches(&old_row.data(), &new_rows[prefix].data())
            })
    {
        prefix += 1;
    }
    let mut suffix = 0usize;
    while suffix < old_len - prefix
        && suffix < new_len - prefix
        && folder_store
            .item((old_len - 1 - suffix) as u32)
            .and_downcast::<FolderRowObject>()
            .is_some_and(|old_row| {
                folder_virtual_row_matches(&old_row.data(), &new_rows[new_len - 1 - suffix].data())
            })
    {
        suffix += 1;
    }

    if trace && prefix < old_len && prefix < new_len {
        if let Some(old_row) = folder_store
            .item(prefix as u32)
            .and_downcast::<FolderRowObject>()
        {
            let old_data = old_row.data();
            let new_data = new_rows[prefix].data();
            eprintln!(
                "UI PERF folder_store_first_row index={} old_kind={:?} new_kind={:?} old_folder_id={} new_folder_id={} old_label={:?} new_label={:?} old_count={} new_count={} old_range={}..{} new_range={}..{}",
                prefix,
                old_data.kind,
                new_data.kind,
                old_data.folder_id,
                new_data.folder_id,
                old_data.label,
                new_data.label,
                old_data.count,
                new_data.count,
                old_data.start,
                old_data.end,
                new_data.start,
                new_data.end,
            );
        }
    }

    if prefix == old_len && prefix == new_len {
        if trace {
            eprintln!(
                "UI PERF folder_store_update strategy=unchanged old_rows={} new_rows={} total_ms={}",
                old_rows,
                new_len,
                started.map(|value| value.elapsed().as_millis()).unwrap_or(0)
            );
        }
        return;
    }

    let update_started = trace.then(Instant::now);
    let removed = (old_len - prefix - suffix) as u32;
    let inserted = &new_rows[prefix..new_len - suffix];
    // Keep the ListView attached. Fixed-height photo lines give GTK a stable
    // geometry estimate, so normal ListStore splicing can reuse the realized
    // row pool without a detach/re-attach storm.
    //
    // Note: this splice is the dominant cost of opening a folder (~400 ms for
    // the ~11.7k-row 66k stream). Measurement showed the ListStore splice itself
    // is ~0-1 ms; GTK's GtkListView spends the time incorporating the new rows.
    // Detaching the model before the splice and re-attaching with set_model
    // moved the cost to the re-attach (413 ms), so it is the ListView
    // population, not the store, that is expensive. Making folder open instant
    // therefore requires reusing an already-populated folder model instead of
    // rebuilding it.
    folder_store.splice(prefix as u32, removed, inserted);
    if trace {
        eprintln!(
            "UI PERF folder_store_update strategy=fixed_lines_attached old_rows={} new_rows={} prefix={} suffix={} model_ms={} total_ms={}",
            old_rows,
            folder_store.n_items(),
            prefix,
            suffix,
            update_started.map(|value| value.elapsed().as_millis()).unwrap_or(0),
            started.map(|value| value.elapsed().as_millis()).unwrap_or(0)
        );
    }
}

fn update_group_header_for_index_for(
    group_mode: &Rc<Cell<GroupMode>>,
    group_ranges: &Rc<RefCell<Vec<GroupRange>>>,
    group_header: &gtk::Box,
    group_title: &gtk::Label,
    group_count: &gtk::Label,
    index: usize,
) {
    if group_mode.get() == GroupMode::None {
        group_header.set_visible(false);
        group_title.set_text("");
        group_count.set_text("");
        return;
    }

    let ranges = group_ranges.borrow();
    let Some(range) = ranges
        .iter()
        .find(|range| index >= range.start && index < range.end)
        .or_else(|| ranges.last())
    else {
        group_header.set_visible(false);
        return;
    };

    group_header.set_visible(true);
    group_title.set_text(&range.label);
    let count = range.end.saturating_sub(range.start);
    group_count.set_text(&format!(
        "•  {} {}",
        format_count(count),
        if count == 1 { "photo" } else { "photos" }
    ));
}

fn group_label(photo: &PhotoObject, mode: GroupMode, date: GroupDate) -> String {
    if mode == GroupMode::Folder {
        let folder_path = photo.folder_path().unwrap_or_default();
        if folder_path.is_empty() {
            return "Unknown Folder".to_string();
        }
        return std::path::Path::new(&folder_path)
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or(folder_path.as_str())
            .to_string();
    }

    let value = match date {
        GroupDate::Taken => photo.taken_at().and_then(|value| parse_photo_date(&value)),
        GroupDate::Added => {
            let mtime = photo.mtime();
            (mtime > 0)
                .then(|| Local.timestamp_opt(mtime, 0).single())
                .flatten()
        }
    };

    let Some(value) = value else {
        return "Unknown Date".to_string();
    };

    match mode {
        GroupMode::None => String::new(),
        GroupMode::Folder => unreachable!("folder grouping returns before date grouping"),
        GroupMode::Month => value.format("%B %Y").to_string(),
        GroupMode::Day => {
            let date = value.date_naive();
            let today = Local::now().date_naive();
            if date == today {
                "Today".to_string()
            } else if date == today.pred_opt().unwrap_or(today) {
                "Yesterday".to_string()
            } else {
                value.format("%-d %B %Y").to_string()
            }
        }
    }
}

fn parse_photo_date(value: &str) -> Option<chrono::DateTime<Local>> {
    if let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(value) {
        return Some(parsed.with_timezone(&Local));
    }
    if let Ok(parsed) = chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S") {
        return Local.from_local_datetime(&parsed).single();
    }
    if let Ok(parsed) = chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H-%M-%S") {
        return Local.from_local_datetime(&parsed).single();
    }
    None
}

fn format_count(value: usize) -> String {
    let digits = value.to_string();
    let mut result = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, character) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            result.push(',');
        }
        result.push(character);
    }
    result
}

fn selection_position_for_id(selection: &gtk::MultiSelection, photo_id: i64) -> Option<u32> {
    SELECTION_POSITION_CALLS.with(|count| count.set(count.get().wrapping_add(1)));
    (0..selection.n_items()).find(|position| {
        selection
            .item(*position)
            .and_downcast::<PhotoObject>()
            .is_some_and(|photo| photo.id() == photo_id)
    })
}

fn find_named_label(root: &gtk::Widget, name: &str) -> Option<gtk::Label> {
    if root.widget_name().as_str() == name {
        if let Ok(label) = root.clone().downcast::<gtk::Label>() {
            return Some(label);
        }
    }
    let mut child = root.first_child();
    while let Some(current) = child {
        if let Some(found) = find_named_label(&current, name) {
            return Some(found);
        }
        child = current.next_sibling();
    }
    None
}

fn selected_photo_id_set(selection: &gtk::MultiSelection) -> HashSet<i64> {
    let selected = selection.selection();
    let mut ids = HashSet::new();
    if let Some((mut iter, first)) = gtk::BitsetIter::init_first(&selected) {
        for position in std::iter::once(first).chain(&mut iter) {
            if let Some(photo) = selection.item(position).and_downcast::<PhotoObject>() {
                ids.insert(photo.id());
            }
        }
    }
    ids
}

fn selected_positions(selection: &gtk::MultiSelection) -> Vec<u32> {
    let selected = selection.selection();
    let mut positions = Vec::new();
    if let Some((mut iter, first)) = gtk::BitsetIter::init_first(&selected) {
        for position in std::iter::once(first).chain(&mut iter) {
            positions.push(position);
        }
    }
    positions
}

/// Resolve the folder-click outcomes into a sorted, deduplicated set of model
/// positions. Plain click replaces the selection, Control toggles one position,
/// and Shift selects the inclusive range between the stored anchor and the
/// clicked position.
fn folder_selection_after_click(
    item_count: u32,
    selected: &[u32],
    anchor: Option<u32>,
    position: u32,
    control: bool,
    shift: bool,
) -> Vec<u32> {
    if item_count == 0 {
        return Vec::new();
    }
    let last = item_count - 1;
    let position = position.min(last);
    let mut result = Vec::new();
    if control {
        result.extend_from_slice(selected);
        if let Some(index) = result.iter().position(|item| *item == position) {
            result.remove(index);
        } else {
            result.push(position);
        }
    } else if shift {
        let anchor = anchor.unwrap_or(position).min(last);
        let start = anchor.min(position);
        let end = anchor.max(position);
        result.extend(start..=end);
    } else {
        result.push(position);
    }
    result.sort_unstable();
    result.dedup();
    result
}

fn apply_folder_click_selection(
    selection: &gtk::MultiSelection,
    anchor: &Cell<Option<u32>>,
    position: u32,
    modifiers: gtk::gdk::ModifierType,
) {
    let control = modifiers.contains(gtk::gdk::ModifierType::CONTROL_MASK);
    let shift = modifiers.contains(gtk::gdk::ModifierType::SHIFT_MASK);
    let selected = selected_positions(selection);
    let desired = folder_selection_after_click(
        selection.n_items(),
        &selected,
        anchor.get(),
        position,
        control,
        shift,
    );
    if !shift || anchor.get().is_none() {
        anchor.set(Some(position));
    }
    selection.unselect_all();
    for item in desired {
        selection.select_item(item, false);
    }
}

/// Axis-aligned bounds for one realized folder tile, expressed in the folder
/// `ListView` coordinate space so drag hit testing is deterministic.
#[derive(Clone, Copy, Debug)]
struct FolderTileBounds {
    position: u32,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

fn folder_dragged_positions(
    tiles: &[FolderTileBounds],
    start: (f64, f64),
    end: (f64, f64),
) -> Vec<u32> {
    let left = start.0.min(end.0);
    let right = start.0.max(end.0);
    let top = start.1.min(end.1);
    let bottom = start.1.max(end.1);
    let mut positions: Vec<u32> = tiles
        .iter()
        .filter(|tile| {
            tile.width > 0.0
                && tile.height > 0.0
                && tile.x <= right
                && tile.x + tile.width >= left
                && tile.y <= bottom
                && tile.y + tile.height >= top
        })
        .map(|tile| tile.position)
        .collect();
    positions.sort_unstable();
    positions.dedup();
    positions
}

#[derive(Default)]
struct FolderDragState {
    start: Option<(f64, f64)>,
    base_ids: HashSet<i64>,
    id_positions: HashMap<i64, u32>,
    positions_ready: bool,
    control: bool,
    active: bool,
    last: Vec<u32>,
}

impl FolderDragState {
    fn clear(&mut self) {
        self.start = None;
        self.base_ids.clear();
        self.id_positions.clear();
        self.positions_ready = false;
        self.control = false;
        self.active = false;
        self.last.clear();
    }
}

/// True when the picked widget sits on the unavailable-source badge button,
/// which owns its own click handler and must be allowed to receive the event.
fn picked_offline_badge(picked: &gtk::Widget) -> bool {
    let mut current = Some(picked.clone());
    while let Some(widget) = current {
        if let Some(button) = widget.downcast_ref::<gtk::Button>() {
            if button.has_css_class("offline-badge") {
                return true;
            }
        }
        current = widget.parent();
    }
    false
}

#[allow(clippy::too_many_arguments)]
fn install_folder_root_input(
    folder_root: &gtk::ListView,
    selection: &gtk::MultiSelection,
    current_photos: &Rc<RefCell<Vec<PhotoObject>>>,
    activate: &Rc<dyn Fn(Vec<PhotoObject>, usize)>,
    context_menu: &Rc<dyn Fn(PhotoObject, gtk::Widget, f64, f64)>,
    collage_mode: &Rc<Cell<bool>>,
    collage_ids: &Rc<RefCell<HashSet<i64>>>,
) {
    let anchor: Rc<Cell<Option<u32>>> = Rc::new(Cell::new(None));
    let trace = std::env::var_os("PICASA_TRACE_VERBOSE").is_some();

    let left_click = gtk::GestureClick::new();
    left_click.set_button(1);
    left_click.set_propagation_phase(gtk::PropagationPhase::Capture);
    let root_for_left = folder_root.clone();
    let selection_for_left = selection.clone();
    let current_photos_for_left = current_photos.clone();
    let activate_for_left = activate.clone();
    let collage_mode_for_left = collage_mode.clone();
    let collage_ids_for_left = collage_ids.clone();
    let anchor_for_left = anchor.clone();
    left_click.connect_pressed(move |gesture, n_press, x, y| {
        if trace {
            eprintln!("FOLDER INPUT press x={x:.0} y={y:.0} n={n_press}");
        }
        let Some(tile) = resolve_folder_tile(&root_for_left, x, y) else {
            if trace {
                eprintln!("FOLDER INPUT resolved=false no-tile");
            }
            return;
        };
        let Some(photo) = tile.imp().photo.borrow().as_ref().cloned() else {
            if trace {
                eprintln!("FOLDER INPUT resolved=false tile-no-photo");
            }
            return;
        };
        let Some(position) = selection_position_for_id(&selection_for_left, photo.id()) else {
            if trace {
                eprintln!("FOLDER INPUT resolved=false no-position id={}", photo.id());
            }
            return;
        };
        if trace {
            eprintln!(
                "FOLDER INPUT resolved=true id={} position={position}",
                photo.id()
            );
        }

        // Do not claim the sequence here: claiming on press prevents the
        // grouped drag gesture from ever emitting drag-update, which is what
        // broke rubberband selection.
        tile.grab_focus();

        if collage_mode_for_left.get() {
            let mut selected_ids = collage_ids_for_left.borrow_mut();
            if selected_ids.remove(&photo.id()) {
                selection_for_left.unselect_item(position);
            } else {
                selected_ids.insert(photo.id());
                selection_for_left.select_item(position, false);
            }
            return;
        }

        if n_press >= 2 {
            selection_for_left.select_item(position, true);
            let photos = current_photos_for_left.borrow().clone();
            if let Some(index) = photos.iter().position(|item| item.id() == photo.id()) {
                (activate_for_left)(photos, index);
            }
            anchor_for_left.set(Some(position));
            return;
        }

        let modifiers = gesture.current_event_state();
        apply_folder_click_selection(&selection_for_left, &anchor_for_left, position, modifiers);
    });
    folder_root.add_controller(left_click.clone());

    let right_click = gtk::GestureClick::new();
    right_click.set_button(3);
    right_click.set_propagation_phase(gtk::PropagationPhase::Capture);
    let root_for_context = folder_root.clone();
    let selection_for_context = selection.clone();
    let context_menu_for_root = context_menu.clone();
    right_click.connect_pressed(move |gesture, _, x, y| {
        let Some(tile) = resolve_folder_tile(&root_for_context, x, y) else {
            return;
        };
        let Some(photo) = tile.imp().photo.borrow().as_ref().cloned() else {
            return;
        };
        let Some(position) = selection_position_for_id(&selection_for_context, photo.id()) else {
            return;
        };
        let Some(frame) = tile.first_child().and_downcast::<gtk::Overlay>() else {
            return;
        };
        let frame_widget = frame.clone().upcast::<gtk::Widget>();
        let local = root_for_context
            .compute_point(
                &frame_widget,
                &gtk::graphene::Point::new(x as f32, y as f32),
            )
            .unwrap_or_else(|| gtk::graphene::Point::new(0.0, 0.0));

        gesture.set_state(gtk::EventSequenceState::Claimed);
        if !selection_for_context.is_selected(position) {
            selection_for_context.select_item(position, true);
        }
        tile.grab_focus();
        (context_menu_for_root)(photo, frame_widget, local.x() as f64, local.y() as f64);
    });
    folder_root.add_controller(right_click);

    let drag = gtk::GestureDrag::new();
    drag.set_button(1);
    drag.set_propagation_phase(gtk::PropagationPhase::Capture);
    let drag_state = Rc::new(RefCell::new(FolderDragState::default()));

    let root_for_begin = folder_root.clone();
    let selection_for_begin = selection.clone();
    let collage_mode_for_begin = collage_mode.clone();
    let state_for_begin = drag_state.clone();
    drag.connect_drag_begin(move |gesture, x, y| {
        state_for_begin.borrow_mut().clear();
        if collage_mode_for_begin.get() {
            gesture.set_state(gtk::EventSequenceState::Denied);
            return;
        }
        // Rubber-band from anywhere in the folder stream, exactly like the
        // Library GridView: a press on a tile focuses it, while a press in the
        // row padding or the empty tail of a row starts an empty
        // rectangle instead of being rejected.
        if let Some(tile) = resolve_folder_tile(&root_for_begin, x, y) {
            tile.grab_focus();
        }
        let modifiers = gesture.current_event_state();
        let mut state = state_for_begin.borrow_mut();
        state.start = Some((x, y));
        state.active = true;
        state.control = modifiers.contains(gtk::gdk::ModifierType::CONTROL_MASK);
        state.base_ids = if state.control {
            selected_photo_id_set(&selection_for_begin)
        } else {
            HashSet::new()
        };
        state.last.clear();
        drop(state);
        if trace {
            eprintln!("FOLDER INPUT drag_begin x={x:.0} y={y:.0}");
        }
        root_for_begin.set_cursor_from_name(Some("crosshair"));
    });

    let root_for_update = folder_root.clone();
    let selection_for_update = selection.clone();
    let current_photos_for_update = current_photos.clone();
    let state_for_update = drag_state.clone();
    drag.connect_drag_update(move |gesture, offset_x, offset_y| {
        // A stationary press still emits tiny drag updates. Do not claim those,
        // otherwise the click gesture is cancelled and loses its double-click
        // counter, which broke double-click-to-open. Only take over once the
        // pointer has actually travelled.
        if offset_x.hypot(offset_y) < DRAG_CLAIM_THRESHOLD {
            return;
        }
        gesture.set_state(gtk::EventSequenceState::Claimed);
        {
            // Build the id -> model-position map only once a real drag starts,
            // so ordinary clicks never pay for a 66k-entry hash map.
            let mut state = state_for_update.borrow_mut();
            if state.active && !state.positions_ready {
                state.id_positions = current_photos_for_update
                    .borrow()
                    .iter()
                    .enumerate()
                    .map(|(index, photo)| (photo.id(), index as u32))
                    .collect();
                state.positions_ready = true;
            }
        }
        let hit = {
            let state = state_for_update.borrow();
            if !state.active {
                return;
            }
            let Some(start) = state.start else {
                return;
            };
            let end = (start.0 + offset_x, start.1 + offset_y);
            let mut bounds = Vec::new();
            let mut tiles = Vec::new();
            collect_tiles(root_for_update.upcast_ref(), &mut tiles);
            for tile in tiles.iter() {
                if !tile.is_mapped() || tile.height() <= 0 {
                    continue;
                }
                let Some(photo) = tile.imp().photo.borrow().as_ref().cloned() else {
                    continue;
                };
                let Some(position) = state.id_positions.get(&photo.id()).copied() else {
                    continue;
                };
                let Some(rect) = tile.compute_bounds(&root_for_update) else {
                    continue;
                };
                bounds.push(FolderTileBounds {
                    position,
                    x: rect.x() as f64,
                    y: rect.y() as f64,
                    width: rect.width() as f64,
                    height: rect.height() as f64,
                });
            }
            let mut positions = folder_dragged_positions(&bounds, start, end);
            if state.control {
                for id in &state.base_ids {
                    if let Some(position) = state.id_positions.get(id) {
                        positions.push(*position);
                    }
                }
                positions.sort_unstable();
                positions.dedup();
            }
            positions
        };
        {
            let mut state = state_for_update.borrow_mut();
            if state.last == hit {
                return;
            }
            state.last = hit.clone();
        }
        if trace {
            let mut realized = Vec::new();
            collect_tiles(root_for_update.upcast_ref(), &mut realized);
            eprintln!(
                "FOLDER INPUT drag_update realized={} selected={}",
                realized.len(),
                hit.len()
            );
        }
        selection_for_update.unselect_all();
        for position in &hit {
            selection_for_update.select_item(*position, false);
        }
    });

    let state_for_end = drag_state.clone();
    let root_for_end = folder_root.clone();
    drag.connect_drag_end(move |_, _, _| {
        if trace {
            eprintln!("FOLDER INPUT drag_end");
        }
        root_for_end.set_cursor_from_name(None);
        state_for_end.borrow_mut().clear();
    });

    // Group the click and drag gestures so a click that turns into a drag can
    // hand the sequence over instead of the click denying the drag (the same
    // coordination GTK's own list widgets rely on for rubberband selection).
    // Both gestures must already belong to the same widget before grouping.
    // Grouping first triggered gtk_gesture_group assertions at startup.
    folder_root.add_controller(drag.clone());
    left_click.group_with(&drag);
}

fn refresh_folder_selection_styles(root: &gtk::ListView, selection: &gtk::MultiSelection) {
    let trace = std::env::var_os("PICASA_TRACE").is_some();
    let verbose = std::env::var_os("PICASA_TRACE_VERBOSE").is_some();
    let started = (trace || verbose).then(Instant::now);
    let calls_before = SELECTION_POSITION_CALLS.with(Cell::get);
    let selected_ids = selected_photo_id_set(selection);
    let mut tiles = Vec::new();
    collect_tiles(root.upcast_ref(), &mut tiles);
    let count = tiles.len();
    for tile in tiles {
        let selected = tile
            .imp()
            .photo
            .borrow()
            .as_ref()
            .is_some_and(|photo| selected_ids.contains(&photo.id()));
        tile.set_manual_selected(selected);
    }
    if let Some(started) = started {
        let elapsed_ms = started.elapsed().as_millis();
        if verbose || elapsed_ms >= 8 {
            eprintln!(
                "UI PERF folder_selection_styles{} tiles={} items={} spfid_calls={} ms={}",
                if elapsed_ms >= 8 { "_slow" } else { "" },
                count,
                selection.n_items(),
                SELECTION_POSITION_CALLS
                    .with(Cell::get)
                    .wrapping_sub(calls_before),
                elapsed_ms
            );
        }
    }
}

fn folder_id_from_named_ancestor(widget: &gtk::Widget) -> Option<i64> {
    let mut current = Some(widget.clone());
    while let Some(candidate) = current {
        let name = candidate.widget_name();
        if let Some(value) = name.as_str().strip_prefix("picasa-folder-row-") {
            if let Ok(folder_id) = value.parse::<i64>() {
                return Some(folder_id);
            }
        }
        current = candidate.parent();
    }
    None
}

/// Nearest `SquareTile` ancestor of a picked widget, if any.
fn tile_ancestor(widget: &gtk::Widget) -> Option<SquareTile> {
    let mut current = Some(widget.clone());
    while let Some(candidate) = current {
        if let Ok(tile) = candidate.clone().downcast::<SquareTile>() {
            return Some(tile);
        }
        current = candidate.parent();
    }
    None
}

/// Resolve a point inside the Folder ListView to the bound SquareTile under it.
/// Folder rows now contain direct tile children, so GTK's normal `pick` result
/// is sufficient; there is no nested FlowBox child to resolve manually.
fn resolve_folder_tile(root: &gtk::ListView, x: f64, y: f64) -> Option<SquareTile> {
    let picked = root.pick(x, y, gtk::PickFlags::DEFAULT)?;
    if picked_offline_badge(&picked) {
        return None;
    }
    let tile = tile_ancestor(&picked)?;
    if tile.imp().photo.borrow().is_some() {
        Some(tile)
    } else {
        None
    }
}

fn schedule_scroll_restore(
    adjustment: Option<gtk::Adjustment>,
    scroll_y: f64,
    replace_generation: Rc<Cell<u64>>,
    generation: u64,
) {
    glib::idle_add_local_once(move || {
        if replace_generation.get() != generation {
            return;
        }
        if let Some(adjustment) = adjustment {
            let upper = (adjustment.upper() - adjustment.page_size()).max(adjustment.lower());
            adjustment.set_value(scroll_y.clamp(adjustment.lower(), upper));
        }
    });
}

fn folder_line_height(tile_height: i32) -> i32 {
    tile_height.max(1) + 12
}

fn box_tiles(line: &gtk::Box) -> Vec<SquareTile> {
    let mut tiles = Vec::new();
    let mut child = line.first_child();
    while let Some(current) = child {
        if let Ok(tile) = current.clone().downcast::<SquareTile>() {
            tiles.push(tile);
        }
        child = current.next_sibling();
    }
    tiles
}

fn update_folder_realized_rows(widget: &gtk::Widget, tile_width: i32, tile_height: i32) {
    if widget.widget_name().as_str() == "picasa-folder-photo-line" {
        if let Some(line) = widget.downcast_ref::<gtk::Box>() {
            for tile in box_tiles(line) {
                tile.set_tile_size(tile_width, tile_height);
            }
            if line.is_visible() {
                if let Some(row_root) = line.parent().and_downcast::<gtk::Box>() {
                    row_root.set_height_request(folder_line_height(tile_height));
                }
            }
        }
    }
    let mut child = widget.first_child();
    while let Some(current) = child {
        update_folder_realized_rows(&current, tile_width, tile_height);
        child = current.next_sibling();
    }
}

fn collect_tiles(widget: &gtk::Widget, tiles: &mut Vec<SquareTile>) {
    if let Some(tile) = widget.downcast_ref::<SquareTile>() {
        tiles.push(tile.clone());
    }
    let mut child = widget.first_child();
    while let Some(current) = child {
        collect_tiles(&current, tiles);
        child = current.next_sibling();
    }
}

#[cfg(test)]
mod folder_stream_tests {
    use super::{
        folder_chunk_size, folder_dragged_positions, folder_line_height,
        folder_selection_after_click, folder_virtual_row_matches, folder_virtual_rows,
        ordered_folder_ranges, FolderRowData, FolderRowKind, FolderTileBounds, GroupRange,
    };

    #[test]
    #[ignore = "requires a GTK display; run with --ignored --test-threads=1"]
    fn sidebar_width_changes_do_not_scroll_or_lose_the_visible_photo() {
        use super::{gtk, Gallery, GroupMode, PhotoObject};
        use gtk::prelude::*;

        fn settle() {
            let context = glib::MainContext::default();
            for _ in 0..30 {
                while context.pending() {
                    context.iteration(false);
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }

        gtk::init().unwrap();
        let gallery = Gallery::new(
            &[], 148, |_| {}, |_, _| {}, |_, _, _, _| {}, |_, _| {}, |_| {},
        );
        gallery.group_mode.set(GroupMode::Folder);
        gallery.current_photos.replace(
            (1..=1200_i64)
                .map(|id| {
                    glib::Object::builder::<PhotoObject>()
                        .property("id", id)
                        .property("folder-id", 1_i64)
                        .property("original-available", true)
                        .build()
                })
                .collect(),
        );
        gallery.rebuild_group_ranges();
        gallery.update_width(1124);
        let scroll = gtk::ScrolledWindow::builder()
            .child(&gallery.folder_root)
            .build();
        let window = gtk::Window::builder()
            .default_width(1124)
            .default_height(600)
            .child(&scroll)
            .build();
        window.present();
        settle();
        scroll.vadjustment().set_value(8000.0);
        settle();

        // Six columns put the centre probe between tiles. The anchor must be
        // a visible photo, never the first photo of this long folder.
        let before = gallery.photo_for_visible_folder_row().unwrap().id();
        assert!(before > 100, "deep viewport anchored to photo {before}");
        let y = scroll.vadjustment().value();
        gallery.zoom_anchor.set(Some(before));
        for width in [1135, 1157, 1184, 1217, 1246, 1273, 1246, 1124] {
            gallery.update_width(width);
            settle();
            assert_eq!(
                scroll.vadjustment().value(), y,
                "sidebar width {width} scrolled"
            );
            assert_eq!(
                gallery.zoom_anchor.get(), Some(before),
                "width-only resize consumed zoom anchor"
            );
            assert_eq!(gallery.photo_for_visible_folder_row().unwrap().id(), before);
        }

        gallery.zoom_anchor.set(None);
        for width in [1440, 1124].into_iter().cycle().take(8) {
            gallery.update_width(width);
            window.set_default_size(width, 600);
            settle();
            let after = gallery.photo_for_visible_folder_row().unwrap().id();
            assert_eq!(after, before, "column change lost anchor at width {width}");
        }
        gallery.zoom_anchor.set(Some(before));

        // An actual zoom within the same columns still updates row geometry
        // and consumes its saved anchor, even when the width is unchanged.
        gallery.tile_height.set(gallery.tile_height.get() + 10);
        gallery.update_layout(1124, true);
        assert_eq!(gallery.zoom_anchor.get(), None);
        settle();
        let after = gallery.photo_for_visible_folder_row().unwrap().id();
        assert_eq!(after, before, "zoom lost anchor");

        // Ordinary scrolling must move the logical anchor too; retaining the
        // resize anchor must never pin the viewport to an old photo.
        scroll.vadjustment().set_value(16000.0);
        settle();
        let scrolled = gallery.photo_for_visible_folder_row().unwrap().id();
        assert_ne!(scrolled, before);
        gallery.update_width(1440);
        window.set_default_size(1440, 600);
        settle();
        assert_eq!(gallery.photo_for_visible_folder_row().unwrap().id(), scrolled);

        // A second column change can arrive before GTK has allocated the first
        // rebuild. Its adjustment may transiently reset to zero (as in the
        // reported 7→6→5 transition). Never capture that transient viewport.
        gallery.update_width(1124);
        scroll.vadjustment().set_value(0.0);
        gallery.update_width(970);
        window.set_default_size(970, 600);
        settle();
        assert_eq!(
            gallery.photo_for_visible_folder_row().unwrap().id(), scrolled,
            "overlapping column changes lost the original anchor"
        );
        window.close();
    }

    #[test]
    fn exact_folder_row_offset_uses_fixed_model_heights() {
        let rows = vec![
            FolderVirtualRow { kind: FolderRowKind::Header, start: 0, end: 0 },
            FolderVirtualRow { kind: FolderRowKind::Photos, start: 0, end: 5 },
            FolderVirtualRow { kind: FolderRowKind::Photos, start: 5, end: 10 },
            FolderVirtualRow { kind: FolderRowKind::Header, start: 10, end: 10 },
            FolderVirtualRow { kind: FolderRowKind::Photos, start: 10, end: 15 },
        ];

        // Header 58 + two 100px photo lines + header 58.
        assert_eq!(super::folder_row_offset(&rows, 4, 88), 316.0);
    }

    fn sample_ranges() -> Vec<GroupRange> {
        vec![
            GroupRange {
                start: 0,
                end: 5,
                label: "Pictures".to_string(),
                folder_id: 10,
            },
            GroupRange {
                start: 5,
                end: 18,
                label: "Drone".to_string(),
                folder_id: 11,
            },
        ]
    }

    #[test]
    fn folder_virtual_stream_uses_one_fixed_photo_line_per_column_width() {
        let rows = folder_virtual_rows(&sample_ranges(), 3);
        assert_eq!(rows.len(), 9);
        assert_eq!(rows[0].kind, FolderRowKind::Header);
        assert_eq!((rows[1].start, rows[1].end), (0, 3));
        assert_eq!((rows[2].start, rows[2].end), (3, 5));
        assert_eq!(rows[3].kind, FolderRowKind::Header);
        assert_eq!((rows[4].start, rows[4].end), (5, 8));
        assert_eq!((rows[5].start, rows[5].end), (8, 11));
        assert_eq!((rows[6].start, rows[6].end), (11, 14));
        assert_eq!((rows[7].start, rows[7].end), (14, 17));
        assert_eq!((rows[8].start, rows[8].end), (17, 18));
    }

    #[test]
    fn folder_chunk_size_equals_columns() {
        for columns in 1..=8u32 {
            assert_eq!(folder_chunk_size(columns), columns as usize);
        }
    }

    #[test]
    fn folder_photo_line_height_is_fixed() {
        assert_eq!(folder_line_height(100), 112);
        assert_eq!(folder_line_height(163), 175);
    }

    #[test]
    fn folder_virtual_photo_lines_cover_each_range_without_gaps() {
        for columns in 1..=8u32 {
            let rows = folder_virtual_rows(&sample_ranges(), folder_chunk_size(columns));
            for range in sample_ranges() {
                let covered = rows
                    .iter()
                    .filter(|row| row.kind == FolderRowKind::Photos)
                    .filter(|row| row.start >= range.start && row.end <= range.end)
                    .flat_map(|row| row.start..row.end)
                    .collect::<Vec<_>>();
                assert_eq!(covered, (range.start..range.end).collect::<Vec<_>>());
                assert!(rows
                    .iter()
                    .filter(|row| row.kind == FolderRowKind::Photos)
                    .filter(|row| row.start >= range.start && row.end <= range.end)
                    .all(|row| row.end - row.start <= columns as usize));
            }
        }
    }

    #[test]
    fn folder_click_selection_matches_grid_semantics() {
        assert_eq!(
            folder_selection_after_click(8, &[], None, 2, false, false),
            vec![2]
        );
        assert_eq!(
            folder_selection_after_click(8, &[2], Some(2), 5, true, false),
            vec![2, 5]
        );
        assert_eq!(
            folder_selection_after_click(8, &[2], Some(2), 5, false, true),
            vec![2, 3, 4, 5]
        );
        assert_eq!(
            folder_selection_after_click(8, &[1, 3], None, 5, false, true),
            vec![5]
        );
        assert_eq!(
            folder_selection_after_click(0, &[], None, 5, false, false),
            Vec::<u32>::new()
        );
    }

    #[test]
    fn folder_drag_selection_returns_intersecting_tiles_in_model_order() {
        let tiles = vec![
            FolderTileBounds {
                position: 0,
                x: 0.0,
                y: 0.0,
                width: 100.0,
                height: 100.0,
            },
            FolderTileBounds {
                position: 1,
                x: 110.0,
                y: 0.0,
                width: 100.0,
                height: 100.0,
            },
            FolderTileBounds {
                position: 2,
                x: 0.0,
                y: 110.0,
                width: 100.0,
                height: 100.0,
            },
        ];
        assert_eq!(
            folder_dragged_positions(&tiles, (50.0, 50.0), (160.0, 160.0)),
            vec![0, 1, 2]
        );
        assert_eq!(
            folder_dragged_positions(&tiles, (250.0, 250.0), (300.0, 300.0)),
            Vec::<u32>::new()
        );
        assert_eq!(
            folder_dragged_positions(&tiles, (160.0, 0.0), (50.0, 50.0)),
            vec![0, 1]
        );
    }

    #[test]
    fn folder_range_order_changes_presentation_without_changing_source_ranges() {
        let original = sample_ranges();
        let ordered = ordered_folder_ranges(&original, &[11, 10]);

        assert_eq!(
            ordered
                .iter()
                .map(|range| range.folder_id)
                .collect::<Vec<_>>(),
            vec![11, 10]
        );
        assert_eq!((ordered[0].start, ordered[0].end), (5, 18));
        assert_eq!((ordered[1].start, ordered[1].end), (0, 5));
        assert_eq!(
            original
                .iter()
                .map(|range| range.folder_id)
                .collect::<Vec<_>>(),
            vec![10, 11]
        );
    }

    #[test]
    fn folder_virtual_row_match_compares_identity_fields() {
        let base = FolderRowData {
            kind: FolderRowKind::Header,
            folder_id: 7,
            folder_path: "/photos".to_string(),
            label: "photos".to_string(),
            count: 3,
            start: 0,
            end: 0,
            photo_ids: Vec::new(),
        };
        assert!(folder_virtual_row_matches(&base, &base.clone()));

        let mut different_folder = base.clone();
        different_folder.folder_id = 8;
        assert!(!folder_virtual_row_matches(&base, &different_folder));

        let mut different_count = base.clone();
        different_count.count = 4;
        assert!(!folder_virtual_row_matches(&base, &different_count));

        let mut different_photos = base.clone();
        different_photos.kind = FolderRowKind::Photos;
        different_photos.start = 0;
        different_photos.end = 2;
        different_photos.photo_ids = vec![10, 11];
        let mut same_geometry_new_photos = different_photos.clone();
        same_geometry_new_photos.photo_ids = vec![10, 12];
        assert!(!folder_virtual_row_matches(
            &different_photos,
            &same_geometry_new_photos
        ));
    }
}
