use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

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
    uri_query_exists(&file(reference))
}

/// Longest a remote existence probe may block. gvfs SMB/NFS lookups can stall
/// for a very long time when a host is unreachable; without a bound, callers
/// on the GTK thread (sidebar population, startup) would freeze for minutes.
/// The probe result is cached per refresh generation, so this bounds the cost
/// of the rare uncached probe instead of recurring per row.
const URI_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(4);

fn uri_query_exists(uri_file: &gio::File) -> bool {
    let cancellable = gio::Cancellable::new();
    let timer = cancellable.clone();
    std::thread::spawn(move || {
        std::thread::sleep(URI_PROBE_TIMEOUT);
        timer.cancel();
    });
    let exists = uri_file.query_exists(Some(&cancellable));
    // Release the timer thread early on a fast answer.
    cancellable.cancel();
    exists
}

/// Probe again after a reconnect, without retaining a previous offline result.
pub fn file_available(reference: &str) -> bool {
    query_exists(reference, false)
}

pub fn cached_source_available(reference: &str) -> bool {
    let key = format!("source:{reference}");
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
    let mut cache = availability_cache().lock().unwrap();
    *cache
        .entry(key)
        .or_insert_with(|| query_exists(reference, false))
}

pub fn refresh_availability() {
    availability_cache().lock().unwrap().clear();
}

/// Trace log for the network-share path, enabled with PICASA_TRACE=1.
/// Emits `NETWORK <event>` lines; never prints credentials.
pub(crate) fn net_trace(message: impl std::fmt::Display) {
    if std::env::var_os("PICASA_TRACE").is_some() {
        eprintln!("NETWORK {message}");
    }
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
    match error.kind() {
        Some(IOErrorEnum::NotFound) | Some(IOErrorEnum::NotMounted) => format!(
            "server or share not found: {reference}. Check the server address and the share name."
        ),
        Some(IOErrorEnum::PermissionDenied) => {
            format!("access denied for {reference}. Check the credentials for the share.")
        }
        Some(IOErrorEnum::TimedOut) => {
            format!("the server did not respond in time: {reference}. Is the NAS online?")
        }
        _ => format!("{reference}: {error}"),
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
/// Best-effort convenience only: runs on a worker thread, is cancelled after
/// ~3 seconds, and delivers each found location to the main loop as it
/// arrives. An empty result never blocks manual server entry.
pub fn discover_network_locations() -> std::sync::mpsc::Receiver<NetworkDiscoveryEvent> {
    net_trace("discovery_started");
    let (sender, receiver) = std::sync::mpsc::channel::<NetworkDiscoveryEvent>();
    std::thread::spawn(move || {
        let send = |event: NetworkDiscoveryEvent| {
            let _ = sender.send(event);
        };
        let cancellable = gio::Cancellable::new();
        let timer = cancellable.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(3));
            timer.cancel();
        });
        let mut count = 0usize;
        let root = gio::File::for_uri("network://");
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
                        count += 1;
                        net_trace(format!("discovery_result uri={uri}"));
                        send(NetworkDiscoveryEvent::Item(name, uri));
                    }
                    Ok(None) => break,
                    Err(_) => break,
                }
            }
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
    let (contents, _) = file(reference)
        .load_contents(gio::Cancellable::NONE)
        .with_context(|| format!("could not read {reference}"))?;
    Ok(contents.as_ref().to_vec())
}

/// Normalize a network browse root for the folder browser:
/// - `network://` discovery shortcuts resolve to their concrete target URI
///   (e.g. `network:///dnssd-server-DIETPI._smb._tcp` -> `smb://DietPi.local:445/`)
/// - `smb://host/share/` stays an SMB URI, `nfs://host/export/` stays NFS
/// - a trailing slash is guaranteed so the location is browsable
/// - a remote URI is NEVER converted to a local `$HOME`/gvfs path
pub fn network_browse_root(reference: &str) -> String {
    if reference.starts_with("network://") {
        if let Some(resolved) = resolve_network_uri(reference) {
            return network_browse_root(&resolved);
        }
    }
    let mut uri = reference.to_string();
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
