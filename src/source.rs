use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use anyhow::{Context, Result};
use gio::prelude::*;

static AVAILABILITY_CACHE: OnceLock<Mutex<HashMap<String, bool>>> = OnceLock::new();

fn availability_cache() -> &'static Mutex<HashMap<String, bool>> {
    AVAILABILITY_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn query_exists(reference: &str, directory: bool) -> bool {
    if !reference.contains("://") {
        return if directory {
            Path::new(reference).is_dir()
        } else {
            Path::new(reference).is_file()
        };
    }
    file(reference).query_exists(gio::Cancellable::NONE)
}

pub fn cached_source_available(reference: &str) -> bool {
    let key = format!("source:{reference}");
    let mut cache = availability_cache().lock().unwrap();
    *cache
        .entry(key)
        .or_insert_with(|| query_exists(reference, true))
}

pub fn cached_file_available(reference: &str) -> bool {
    let key = format!("file:{reference}");
    let mut cache = availability_cache().lock().unwrap();
    *cache
        .entry(key)
        .or_insert_with(|| query_exists(reference, false))
}

pub fn refresh_availability() {
    availability_cache().lock().unwrap().clear();
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
