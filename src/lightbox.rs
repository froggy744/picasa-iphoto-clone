use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Condvar, Mutex, OnceLock,
};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use gtk::prelude::*;
use gtk4 as gtk;

use crate::photo_object::PhotoObject;

/// A cooperative decode used to be gated behind a single-permit Mutex, which
/// serialized every navigation: a new photo's decode thread would block on
/// the mutex until the *previous* photo's full decode finished, even though
/// that previous result was already stale and about to be discarded. That
/// made rapid navigation (arrow keys / scroll) as slow as decoding every
/// skipped photo in sequence.
///
/// A small counting semaphore keeps a handful of decodes in flight at once
/// (so we still don't spawn unbounded full-resolution RAW decodes if someone
/// holds an arrow key down) without forcing them to run one at a time.
struct DecodeSemaphore {
    available: Mutex<usize>,
    condvar: Condvar,
}

impl DecodeSemaphore {
    fn new(permits: usize) -> Self {
        Self {
            available: Mutex::new(permits),
            condvar: Condvar::new(),
        }
    }

    fn acquire_cancelled(&self, cancelled: &AtomicBool) -> Option<DecodePermit<'_>> {
        let mut available = self.available.lock().unwrap();
        while *available == 0 {
            if cancelled.load(Ordering::Acquire) {
                return None;
            }
            let (next, _) = self
                .condvar
                .wait_timeout(available, Duration::from_millis(5))
                .unwrap();
            available = next;
        }
        if cancelled.load(Ordering::Acquire) {
            return None;
        }
        *available -= 1;
        Some(DecodePermit { semaphore: self })
    }
}

struct DecodePermit<'a> {
    semaphore: &'a DecodeSemaphore,
}

impl Drop for DecodePermit<'_> {
    fn drop(&mut self) {
        let mut available = self.semaphore.available.lock().unwrap();
        *available += 1;
        self.semaphore.condvar.notify_one();
    }
}

const MAX_CONCURRENT_VIEWER_DECODES: usize = 3;
const VIEWER_PADDING: i32 = 0;
static VIEWER_DECODE_GATE: OnceLock<DecodeSemaphore> = OnceLock::new();

type PhotoChangedHandler = Rc<RefCell<Option<Box<dyn Fn(PhotoObject)>>>>;
type OneToOneSyncHandler = Rc<RefCell<Option<Box<dyn Fn(bool)>>>>;
type ContextMenuHandler = Rc<RefCell<Option<Box<dyn Fn(PhotoObject, gtk::Widget, f64, f64)>>>>;
type CollectionNavigationHandler = Rc<RefCell<Option<Box<dyn Fn(i32)>>>>;

#[derive(Clone)]
struct NativeTextureCache {
    path: String,
    rotation: i32,
    edit_recipe: String,
    texture: gtk::gdk::MemoryTexture,
}

struct DisplayTextureCacheEntry {
    path: String,
    rotation: i32,
    edit_recipe: String,
    target_width: u32,
    target_height: u32,
    texture: gtk::gdk::MemoryTexture,
    // Approximate RGBA footprint, used by the byte budget below.
    bytes: usize,
    // True when this texture was produced by the directional prefetch rather
    // than the visible decode. A later hit on a prefetched entry is what makes
    // prefetch useful, so the two are counted separately.
    prefetched: bool,
}

type DisplayTextureCache = Rc<RefCell<VecDeque<DisplayTextureCacheEntry>>>;

// Full-size RAW/NEF viewer decodes cost hundreds of milliseconds. Keep enough
// recently viewed display textures in RAM that stepping back through a burst
// is served from the cache instead of decoding again. Counters added in Phase
// 3.1 showed capacity 8 evicted constantly during 20-photo forward/back runs.
const DISPLAY_TEXTURE_CACHE_CAPACITY: usize = 32;
// Texture memory scales with the viewport, so a pure count is unsafe on large
// displays: a 4K viewer texture is ~33 MB. Cap total cached pixels as well and
// evict least-recently-used entries past either limit. At the default window
// size a texture is ~3.5 MB, so the count limit binds first and the budget only
// protects large/zoomed viewports.
const DISPLAY_TEXTURE_CACHE_BYTE_BUDGET: usize = 256 * 1024 * 1024;

/// Phase 3 lightbox performance counters.
///
/// Global atomics rather than thread-local state because decode completion and
/// cancellation happen on worker threads, while cache hits/misses happen on the
/// GTK main thread. `take_lightbox_stats` drains the counters so the periodic
/// reporter shows activity only for the interval it covers.
struct LightboxStats {
    preview_hits: AtomicU64,
    cache_hits: AtomicU64,
    cache_misses: AtomicU64,
    cache_misses_raw: AtomicU64,
    cache_misses_edited: AtomicU64,
    decodes_queued: AtomicU64,
    decodes_completed: AtomicU64,
    decodes_failed: AtomicU64,
    decodes_cancelled: AtomicU64,
    prefetches_started: AtomicU64,
    prefetches_completed: AtomicU64,
    // Cache hits on entries that were produced by prefetch.
    prefetch_used: AtomicU64,
    evictions: AtomicU64,
    // Gauges, not counters: last observed cache occupancy.
    cache_size: AtomicU64,
    cache_bytes: AtomicU64,
}

static LIGHTBOX_STATS: LightboxStats = LightboxStats {
    preview_hits: AtomicU64::new(0),
    cache_hits: AtomicU64::new(0),
    cache_misses: AtomicU64::new(0),
    cache_misses_raw: AtomicU64::new(0),
    cache_misses_edited: AtomicU64::new(0),
    decodes_queued: AtomicU64::new(0),
    decodes_completed: AtomicU64::new(0),
    decodes_failed: AtomicU64::new(0),
    decodes_cancelled: AtomicU64::new(0),
    prefetches_started: AtomicU64::new(0),
    prefetches_completed: AtomicU64::new(0),
    prefetch_used: AtomicU64::new(0),
    evictions: AtomicU64::new(0),
    cache_size: AtomicU64::new(0),
    cache_bytes: AtomicU64::new(0),
};

fn lightbox_cache_size_observed(size: usize, bytes: usize) {
    LIGHTBOX_STATS
        .cache_size
        .store(size as u64, Ordering::Relaxed);
    LIGHTBOX_STATS
        .cache_bytes
        .store(bytes as u64, Ordering::Relaxed);
}

/// Drain and format lightbox performance counters. Returns `None` when nothing
/// happened in the interval, so the reporter stays quiet while idle.
pub(crate) fn take_lightbox_stats() -> Option<String> {
    let swap = |cell: &AtomicU64| cell.swap(0, Ordering::Relaxed);
    let preview_hits = swap(&LIGHTBOX_STATS.preview_hits);
    let cache_hits = swap(&LIGHTBOX_STATS.cache_hits);
    let cache_misses = swap(&LIGHTBOX_STATS.cache_misses);
    let cache_misses_raw = swap(&LIGHTBOX_STATS.cache_misses_raw);
    let cache_misses_edited = swap(&LIGHTBOX_STATS.cache_misses_edited);
    let decodes_queued = swap(&LIGHTBOX_STATS.decodes_queued);
    let decodes_completed = swap(&LIGHTBOX_STATS.decodes_completed);
    let decodes_failed = swap(&LIGHTBOX_STATS.decodes_failed);
    let decodes_cancelled = swap(&LIGHTBOX_STATS.decodes_cancelled);
    let prefetches_started = swap(&LIGHTBOX_STATS.prefetches_started);
    let prefetches_completed = swap(&LIGHTBOX_STATS.prefetches_completed);
    let prefetch_used = swap(&LIGHTBOX_STATS.prefetch_used);
    let evictions = swap(&LIGHTBOX_STATS.evictions);
    if preview_hits == 0
        && cache_hits == 0
        && cache_misses == 0
        && decodes_queued == 0
        && decodes_completed == 0
        && decodes_failed == 0
        && decodes_cancelled == 0
        && prefetches_started == 0
        && prefetches_completed == 0
        && prefetch_used == 0
        && evictions == 0
    {
        return None;
    }
    Some(format!(
        "preview_hits={preview_hits} cache_hits={cache_hits} cache_misses={cache_misses} \
         miss_raw={cache_misses_raw} miss_edited={cache_misses_edited} \
         queued={decodes_queued} completed={decodes_completed} failed={decodes_failed} \
         cancelled={decodes_cancelled} prefetch_started={prefetches_started} \
         prefetch_done={prefetches_completed} prefetch_used={prefetch_used} \
         evictions={evictions} cache_size={} cache_mb={} capacity={} budget_mb={}",
        LIGHTBOX_STATS.cache_size.load(Ordering::Relaxed),
        LIGHTBOX_STATS.cache_bytes.load(Ordering::Relaxed) / (1024 * 1024),
        DISPLAY_TEXTURE_CACHE_CAPACITY,
        DISPLAY_TEXTURE_CACHE_BYTE_BUDGET / (1024 * 1024)
    ))
}

struct ResultSlot<T> {
    value: Mutex<Option<T>>,
    waker: Mutex<Option<Waker>>,
}

impl<T> ResultSlot<T> {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            value: Mutex::new(None),
            waker: Mutex::new(None),
        })
    }

    fn send(&self, value: T) {
        *self.value.lock().unwrap() = Some(value);
        if let Some(waker) = self.waker.lock().unwrap().take() {
            waker.wake();
        }
    }

    fn wait(slot: Arc<Self>) -> ResultSlotFuture<T> {
        ResultSlotFuture { slot }
    }
}

struct ResultSlotFuture<T> {
    slot: Arc<ResultSlot<T>>,
}

impl<T> Future for ResultSlotFuture<T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        if let Some(value) = self.slot.value.lock().unwrap().take() {
            return Poll::Ready(value);
        }
        *self.slot.waker.lock().unwrap() = Some(context.waker().clone());
        if let Some(value) = self.slot.value.lock().unwrap().take() {
            *self.slot.waker.lock().unwrap() = None;
            Poll::Ready(value)
        } else {
            Poll::Pending
        }
    }
}

pub struct Lightbox {
    pub root: gtk::Overlay,
    backdrop: gtk::Box,
    picture: gtk::Picture,
    picture_viewport: gtk::ScrolledWindow,
    photos: Rc<RefCell<Vec<PhotoObject>>>,
    index: Rc<Cell<usize>>,
    last_width: Rc<Cell<i32>>,
    last_height: Rc<Cell<i32>>,
    zoom: Rc<Cell<f64>>,
    zoom_before_one_to_one: Rc<Cell<f64>>,
    one_to_one_active: Rc<Cell<bool>>,
    native_texture: Rc<RefCell<Option<NativeTextureCache>>>,
    display_texture_cache: DisplayTextureCache,
    load_generation: Rc<Cell<u64>>,
    decode_cancel: Rc<RefCell<Option<Arc<AtomicBool>>>>,
    photo_changed: PhotoChangedHandler,
    // Keeps the toolbar 1:1 toggle in sync when the lightbox changes the mode
    // itself (for example Ctrl+wheel leaves 1:1 for a manual zoom).
    one_to_one_sync: OneToOneSyncHandler,
    context_menu: ContextMenuHandler,
    collection_navigation: CollectionNavigationHandler,
}

// Structural split only: included files remain in this module scope.
include!("lightbox/impl.rs");
include!("lightbox/render.rs");
include!("lightbox/tests.rs");
