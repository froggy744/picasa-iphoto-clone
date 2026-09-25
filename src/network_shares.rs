//! Direct, read-only userspace SMB + NFS. No GIO mount or kernel mounts.
use anyhow::{Context, Result};
use gio::prelude::*;
use glib::variant::ToVariant;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub uri: String,
    pub is_dir: bool,
    /// Best-effort PC/server name, empty when unknown.
    pub server: String,
}
#[derive(Clone, Debug)]
pub struct Metadata {
    pub size: u64,
    pub mtime: Option<i64>,
    pub is_dir: bool,
}

pub fn private(uri: &str) -> bool {
    uri.starts_with("nfs://") || uri.starts_with("smb://")
}
/// Accept the kinds of address a user might type into the picker and turn them
/// into a canonical `smb://` or `nfs://` URI. Bare hostnames, IPs and UNC-style
/// `\\server\share` paths default to SMB; explicit `smb://`/`nfs://` pass
/// through unchanged. Returns `None` for emptiness or anything that is not a
/// network share address (local paths, http, etc.).
pub fn normalize_input(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed == "network:///" || trimmed.starts_with("smb://") || trimmed.starts_with("nfs://") {
        return Some(trimmed.to_owned());
    }
    let forward = trimmed.replace('\\', "/");
    let forward = forward.trim_end_matches('/');
    let forward = forward.strip_prefix("//").unwrap_or(&forward);
    if forward.contains("://") {
        return None;
    }
    let (host, path) = forward.split_once('/').unwrap_or((forward, ""));
    if host.is_empty() || host.contains('/') || host.contains('@') {
        return None;
    }
    let uri = if path.is_empty() {
        format!("smb://{host}/")
    } else {
        format!("smb://{host}/{path}")
    };
    Some(uri)
}
pub fn trace(area: &str, message: impl std::fmt::Display) {
    if crate::diagnostics::trace_enabled() {
        eprintln!(
            "PIC_NETWORK tid={:?} {area} {message}",
            std::thread::current().id()
        );
    }
}

pub fn list(uri: &str) -> Result<Vec<Entry>> {
    let routed = resolved_uri(uri);
    let mut entries = if uri.starts_with("nfs://") {
        crate::private_nfs::list(&routed)?
    } else if uri.starts_with("smb://") {
        crate::private_smb::list(&routed)?
    } else {
        anyhow::bail!("Not a direct SMB/NFS URI: {uri}")
    };
    // Use the numeric address only for the connection. Keep the discovered or
    // stored hostname in child URIs so library records remain stable.
    if routed != uri {
        let original_host = uri_host(uri).map(|(host, _)| host.to_owned());
        for entry in &mut entries {
            if let (Some(host), Some((_, range))) = (original_host.as_ref(), uri_host(&entry.uri)) {
                entry.uri = replace_host(&entry.uri, range, host.clone());
            }
        }
    }
    Ok(entries)
}
pub fn read(uri: &str) -> Result<Vec<u8>> {
    let routed = resolved_uri(uri);
    if uri.starts_with("nfs://") {
        return crate::private_nfs::read(&routed);
    }
    if uri.starts_with("smb://") {
        return crate::private_smb::read(&routed);
    }
    anyhow::bail!("Not a direct SMB/NFS URI: {uri}")
}
/// Bounded offset read for embedded remote RAW previews.
pub fn read_range(uri: &str, offset: u64, length: usize) -> Result<Vec<u8>> {
    let routed = resolved_uri(uri);
    if uri.starts_with("nfs://") {
        return crate::private_nfs::read_range(&routed, offset, length);
    }
    if uri.starts_with("smb://") {
        return crate::private_smb::read_range(&routed, offset, length);
    }
    anyhow::bail!("Not a direct SMB/NFS URI: {uri}")
}
pub fn stat(uri: &str) -> Result<Metadata> {
    let routed = resolved_uri(uri);
    if uri.starts_with("nfs://") {
        return crate::private_nfs::stat(&routed);
    }
    if uri.starts_with("smb://") {
        return crate::private_smb::stat(&routed);
    }
    anyhow::bail!("Not a direct SMB/NFS URI: {uri}")
}

static LOCAL_ADDRESSES: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();

fn uri_host(uri: &str) -> Option<(&str, std::ops::Range<usize>)> {
    let authority_start = uri.find("://")? + 3;
    let authority_end = uri[authority_start..]
        .find('/')
        .map_or(uri.len(), |n| authority_start + n);
    let authority = &uri[authority_start..authority_end];
    if authority.is_empty() || authority.contains('@') || authority.starts_with('[') {
        return None;
    }
    let host = authority
        .rsplit_once(':')
        .filter(|(_, port)| !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()))
        .map_or(authority, |(host, _)| host);
    Some((host, authority_start..authority_start + host.len()))
}

fn resolved_uri(uri: &str) -> String {
    let Some((host, range)) = uri_host(uri) else {
        return uri.to_owned();
    };
    if !host.to_ascii_lowercase().ends_with(".local") {
        return uri.to_owned();
    }
    let cache = LOCAL_ADDRESSES.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(address) = cache.lock().ok().and_then(|map| map.get(host).cloned()) {
        return replace_host(uri, range, address);
    }
    match resolve_local_host(host) {
        Ok(address) => {
            trace("MDNS", format!("resolved host={host} address={address}"));
            if let Ok(mut map) = cache.lock() {
                map.insert(host.to_owned(), address.clone());
            }
            replace_host(uri, range, address)
        }
        Err(error) => {
            trace("MDNS", format!("resolve_failed host={host} error={error}"));
            // Avoid a five-second D-Bus retry for every file on hosts without
            // Avahi. Native hostname resolution still gets its normal chance.
            if let Ok(mut map) = cache.lock() {
                map.insert(host.to_owned(), host.to_owned());
            }
            uri.to_owned()
        }
    }
}

fn replace_host(uri: &str, range: std::ops::Range<usize>, address: String) -> String {
    let mut routed = uri.to_owned();
    routed.replace_range(range, &address);
    routed
}

/// Flatpak runtimes do not necessarily include the host's nss-mdns module.
/// Resolve discovered `.local` names through the host Avahi daemon instead.
fn resolve_local_host(host: &str) -> Result<String> {
    let connection = gio::bus_get_sync(gio::BusType::System, gio::Cancellable::NONE)
        .context("Cannot connect to the system bus for mDNS resolution")?;
    let parameters = (-1i32, -1i32, host, 0i32, 0u32).to_variant();
    let reply = connection
        .call_sync(
            Some("org.freedesktop.Avahi"),
            "/",
            "org.freedesktop.Avahi.Server",
            "ResolveHostName",
            Some(&parameters),
            None,
            gio::DBusCallFlags::NONE,
            5000,
            gio::Cancellable::NONE,
        )
        .context("Avahi could not resolve the network server")?;
    let (_, _, _, _, address, _) = reply
        .get::<(i32, i32, String, i32, String, u32)>()
        .context("Avahi returned an unexpected response")?;
    anyhow::ensure!(!address.is_empty(), "Avahi returned an empty address");
    Ok(address)
}
/// Uses GNOME's network:/// only to discover advertised services. Never opens
/// smb:// or nfs:// through GIO: the private libraries handle those paths.
pub fn discover() -> Result<Vec<Entry>> {
    let root = gio::File::for_uri("network:///");
    let iterator = root
        .enumerate_children(
            "standard::display-name,standard::target-uri,standard::type",
            gio::FileQueryInfoFlags::NONE,
            gio::Cancellable::NONE,
        )
        .context("Cannot discover local network services")?;
    let mut entries = Vec::new();
    while let Some(info) = iterator.next_file(gio::Cancellable::NONE)? {
        let uri = info
            .attribute_string("standard::target-uri")
            .map(|v| v.to_string())
            .unwrap_or_else(|| iterator.child(&info).uri().to_string());
        if private(&uri) {
entries.push(Entry {
                name: info.display_name().to_string(),
                uri,
                is_dir: true,
                server: String::new(),
            });
        }
    }
    entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    entries.dedup_by(|a, b| a.uri == b.uri);
    Ok(entries)
}

/// Automatic subnet scan: probe the local /24 for SMB/NFS servers, then list
/// each reachable SMB server's shares and each NFS-only server's exports.
/// Independent of the network:/// (mDNS/WSD) discovery so a server that is not
/// advertised still appears. Runs the same native transport as everything else.
pub fn scan_subnet() -> Result<Vec<Entry>> {
    let hosts = crate::private_smb::scan_hosts(None)?;
    trace("SCAN", format!("start hosts={}", hosts.len()));
    let mut entries = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (host, server, kind) in hosts {
        let listed = if kind == 7 {
            let uri = format!("nfs://{host}/");
            crate::private_nfs::list(&uri)
        } else {
            let uri = format!("smb://{host}/");
            crate::private_smb::list(&uri)
        };
        match listed {
            Ok(children) => {
                for mut entry in children {
                    if entry.server.is_empty() { entry.server = server.clone(); }
                    trace("SCAN", format!("host={host} server={server} share={} uri={}", entry.name, entry.uri));
                    if seen.insert(entry.uri.clone()) {
                        entries.push(entry);
                    }
                }
            }
            Err(error) => trace("SCAN", format!("host={host} list_failed {error}")),
        }
    }
    entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(entries)
}

pub fn info(uri: &str) -> Result<gio::FileInfo> {
    let meta = stat(uri)?;
    let info = gio::FileInfo::new();
    let name = gio::File::for_uri(uri).basename().unwrap_or_default();
    info.set_name(&name);
    info.set_file_type(if meta.is_dir {
        gio::FileType::Directory
    } else {
        gio::FileType::Regular
    });
    info.set_size(i64::try_from(meta.size).unwrap_or(i64::MAX));
    if let Some(mtime) = meta.mtime {
        if mtime >= 0 {
            info.set_attribute_uint64("time::modified", mtime as u64)
        }
    }
    Ok(info)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "live network scan"]
    fn live_scan_subnet_finds_shares() {
        let entries = scan_subnet().expect("subnet scan should run");
        assert!(!entries.is_empty(), "expected at least one share on the local subnet");
        assert!(entries.iter().any(|entry| entry.uri.starts_with("smb://") || entry.uri.starts_with("nfs://")));
    }

    #[test]
    fn finds_uri_hostname_without_consuming_port_or_path() {
        let (host, range) = uri_host("smb://DietPi.local:445/Photos").unwrap();
        assert_eq!(host, "DietPi.local");
        assert_eq!(
            replace_host("smb://DietPi.local:445/Photos", range, "10.0.0.1".into()),
            "smb://10.0.0.1:445/Photos"
        );
    }

    #[test]
    fn ignores_non_network_and_credentialed_uris() {
        assert!(uri_host("/media/photos").is_none());
        assert!(uri_host("smb://user@server.local/photos").is_none());
    }

    #[test]
    fn restores_hostname_when_nfs_listing_changes_the_path() {
        let original = "nfs://DietPi.local:2049/mnt";
        let routed = "nfs://10.0.0.1:2049/4TBP";
        let host = uri_host(original).unwrap().0.to_owned();
        let (_, range) = uri_host(routed).unwrap();
        assert_eq!(
            replace_host(routed, range, host),
            "nfs://DietPi.local:2049/4TBP"
        );
    }

    #[test]
    fn nfs_uris_use_the_private_direct_transport() {
        assert!(private("nfs://10.0.0.1/mnt/4TBP"));
        assert_eq!(normalize_input("nfs://10.0.0.1/mnt/4TBP"),
                   Some("nfs://10.0.0.1/mnt/4TBP".into()));
    }

    #[test]
    fn normalizes_typed_addresses_to_canonical_smb_uris() {
        assert_eq!(normalize_input("Ella.local"), Some("smb://Ella.local/".into()));
        assert_eq!(
            normalize_input("Ella.local/Documents/2025"),
            Some("smb://Ella.local/Documents/2025".into())
        );
        assert_eq!(
            normalize_input(r"\\Ella.local\Documents\2025"),
            Some("smb://Ella.local/Documents/2025".into())
        );
        assert_eq!(normalize_input("10.0.0.119"), Some("smb://10.0.0.119/".into()));
        assert_eq!(
            normalize_input("smb://DietPi.local:445/Photos"),
            Some("smb://DietPi.local:445/Photos".into())
        );
        assert_eq!(normalize_input("nfs://10.0.0.2/export"), Some("nfs://10.0.0.2/export".into()));
        assert_eq!(normalize_input(""), None);
        assert_eq!(normalize_input("/home/peet/photos"), None);
        assert_eq!(normalize_input("https://example.com/share"), None);
    }
}
