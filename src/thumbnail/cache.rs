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
    } else if is_dng(path) {
        DNG_THUMBNAIL_CACHE_VERSION
    } else if crate::image_format::uses(
        path,
        crate::image_format::DecoderKind::Raw,
    ) {
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

fn remote_nef(path:&str)->bool {
    #[cfg(target_os="linux")]
    {crate::network_shares::private(path) && is_nikon_raw(path)}
    #[cfg(not(target_os="linux"))]
    {let _=path;false}
}

pub fn existing_cache_path(
    path: &str,
    mtime: Option<i64>,
    size_bytes: Option<i64>,
) -> Result<Option<PathBuf>> {
    let candidate = cache_path(path, mtime, size_bytes)?;
    Ok(candidate.is_file().then_some(candidate))
}

pub fn create(path: &str, mtime: Option<i64>, size_bytes: Option<i64>) -> Result<PathBuf> {
    let destination = cache_path(path, mtime, size_bytes)?;
    let failure_marker = destination.with_extension("failed");
    if destination.is_file() {
        if std::env::var_os("PICASA_TRACE").is_some(){eprintln!("PIC_THUMBNAIL cache_hit uri={path} cache={}",destination.display());}
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
    if std::env::var_os("PICASA_TRACE").is_some(){match &result{Ok(cache)=>eprintln!("PIC_THUMBNAIL cache_write uri={path} cache={}",cache.display()),Err(error)=>eprintln!("PIC_THUMBNAIL failed uri={path} error={error:#}")}}
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
