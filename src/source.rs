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

fn query_exists(reference: &str, directory: bool) -> bool {
    if !reference.contains("://") {
        return if directory {
            Path::new(reference).is_dir()
        } else {
            Path::new(reference).is_file()
        };
    }
    uri_query_exists(&file(&normalize_nfs_uri(reference)))
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
    query_exists(reference, false)
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
    let result = query_exists(reference, true);
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
    let result = query_exists(reference, false);
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
    // NFS discovery entries are service endpoints (`nfs://host:2049/export`);
    // the GIO NFS backend mounts the plain `nfs://host/export` URI. Normalize
    // before any mount/enclosing-mount check so a discovered NFS row and a
    // stored ported URI mount successfully.
    let reference = &normalize_nfs_uri(reference);
    let file = file(reference);
    net_trace(format!("mount_started uri={reference}"));
    let mount_operation = gtk::MountOperation::new(parent);
    let reference_for_callback = reference.to_string();
    // "Already mounted" counts as success: skip straight to the reachability
    // check when the location has an enclosing mount.
    if file.find_enclosing_mount(gio::Cancellable::NONE).is_ok()
        && query_exists(&reference_for_callback, true)
    {
        net_trace(format!(
            "mount_success uri={reference_for_callback} (already mounted)"
        ));
        on_result(Ok(()));
        return;
    }
    file.mount_enclosing_volume(
        gio::MountMountFlags::NONE,
        Some(&mount_operation),
        gio::Cancellable::NONE,
        move |result| {
            let outcome = match result {
                Ok(()) if query_exists(&reference_for_callback, true) => {
                    net_trace(format!("mount_success uri={}", reference_for_callback));
                    Ok(())
                }
                Ok(()) => Err(format!(
                    "mounted, but the location is not reachable: {}",
                    reference_for_callback
                )),
                Err(error) => {
                    if error.kind() == Some(gio::IOErrorEnum::AlreadyMounted) {
                        net_trace(format!(
                            "mount_success uri={} (already mounted)",
                            reference_for_callback
                        ));
                        on_result(Ok(()));
                        return;
                    }
                    net_trace(format!(
                        "mount_failed uri={} error={}",
                        reference_for_callback, error
                    ));
                    Err(describe_mount_error(&reference_for_callback, &error))
                }
            };
            on_result(outcome);
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
        _ => format!("{reference}: {detail}"),
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
    if !reference.contains("://") {
        return Ok(PathBuf::from(reference));
    }
    let extension = Path::new(reference)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("raw");
    let mut hasher = blake3::Hasher::new();
    hasher.update(reference.as_bytes());
    let path = crate::thumbnail::cache_dir()?.join("source").join(format!(
        "{}.{}",
        hasher.finalize().to_hex(),
        extension
    ));
    if !path.is_file() {
        let parent = path.parent().expect("cached source has a parent");
        fs::create_dir_all(parent)?;
        fs::write(&path, read(reference)?)?;
    }
    Ok(path)
}

pub fn read(reference: &str) -> Result<Vec<u8>> {
    net_trace(format!("read_start uri={reference}"));
    let read_started = Instant::now();
    let loaded = file(reference)
        .load_contents(gio::Cancellable::NONE)
        .with_context(|| format!("could not read {reference}"));
    let result = loaded.map(|(contents, _)| contents.as_ref().to_vec());
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
