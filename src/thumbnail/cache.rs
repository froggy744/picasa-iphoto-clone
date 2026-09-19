static CACHE_DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

pub fn cache_dir() -> Result<PathBuf> {
    // Cache the resolved directory: this is called once per PhotoObject built
    // (tens of thousands per refresh), and create_dir_all is a syscall each
    // time. The directory never changes within a session.
    if let Some(directory) = CACHE_DIR.get() {
        return Ok(directory.clone());
    }
    let directory = dirs::cache_dir()
        .context("could not determine the user's cache directory")?
        .join("picasa-rs")
        .join("thumbs");
    fs::create_dir_all(&directory)
        .with_context(|| format!("could not create cache directory {}", directory.display()))?;
    let _ = CACHE_DIR.set(directory.clone());
    relocate_legacy_aux_root(&directory, "source");
    relocate_legacy_aux_root(&directory, "wallpaper");
    Ok(directory)
}

/// `source/` (materialized remote RAW files) and `wallpaper/` used to live
/// inside the thumbnail directory. They are cache data, not thumbnails, and
/// dominated its size, so they now live beside it. One-time move; failures
/// are non-fatal (the legacy location keeps working).
fn relocate_legacy_aux_root(thumbs: &Path, name: &str) {
    let legacy = thumbs.join(name);
    if !legacy.is_dir() {
        return;
    }
    let Some(parent) = thumbs.parent() else {
        return;
    };
    let destination = parent.join(name);
    if destination.exists() {
        return;
    }
    if fs::rename(&legacy, &destination).is_err() {
        if fs::create_dir_all(&destination).is_ok() {
            move_dir_contents(&legacy, &destination);
            let _ = fs::remove_dir_all(&legacy);
        }
    }
    prune_empty_shard_dirs(thumbs);
}

fn move_dir_contents(source: &Path, destination: &Path) {
    let Ok(entries) = fs::read_dir(source) else {
        return;
    };
    for entry in entries.flatten() {
        let target = destination.join(entry.file_name());
        if entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
            if fs::create_dir_all(&target).is_ok() {
                move_dir_contents(&entry.path(), &target);
            }
            continue;
        }
        if fs::rename(entry.path(), &target).is_err() {
            let _ = fs::copy(entry.path(), &target);
        }
    }
}

/// Remove shard directories that a finished operation emptied.
fn prune_empty_shard_dirs(thumbs: &Path) {
    let files_root = thumbs.join("files");
    let Ok(shards) = fs::read_dir(&files_root) else {
        return;
    };
    for shard in shards.flatten() {
        if !shard.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
            continue;
        }
        let _ = fs::remove_dir(shard.path());
    }
}

pub fn cache_path(path: &str, mtime: Option<i64>, size_bytes: Option<i64>) -> Result<PathBuf> {
    let file_name = cache_file_name(path, mtime, size_bytes);
    Ok(shard_dir_for(&file_name)?.join(file_name))
}

/// A 143k-file flat directory makes every scan, cleanup and stats pass crawl
/// and can exceed filesystem directory limits, so thumbnails are sharded by
/// the first two hex characters of the cache key (256 buckets). The shard
/// prefix is derived from the key, never from a directory listing.
pub fn shard_dir_for(file_name: &str) -> Result<PathBuf> {
    Ok(shard_dir_in(&cache_dir()?.join("files"), file_name))
}

/// The shard directory for `file_name` under a shard root: the first two hex
/// characters of the key (256 buckets), or `misc` for foreign names.
fn shard_dir_in(shard_root: &Path, file_name: &str) -> PathBuf {
    let prefix = file_name
        .get(..2)
        .filter(|prefix| prefix.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .unwrap_or("misc");
    shard_root.join(prefix)
}

/// Materialized remote RAW sources: a sibling of the thumbnail directory, not
/// inside it (they dominate its size and are cache data, not thumbnails).
pub fn sources_dir() -> Result<PathBuf> {
    aux_dir("source")
}

/// Composed wallpaper exports: a sibling of the thumbnail directory.
pub fn wallpaper_dir() -> Result<PathBuf> {
    aux_dir("wallpaper")
}

fn aux_dir(name: &str) -> Result<PathBuf> {
    let thumbs = cache_dir()?;
    let directory = thumbs
        .parent()
        .map(|parent| parent.join(name))
        .unwrap_or_else(|| thumbs.join(name));
    fs::create_dir_all(&directory)
        .with_context(|| format!("could not create cache directory {}", directory.display()))?;
    Ok(directory)
}

/// The shard directory for a materialized source file under the sources
/// root. Public because `source::materialize` writes into this layout.
pub fn shard_dir_for_sources(sources_root: &Path, file_name: &str) -> PathBuf {
    shard_dir_in(sources_root, file_name)
}

/// Resolve a thumbnail's current sharded path, transparently migrating the
/// legacy flat-layout file on first access. After this returns an existing
/// path, only the sharded copy remains: reads, rename-flows and cleanup all
/// see one canonical location, and the flat directory empties over time.
pub fn resolve_cache_path(
    path: &str,
    mtime: Option<i64>,
    size_bytes: Option<i64>,
) -> Result<PathBuf> {
    resolve_cache_path_in(&cache_dir()?, path, mtime, size_bytes)
}

pub(crate) fn resolve_cache_path_in(
    thumbs: &Path,
    path: &str,
    mtime: Option<i64>,
    size_bytes: Option<i64>,
) -> Result<PathBuf> {
    let file_name = cache_file_name(path, mtime, size_bytes);
    let destination = shard_dir_in(&thumbs.join("files"), &file_name).join(&file_name);
    if destination.exists() {
        return Ok(destination);
    }
    let legacy = thumbs.join(&file_name);
    if legacy.is_file() {
        let _ = fs::create_dir_all(destination.parent().expect("shard path has a parent"));
        if fs::rename(&legacy, &destination).is_ok() {
            let _ = fs::remove_file(legacy.with_extension("failed"));
            return Ok(destination);
        }
        if destination.exists() {
            let _ = fs::remove_file(&legacy);
            return Ok(destination);
        }
        // The legacy file could not be moved (permissions, transient loss);
        // keep serving it where it is rather than regenerating.
        return Ok(legacy);
    }
    Ok(destination)
}

/// The cache file name for a photo's current fingerprint: pure hashing with no
/// filesystem access, so maintenance code can compute expected keys for any
/// cache directory. The key inputs (path, cache version, mtime, size) are the
/// original ones and must not change.
pub fn cache_file_name(path: &str, mtime: Option<i64>, size_bytes: Option<i64>) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(path.as_bytes());
    let cache_version = if is_dng(path) {
        DNG_THUMBNAIL_CACHE_VERSION
    } else if crate::image_format::uses(path, crate::image_format::DecoderKind::Raw) {
        RAW_THUMBNAIL_CACHE_VERSION
    } else {
        THUMBNAIL_CACHE_VERSION
    };
    hasher.update(cache_version);
    hasher.update(b"\0");
    hasher.update(mtime.unwrap_or_default().to_string().as_bytes());
    hasher.update(b"\0");
    hasher.update(size_bytes.unwrap_or_default().to_string().as_bytes());
    format!("{}.jpg", hasher.finalize().to_hex())
}

pub fn existing_cache_path(
    path: &str,
    mtime: Option<i64>,
    size_bytes: Option<i64>,
) -> Result<Option<PathBuf>> {
    let candidate = resolve_cache_path(path, mtime, size_bytes)?;
    Ok(candidate.is_file().then_some(candidate))
}

pub fn create(path: &str, mtime: Option<i64>, size_bytes: Option<i64>) -> Result<PathBuf> {
    let destination = resolve_cache_path(path, mtime, size_bytes)?;
    let failure_marker = destination.with_extension("failed");
    if destination.is_file() {
        return Ok(destination);
    }
    if failure_marker.is_file() {
        if !known_decode_failure(path, &destination) {
            // Legacy markers also recorded offline/read errors. Retry them
            // once; only confirmed decode failures now suppress future work.
            let _ = fs::remove_file(&failure_marker);
        } else {
            return Ok(destination);
        }
    }
    let in_flight = IN_FLIGHT.get_or_init(|| Mutex::new(HashSet::new()));
    let claimed = in_flight
        .lock()
        .map_err(|_| anyhow::anyhow!("thumbnail in-flight registry poisoned"))?
        .insert(destination.clone());
    if !claimed {
        return Ok(destination);
    }

    let result = create_uncached(path, &destination);
    if let Ok(mut entries) = in_flight.lock() {
        entries.remove(&destination);
    }
    if result.as_ref().is_err_and(|error| {
        !error
            .chain()
            .any(|cause| cause.is::<std::io::Error>() || cause.is::<glib::Error>())
            && crate::source::file_available(path)
    }) {
        // Avoid retrying a known corrupt/unsupported source on every launch.
        // The marker is keyed by the source fingerprint, so a changed file
        // naturally gets a new cache key and can be attempted again.
        let _ = fs::write(&failure_marker, DECODE_FAILURE_MARKER);
    }
    result
}

const DECODE_FAILURE_MARKER: &[u8] = b"thumbnail decode failed v2\n";

fn known_decode_failure(path: &str, destination: &Path) -> bool {
    !is_heif(path)
        && fs::read(destination.with_extension("failed"))
            .is_ok_and(|contents| contents == DECODE_FAILURE_MARKER)
}

fn create_uncached(path: &str, destination: &PathBuf) -> Result<PathBuf> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }

    let source = if is_raw(path) {
        decode_raw_thumbnail(path)?.image
    } else {
        let bytes = crate::source::read(path)?;
        if is_jpeg(path) {
            match decode_jpeg_turbo(&bytes) {
                Ok(decoded) => decoded.image,
                Err(_) => decode_with_image(&bytes)?.image,
            }
        } else if is_heif(path) {
            decode_heif(&bytes)?.image
        } else {
            decode_with_image(&bytes)?.image
        }
    };

    // heif-oxide applies HEIF container transforms (`irot`/`imir`/`clap`) as
    // part of decoding, so its pixels already have display orientation. Do
    // not apply an EXIF orientation a second time; some HEIC files contain
    // both forms of orientation metadata.
    let orientation = if is_heif(path) {
        1
    } else {
        exif_orientation(path)
    };
    let source = apply_orientation(DynamicImage::ImageRgb8(source), orientation).to_rgb8();
    let resized = resize(source)?;
    let output_width = resized.width();
    let output_height = resized.height();

    let mut encoded = Vec::new();
    JpegEncoder::new(&mut encoded).write_image(
        resized.as_raw(),
        output_width,
        output_height,
        ColorType::Rgb8.into(),
    )?;
    fs::write(destination, encoded)?;
    Ok(destination.clone())
}
