use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use anyhow::{Context, Result};
use gio::prelude::*;
use gtk4 as gtk;

static AVAILABILITY_CACHE: OnceLock<Mutex<HashMap<String, bool>>> = OnceLock::new();
static FOLDER_AVAILABILITY: OnceLock<Mutex<HashMap<i64, bool>>> = OnceLock::new();

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

fn query_exists(reference: &str, directory: bool, lane: crate::smb_transport::SmbLane) -> bool {
    // Legacy gvfs-FUSE paths normalize to smb:// URIs (see `read`).
    let reference = crate::smb_transport::normalize_smb_reference(reference);
    if !reference.contains("://") {
        return if directory {
            Path::new(&reference).is_dir()
        } else {
            Path::new(&reference).is_file()
        };
    }
    if reference.starts_with("smb://") {
        // Direct SMB probe through libsmbclient (worker threads only - this
        // blocks up to the transport timeout). The lane keeps availability
        // probes from delaying photo reads. NFS stays on gvfs. Without
        // libsmbclient (sandboxed flatpak) the probe rides gvfs instead.
        if crate::smb_transport::direct_available() {
            return crate::smb_transport::stat_in_lane(&reference, lane).is_ok();
        }
        return uri_query_exists(&file(&normalize_nfs_uri(&reference)));
    }
    if reference.starts_with("nfs://") {
        // NFS availability is a plain TCP probe of the server's NFS port.
        // A gvfs existence probe would answer "no" for an unmounted export,
        // and forcing the mount just to answer a probe would put the share
        // in Nautilus; PIC never mounts NFS for probing.
        return nfs_online(&reference);
    }
    uri_query_exists(&file(&normalize_nfs_uri(&reference)))
}

// --- NFS gvfs mounts: on demand only, never persistent ---------------------
// NFS has no direct transport, so photo reads ride gvfs and need an explicit
// mount. A gvfs mount is session-wide and would otherwise sit in Nautilus
// forever, so PIC mounts an export on demand when a read needs it, and
// unmounts it again once no NFS read happened for a few minutes. Shares show
// as online through a plain TCP probe (nfs_online) without any mount.

/// How long an NFS export stays mounted after its last use.
const NFS_IDLE_UNMOUNT_AFTER: std::time::Duration = std::time::Duration::from_secs(300);

static NFS_LAST_ACTIVITY: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();
static NFS_UNMOUNT_IN_FLIGHT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn nfs_last_activity() -> &'static Mutex<Option<Instant>> {
    NFS_LAST_ACTIVITY.get_or_init(|| Mutex::new(None))
}

/// Record that an NFS location was just used, so the idle unmount leaves
/// freshly used shares alone.
pub fn nfs_note_activity() {
    if let Ok(mut last) = nfs_last_activity().lock() {
        *last = Some(Instant::now());
    }
}

/// NFS gvfs mounts PIC itself created (Add-share connect, Retry Connection,
/// on-demand reads). These are reclaimed by the idle unmount even when the
/// location was never registered. The explicit "Open in file manager" flow
/// deliberately stays outside this set: its mount exists FOR Nautilus.
static NFS_PIC_MOUNTS: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();

fn nfs_pic_mounts() -> &'static Mutex<std::collections::HashSet<String>> {
    NFS_PIC_MOUNTS.get_or_init(|| Mutex::new(std::collections::HashSet::new()))
}

fn nfs_note_pic_mount(reference: &str) {
    if !reference.starts_with("nfs://") {
        return;
    }
    nfs_note_activity();
    if let Ok(mut mounts) = nfs_pic_mounts().lock() {
        mounts.insert(normalize_nfs_uri(reference));
    }
}

fn nfs_forget_pic_mount(uri: &str) {
    if let Ok(mut mounts) = nfs_pic_mounts().lock() {
        mounts.remove(uri.trim_end_matches('/'));
    }
}

fn nfs_note_pic_mount_if_tracked(track: bool, reference: &str) {
    if track {
        nfs_note_pic_mount(reference);
    }
}

/// Cheap poll hook (UI thread): reclaim gvfs mounts PIC no longer needs
/// (PIC-mounted NFS exports, registered NFS exports, and SMB leftovers on
/// registered roots) once nothing used them for a while. The check itself is
/// a single flag comparison; the DBus and disk work happens on a worker.
pub fn nfs_idle_unmount_tick() {
    let idle = match nfs_last_activity().lock() {
        // Never used this session (or nothing to protect): unmount candidates
        // are stale mounts from earlier sessions or manual mounts.
        Ok(last) => match *last {
            Some(last) => last.elapsed() > NFS_IDLE_UNMOUNT_AFTER,
            None => true,
        },
        Err(_) => false,
    };
    if !idle {
        return;
    }
    if NFS_UNMOUNT_IN_FLIGHT
        .compare_exchange(
            false,
            true,
            std::sync::atomic::Ordering::SeqCst,
            std::sync::atomic::Ordering::SeqCst,
        )
        .is_err()
    {
        return;
    }
    std::thread::spawn(|| {
        let result = nfs_idle_unmount_pass();
        NFS_UNMOUNT_IN_FLIGHT.store(false, std::sync::atomic::Ordering::SeqCst);
        if let Err(error) = result {
            net_trace(format!("nfs_idle_unmount failed error={error:#}"));
        }
    });
}

/// Reclaim gvfs mounts PIC no longer needs: NFS exports PIC mounted or has
/// registered, and SMB shares on registered roots while the direct transport
/// is available (which makes gvfs SMB mounts pure leftovers). Runs only when
/// the whole share has been idle, so active imports or reads never lose
/// their mount mid-use.
fn nfs_idle_unmount_pass() -> Result<()> {
    let connection = crate::db::open_default().context("could not open the library")?;
    let mut reclaim = crate::db::network_shares(&connection)?
        .into_iter()
        .map(|folder| folder.path)
        .filter(|path| path.starts_with("nfs://"))
        .collect::<Vec<_>>();
    match nfs_pic_mounts().lock() {
        Ok(mounts) => reclaim.extend(mounts.iter().cloned()),
        Err(_) => {}
    }
    // Native builds read SMB exclusively through libsmbclient, so any gvfs
    // SMB mount on a registered root is a leftover (older versions mounted
    // for browsing) and only serves to confuse file-manager users.
    let smb_reclaimable = crate::smb_transport::direct_available();
    if smb_reclaimable {
        reclaim.extend(
            crate::db::network_shares(&connection)?
                .into_iter()
                .map(|folder| folder.path)
                .filter(|path| path.starts_with("smb://")),
        );
    }
    if reclaim.is_empty() {
        return Ok(());
    }
    let monitor = gio::VolumeMonitor::get();
    for mount in monitor.mounts() {
        let uri = mount.root().uri();
        let is_nfs = uri.starts_with("nfs://");
        let is_smb = uri.starts_with("smb://");
        if !is_nfs && !(is_smb && smb_reclaimable) {
            continue;
        }
        if !reclaim
            .iter()
            .any(|reclaim| nfs_same_location(reclaim, &uri))
        {
            continue;
        }
        net_trace(format!("gvfs_idle_unmount_start uri={uri}"));
        if let Err(error) = unmount_blocking(&mount) {
            net_trace(format!("gvfs_idle_unmount_failed uri={uri} error={error}"));
        } else {
            nfs_forget_pic_mount(&uri);
            net_trace(format!("gvfs_idle_unmount_done uri={uri}"));
        }
    }
    Ok(())
}

/// Whether two NFS URIs address the same export area (hosts equal, either
/// path a prefix of the other). GVfs mounts exports at their root, while a
/// registered share can be a subfolder of an export.
fn nfs_same_location(left: &str, right: &str) -> bool {
    let (left, right) = (
        left.trim_end_matches('/'),
        right.trim_end_matches('/'),
    );
    let left_file = file(&normalize_nfs_uri(left));
    let right_file = file(&normalize_nfs_uri(right));
    let (left_host, right_host) = match (left_file.uri(), right_file.uri()) {
        (left, right) => match (glib::Uri::parse(&left, glib::UriFlags::NONE), glib::Uri::parse(&right, glib::UriFlags::NONE)) {
            (Ok(left), Ok(right)) => (
                left.host().unwrap_or_default().to_lowercase(),
                right.host().unwrap_or_default().to_lowercase(),
            ),
            _ => return false,
        },
    };
    if left_host != right_host {
        return false;
    }
    let (left_path, right_path) = (
        left_file.path().unwrap_or_default(),
        right_file.path().unwrap_or_default(),
    );
    left_path == right_path
        || left_path.starts_with(&right_path)
        || right_path.starts_with(&left_path)
}

/// Silent unmount through a private main context, for worker threads.
fn unmount_blocking(mount: &gio::Mount) -> Result<(), String> {
    let outcome: std::rc::Rc<std::cell::RefCell<Option<Result<(), String>>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    let context = glib::MainContext::new();
    let loop_ = glib::MainLoop::new(Some(&context), false);
    let loop_for_callback = loop_.clone();
    let outcome_for_callback = outcome.clone();
    let _ = context.with_thread_default(|| {
        let loop_for_timeout = loop_.clone();
        glib::timeout_add_local(std::time::Duration::from_secs(20), move || {
            loop_for_timeout.quit();
            glib::ControlFlow::Break
        });
        mount.unmount_with_operation(
            gio::MountUnmountFlags::NONE,
            Some(&gio::MountOperation::new()),
            gio::Cancellable::NONE,
            move |result| {
                *outcome_for_callback.borrow_mut() =
                    Some(result.map_err(|error| error.to_string()));
                loop_for_callback.quit();
            },
        );
        loop_.run();
    });
    let outcome = outcome.borrow_mut().take();
    outcome.unwrap_or_else(|| Err(String::from("unmount timed out")))
}

/// Mount an NFS export (or confirm it already is) without any dialog, on a
/// worker thread. gvfs only mounts on explicit request, so a photo read on an
/// unmounted export fails with NotMounted; callers mount, then retry.
pub fn ensure_nfs_mounted_blocking(reference: &str) -> Result<(), String> {
    let reference = normalize_nfs_uri(reference);
    let file = file(&reference);
    if file.find_enclosing_mount(gio::Cancellable::NONE).is_ok() {
        nfs_note_activity();
        return Ok(());
    }
    net_trace(format!("nfs_mount_start uri={reference}"));
    let outcome: std::rc::Rc<std::cell::RefCell<Option<Result<(), String>>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    let context = glib::MainContext::new();
    let loop_ = glib::MainLoop::new(Some(&context), false);
    let loop_for_callback = loop_.clone();
    let outcome_for_callback = outcome.clone();
    let reference_for_callback = reference.clone();
    let _ = context.with_thread_default(|| {
        let loop_for_timeout = loop_.clone();
        glib::timeout_add_local(std::time::Duration::from_secs(20), move || {
            loop_for_timeout.quit();
            glib::ControlFlow::Break
        });
        file.mount_enclosing_volume(
            gio::MountMountFlags::NONE,
            Some(&gio::MountOperation::new()),
            gio::Cancellable::NONE,
            move |result| {
                *outcome_for_callback.borrow_mut() = Some(match result {
                    Ok(()) => Ok(()),
                    Err(ref error)
                        if error.kind() == Some(gio::IOErrorEnum::AlreadyMounted) =>
                    {
                        Ok(())
                    }
                    Err(error) => Err(describe_mount_error(&reference_for_callback, &error)),
                });
                loop_for_callback.quit();
            },
        );
        loop_.run();
    });
    let outcome = outcome
        .borrow_mut()
        .take()
        .unwrap_or_else(|| Err(String::from("mount timed out")));
    net_trace(format!(
        "nfs_mount_done uri={reference} ok={}",
        outcome.is_ok()
    ));
    nfs_note_activity();
    if outcome.is_ok() {
        nfs_note_pic_mount(&reference);
    }
    outcome
}

/// Cheap NFS availability probe: a bounded TCP connect to the server's NFS
/// port. Never mounts anything. BLOCKING, worker threads only.
fn nfs_online(reference: &str) -> bool {
    let uri = file(&normalize_nfs_uri(reference)).uri();
    let parsed = match glib::Uri::parse(&uri, glib::UriFlags::NONE) {
        Ok(parsed) => parsed,
        Err(_) => return false,
    };
    let host = parsed.host().unwrap_or_default();
    if host.is_empty() {
        return false;
    }
    let port = if parsed.port() > 0 { parsed.port() as u16 } else { 2049 };
    let started = Instant::now();
    // IPv6 hosts need the bracket form; anything unparseable reads as offline.
    let address = format!("{host}:{port}")
        .parse()
        .or_else(|_| format!("[{host}]:{port}").parse());
    let online = match address {
        Ok(address) => std::net::TcpStream::connect_timeout(&address, URI_PROBE_TIMEOUT).is_ok(),
        Err(_) => false,
    };
    net_trace(format!(
        "nfs_online uri={reference} online={online} ms={:.1}",
        started.elapsed().as_secs_f64() * 1000.0
    ));
    online
}

/// Rewrite an imported reference to its stable form. gvfs FUSE paths
/// (`/run/user/N/gvfs/smb-share:...`, `/run/user/N/gvfs/nfs:host=...`) become
/// the canonical smb:// or nfs:// URI; everything else passes through.
/// Registering a FUSE path would create a library PIC displays nowhere: the
/// Folders tree excludes gvfs paths and Network Shares only lists scheme
/// paths.
pub fn normalize_import_reference(reference: &str) -> String {
    let smb = crate::smb_transport::normalize_smb_reference(reference);
    if smb != reference {
        return smb;
    }
    if reference.contains("://") {
        return reference.to_string();
    }
    nfs_fuse_reference(reference).unwrap_or_else(|| reference.to_string())
}

/// `/run/user/1000/gvfs/nfs:host=DietPi.local,prefix=%2Fmnt%2F4TBS/a/b`
/// -> `nfs://dietpi.local/mnt/4TBS/a/b`. Returns None for anything else.
fn nfs_fuse_reference(reference: &str) -> Option<String> {
    let marker = "/gvfs/nfs:";
    let start = reference.find(marker)? + marker.len();
    // The fuse root itself has no path beyond the options segment; anything
    // deeper is `<options>/<rest>`.
    let (options, rest) = match reference[start..].split_once('/') {
        Some((options, rest)) => (options, rest),
        None => (reference[start..].as_ref(), ""),
    };
    let mut host = None;
    let mut export = String::new();
    for option in options.split(',') {
        if let Some(value) = option.strip_prefix("host=") {
            host = Some(value.to_string());
        }
        if let Some(value) = option.strip_prefix("prefix=") {
            export = percent_decode(value);
        }
    }
    let host = host.filter(|host| !host.is_empty())?;
    let tail = if rest.is_empty() {
        String::new()
    } else {
        rest.split('/')
            .filter(|segment| !segment.is_empty())
            .map(percent_encode_segment)
            .collect::<Vec<_>>()
            .join("/")
    };
    let mut uri = format!("nfs://{host}/{}", export.trim_matches('/'));
    if !tail.is_empty() {
        uri.push('/');
        uri.push_str(&tail);
    }
    Some(uri)
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&value[index + 1..index + 3], 16) {
                decoded.push(byte);
                index += 3;
                continue;
            }
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

fn percent_encode_segment(segment: &str) -> String {
    crate::smb_transport::percent_encode_segment(segment)
}

/// Longest a remote existence probe may block. gvfs SMB/NFS lookups can stall
/// for a very long time when a host is unreachable; without a bound, callers
/// on the GTK thread (sidebar population, startup) would freeze for minutes.
/// The probe result is cached per refresh generation, so this bounds the cost
/// of the rare uncached probe instead of recurring per row.
const URI_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(4);

fn uri_query_exists(uri_file: &gio::File) -> bool {
    net_trace(format!("probe_start uri={}", uri_file.uri()));
    let probe_started = Instant::now();
    let cancellable = gio::Cancellable::new();
    let timer = cancellable.clone();
    std::thread::spawn(move || {
        std::thread::sleep(URI_PROBE_TIMEOUT);
        timer.cancel();
    });
    let exists = uri_file.query_exists(Some(&cancellable));
    // Release the timer thread early on a fast answer.
    cancellable.cancel();
    net_trace(format!(
        "probe_done uri={} exists={exists} ms={:.1}",
        uri_file.uri(),
        probe_started.elapsed().as_secs_f64() * 1000.0
    ));
    exists
}

/// Probe again after a reconnect, without retaining a previous offline result.
/// Blocking (up to `URI_PROBE_TIMEOUT` for a remote probe); only call from a
/// worker thread. No cache lock is held while the probe runs.
pub fn file_available(reference: &str) -> bool {
    query_exists(reference, false, crate::smb_transport::SmbLane::Background)
}

/// Read the cached availability of an imported source root. Never probes:
/// absence from the cache means "not probed yet" and defaults to available,
/// matching the historical online default. The asynchronous availability
/// refresher (worker thread) fills this cache via `probe_source_available`, so
/// the GTK thread never blocks on gvfs.
pub fn cached_source_available(reference: &str) -> bool {
    if !is_network_location(reference) {
        // Local filesystem paths are cheap and can change when a removable
        // drive is mounted or unmounted, so check them directly.
        return local_exists(reference, true);
    }
    let key = format!("source:{reference}");
    availability_cache()
        .lock()
        .unwrap()
        .get(&key)
        .copied()
        .unwrap_or(true)
}

/// Read availability for a single photo's original file. Never probes the
/// network. Local filesystem paths (and `file://` URIs) are checked directly
/// and cheaply so removable-drive mount state stays accurate; remote paths read
/// the per-folder cache, which a photo inherits from its imported source root.
pub fn cached_file_available(reference: &str) -> bool {
    // Local paths are cheap to check and can change when a removable drive is
    // mounted or unmounted, so do not retain a stale result for them.
    if !is_network_location(reference) {
        return local_exists(reference, false);
    }
    let key = format!("file:{reference}");
    availability_cache()
        .lock()
        .unwrap()
        .get(&key)
        .copied()
        .unwrap_or(true)
}

/// Existence check that needs no gvfs daemon: resolves `file://` URIs to their
/// local path and stats the filesystem directly.
fn local_exists(reference: &str, directory: bool) -> bool {
    let uri = file(reference).uri().to_string();
    let path = match uri.strip_prefix("file://") {
        Some(path) => Path::new(path),
        None => Path::new(reference),
    };
    if directory {
        path.is_dir()
    } else {
        path.is_file()
    }
}

/// Blocking probe of an imported source root, storing the result in the cache
/// after the probe finishes. Worker threads only: the probe may block for up to
/// `URI_PROBE_TIMEOUT`, but the cache lock is never held while it runs.
pub fn probe_source_available(reference: &str) -> bool {
    let key = format!("source:{reference}");
    let result = query_exists(reference, true, crate::smb_transport::SmbLane::Background);
    availability_cache().lock().unwrap().insert(key, result);
    result
}

/// Blocking probe of a photo's original file, stored afterwards. Worker threads
/// only.
pub fn probe_file_available(reference: &str) -> bool {
    if !reference.contains("://") {
        return Path::new(reference).is_file();
    }
    let key = format!("file:{reference}");
    let result = query_exists(reference, false, crate::smb_transport::SmbLane::Background);
    availability_cache().lock().unwrap().insert(key, result);
    result
}

pub fn refresh_availability() {
    availability_cache().lock().unwrap().clear();
}

static NET_TRACE_EPOCH: OnceLock<Instant> = OnceLock::new();
static NET_TRACE_LAST: OnceLock<Mutex<Instant>> = OnceLock::new();

std::thread_local! {
    static CURRENT_TRACE_OP: std::cell::RefCell<u64> = const { std::cell::RefCell::new(0) };
}

/// Start a new operation id for the GTK-thread interaction being measured, so
/// `NETWORK` lines emitted across the click -> gallery pipeline share one `op=`.
pub(crate) fn new_trace_op() -> u64 {
    static NEXT_TRACE_OP: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT_TRACE_OP.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// Set the operation id attached to subsequent `net_trace` lines. Reset with
/// `clear_trace_op` when the operation finishes so unrelated GTK work is not
/// misattributed.
pub(crate) fn set_trace_op(op: u64) {
    CURRENT_TRACE_OP.with(|current| *current.borrow_mut() = op);
}

pub(crate) fn clear_trace_op() {
    CURRENT_TRACE_OP.with(|current| *current.borrow_mut() = 0);
}

/// Clear the current operation id only if it still matches `op`. Lets an
/// already-finished interaction release its id without wiping a newer
/// interaction that took over the main thread in the meantime.
pub(crate) fn clear_trace_op_after(op: u64) {
    CURRENT_TRACE_OP.with(|current| {
        if *current.borrow() == op {
            *current.borrow_mut() = 0;
        }
    });
}

pub(crate) fn current_trace_op() -> u64 {
    CURRENT_TRACE_OP.with(|current| *current.borrow())
}

fn trace_thread_label() -> String {
    match std::thread::current().name() {
        Some(name) => name.to_string(),
        None => format!("{:?}", std::thread::current().id()),
    }
}

/// Trace log for the network-share path, enabled with PICASA_TRACE=1.
/// Emits `NETWORK <event>` lines; never prints credentials. Every line is
/// timestamped relative to the first trace (`t=`, ms) and to the previous
/// trace (`dt=`, ms), and carries the emitting thread (`tid=`) plus the current
/// operation id (`op=`) when one is active, so delays in the
/// click -> populate pipeline show up at a glance.
pub(crate) fn net_trace(message: impl std::fmt::Display) {
    if std::env::var_os("PICASA_TRACE").is_none() {
        return;
    }
    let epoch = *NET_TRACE_EPOCH.get_or_init(Instant::now);
    let mut last = NET_TRACE_LAST
        .get_or_init(|| Mutex::new(Instant::now()))
        .lock()
        .unwrap();
    let since_epoch = epoch.elapsed().as_secs_f64() * 1000.0;
    let since_last = last.elapsed().as_secs_f64() * 1000.0;
    *last = Instant::now();
    let op_suffix = if current_trace_op() != 0 {
        format!(" op={}", current_trace_op())
    } else {
        String::new()
    };
    eprintln!(
        "NETWORK t={since_epoch:>8.1}ms dt={since_last:>7.1}ms tid={} {message}{op_suffix}",
        trace_thread_label()
    );
}

/// Diagnose GTK main-loop stalls. Installs a 100 ms heartbeat on the GTK
/// thread; whenever a beat arrives more than 100 ms late it logs a `ui_stall`
/// line with the actual delay, exposing exactly where the ~4 s click-to-grid
/// latency is spent on the UI thread. Pure diagnostics: no behaviour depends on
/// the heartbeat.
pub(crate) fn install_ui_heartbeat() {
    // One beat per 100 ms; a real stall (>200 ms without the main loop
    // returning) is far above normal tick jitter.
    const STALL_THRESHOLD_MS: f64 = 250.0;
    let last = std::rc::Rc::new(std::cell::Cell::new(Instant::now()));
    let last_for_beat = last.clone();
    glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
        let now = Instant::now();
        let elapsed = now.duration_since(last_for_beat.get());
        last_for_beat.set(now);
        let ms = elapsed.as_secs_f64() * 1000.0;
        if ms > STALL_THRESHOLD_MS {
            net_trace(format!("ui_stall ms={ms:.1}"));
        }
        glib::ControlFlow::Continue
    });
}

/// True when a URI points at a remote, gvfs-managed location (smb://, nfs://,
/// sftp://, ...) instead of a local path.
pub fn is_network_location(reference: &str) -> bool {
    let Some(scheme_end) = reference.find("://") else {
        return false;
    };
    let scheme = reference[..scheme_end].to_ascii_lowercase();
    matches!(
        scheme.as_str(),
        "smb" | "cifs" | "nfs" | "sftp" | "ssh" | "ftp" | "dav" | "davs" | "afc"
    )
}

/// Whether SMB reads go through the direct libsmbclient transport (native) or
/// gvfs (sandboxed flatpak, where libsmbclient.so.0 is absent). Native builds
/// never mount SMB shares through gvfs; the flatpak has no alternative.
fn is_smb_direct() -> bool {
    crate::smb_transport::direct_available()
}

/// Mount a remote location through gvfs (never a manual mount, never fstab).
/// gvfs does not mount on demand for plain existence checks, so an unmounted
/// share would always read as "unavailable" and scans would abort with "scan
/// root is unavailable". If the server asks for credentials, the standard GTK
/// mount dialog appears. `on_result` runs on the main thread.
pub fn mount_share_async(
    reference: &str,
    parent: Option<&gtk::Window>,
    on_result: impl FnOnce(Result<(), String>) + 'static,
) {
    mount_share_async_inner(reference, parent, true, on_result);
}

/// Like [`mount_share_async`], but the mount stays PIC-untracked. Used by the
/// explicit "Open in file manager" action, whose mount exists FOR the desktop
/// browser and therefore must not be reclaimed by the idle unmount.
pub fn mount_share_async_visible(
    reference: &str,
    parent: Option<&gtk::Window>,
    on_result: impl FnOnce(Result<(), String>) + 'static,
) {
    mount_share_async_inner(reference, parent, false, on_result);
}

fn mount_share_async_inner(
    reference: &str,
    parent: Option<&gtk::Window>,
    track_nfs: bool,
    on_result: impl FnOnce(Result<(), String>) + 'static,
) {
    // NFS discovery entries are service endpoints (`nfs://host:2049/export`);
    // the GIO NFS backend mounts the plain `nfs://host/export` URI. Normalize
    // before any mount/enclosing-mount check so a discovered NFS row and a
    // stored ported URI mount successfully.
    let reference = &normalize_nfs_uri(reference);
    let file = file(reference);
    net_trace(format!("mount_started uri={reference}"));
    // GtkMountOperation routes credential requests to the GTK auth dialog.
    let mount_operation = gtk::MountOperation::new(parent);
    mount_share_run(
        reference.to_string(),
        file,
        mount_operation,
        track_nfs,
        move |outcome, _| on_result(outcome),
    );
}

/// Failure of a silent (`mount_share_async_silent`) mount. `auth_required`
/// distinguishes "credentials needed" (stop retrying, hand over to the
/// interactive Retry Connection) from offline/unreachable servers
/// (safe to retry with backoff).
pub struct SilentMountFailure {
    pub message: String,
    pub auth_required: bool,
}

/// Mount a remote location WITHOUT any user interaction. Uses a plain
/// `GMountOperation` instead of `GtkMountOperation`: it shows no dialogs, so
/// a credential request goes unanswered and the backend fails the mount.
/// Saved gvfs/keyring credentials are still used, and anonymous/guest SMB and
/// NFS exports connect silently. Used by the automatic startup reconnect -
/// shares needing a password stay registered and offline until the user
/// runs the interactive Retry Connection. `on_result` runs on the main
/// thread.
pub fn mount_share_async_silent(
    reference: &str,
    on_result: impl FnOnce(Result<(), SilentMountFailure>) + 'static,
) {
    let reference = &normalize_nfs_uri(reference);
    let file = file(reference);
    net_trace(format!("mount_started mode=silent uri={reference}"));
    let mount_operation = gio::MountOperation::new();
    mount_share_run(
        reference.to_string(),
        file,
        mount_operation,
        true,
        move |outcome, auth_required| {
            on_result(outcome.map_err(|message| SilentMountFailure {
                message,
                auth_required,
            }));
        },
    );
}

/// True when a mount failure means "credentials are required but unavailable"
/// rather than an offline/unreachable server. Backend texts differ (SMB logon
/// failure, NFS access denied, generic "Authentication required"), so the
/// error kind is checked first and the message text second.
fn is_auth_error(error: &glib::Error) -> bool {
    if error.kind() == Some(gio::IOErrorEnum::PermissionDenied) {
        return true;
    }
    let detail = error.to_string().to_lowercase();
    detail.contains("authenticat")
        || detail.contains("password")
        || detail.contains("logon failure")
        || detail.contains("not authorized")
        || detail.contains("access denied")
}

/// Shared mount core: already-mounted fast path, async
/// `mount_enclosing_volume`, reachability check and error mapping. The
/// second `on_result` argument reports whether a failure looks like missing
/// credentials (see `is_auth_error`); interactive callers ignore it.
/// `track_nfs` records NFS successes as PIC-owned so the idle unmount can
/// reclaim them (see `nfs_idle_unmount_tick`).
fn mount_share_run(
    reference: String,
    file: gio::File,
    mount_operation: impl IsA<gio::MountOperation> + 'static,
    track_nfs: bool,
    on_result: impl FnOnce(Result<(), String>, bool) + 'static,
) {
    let reference_for_callback = reference;
    // "Already mounted" counts as success: skip straight to the reachability
    // check when the location has an enclosing mount.
    if file.find_enclosing_mount(gio::Cancellable::NONE).is_ok()
        && query_exists(
            &reference_for_callback,
            true,
            crate::smb_transport::SmbLane::User,
        )
    {
        net_trace(format!(
            "mount_success uri={reference_for_callback} (already mounted)"
        ));
        nfs_note_pic_mount_if_tracked(track_nfs, &reference_for_callback);
        on_result(Ok(()), false);
        return;
    }
    file.mount_enclosing_volume(
        gio::MountMountFlags::NONE,
        Some(&mount_operation),
        gio::Cancellable::NONE,
        move |result| {
            let mut auth_required = false;
            let outcome = match result {
                Ok(())
                    if query_exists(
                        &reference_for_callback,
                        true,
                        crate::smb_transport::SmbLane::User,
                    ) =>
                {
                    net_trace(format!("mount_success uri={}", reference_for_callback));
                    Ok(())
                }
                Ok(()) => {
                    // Mounted but the reachability probe says no; for NFS the
                    // probe is a plain TCP check that can lag a fresh mount.
                    // Treat a completed mount as success rather than fail a
                    // share that is provably mounted.
                    net_trace(format!(
                        "mount_success uri={} (probe inconclusive)",
                        reference_for_callback
                    ));
                    Ok(())
                }
                Err(error) => {
                    if error.kind() == Some(gio::IOErrorEnum::AlreadyMounted) {
                        net_trace(format!(
                            "mount_success uri={} (already mounted)",
                            reference_for_callback
                        ));
                        nfs_note_pic_mount_if_tracked(track_nfs, &reference_for_callback);
                        on_result(Ok(()), false);
                        return;
                    }
                    auth_required = is_auth_error(&error);
                    if auth_required {
                        net_trace(format!(
                            "mount_auth_required uri={} error={}",
                            reference_for_callback, error
                        ));
                    } else {
                        net_trace(format!(
                            "mount_failed uri={} error={}",
                            reference_for_callback, error
                        ));
                    }
                    Err(describe_mount_error(&reference_for_callback, &error))
                }
            };
            if outcome.is_ok() {
                nfs_note_pic_mount_if_tracked(track_nfs, &reference_for_callback);
            }
            on_result(outcome, auth_required);
        },
    );
}

fn describe_mount_error(reference: &str, error: &glib::Error) -> String {
    use gio::IOErrorEnum;
    // Always carry the underlying GIO/GVfs error text: the friendly framing
    // alone hides e.g. "The specified location is not supported" when a scheme
    // backend is missing or a discovered NFS endpoint is malformed.
    let detail = error.to_string();
    match error.kind() {
        Some(IOErrorEnum::NotFound) | Some(IOErrorEnum::NotMounted) => format!(
            "server or share not found: {reference}. Check the server address and the share name. ({detail})"
        ),
        Some(IOErrorEnum::PermissionDenied) => format!(
            "access denied for {reference}. Check the credentials for the share. ({detail})"
        ),
        Some(IOErrorEnum::TimedOut) => format!(
            "the server did not respond in time: {reference}. Is the NAS online? ({detail})"
        ),
        Some(IOErrorEnum::NotSupported) if reference.starts_with("nfs://") => format!(
            "NFS is not supported by this system's gvfs backends: {reference}. Install the gvfs NFS backend (for example: sudo dnf install gvfs-nfs) and try again. ({detail})"
        ),
        _ => {
            // The backend-missing error also arrives with other error kinds
            // ("The specified location is not supported"), so fall back to a
            // text check for NFS references.
            if detail.contains("not supported") && reference.starts_with("nfs://") {
                format!(
                    "NFS is not supported by this system's gvfs backends: {reference}. Install the gvfs NFS backend (for example: sudo dnf install gvfs-nfs) and try again. ({detail})"
                )
            } else {
                format!("{reference}: {detail}")
            }
        }
    }
}

/// Build a stable network URI from user input, applying the Phase 1 URI rules:
/// strip any scheme the user typed (`smb://`, `nfs://`, `cifs://`, ...), trim
/// redundant slashes, and URI-escape the path components. The result is always
/// `scheme://server[/path]/` - never a gvfs `/run/user/...` mount path.
/// Host (with optional port) from a network URI, e.g.
/// `smb://DietPi.local:445/share` -> `DietPi.local:445`.
pub fn network_uri_host(uri: &str) -> String {
    let after_scheme = uri.split("://").nth(1).unwrap_or(uri);
    after_scheme
        .split('/')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// NFS discovery entries are service endpoints, not mountable exports. The
/// `network://` DNS-SD backend advertises the NFS RPC port in the URI
/// (`nfs://DietPi.local:2049/mnt`), but the GIO NFS backend mounts and browses
/// the canonical `nfs://host/export` form and does not treat a `host:port`
/// authority as `host` + a share the way SMB does. Strip a numeric service
/// port from the authority; every non-NFS reference passes through unchanged.
pub fn normalize_nfs_uri(reference: &str) -> String {
    let Some((scheme, rest)) = reference.split_once("://") else {
        return reference.to_string();
    };
    if !scheme.eq_ignore_ascii_case("nfs") {
        return reference.to_string();
    }
    let (authority, path) = match rest.split_once('/') {
        Some((authority, path)) => (authority, Some(path)),
        None => (rest, None),
    };
    // Only strip a numeric :port, and never inside an IPv6 bracket literal
    // (`[::1]:2049` and `[::1]` both pass through untouched).
    let authority = if authority.starts_with('[') {
        authority
    } else {
        match authority.rsplit_once(':') {
            Some((host, port))
                if !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit()) =>
            {
                host
            }
            _ => authority,
        }
    };
    match path {
        Some(path) if !path.is_empty() => format!("nfs://{authority}/{path}"),
        _ => format!("nfs://{authority}"),
    }
}

pub fn build_network_uri(scheme: &str, server: &str, path: &str) -> String {
    let clean_scheme = scheme.trim().to_ascii_lowercase();
    let mut server = server.trim().to_string();
    for prefix in [
        format!("{clean_scheme}://"),
        "cifs://".to_string(),
        "smb://".to_string(),
        "nfs://".to_string(),
        "//".to_string(),
    ] {
        if server
            .to_ascii_lowercase()
            .starts_with(&prefix.to_ascii_lowercase())
        {
            server = server[prefix.len()..].to_string();
            break;
        }
    }
    let server = server.trim_matches('/').to_string();
    let path = path.trim().trim_matches('/');
    let mut uri = format!("{clean_scheme}://{server}");
    if !path.is_empty() {
        for segment in path.split('/') {
            if segment.is_empty() {
                continue;
            }
            let escaped = glib::uri_escape_string(segment, Some("/"), false);
            uri.push('/');
            uri.push_str(&escaped);
        }
    }
    uri.push('/');
    uri
}

/// Discovery messages delivered to the main loop while a network:// scan is
/// running.
#[derive(Debug, Clone)]
pub enum NetworkDiscoveryEvent {
    /// A discovered location: (display name, URI supplied by gvfs).
    Item(String, String),
    /// Discovery finished (completed or timed out) with the item count.
    Done(usize),
}

/// Discover servers/locations the GVfs network backends expose (network://).
/// Best-effort convenience only: runs on a worker thread, is cancelled after a
/// short deadline, and delivers each found location to the main loop as it
/// arrives. An empty result never blocks manual server entry.
///
/// The `network://` backend can need a moment to warm up on first use (gvfs
/// daemon + DNS-SD backends); a single cold pass commonly errors or returns
/// nothing, which previously surfaced as an empty list on the FIRST dialog
/// open that only worked after Cancel + reopen. The worker therefore retries
/// an errored pass up to the deadline, delivering each distinct location once.
/// NFS entries are normalized here too, so the UI only ever sees mountable
/// `nfs://host/export` URIs rather than the advertised `nfs://host:2049/export`
/// service endpoint.
pub fn discover_network_locations() -> std::sync::mpsc::Receiver<NetworkDiscoveryEvent> {
    const DISCOVERY_PASS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);
    const DISCOVERY_DEADLINE: std::time::Duration = std::time::Duration::from_secs(8);
    const MAX_ATTEMPTS: u32 = 3;
    net_trace("discovery_started");
    let (sender, receiver) = std::sync::mpsc::channel::<NetworkDiscoveryEvent>();
    std::thread::spawn(move || {
        let send = |event: NetworkDiscoveryEvent| {
            let _ = sender.send(event);
        };
        let deadline = std::time::Instant::now() + DISCOVERY_DEADLINE;
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut count = 0usize;
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            net_trace(format!("discovery_attempt attempt={attempt}"));
            let cancellable = gio::Cancellable::new();
            let timer = cancellable.clone();
            std::thread::spawn(move || {
                std::thread::sleep(DISCOVERY_PASS_TIMEOUT);
                timer.cancel();
            });
            let root = gio::File::for_uri("network://");
            let mut pass_items: Vec<(String, String)> = Vec::new();
            let mut errored = false;
            if let Ok(enumerator) = root.enumerate_children(
                "standard::name,standard::display-name,standard::target-uri",
                gio::FileQueryInfoFlags::NONE,
                Some(&cancellable),
            ) {
                loop {
                    match enumerator.next_file(Some(&cancellable)) {
                        Ok(Some(info)) => {
                            // network:// entries are shortcuts: the target-uri is
                            // the real location (smb://DietPi.local:445/ etc).
                            let target = info
                                .attribute_as_string("standard::target-uri")
                                .map(|value| value.to_string());
                            let child = enumerator.child(&info);
                            let uri = target.unwrap_or_else(|| child.uri().to_string());
                            let name = info.display_name().to_string();
                            pass_items.push((name, normalize_nfs_uri(&uri)));
                        }
                        Ok(None) => break,
                        Err(error) => {
                            // Any interruption (including the pass timeout,
                            // i.e. Cancelled) means this pass is incomplete:
                            // cold gvfs backends commonly stall, so a retry
                            // pass collects the rest (deduped by URI).
                            errored = true;
                            if error.kind() != Some(gio::IOErrorEnum::Cancelled) {
                                net_trace(format!(
                                    "discovery_error attempt={attempt} error={error}"
                                ));
                            } else {
                                net_trace(format!(
                                    "discovery_error attempt={attempt} error=pass timed out"
                                ));
                            }
                            break;
                        }
                    }
                }
            } else {
                errored = true;
                net_trace(format!(
                    "discovery_error attempt={attempt} error=network:// enumeration failed"
                ));
            }
            let mut fresh = 0usize;
            for (name, uri) in pass_items {
                if seen.insert(uri.clone()) {
                    count += 1;
                    fresh += 1;
                    net_trace(format!("discovery_result attempt={attempt} uri={uri}"));
                    send(NetworkDiscoveryEvent::Item(name, uri));
                }
            }
            net_trace(format!(
                "discovery_pass_done attempt={attempt} found={fresh} total={count} errored={errored}"
            ));
            if !errored || attempt >= MAX_ATTEMPTS || std::time::Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(400));
        }
        net_trace(format!("discovery_complete count={count}"));
        send(NetworkDiscoveryEvent::Done(count));
    });
    receiver
}

/// Process-global cache of discovered network locations, deduped by URI and
/// kept in discovery order. Warmed by the startup prefetch and by every
/// dialog run, so the FIRST Add Network Share dialog can list the shares
/// immediately instead of staring at an empty list until Cancel + reopen.
static DISCOVERY_CACHE: std::sync::OnceLock<std::sync::Mutex<Vec<(String, String)>>> =
    std::sync::OnceLock::new();

fn discovery_cache() -> &'static std::sync::Mutex<Vec<(String, String)>> {
    DISCOVERY_CACHE.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

/// Store a discovered location for later dialogs. Deduped by URI.
pub fn remember_discovered_location(name: String, uri: String) {
    let mut cache = match discovery_cache().lock() {
        Ok(cache) => cache,
        Err(poisoned) => poisoned.into_inner(),
    };
    if !cache.iter().any(|(_, existing)| existing == &uri) {
        cache.push((name, uri));
    }
}

/// Snapshot of the cached locations in discovery order.
pub fn cached_network_locations() -> Vec<(String, String)> {
    let cache = match discovery_cache().lock() {
        Ok(cache) => cache,
        Err(poisoned) => poisoned.into_inner(),
    };
    cache.clone()
}

/// Warm the discovery cache in the background at app startup. Runs one full
/// discovery pass (which also warms the gvfs DNS-SD backends for the whole
/// session), so by the time the user opens the Add Network Share dialog the
/// shares are already known and the first open lists them instantly.
pub fn prefetch_network_locations() {
    std::thread::spawn(|| {
        let receiver = discover_network_locations();
        while let Ok(event) = receiver.recv() {
            if let NetworkDiscoveryEvent::Item(name, uri) = event {
                remember_discovered_location(name, uri);
            }
        }
        net_trace("discovery_prefetch_done");
    });
}

/// List the NFS exports a server offers, queried from the server's mountd
/// with `showmount -e` (nfs-utils). gvfs cannot enumerate NFS exports - NFS
/// has no share-listing protocol - which is why an NFS browser at a server
/// root always looks empty and why the DNS-SD-advertised path (`/mnt`) is
/// often not itself an export. BLOCKING: only call from a worker thread.
/// Returns export paths without the leading slash (`exports/Work`,
/// `mnt/4TBP`), sorted and deduped; empty when the query fails.
pub fn query_nfs_exports(host: &str) -> Vec<String> {
    let output = std::process::Command::new("showmount")
        .arg("-e")
        .arg(host)
        .output();
    let Ok(output) = output else {
        net_trace(format!("nfs_exports host={host} failed=showmount-missing"));
        return Vec::new();
    };
    if !output.status.success() {
        net_trace(format!(
            "nfs_exports host={host} failed=status={} {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
        return Vec::new();
    }
    let mut exports: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let path = line.trim().split_whitespace().next()?;
            let path = path.strip_prefix('/')?;
            (!path.is_empty()).then(|| path.to_string())
        })
        .collect();
    exports.sort();
    exports.dedup();
    net_trace(format!("nfs_exports host={host} count={}", exports.len()));
    exports
}
/// Resolve a `network://` shortcut URI to the concrete location gvfs says it
/// targets (e.g. `smb://DietPi.local:445/`). Returns None for non-network
/// URIs or when resolution fails; callers fall back to the original URI.
pub fn resolve_network_uri(reference: &str) -> Option<String> {
    if !reference.starts_with("network://") {
        return None;
    }
    let file = file(reference);
    let info = file.query_info(
        "standard::target-uri",
        gio::FileQueryInfoFlags::NONE,
        gio::Cancellable::NONE,
    );
    let target = info
        .ok()
        .and_then(|info| info.attribute_as_string("standard::target-uri"))
        .map(|value| value.to_string());
    net_trace(format!(
        "resolve {reference} -> {}",
        target.as_deref().unwrap_or("<unresolved>")
    ));
    target
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

/// RAW decoders require a native path. Remote files are cached locally only
/// when such a decoder needs one; ordinary JPEG/PNG/WebP reads stay streaming.
pub fn materialize(reference: &str) -> Result<PathBuf> {
    net_trace(format!("materialize_start uri={reference}"));
    let materialize_started = Instant::now();
    let result = materialize_inner(reference);
    match &result {
        Ok(path) => net_trace(format!(
            "materialize_done uri={reference} path={} ms={:.1}",
            path.display(),
            materialize_started.elapsed().as_secs_f64() * 1000.0
        )),
        Err(error) => net_trace(format!(
            "materialize_failed uri={reference} error={error} ms={:.1}",
            materialize_started.elapsed().as_secs_f64() * 1000.0
        )),
    }
    result
}

fn materialize_inner(reference: &str) -> Result<PathBuf> {
    // Normalize BEFORE the local-passthrough check: legacy gvfs-FUSE paths
    // (/run/user/.../gvfs/smb-share:...) contain no "://" and would be
    // treated as local files. When the gvfs mount is stale, every std::fs
    // operation on such a path blocks indefinitely (Fedora v19: lightbox
    // decodes hung forever inside exif_orientation's materialize call). The
    // normalized smb:// URI downloads through the transport into the local
    // cache instead - std::fs never touches a FUSE path.
    let reference = crate::smb_transport::normalize_smb_reference(reference);
    if !reference.contains("://") {
        return Ok(PathBuf::from(&reference));
    }
    let extension = Path::new(&reference)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("raw");
    let mut hasher = blake3::Hasher::new();
    hasher.update(reference.as_bytes());
    let hex = hasher.finalize().to_hex();
    let file_name = format!("{hex}.{extension}");
    let sources_root = crate::thumbnail::sources_dir()?;
    let path = crate::thumbnail::shard_dir_for_sources(&sources_root, &file_name).join(&file_name);
    if !path.is_file() {
        // Pre-sharding materialized sources live flat in the sources root;
        // move them into the shard on first use instead of re-downloading.
        let legacy = sources_root.join(&file_name);
        if legacy.is_file() {
            let parent = path.parent().expect("shard path has a parent");
            let _ = fs::create_dir_all(parent);
            if fs::rename(&legacy, &path).is_ok() {
                return Ok(path);
            }
            if path.is_file() {
                let _ = fs::remove_file(&legacy);
                return Ok(path);
            }
        }
        let parent = path.parent().expect("shard path has a parent");
        fs::create_dir_all(parent)?;
        fs::write(&path, read(&reference)?)?;
    }
    Ok(path)
}

pub fn read(reference: &str) -> Result<Vec<u8>> {
    // Legacy gvfs-FUSE photo paths (`/run/user/.../gvfs/smb-share:...`)
    // normalize to canonical smb:// URIs so the direct transport can serve
    // records imported before the redesign (no gvfs mount exists anymore).
    let reference = crate::smb_transport::normalize_smb_reference(reference);
    net_trace(format!("read_start uri={reference}"));
    let read_started = Instant::now();
    // Direct SMB: reads go through libsmbclient (no gvfs mount, nothing in
    // Nautilus) when the library is present. Sandboxed builds without
    // libsmbclient (the flatpak) read smb:// through gvfs instead; NFS and
    // every other scheme keep riding gvfs unconditionally.
    let reads_smb_direct = reference.starts_with("smb://") && is_smb_direct();
    let loaded = if reads_smb_direct {
        crate::smb_transport::read_file(&reference).map_err(anyhow::Error::msg)
    } else {
        let attempt = || {
            file(&reference)
                .load_contents(gio::Cancellable::NONE)
                .map(|(contents, _)| contents.as_ref().to_vec())
        };
        let first = attempt();
        match first {
            // An unmounted NFS export reads as NotMounted: mount it on demand
            // (never persists - the idle unmount reclaims it), then retry.
            Err(ref error)
                if reference.starts_with("nfs://")
                    && error.kind() == Some(gio::IOErrorEnum::NotMounted) =>
            {
                net_trace(format!("nfs_mount_on_demand uri={reference}"));
                ensure_nfs_mounted_blocking(&reference).map_err(anyhow::Error::msg)?;
                attempt().with_context(|| format!("could not read {reference}"))
            }
            other => other.with_context(|| format!("could not read {reference}")),
        }
    };
    if loaded.is_ok() && reference.starts_with("nfs://") {
        nfs_note_activity();
    }
    let result = loaded;
    match &result {
        Ok(bytes) => net_trace(format!(
            "read_done uri={reference} bytes={} ms={:.1}",
            bytes.len(),
            read_started.elapsed().as_secs_f64() * 1000.0
        )),
        Err(error) => net_trace(format!(
            "read_failed uri={reference} error={error} ms={:.1}",
            read_started.elapsed().as_secs_f64() * 1000.0
        )),
    }
    result
}

/// Normalize a network browse root for the folder browser:
/// - `network://` discovery shortcuts resolve to their concrete target URI
///   (e.g. `network:///dnssd-server-DIETPI._smb._tcp` -> `smb://DietPi.local:445/`)
/// - `smb://host/share/` stays an SMB URI, `nfs://host/export/` stays NFS
/// - a discovered NFS service endpoint (`nfs://host:2049/export`) becomes the
///   mountable export root `nfs://host/export/` (port stripped)
/// - a trailing slash is guaranteed so the location is browsable
/// - a remote URI is NEVER converted to a local `$HOME`/gvfs path
pub fn network_browse_root(reference: &str) -> String {
    net_trace(format!("browse_root_in uri={reference}"));
    let result = network_browse_root_inner(reference);
    net_trace(format!("browse_root_out uri={result}"));
    result
}

fn network_browse_root_inner(reference: &str) -> String {
    if reference.starts_with("network://") {
        if let Some(resolved) = resolve_network_uri(reference) {
            return network_browse_root_inner(&resolved);
        }
    }
    // NFS discovery entries carry the advertised RPC port
    // (`nfs://host:2049/export`); the browsable export root drops it.
    let mut uri = normalize_nfs_uri(reference);
    if !uri.contains("://") {
        // Local path: out of scope for the network browser - return unchanged
        // rather than ever guessing a home directory.
        return uri;
    }
    if !uri.ends_with('/') {
        uri.push('/');
    }
    uri
}

#[cfg(test)]
mod network_uri_tests {
    use super::network_browse_root;

    #[test]
    fn browse_root_keeps_the_uri_scheme() {
        assert_eq!(
            network_browse_root("smb://DietPi.local:445/"),
            "smb://DietPi.local:445/"
        );
        assert_eq!(
            network_browse_root("nfs://server/export"),
            "nfs://server/export/"
        );
        assert_eq!(
            network_browse_root("smb://nas/Photos/2026"),
            "smb://nas/Photos/2026/"
        );
    }

    #[test]
    fn browse_root_never_becomes_a_local_path() {
        let root = network_browse_root("smb://nas/share");
        assert!(root.contains("://"), "remote root must stay a URI: {root}");
        assert!(
            !root.starts_with('/'),
            "must not become a local path: {root}"
        );
        assert!(!root.contains("/home/"), "must not map to home: {root}");
    }

    #[test]
    fn unresolved_network_shortcuts_pass_through() {
        // A network:// entry that gvfs cannot resolve stays a network:// URI
        // (the caller surfaces the browse error instead of guessing home).
        let root = network_browse_root("network:///nonexistent-entry-xyz");
        assert!(
            root.starts_with("network://")
                || root.starts_with("smb://")
                || root.starts_with("nfs://"),
            "must remain a remote URI: {root}"
        );
    }
}

#[cfg(test)]
mod normalize_nfs_uri_tests {
    use super::normalize_nfs_uri;

    #[test]
    fn discovered_service_endpoint_becomes_a_mountable_export() {
        // network:// DNS-SD advertises the RPC port; GIO NFS mounts the export
        // path without it.
        assert_eq!(
            normalize_nfs_uri("nfs://DietPi.local:2049/mnt"),
            "nfs://DietPi.local/mnt"
        );
        assert_eq!(
            normalize_nfs_uri("nfs://nas:2049/export/photos"),
            "nfs://nas/export/photos"
        );
        assert_eq!(normalize_nfs_uri("nfs://nas:2049"), "nfs://nas");
    }

    #[test]
    fn canonical_nfs_uris_pass_through_unchanged() {
        assert_eq!(normalize_nfs_uri("nfs://nas/mnt"), "nfs://nas/mnt");
        assert_eq!(
            normalize_nfs_uri("nfs://DietPi.local/mnt/"),
            "nfs://DietPi.local/mnt/"
        );
        assert_eq!(normalize_nfs_uri("nfs://nas"), "nfs://nas");
    }

    #[test]
    fn non_nfs_schemes_and_local_paths_are_left_alone() {
        assert_eq!(
            normalize_nfs_uri("smb://DietPi.local:445/"),
            "smb://DietPi.local:445/"
        );
        assert_eq!(
            normalize_nfs_uri("sftp://host:22/stuff"),
            "sftp://host:22/stuff"
        );
        assert_eq!(normalize_nfs_uri("dav://host:443/x"), "dav://host:443/x");
        assert_eq!(normalize_nfs_uri("/mnt/4TBP"), "/mnt/4TBP");
        assert_eq!(
            normalize_nfs_uri("NFS://Host:2049/Export"),
            "nfs://Host/Export"
        );
    }

    #[test]
    fn ipv6_bracketed_authorities_keep_their_port() {
        assert_eq!(
            normalize_nfs_uri("nfs://[::1]:2049/mnt"),
            "nfs://[::1]:2049/mnt"
        );
        assert_eq!(normalize_nfs_uri("nfs://[::1]/mnt"), "nfs://[::1]/mnt");
    }
}

#[cfg(test)]
mod nfs_browse_root_tests {
    use super::network_browse_root;

    #[test]
    fn browse_root_strips_discovered_nfs_service_ports() {
        assert_eq!(
            network_browse_root("nfs://DietPi.local:2049/mnt"),
            "nfs://DietPi.local/mnt/"
        );
        assert_eq!(network_browse_root("nfs://nas:2049"), "nfs://nas/");
        assert_eq!(
            network_browse_root("nfs://nas:2049/export/photos"),
            "nfs://nas/export/photos/"
        );
    }
}

#[cfg(test)]
mod import_reference_tests {
    use super::normalize_import_reference;

    #[test]
    fn gvfs_fuse_paths_rewrite_to_stable_uris() {
        // SMB fuse path (the accidental-import shape).
        assert_eq!(
            normalize_import_reference(
                "/run/user/1000/gvfs/smb-share:server=dietpi.local,share=4tbs/pics-sport/local choice"
            ),
            "smb://dietpi.local/4tbs/pics-sport/local%20choice"
        );
        // NFS fuse path with a percent-encoded export prefix.
        assert_eq!(
            normalize_import_reference(
                "/run/user/1000/gvfs/nfs:host=DietPi.local,prefix=%2Fmnt%2F4TBS/mypics-2026"
            ),
            "nfs://DietPi.local/mnt/4TBS/mypics-2026"
        );
        // NFS fuse root itself.
        assert_eq!(
            normalize_import_reference("/run/user/1000/gvfs/nfs:host=nas.local,prefix=%2Fexport"),
            "nfs://nas.local/export"
        );
    }

    #[test]
    fn stable_references_pass_through_unchanged() {
        assert_eq!(
            normalize_import_reference("smb://dietpi.local/4tbs/pics-sport"),
            "smb://dietpi.local/4tbs/pics-sport"
        );
        assert_eq!(
            normalize_import_reference("nfs://dietpi.local/mnt/4TBP"),
            "nfs://dietpi.local/mnt/4TBP"
        );
        assert_eq!(
            normalize_import_reference("/home/peet/Pictures"),
            "/home/peet/Pictures"
        );
    }
}
