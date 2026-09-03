use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::{
    atomic::{AtomicBool, Ordering},
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
type ContextMenuHandler = Rc<RefCell<Option<Box<dyn Fn(PhotoObject, gtk::Widget, f64, f64)>>>>;
type CollectionNavigationHandler = Rc<RefCell<Option<Box<dyn Fn(i32)>>>>;

#[derive(Clone)]
struct NativeTextureCache {
    path: String,
    rotation: i32,
    texture: gtk::gdk::MemoryTexture,
}

struct DisplayTextureCacheEntry {
    path: String,
    rotation: i32,
    target_width: u32,
    target_height: u32,
    texture: gtk::gdk::MemoryTexture,
}

type DisplayTextureCache = Rc<RefCell<VecDeque<DisplayTextureCacheEntry>>>;

const DISPLAY_TEXTURE_CACHE_CAPACITY: usize = 8;

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
    context_menu: ContextMenuHandler,
    collection_navigation: CollectionNavigationHandler,
}

// Structural split only: included files remain in this module scope.
include!("lightbox/impl.rs");
include!("lightbox/render.rs");
include!("lightbox/tests.rs");
