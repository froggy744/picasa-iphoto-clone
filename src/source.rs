use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use anyhow::{Context, Result};
use gio::prelude::*;

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
