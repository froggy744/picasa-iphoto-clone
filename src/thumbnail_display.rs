use std::collections::{HashSet, VecDeque};
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex, OnceLock};

const DISPLAY_QUEUE_CAPACITY: usize = 1024;
fn display_worker_count() -> usize {
    // Thumbnail presentation reads only PIC's small local JPEG cache. On
    // modern CPUs a few extra workers substantially improve dense 8-column /
    // ~96-thumbnail Page Up/Down without touching originals or blocking GTK.
    std::thread::available_parallelism()
        .map(|count| count.get().clamp(4, 8))
        .unwrap_or(4)
}

type Queue = (Mutex<VecDeque<DisplayRequest>>, Condvar);

static DISPLAY_QUEUE: OnceLock<Arc<Queue>> = OnceLock::new();
static DISPLAY_PENDING: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
static DISPLAY_COMPLETIONS: OnceLock<Mutex<Vec<DisplayCompletion>>> = OnceLock::new();

#[derive(Debug, Clone)]
pub struct DisplayRequest {
    pub key: String,
    pub cached_path: String,
    pub source_path: String,
    pub mtime: i64,
    pub size_bytes: i64,
    pub rotation: i32,
    pub edit_recipe: String,
    pub source_width: i64,
    pub source_height: i64,
    pub visible_priority: bool,
}

#[derive(Debug)]
pub enum DisplayOutcome {
    Loaded {
        width: i32,
        height: i32,
        pixels: Vec<u8>,
    },
    Missing,
    Failed,
}

#[derive(Debug)]
pub struct DisplayCompletion {
    pub key: String,
    pub outcome: DisplayOutcome,
}

pub fn presentation_key(
    cached_path: &str,
    source_path: &str,
    rotation: i32,
    edit_recipe: &str,
    source_width: i64,
    source_height: i64,
) -> String {
    let raw = crate::image_format::uses(source_path, crate::image_format::DecoderKind::Raw);
    format!(
        "{}\0rot={}\0edit={}\0raw={}\0dims={}x{}",
        cached_path,
        rotation.rem_euclid(360),
        edit_recipe,
        raw as u8,
        if raw { source_width } else { 0 },
        if raw { source_height } else { 0 }
    )
}

pub fn request_for(
    cached_path: String,
    source_path: String,
    mtime: i64,
    size_bytes: i64,
    rotation: i32,
    edit_recipe: String,
    source_width: i64,
    source_height: i64,
    visible_priority: bool,
) -> DisplayRequest {
    let key = presentation_key(
        &cached_path,
        &source_path,
        rotation,
        &edit_recipe,
        source_width,
        source_height,
    );
    DisplayRequest {
        key,
        cached_path,
        source_path,
        mtime,
        size_bytes,
        rotation,
        edit_recipe,
        source_width,
        source_height,
        visible_priority,
    }
}

pub fn submit(request: DisplayRequest) -> bool {
    submit_with_policy(request, false)
}

/// Submit a visible request that represents the newest recycled Folder tile.
///
/// Folder ListView can bind the new viewport while older visible requests from
/// rows already scrolled past are still queued. Those newest tiles must not sit
/// behind stale visible work, otherwise the last visible row appears blank for
/// several frames. This path inserts at the front and, if necessary, evicts the
/// oldest queued work from the back. Already-running work is never cancelled.
pub fn submit_latest_visible(mut request: DisplayRequest) -> bool {
    request.visible_priority = true;
    submit_with_policy(request, true)
}

fn submit_with_policy(request: DisplayRequest, newest_visible_first: bool) -> bool {
    let pending = DISPLAY_PENDING.get_or_init(|| Mutex::new(HashSet::new()));
    let Ok(mut pending) = pending.lock() else {
        return false;
    };
    if !pending.insert(request.key.clone()) {
        return false;
    }
    drop(pending);

    let queue = DISPLAY_QUEUE.get_or_init(|| {
        let queue = Arc::new((Mutex::new(VecDeque::<DisplayRequest>::new()), Condvar::new()));
        for _ in 0..display_worker_count() {
            let queue = queue.clone();
            std::thread::spawn(move || worker_loop(queue));
        }
        queue
    });

    let (jobs, wake) = &**queue;
    let Ok(mut jobs) = jobs.lock() else {
        if let Ok(mut pending) = DISPLAY_PENDING
            .get_or_init(|| Mutex::new(HashSet::new()))
            .lock()
        {
            pending.remove(&request.key);
        }
        return false;
    };

    let mut evicted_key = None;
    if jobs.len() >= DISPLAY_QUEUE_CAPACITY {
        if request.visible_priority {
            // Normal visible work preserves centre-out FIFO order and only
            // sacrifices background prefetch. Folder bind's newest-first path
            // is different: when the queue is entirely visible work, the tail
            // is necessarily older than the tile being bound now, so evict it.
            if let Some(index) = jobs.iter().rposition(|job| !job.visible_priority) {
                evicted_key = jobs.remove(index).map(|job| job.key);
            } else if newest_visible_first {
                evicted_key = jobs.pop_back().map(|job| job.key);
            } else {
                drop(jobs);
                if let Ok(mut pending) = DISPLAY_PENDING
                    .get_or_init(|| Mutex::new(HashSet::new()))
                    .lock()
                {
                    pending.remove(&request.key);
                }
                return false;
            }
        } else {
            drop(jobs);
            if let Ok(mut pending) = DISPLAY_PENDING
                .get_or_init(|| Mutex::new(HashSet::new()))
                .lock()
            {
                pending.remove(&request.key);
            }
            return false;
        }
    }

    if request.visible_priority {
        if newest_visible_first {
            jobs.push_front(request);
        } else {
            let index = jobs
                .iter()
                .position(|job| !job.visible_priority)
                .unwrap_or(jobs.len());
            jobs.insert(index, request);
        }
    } else {
        jobs.push_back(request);
    }
    wake.notify_one();
    drop(jobs);

    if let Some(evicted_key) = evicted_key {
        if let Ok(mut pending) = DISPLAY_PENDING
            .get_or_init(|| Mutex::new(HashSet::new()))
            .lock()
        {
            pending.remove(&evicted_key);
        }
    }
    true
}

/// Replace queued visible-priority work with the thumbnails for the viewport
/// GTK is painting *now*.
///
/// GridView/ListView can recycle hundreds of items during a scrollbar jump. If
/// every bind is allowed to leave a visible-priority request behind, old
/// viewports can fill the queue and starve the final viewport. This operation
/// drops only queued (not already-running) visible work, keeps background
/// prefetch, promotes matching background requests, and inserts the current
/// viewport in caller order.
pub fn replace_visible_requests(mut requests: Vec<DisplayRequest>) -> usize {
    if requests.is_empty() {
        return 0;
    }

    for request in &mut requests {
        request.visible_priority = true;
    }

    let pending_mutex = DISPLAY_PENDING.get_or_init(|| Mutex::new(HashSet::new()));
    let Ok(mut pending) = pending_mutex.lock() else {
        return 0;
    };

    let queue = DISPLAY_QUEUE.get_or_init(|| {
        let queue = Arc::new((Mutex::new(VecDeque::<DisplayRequest>::new()), Condvar::new()));
        for _ in 0..display_worker_count() {
            let queue = queue.clone();
            std::thread::spawn(move || worker_loop(queue));
        }
        queue
    });
    let (jobs_mutex, wake) = &**queue;
    let Ok(mut jobs) = jobs_mutex.lock() else {
        return 0;
    };

    // Remove stale visible jobs from previous viewports. Jobs already popped by
    // workers are not in this deque and remain protected by DISPLAY_PENDING.
    let mut retained = VecDeque::with_capacity(jobs.len());
    while let Some(job) = jobs.pop_front() {
        if job.visible_priority {
            pending.remove(&job.key);
        } else {
            retained.push_back(job);
        }
    }
    *jobs = retained;

    let mut inserted = 0usize;
    for request in requests {
        // A background-prefetch request for the same thumbnail can be promoted
        // instead of decoded twice.
        if let Some(index) = jobs.iter().position(|job| job.key == request.key) {
            if let Some(mut job) = jobs.remove(index) {
                job.visible_priority = true;
                jobs.insert(inserted, job);
                inserted += 1;
            }
            continue;
        }

        // If the key is still pending after stale queued work was removed, one
        // of the workers is already decoding it. Do not enqueue a duplicate.
        if pending.contains(&request.key) {
            continue;
        }

        // Current viewport work wins over speculative prefetch when the queue
        // is full. Background jobs are discarded from the far end first.
        while jobs.len() >= DISPLAY_QUEUE_CAPACITY {
            let Some(index) = jobs.iter().rposition(|job| !job.visible_priority) else {
                break;
            };
            if let Some(evicted) = jobs.remove(index) {
                pending.remove(&evicted.key);
            }
        }
        if jobs.len() >= DISPLAY_QUEUE_CAPACITY {
            break;
        }

        pending.insert(request.key.clone());
        jobs.insert(inserted, request);
        inserted += 1;
    }

    if inserted > 0 {
        wake.notify_all();
    }
    inserted
}

pub fn pending_count() -> usize {
    DISPLAY_PENDING
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .map(|pending| pending.len())
        .unwrap_or_default()
}

pub fn take_completions() -> Vec<DisplayCompletion> {
    DISPLAY_COMPLETIONS
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .map(|mut completions| std::mem::take(&mut *completions))
        .unwrap_or_default()
}

fn worker_loop(queue: Arc<Queue>) {
    loop {
        let request = {
            let (jobs, wake) = &*queue;
            let mut jobs = jobs.lock().expect("thumbnail display queue should not be poisoned");
            while jobs.is_empty() {
                jobs = wake
                    .wait(jobs)
                    .expect("thumbnail display queue should not be poisoned");
            }
            jobs.pop_front()
                .expect("thumbnail display queue was checked above")
        };

        let outcome = load_display_thumbnail(&request);
        let completion = DisplayCompletion {
            key: request.key.clone(),
            outcome,
        };
        if let Ok(mut completions) = DISPLAY_COMPLETIONS
            .get_or_init(|| Mutex::new(Vec::new()))
            .lock()
        {
            completions.push(completion);
        }
        if let Ok(mut pending) = DISPLAY_PENDING
            .get_or_init(|| Mutex::new(HashSet::new()))
            .lock()
        {
            pending.remove(&request.key);
        }
    }
}

fn load_display_thumbnail(request: &DisplayRequest) -> DisplayOutcome {
    let path = Path::new(&request.cached_path);
    if !path.is_file() {
        crate::thumbnail::request_priority(
            request.source_path.clone(),
            Some(request.mtime),
            Some(request.size_bytes),
        );
        return DisplayOutcome::Missing;
    }

    let mut image = match image::open(path) {
        Ok(image) => image.to_rgba8(),
        Err(error) => {
            if std::env::var_os("PICASA_TRACE").is_some() {
                eprintln!(
                    "THUMB DISPLAY corrupt_cache cache={} source={} error={}",
                    request.cached_path, request.source_path, error
                );
            }
            // This is PIC's own cache file. A decode failure means the cache is
            // unusable, not that the original is corrupt. Remove it and let the
            // normal priority generator rebuild it from the source in the
            // background.
            let _ = std::fs::remove_file(path);
            crate::thumbnail::request_priority(
                request.source_path.clone(),
                Some(request.mtime),
                Some(request.size_bytes),
            );
            return DisplayOutcome::Failed;
        }
    };

    if crate::image_format::uses(
        &request.source_path,
        crate::image_format::DecoderKind::Raw,
    ) {
        image = crop_raw_cached_preview(image, request.source_width, request.source_height);
    }

    image = crate::edit::render::apply_library_rotation(image, request.rotation);
    let recipe = crate::edit::EditRecipe::decode(&request.edit_recipe);
    if !recipe.is_default() {
        image = crate::edit::render::apply_recipe(image, &recipe);
    }

    DisplayOutcome::Loaded {
        width: image.width() as i32,
        height: image.height() as i32,
        pixels: image.into_raw(),
    }
}

fn crop_raw_cached_preview(
    mut image: image::RgbaImage,
    source_width: i64,
    source_height: i64,
) -> image::RgbaImage {
    if source_width <= 0 || source_height <= 0 || image.width() == 0 || image.height() == 0 {
        return image;
    }

    let image_ratio = image.width() as f64 / image.height() as f64;
    let direct_ratio = source_width as f64 / source_height as f64;
    let swapped_ratio = source_height as f64 / source_width as f64;
    // EXIF orientation is already baked into the disk thumbnail. Avoid reading
    // the original RAW merely to discover whether its axes were swapped: the
    // cached preview itself tells us which sensor ratio is the plausible one.
    let direct_error = (image_ratio.ln() - direct_ratio.ln()).abs();
    let swapped_error = (image_ratio.ln() - swapped_ratio.ln()).abs();
    let target_ratio = if swapped_error < direct_error {
        swapped_ratio
    } else {
        direct_ratio
    };

    let image_width = image.width();
    let image_height = image.height();
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
    image
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presentation_key_changes_with_visual_state() {
        let base = presentation_key("/cache/a.jpg", "/photo/a.nef", 0, "", 6000, 4000);
        assert_ne!(
            base,
            presentation_key("/cache/a.jpg", "/photo/a.nef", 90, "", 6000, 4000)
        );
        assert_ne!(
            base,
            presentation_key("/cache/a.jpg", "/photo/a.nef", 0, "v=1;e=0.5", 6000, 4000)
        );
        assert_ne!(
            base,
            presentation_key("/cache/a.jpg", "/photo/a.nef", 0, "", 4000, 6000)
        );
    }

    #[test]
    fn raw_crop_accepts_rotated_sensor_ratio_without_original_io() {
        let image = image::RgbaImage::new(200, 300);
        let cropped = crop_raw_cached_preview(image, 6000, 4000);
        assert_eq!(cropped.dimensions(), (200, 300));
    }
}
