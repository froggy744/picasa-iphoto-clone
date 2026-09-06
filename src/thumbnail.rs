use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufReader, Cursor, Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::mpsc::{sync_channel, SyncSender, TrySendError};
use std::sync::{Mutex, OnceLock};
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
const RAW_THUMBNAIL_CACHE_VERSION: &[u8] = b"picasa-thumb-v5-raw-preview";

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

static PRIORITY_SENDER: OnceLock<SyncSender<PriorityRequest>> = OnceLock::new();
static PRIORITY_PENDING: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
static PRIORITY_ATTEMPTED: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
static PRIORITY_COMPLETIONS: OnceLock<Mutex<Vec<PathBuf>>> = OnceLock::new();

/// Ask the dedicated foreground thumbnail worker to create a thumbnail for a
/// tile that is currently being bound. The request is best-effort and
/// deduplicated; the regular bulk recovery/import pass remains untouched.
pub fn request_priority(path: String, mtime: Option<i64>, size_bytes: Option<i64>) {
    let Ok(destination) = cache_path(&path, mtime, size_bytes) else {
        return;
    };
    if destination.is_file() {
        return;
    }

    let pending = PRIORITY_PENDING.get_or_init(|| Mutex::new(HashSet::new()));
    let Ok(mut pending) = pending.lock() else {
        return;
    };
    if !pending.insert(destination.clone()) {
        return;
    }
    if let Ok(mut attempted) = PRIORITY_ATTEMPTED
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
    {
        if !attempted.insert(destination.clone()) {
            pending.remove(&destination);
            return;
        }
    }
    drop(pending);

    let sender = PRIORITY_SENDER.get_or_init(|| {
        let (sender, receiver) = sync_channel::<PriorityRequest>(256);
        std::thread::spawn(move || {
            while let Ok((path, mtime, size_bytes, destination)) = receiver.recv() {
                // A previous run may have recorded a transient failure. A
                // visible request gets one fresh attempt in this process.
                let failure_marker = destination.with_extension("failed");
                let _ = std::fs::remove_file(&failure_marker);
                for _ in 0..20 {
                    let _ = create(&path, mtime, size_bytes);
                    if destination.is_file() || failure_marker.is_file() {
                        break;
                    }
                    // If the bulk recovery worker already owns this cache
                    // entry, create() returns without doing work. Give it a
                    // short chance to finish instead of dropping the visible
                    // request as a false success.
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                if let Ok(mut pending) = PRIORITY_PENDING
                    .get_or_init(|| Mutex::new(HashSet::new()))
                    .lock()
                {
                    pending.remove(&destination);
                }
                if destination.is_file() {
                    if let Ok(mut completions) = PRIORITY_COMPLETIONS
                        .get_or_init(|| Mutex::new(Vec::new()))
                        .lock()
                    {
                        completions.push(PathBuf::from(path));
                    }
                }
            }
        });
        sender
    });

    if let Err(error) = sender.try_send((path, mtime, size_bytes, destination.clone())) {
        if let Ok(mut pending) = PRIORITY_PENDING
            .get_or_init(|| Mutex::new(HashSet::new()))
            .lock()
        {
            pending.remove(&destination);
        }
        if let Ok(mut attempted) = PRIORITY_ATTEMPTED
            .get_or_init(|| Mutex::new(HashSet::new()))
            .lock()
        {
            attempted.remove(&destination);
        }
        if !matches!(error, TrySendError::Full(_)) {
            return;
        }
    }
}

/// Return the number of foreground completions since the last UI poll.
pub fn take_priority_completions() -> usize {
    PRIORITY_COMPLETIONS
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .map(|mut completions| {
            let count = completions.len();
            completions.clear();
            count
        })
        .unwrap_or_default()
}

pub fn priority_pending_count() -> usize {
    PRIORITY_PENDING
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .map(|pending| pending.len())
        .unwrap_or_default()
}

// Structural split only: included files remain in this module scope.
include!("thumbnail/cache.rs");
include!("thumbnail/viewer.rs");
include!("thumbnail/nef.rs");
include!("thumbnail/decoders.rs");
include!("thumbnail/batch.rs");
