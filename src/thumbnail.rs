use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::io::{BufReader, Cursor, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Instant;

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
const DNG_THUMBNAIL_CACHE_VERSION: &[u8] = b"picasa-thumb-v7-dng-full-raw";

macro_rules! thumb_trace {
    ($($arg:tt)*) => {
        if false {
            eprintln!($($arg)*);
        }
    };
}

// A folder import, startup recovery, and a manual refresh can overlap their
// thumbnail passes. Keep cache-key ownership separate from the filesystem
// existence check so two workers cannot generate the same preview together.
static IN_FLIGHT: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();

type PriorityRequest = (String, Option<i64>, Option<i64>, PathBuf);

type PriorityQueue = (Mutex<VecDeque<PriorityRequest>>, Condvar);

static PRIORITY_QUEUE: OnceLock<Arc<PriorityQueue>> = OnceLock::new();
static PRIORITY_PENDING: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
static PRIORITY_COMPLETIONS: OnceLock<Mutex<Vec<PathBuf>>> = OnceLock::new();
static PRIORITY_DISPATCHES: AtomicUsize = AtomicUsize::new(0);

const PRIORITY_QUEUE_CAPACITY: usize = 512;
const PRIORITY_WORKERS: usize = 2;
const PRIORITY_NEWEST_DISPATCHES: usize = 7;
const PRIORITY_HANDOFF_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

fn priority_trace(event: &str, path: &str, destination: &Path) {
    if std::env::var_os("PICASA_TRACE").is_none() {
        return;
    }
    // Per-photo skip traces are emitted from the GTK main thread whenever a
    // settled viewport tile probes a missing/known-bad original. A single
    // scroll pass can emit thousands of lines, and synchronous stderr writes
    // then become part of the scroll workload. Keep them behind the verbose
    // opt-in; queue/queue_len summaries remain on the normal trace flag.
    match event {
        "skip_known_bad" | "skip_existing" | "skip_offline" | "skip_pending" => {
            if std::env::var_os("PICASA_TRACE_VERBOSE").is_none() {
                return;
            }
        }
        _ => {}
    }
    eprintln!(
        "THUMB PRIORITY {event} path={path} cache={}",
        destination.display()
    );
}

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
    if destination.is_file() {
        priority_trace("skip_existing", &path, &destination);
        return;
    }
    if known_decode_failure(&path, &destination) {
        priority_trace("skip_known_bad", &path, &destination);
        return;
    }
    if !crate::source::cached_file_available(&path) {
        priority_trace("skip_offline", &path, &destination);
        return;
    }

    let pending = PRIORITY_PENDING.get_or_init(|| Mutex::new(HashSet::new()));
    let Ok(mut pending) = pending.lock() else {
        return;
    };
    if !pending.insert(destination.clone()) {
        priority_trace("skip_pending", &path, &destination);
        return;
    }
    drop(pending);

    let queue = PRIORITY_QUEUE.get_or_init(|| {
        let queue = Arc::new((Mutex::new(VecDeque::<PriorityRequest>::new()), Condvar::new()));
        for worker_id in 0..PRIORITY_WORKERS {
            let queue = queue.clone();
            std::thread::spawn(move || loop {
                let (path, mtime, size_bytes, destination) = {
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
                let started = std::time::Instant::now();
                priority_trace("worker_start", &path, &destination);
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
                    if destination.is_file() || failure_marker.is_file()
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
                priority_trace("worker_done", &path, &destination);
                if std::env::var_os("PICASA_TRACE").is_some() {
                    eprintln!(
                        "THUMB PRIORITY worker_done_detail worker={} cache_exists={} failed_marker={} elapsed_ms={}",
                        worker_id,
                        destination.is_file(),
                        failure_marker.is_file(),
                        started.elapsed().as_millis()
                    );
                }
            });
        }
        queue
    });

    let (queue, wake) = &**queue;
    let mut queue = queue.lock().expect("priority queue should not be poisoned");
    if queue.len() >= PRIORITY_QUEUE_CAPACITY {
        if let Some((_, _, _, evicted)) = queue.pop_back() {
            if let Ok(mut pending) = PRIORITY_PENDING
                .get_or_init(|| Mutex::new(HashSet::new()))
                .lock()
            {
                pending.remove(&evicted);
            }
            priority_trace("evicted_oldest", "", &evicted);
        }
    }
    let queued = queue.len() + 1;
    priority_trace("queued_front", &path, &destination);
    queue.push_front((path, mtime, size_bytes, destination.clone()));
    wake.notify_one();
    if std::env::var_os("PICASA_TRACE").is_some() {
        eprintln!("THUMB PRIORITY queue_len={queued}");
    }
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
include!("thumbnail/recovery.rs");
include!("thumbnail/viewer.rs");
include!("thumbnail/nef.rs");
include!("thumbnail/decoders.rs");
include!("thumbnail/batch.rs");
