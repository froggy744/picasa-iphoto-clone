use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use gio::prelude::*;

static AVAILABILITY_CACHE: OnceLock<Mutex<HashMap<String, bool>>> = OnceLock::new();
static FOLDER_AVAILABILITY: OnceLock<Mutex<HashMap<i64, bool>>> = OnceLock::new();

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ViewerReadLane {
    Foreground,
    Prefetch,
}

#[derive(Clone)]
pub struct ViewerReadContext {
    lane: Arc<dyn Fn() -> ViewerReadLane + Send + Sync>,
    cancelled: Arc<dyn Fn() -> bool + Send + Sync>,
}

impl ViewerReadContext {
    pub fn new<L, C>(lane: L, cancelled: C) -> Self
    where
        L: Fn() -> ViewerReadLane + Send + Sync + 'static,
        C: Fn() -> bool + Send + Sync + 'static,
    {
        Self {
            lane: Arc::new(lane),
            cancelled: Arc::new(cancelled),
        }
    }

    fn lane(&self) -> ViewerReadLane {
        (self.lane)()
    }

    fn cancelled(&self) -> bool {
        (self.cancelled)()
    }
}

struct ReadWaiter {
    id: u64,
    context: ViewerReadContext,
}

#[derive(Default)]
struct PriorityReadState {
    active: bool,
    next_id: u64,
    waiters: Vec<ReadWaiter>,
}

struct PriorityReadGate {
    state: Mutex<PriorityReadState>,
    changed: Condvar,
}

impl PriorityReadGate {
    fn new() -> Self {
        Self {
            state: Mutex::new(PriorityReadState::default()),
            changed: Condvar::new(),
        }
    }

    fn acquire(&self, context: &ViewerReadContext) -> Option<PriorityReadPermit<'_>> {
        if context.cancelled() {
            return None;
        }
        let queued_at = Instant::now();
        let mut state = self.state.lock().unwrap();
        let id = state.next_id;
        state.next_id = state.next_id.wrapping_add(1);
        state.waiters.push(ReadWaiter {
            id,
            context: context.clone(),
        });

        loop {
            state.waiters.retain(|waiter| !waiter.context.cancelled());
            if context.cancelled() {
                state.waiters.retain(|waiter| waiter.id != id);
                self.changed.notify_all();
                return None;
            }
            let next = state
                .waiters
                .iter()
                .min_by_key(|waiter| {
                    let priority = match waiter.context.lane() {
                        ViewerReadLane::Foreground => 0,
                        ViewerReadLane::Prefetch => 1,
                    };
                    (priority, waiter.id)
                })
                .map(|waiter| waiter.id);
            if !state.active && next == Some(id) {
                state.active = true;
                state.waiters.retain(|waiter| waiter.id != id);
                return Some(PriorityReadPermit {
                    gate: self,
                    wait_ms: queued_at.elapsed().as_millis(),
                });
            }
            let (next_state, _) = self
                .changed
                .wait_timeout(state, Duration::from_millis(5))
                .unwrap();
            state = next_state;
        }
    }

    fn run<T>(&self, context: &ViewerReadContext, operation: impl FnOnce() -> T) -> Option<T> {
        let _permit = self.acquire(context)?;
        Some(operation())
    }

    #[cfg(test)]
    fn waiting_count(&self) -> usize {
        self.state.lock().unwrap().waiters.len()
    }
}

struct PriorityReadPermit<'a> {
    gate: &'a PriorityReadGate,
    wait_ms: u128,
}

impl Drop for PriorityReadPermit<'_> {
    fn drop(&mut self) {
        let mut state = self.gate.state.lock().unwrap();
        state.active = false;
        self.gate.changed.notify_all();
    }
}

static SMB_VIEWER_READ_GATE: OnceLock<PriorityReadGate> = OnceLock::new();
static NFS_VIEWER_READ_GATE: OnceLock<PriorityReadGate> = OnceLock::new();

fn availability_cache() -> &'static Mutex<HashMap<String, bool>> {
    AVAILABILITY_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn folder_availability() -> &'static Mutex<HashMap<i64, bool>> {
    FOLDER_AVAILABILITY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Replace the current imported-source state. This is intentionally keyed by
/// folder, never by photo: every photo in a folder inherits one source result.
pub fn replace_folder_availability(availability: HashMap<i64, bool>) {
    *folder_availability().lock().unwrap() = availability;
}

/// A photo without a registered folder keeps the historical online default.
/// Registered photos are online only when their imported source is available.
pub fn folder_available(folder_id: Option<i64>) -> bool {
    folder_id
        .and_then(|id| folder_availability().lock().unwrap().get(&id).copied())
        .unwrap_or(true)
}

fn query_exists(reference: &str, directory: bool) -> bool {
    if !reference.contains("://") {
        return if directory {
            Path::new(reference).is_dir()
        } else {
            Path::new(reference).is_file()
        };
    }
    #[cfg(target_os = "linux")]
    if crate::network_shares::private(reference) {
        return crate::network_shares::stat(reference)
            .map(|metadata| metadata.is_dir == directory).unwrap_or(false);
    }
    file(reference).query_exists(gio::Cancellable::NONE)
}

/// Probe again after a reconnect, without retaining a previous offline result.
pub fn file_available(reference: &str) -> bool {
    query_exists(reference, false)
}

pub fn cached_source_available(reference: &str) -> bool {
    let key = format!("source:{reference}");
    #[cfg(target_os = "linux")]
    if crate::network_shares::private(reference) {
        // Never block GTK to probe DietPi; explicit import/refresh workers probe.
        return availability_cache().lock().unwrap().get(&key).copied().unwrap_or(true);
    }
    let mut cache = availability_cache().lock().unwrap();
    *cache
        .entry(key)
        .or_insert_with(|| query_exists(reference, true))
}

pub fn cached_file_available(reference: &str) -> bool {
    // Local paths are cheap to check and can change when a removable drive is
    // mounted or unmounted, so do not retain a stale result for them.
    if !reference.contains("://") {
        return Path::new(reference).is_file();
    }
    let key = format!("file:{reference}");
    #[cfg(target_os = "linux")]
    if crate::network_shares::private(reference) {
        return availability_cache().lock().unwrap().get(&key).copied().unwrap_or(true);
    }
    let mut cache = availability_cache().lock().unwrap();
    *cache
        .entry(key)
        .or_insert_with(|| query_exists(reference, false))
}

pub fn refresh_availability() {
    availability_cache().lock().unwrap().clear();
}

/// Probe only imported network roots off the GTK thread, then populate the
/// per-root cache used by db::folders() and all descendants' offline badges.
#[cfg(target_os="linux")]
pub fn probe_network_roots(roots: &[String]) {
    let probes=roots.iter().filter(|root|crate::network_shares::private(root))
        .map(|root|(format!("source:{root}"),
            crate::network_shares::stat(root).map(|stat|stat.is_dir).unwrap_or(false)))
        .collect::<Vec<_>>();
    let mut cache=availability_cache().lock().unwrap();
    for (key,available) in probes {cache.insert(key,available);}
}

/// A stored location is either a local path or a GIO URI (such as nfs://...).
pub fn file(reference: &str) -> gio::File {
    if reference.contains("://") {
        gio::File::for_uri(reference)
    } else {
        gio::File::for_path(reference)
    }
}

pub fn reference(file: &gio::File) -> String {
    file.path()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|| file.uri().to_string())
}

pub fn filename(reference: &str) -> String {
    file(reference)
        .basename()
        .map(|name| name.to_string_lossy().into_owned())
        .or_else(|| {
            Path::new(reference)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| reference.to_string())
}

/// Never persist a network original in the thumbnail cache.  A RAW decoder
/// that requires a native path must use an explicitly scoped, on-demand
/// temporary file; silently copying the entire source into `cache/source/`
/// is prohibited.  Local files retain their original paths.
pub fn materialize(reference: &str) -> Result<PathBuf> {
    if !reference.contains("://") {
        return Ok(PathBuf::from(reference));
    }
    anyhow::bail!(
        "Network original cannot be materialized into the source cache: {reference}; \
         use on-demand remote reading or an explicitly scoped temporary RAW decode"
    )
}

pub fn read(reference: &str) -> Result<Vec<u8>> {
    #[cfg(target_os = "linux")]
    if crate::network_shares::private(reference) {
        return crate::network_shares::read(reference);
    }
    let (contents, _) = file(reference)
        .load_contents(gio::Cancellable::NONE)
        .with_context(|| format!("could not read {reference}"))?;
    Ok(contents.as_ref().to_vec())
}

fn trace_reference(reference: &str) -> String {
    let Some((scheme, rest)) = reference.split_once("://") else {
        return reference.to_owned();
    };
    let safe_rest = rest.rsplit_once('@').map(|(_, value)| value).unwrap_or(rest);
    format!("{scheme}://{safe_rest}")
}

/// Read an original for the lightbox. Direct SMB and NFS already serialize
/// their process-wide sessions; this matching Rust-side gate makes that queue
/// foreground-aware and removes cancelled waiters before they enter FFI.
pub fn read_for_viewer(reference: &str, context: &ViewerReadContext) -> Result<Vec<u8>> {
    if context.cancelled() {
        anyhow::bail!("cancelled before source read");
    }
    #[cfg(target_os = "linux")]
    if crate::network_shares::private(reference) {
        let gate = if reference.starts_with("smb://") {
            SMB_VIEWER_READ_GATE.get_or_init(PriorityReadGate::new)
        } else {
            NFS_VIEWER_READ_GATE.get_or_init(PriorityReadGate::new)
        };
        let permit = gate
            .acquire(context)
            .ok_or_else(|| anyhow::anyhow!("cancelled while waiting for network read slot"))?;
        if std::env::var_os("PICASA_TRACE").is_some() {
            eprintln!(
                "PIC_VIEWER network_slot lane={} wait_ms={} uri={}",
                match context.lane() {
                    ViewerReadLane::Foreground => "foreground",
                    ViewerReadLane::Prefetch => "prefetch",
                },
                permit.wait_ms,
                trace_reference(reference),
            );
        }
        return crate::network_shares::read(reference);
    }
    read(reference)
}

#[cfg(test)]
mod viewer_read_tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    };
    use std::time::{Duration, Instant};

    fn context(lane: ViewerReadLane, cancelled: Arc<AtomicBool>) -> ViewerReadContext {
        ViewerReadContext::new(
            move || lane,
            move || cancelled.load(Ordering::Acquire),
        )
    }

    fn wait_for_waiters(gate: &PriorityReadGate, expected: usize) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while gate.waiting_count() < expected {
            assert!(Instant::now() < deadline, "waiter did not reach read gate");
            std::thread::yield_now();
        }
    }

    #[test]
    fn foreground_overtakes_waiting_prefetch() {
        let gate = Arc::new(PriorityReadGate::new());
        let (entered_send, entered_receive) = mpsc::channel();
        let (release_send, release_receive) = mpsc::channel();

        let active_gate = gate.clone();
        let active = std::thread::spawn(move || {
            let context = context(ViewerReadLane::Prefetch, Arc::new(AtomicBool::new(false)));
            active_gate
                .run(&context, || {
                    entered_send.send("active").unwrap();
                    release_receive.recv().unwrap();
                    "active"
                })
                .unwrap()
        });
        assert_eq!(entered_receive.recv().unwrap(), "active");

        let (order_send, order_receive) = mpsc::channel();
        let prefetch_gate = gate.clone();
        let prefetch_send = order_send.clone();
        let prefetch = std::thread::spawn(move || {
            let context = context(ViewerReadLane::Prefetch, Arc::new(AtomicBool::new(false)));
            prefetch_gate.run(&context, || prefetch_send.send("prefetch").unwrap())
        });
        wait_for_waiters(&gate, 1);

        let foreground_gate = gate.clone();
        let foreground = std::thread::spawn(move || {
            let context = context(ViewerReadLane::Foreground, Arc::new(AtomicBool::new(false)));
            foreground_gate.run(&context, || order_send.send("foreground").unwrap())
        });
        wait_for_waiters(&gate, 2);
        release_send.send(()).unwrap();

        assert_eq!(order_receive.recv().unwrap(), "foreground");
        assert_eq!(order_receive.recv().unwrap(), "prefetch");
        assert_eq!(active.join().unwrap(), "active");
        assert!(foreground.join().unwrap().is_some());
        assert!(prefetch.join().unwrap().is_some());
    }

    #[test]
    fn cancelled_waiter_never_calls_reader() {
        let gate = Arc::new(PriorityReadGate::new());
        let (entered_send, entered_receive) = mpsc::channel();
        let (release_send, release_receive) = mpsc::channel();
        let active_gate = gate.clone();
        let active = std::thread::spawn(move || {
            let context = context(ViewerReadLane::Foreground, Arc::new(AtomicBool::new(false)));
            active_gate.run(&context, || {
                entered_send.send(()).unwrap();
                release_receive.recv().unwrap();
            })
        });
        entered_receive.recv().unwrap();

        let cancelled = Arc::new(AtomicBool::new(false));
        let reader_called = Arc::new(AtomicBool::new(false));
        let waiting_gate = gate.clone();
        let waiting_cancelled = cancelled.clone();
        let called = reader_called.clone();
        let waiting = std::thread::spawn(move || {
            let context = context(ViewerReadLane::Prefetch, waiting_cancelled);
            waiting_gate.run(&context, || called.store(true, Ordering::Release))
        });
        wait_for_waiters(&gate, 1);
        cancelled.store(true, Ordering::Release);
        release_send.send(()).unwrap();

        assert!(active.join().unwrap().is_some());
        assert!(waiting.join().unwrap().is_none());
        assert!(!reader_called.load(Ordering::Acquire));
    }
}
