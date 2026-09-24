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
    Ok(directory)
}

pub fn cache_path(path: &str, mtime: Option<i64>, size_bytes: Option<i64>) -> Result<PathBuf> {
    let file_name = cache_file_name(path, mtime, size_bytes);
    Ok(shard_dir_for(&file_name)?.join(file_name))
}

fn cache_path_in(
    thumbs: &Path,
    path: &str,
    mtime: Option<i64>,
    size_bytes: Option<i64>,
) -> PathBuf {
    let file_name = cache_file_name(path, mtime, size_bytes);
    shard_dir_in(&thumbs.join("files"), &file_name).join(file_name)
}

/// The original RC3 cache layout: `thumbs/files/<bucket>/<hash>.jpg`.
/// Buckets are derived from the key, never discovered by scanning the cache.
pub fn shard_dir_for(file_name: &str) -> Result<PathBuf> {
    Ok(shard_dir_in(&cache_dir()?.join("files"), file_name))
}

fn shard_dir_in(shard_root: &Path, file_name: &str) -> PathBuf {
    let folder = cache_shard_char(file_name)
        .map(|folder| folder.to_string())
        .unwrap_or_else(|| "misc".to_owned());
    shard_root.join(folder)
}

/// Map the first four hexadecimal characters of a cache key into the original
/// 36 alphanumeric buckets (`0-9`, `a-z`).
pub fn cache_shard_char(file_name: &str) -> Option<char> {
    const SHARDS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let value = u16::from_str_radix(file_name.get(..4)?, 16).ok()?;
    Some(SHARDS[(value % SHARDS.len() as u16) as usize] as char)
}

fn flat_cache_path(path: &str, mtime: Option<i64>, size_bytes: Option<i64>) -> Result<PathBuf> {
    Ok(cache_dir()?.join(cache_file_name(path, mtime, size_bytes)))
}

/// The cache file name for a photo's current fingerprint: pure hashing with no
/// filesystem access, so maintenance code can compute expected keys for any
/// cache directory. The key inputs (path, cache version, mtime, size) are the
/// original ones and must not change.
pub fn cache_file_name(path: &str, mtime: Option<i64>, size_bytes: Option<i64>) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(path.as_bytes());
    let cache_version = if remote_nef(path) {
        REMOTE_NEF_THUMBNAIL_CACHE_VERSION
    } else if remote_jpeg(path) {
        REMOTE_JPEG_THUMBNAIL_CACHE_VERSION
    } else if is_dng(path) {
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

fn remote_jpeg(path: &str) -> bool {
    #[cfg(target_os = "linux")]
    {
        crate::network_shares::private(path)
            && crate::image_format::uses(path, crate::image_format::DecoderKind::TurboJpeg)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = path;
        false
    }
}

fn remote_nef(path: &str) -> bool {
    #[cfg(target_os = "linux")]
    {
        crate::network_shares::private(path) && is_nikon_raw(path)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = path;
        false
    }
}

pub fn existing_cache_path(
    path: &str,
    mtime: Option<i64>,
    size_bytes: Option<i64>,
) -> Result<Option<PathBuf>> {
    let started = std::time::Instant::now();
    existing_cache_path_in(&cache_dir()?, path, mtime, size_bytes).map(|result| {
        if std::env::var_os("PICASA_TRACE").is_some() {
            eprintln!("PIC_THUMBNAIL cache_lookup result={} elapsed_us={} uri={}", if result.is_some() { "hit" } else { "miss" }, started.elapsed().as_micros(), path);
        }
        result
    })
}

fn existing_cache_path_in(
    thumbs: &Path,
    path: &str,
    mtime: Option<i64>,
    size_bytes: Option<i64>,
) -> Result<Option<PathBuf>> {
    let candidate = cache_path_in(thumbs, path, mtime, size_bytes);
    if candidate.is_file() {
        return Ok(Some(candidate));
    }
    // Flat caches predate sharding. This is a pair of direct `is_file`
    // lookups, not a directory scan, and lets offline originals stay usable.
    let legacy = thumbs.join(cache_file_name(path, mtime, size_bytes));
    Ok(legacy.is_file().then_some(legacy))
}

pub fn create(path: &str, mtime: Option<i64>, size_bytes: Option<i64>) -> Result<PathBuf> {
    let started = std::time::Instant::now();
    let destination = cache_path(path, mtime, size_bytes)?;
    let failure_marker = destination.with_extension("failed");
    if destination.is_file() {
        if std::env::var_os("PICASA_TRACE").is_some() {
            eprintln!(
                "PIC_THUMBNAIL cache_hit uri={path} cache={}",
                destination.display()
            );
        }
        return Ok(destination);
    }
    let legacy = flat_cache_path(path, mtime, size_bytes)?;
    if legacy.is_file() {
        // Do not move an entry from a GTK-visible lookup path. Keeping it is
        // safe, avoids a blocking migration, and preserves offline access.
        return Ok(legacy);
    }
    let legacy_failure_marker = legacy.with_extension("failed");
    if legacy_failure_marker.is_file() && known_decode_failure(path, &legacy) {
        return Ok(legacy);
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
    if std::env::var_os("PICASA_TRACE").is_some() {
        match &result {
            Ok(cache) => eprintln!(
                "PIC_THUMBNAIL cache_write uri={path} cache={} elapsed_ms={}",
                cache.display(),
                started.elapsed().as_millis()
            ),
            Err(error) => eprintln!("PIC_THUMBNAIL failed uri={path} error={error:#}"),
        }
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

    let mut remote_jpeg_orientation = None;
    let source = if is_raw(path) {
        decode_raw_thumbnail(path)?.image
    } else {
        let bytes = crate::source::read(path)?;
        if is_jpeg(path) {
            #[cfg(target_os = "linux")]
            if crate::network_shares::private(path) {
                // The original JPEG is already in memory for thumbnail
                // decoding. Read its orientation from those bytes instead of
                // issuing a second NFS/SMB metadata range request.
                remote_jpeg_orientation = Some(jpeg_orientation_from_header(&bytes));
            }
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
        remote_jpeg_orientation.unwrap_or_else(|| exif_orientation(path))
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

#[cfg(test)]
mod cache_layout_tests {
    use super::*;

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    fn fixture() -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "picasa-thumbnail-layout-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
        directory
    }

    #[test]
    fn paths_are_deterministic_and_use_original_alphanumeric_shards() {
        let root = fixture();
        let first = cache_path_in(&root, "/photos/a.jpg", Some(1), Some(2));
        let second = cache_path_in(&root, "/photos/a.jpg", Some(1), Some(2));
        assert_eq!(first, second);
        assert!(first.starts_with(root.join("files")));
        let bucket = first
            .parent()
            .unwrap()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(bucket.len(), 1);
        assert!(bucket.as_bytes()[0].is_ascii_alphanumeric());
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn native_network_jpegs_use_the_orientation_cache_version() {
        assert!(remote_jpeg("nfs://DietPi.local/photos/test.jpg"));
        assert!(remote_jpeg("smb://server/share/test.JPG"));
        assert!(!remote_jpeg("/photos/test.jpg"));
        assert!(!remote_jpeg("nfs://DietPi.local/photos/test.png"));
    }

    #[test]
    fn distinct_hash_prefixes_distribute_across_buckets() {
        let buckets: HashSet<_> = (0..2000u64)
            .map(|value| cache_shard_char(&format!("{value:04x}{value:060x}.jpg")).unwrap())
            .collect();
        assert_eq!(buckets.len(), 36);
    }

    #[test]
    fn existing_sharded_and_flat_entries_are_found_without_a_scan() {
        let root = fixture();
        let sharded = cache_path_in(&root, "/photos/sharded.jpg", Some(1), Some(2));
        fs::create_dir_all(sharded.parent().unwrap()).unwrap();
        fs::write(&sharded, b"jpeg").unwrap();
        assert_eq!(
            existing_cache_path_in(&root, "/photos/sharded.jpg", Some(1), Some(2)).unwrap(),
            Some(sharded)
        );
        let name = cache_file_name("/photos/flat.jpg", Some(3), Some(4));
        let flat = root.join(name);
        fs::write(&flat, b"jpeg").unwrap();
        assert_eq!(
            existing_cache_path_in(&root, "/photos/flat.jpg", Some(3), Some(4)).unwrap(),
            Some(flat)
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn writing_a_thumbnail_creates_its_missing_shard_directory() {
        let root = fixture();
        let source = root.join("source.png");
        image::RgbImage::from_pixel(8, 6, image::Rgb([20, 90, 160]))
            .save(&source)
            .unwrap();
        let destination = cache_path_in(&root, "/photos/new.png", Some(5), Some(6));
        assert!(!destination.parent().unwrap().exists());
        create_uncached(source.to_str().unwrap(), &destination).unwrap();
        assert!(destination.is_file());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn exif_rotated_test_photo_is_cached_in_display_orientation() {
        let root = fixture();
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/20151128_144228.jpg");
        let destination = root.join("oriented.jpg");
        create_uncached(source.to_str().unwrap(), &destination).unwrap();
        let cached = image::open(&destination).unwrap();
        assert!(cached.height() > cached.width());
        let _ = fs::remove_dir_all(root);
    }
}
