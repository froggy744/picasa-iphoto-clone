use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::io::{BufReader, Cursor, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};

use anyhow::{Context, Result};
use fast_image_resize::{
    images::Image, pixels::PixelType, FilterType, ResizeAlg, ResizeOptions, Resizer,
};
use image::codecs::jpeg::JpegEncoder;
use image::ImageEncoder;
use image::{ColorType, DynamicImage, ImageReader};
use turbojpeg::{Decompressor, Image as TurboImage, PixelFormat, ScalingFactor};

const THUMBNAIL_SIZE: u32 = 320;
const THUMBNAIL_CACHE_VERSION: &[u8] = b"picasa-thumb-v4-heif-orientation";
const RAW_THUMBNAIL_CACHE_VERSION: &[u8] = b"picasa-thumb-v6-generic-raw";
const REMOTE_NEF_THUMBNAIL_CACHE_VERSION: &[u8] = b"picasa-thumb-v1-remote-nef-preview";
const REMOTE_JPEG_THUMBNAIL_CACHE_VERSION: &[u8] = b"picasa-thumb-v1-remote-jpeg-orientation";
const DNG_THUMBNAIL_CACHE_VERSION: &[u8] = b"picasa-thumb-v7-dng-full-raw";

// A folder import, startup recovery, and a manual refresh can overlap their
// thumbnail passes. Keep cache-key ownership separate from the filesystem
// existence check so two workers cannot generate the same preview together.
static IN_FLIGHT: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();

type PriorityRequest = (String, Option<i64>, Option<i64>, PathBuf, std::time::Instant);

type PriorityQueue = (Mutex<VecDeque<PriorityRequest>>, Condvar);

static PRIORITY_QUEUE: OnceLock<Arc<PriorityQueue>> = OnceLock::new();
static PRIORITY_PENDING: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
static PRIORITY_COMPLETIONS: OnceLock<Mutex<Vec<PathBuf>>> = OnceLock::new();
static PRIORITY_DISPATCHES: AtomicUsize = AtomicUsize::new(0);

const PRIORITY_QUEUE_CAPACITY: usize = 512;
// Visible requests must not queue behind the single bulk RAW worker. Keep a
// small dedicated pool so several newly visible tiles can make progress while
// background generation continues; cache-key deduplication still prevents
// duplicate generation.
const PRIORITY_WORKERS: usize = 4;
const PRIORITY_NEWEST_DISPATCHES: usize = 7;
const PRIORITY_HANDOFF_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

fn cache_entry_in_flight(destination: &Path) -> bool {
    IN_FLIGHT
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .map(|in_flight| in_flight.contains(destination))
        .unwrap_or(false)
}

struct PendingGuard(PathBuf);

impl Drop for PendingGuard {
    fn drop(&mut self) {
        if let Ok(mut pending) = PRIORITY_PENDING
            .get_or_init(|| Mutex::new(HashSet::new()))
            .lock()
        {
            pending.remove(&self.0);
        }
    }
}

/// Ask the dedicated foreground thumbnail worker to create a thumbnail for a
/// tile that is currently being bound. The request is best-effort and
/// deduplicated; the regular bulk recovery/import pass remains untouched.
pub fn request_priority(path: String, mtime: Option<i64>, size_bytes: Option<i64>) {
    let Ok(destination) = cache_path(&path, mtime, size_bytes) else {
        return;
    };
    if existing_cache_path(&path, mtime, size_bytes)
        .ok()
        .flatten()
        .is_some()
    {
        return;
    }
    if known_decode_failure(&path, &destination) {
        return;
    }
    if !crate::source::cached_file_available(&path) {
        if std::env::var_os("PICASA_TRACE").is_some() {
            eprintln!("PIC_THUMBNAIL skip reason=unavailable uri={path}");
        }
        return;
    }

    let pending = PRIORITY_PENDING.get_or_init(|| Mutex::new(HashSet::new()));
    let Ok(mut pending) = pending.lock() else {
        return;
    };
    if !pending.insert(destination.clone()) {
        return;
    }
    drop(pending);
    if std::env::var_os("PICASA_TRACE").is_some() {
        eprintln!(
            "PIC_THUMBNAIL schedule source=visible uri={path} cache={}",
            destination.display()
        );
    }

    let queue = PRIORITY_QUEUE.get_or_init(|| {
        let queue = Arc::new((
            Mutex::new(VecDeque::<PriorityRequest>::new()),
            Condvar::new(),
        ));
        for _ in 0..PRIORITY_WORKERS {
            let queue = queue.clone();
            std::thread::spawn(move || loop {
                let (path, mtime, size_bytes, destination, queued_at) = {
                    let (queue, wake) = &*queue;
                    let mut queue = queue.lock().expect("priority queue should not be poisoned");
                    while queue.is_empty() {
                        queue = wake
                            .wait(queue)
                            .expect("priority queue should not be poisoned");
                    }
                    let dispatch = PRIORITY_DISPATCHES.fetch_add(1, Ordering::Relaxed);
                    if dispatch % (PRIORITY_NEWEST_DISPATCHES + 1) == PRIORITY_NEWEST_DISPATCHES {
                        queue.pop_back().expect("priority queue was checked above")
                    } else {
                        queue.pop_front().expect("priority queue was checked above")
                    }
                };
                if std::env::var_os("PICASA_TRACE").is_some() { eprintln!("PIC_THUMBNAIL queue_wait kind=visible elapsed_us={}", queued_at.elapsed().as_micros()); }
                let started = std::time::Instant::now();
                let failure_marker = destination.with_extension("failed");
                let _pending_guard = PendingGuard(destination.clone());
                let deadline = started + PRIORITY_HANDOFF_TIMEOUT;
                while !destination.is_file() && !known_decode_failure(&path, &destination) {
                    if cache_entry_in_flight(&destination) {
                        if std::time::Instant::now() >= deadline {
                            break;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(25));
                        continue;
                    }
                    if create(&path, mtime, size_bytes).is_err() {
                        break;
                    }
                    if destination.is_file()
                        || failure_marker.is_file()
                        || std::time::Instant::now() >= deadline
                    {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                if destination.is_file() {
                    if let Ok(mut completions) = PRIORITY_COMPLETIONS
                        .get_or_init(|| Mutex::new(Vec::new()))
                        .lock()
                    {
                        completions.push(PathBuf::from(&path));
                    }
                }
            });
        }
        queue
    });

    let (queue, wake) = &**queue;
    let mut queue = queue.lock().expect("priority queue should not be poisoned");
    if queue.len() >= PRIORITY_QUEUE_CAPACITY {
        if let Some((_, _, _, evicted, _)) = queue.pop_back() {
            if let Ok(mut pending) = PRIORITY_PENDING
                .get_or_init(|| Mutex::new(HashSet::new()))
                .lock()
            {
                pending.remove(&evicted);
            }
        }
    }
    queue.push_front((path, mtime, size_bytes, destination.clone(), std::time::Instant::now()));
    wake.notify_one();
}

/// Return the source paths whose foreground thumbnails finished since the last UI poll.
pub fn take_priority_completions() -> Vec<PathBuf> {
    PRIORITY_COMPLETIONS
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .map(|mut completions| std::mem::take(&mut *completions))
        .unwrap_or_default()
}

pub fn priority_pending_count() -> usize {
    PRIORITY_PENDING
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .map(|pending| pending.len())
        .unwrap_or_default()
}

/// Keep bulk thumbnail work from competing with thumbnails currently needed
/// by visible gallery tiles. Foreground workers do not call this gate.
pub fn wait_for_priority_requests() {
    while priority_pending_count() > 0 {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

// Structural split only: included files remain in this module scope.
include!("thumbnail/cache.rs");
include!("thumbnail/maintenance.rs");
include!("thumbnail/recovery.rs");
include!("thumbnail/viewer.rs");
include!("thumbnail/nef.rs");
include!("thumbnail/decoders.rs");
include!("thumbnail/batch.rs");
