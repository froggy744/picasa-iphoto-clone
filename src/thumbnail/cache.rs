pub fn cache_dir() -> Result<PathBuf> {
    let directory = dirs::cache_dir()
        .context("could not determine the user's cache directory")?
        .join("picasa-rs")
        .join("thumbs");
    fs::create_dir_all(&directory)
        .with_context(|| format!("could not create cache directory {}", directory.display()))?;
    Ok(directory)
}

pub fn cache_size() -> Result<u64> {
    let directory = cache_dir()?;
    let Ok(entries) = fs::read_dir(directory) else {
        return Ok(0);
    };
    Ok(entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| entry.metadata().ok())
        .filter(|metadata| metadata.is_file())
        .map(|metadata| metadata.len())
        .sum())
}

pub fn cache_path(path: &str, mtime: Option<i64>, size_bytes: Option<i64>) -> Result<PathBuf> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(path.as_bytes());
    hasher.update(THUMBNAIL_CACHE_VERSION);
    hasher.update(b"\0");
    hasher.update(mtime.unwrap_or_default().to_string().as_bytes());
    hasher.update(b"\0");
    hasher.update(size_bytes.unwrap_or_default().to_string().as_bytes());
    Ok(cache_dir()?.join(format!("{}.jpg", hasher.finalize().to_hex())))
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
        thumb_trace!(
            "THUMB TRACE cache-hit path={} cache={}",
            path,
            destination.display()
        );
        return Ok(destination);
    }
    if failure_marker.is_file() {
        if is_heif(path) {
            // HEIF was previously admitted by the scanner without having a
            // decoder, so old libraries can contain a stale failure marker.
            // Retry those files now that an HEIF decoder is available.
            let _ = fs::remove_file(&failure_marker);
        } else {
            thumb_trace!(
                "THUMB TRACE failed-cache-suppressed path={} marker={}",
                path,
                failure_marker.display()
            );
            return Ok(destination);
        }
    }
    let in_flight = IN_FLIGHT.get_or_init(|| Mutex::new(HashSet::new()));
    let claimed = in_flight
        .lock()
        .map_err(|_| anyhow::anyhow!("thumbnail in-flight registry poisoned"))?
        .insert(destination.clone());
    if !claimed {
        thumb_trace!(
            "THUMB TRACE duplicate-suppressed path={} cache={}",
            path,
            destination.display()
        );
        return Ok(destination);
    }

    let result = create_uncached(path, &destination);
    if let Ok(mut entries) = in_flight.lock() {
        entries.remove(&destination);
    }
    if result.is_err() {
        // Avoid retrying a known corrupt/unsupported source on every launch.
        // The marker is keyed by the source fingerprint, so a changed file
        // naturally gets a new cache key and can be attempted again.
        let _ = fs::write(&failure_marker, b"thumbnail generation failed\n");
    }
    result
}

fn create_uncached(path: &str, destination: &PathBuf) -> Result<PathBuf> {
    let started = Instant::now();
    thumb_trace!(
        "THUMB TRACE start path={} cache={}",
        path,
        destination.display()
    );
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    let decode_started = Instant::now();
    let (source, source_width, source_height, decoder, scale, read_ms) = if is_raw(path) {
        let decoded = decode_nef(path)?;
        (
            decoded.image,
            decoded.source_width,
            decoded.source_height,
            "rawler",
            "embedded preview",
            0,
        )
    } else {
        let read_started = Instant::now();
        let bytes = crate::source::read(path)?;
        let read_ms = read_started.elapsed().as_millis();
        if is_jpeg(path) {
            match decode_jpeg_turbo(&bytes) {
                Ok(decoded) => (
                    decoded.image,
                    decoded.source_width,
                    decoded.source_height,
                    "turbojpeg",
                    decoded.scale,
                    read_ms,
                ),
                Err(error) => {
                    thumb_trace!(
                        "THUMB TRACE turbojpeg-fallback path={} reason={}",
                        path,
                        error
                    );
                    let decoded = decode_with_image(&bytes)?;
                    (
                        decoded.image,
                        decoded.source_width,
                        decoded.source_height,
                        "image",
                        "1/1",
                        read_ms,
                    )
                }
            }
        } else if is_heif(path) {
            let decoded = decode_heif(&bytes)?;
            (
                decoded.image,
                decoded.source_width,
                decoded.source_height,
                "heif-oxide",
                "1/1",
                read_ms,
            )
        } else {
            let decoded = decode_with_image(&bytes)?;
            (
                decoded.image,
                decoded.source_width,
                decoded.source_height,
                "image",
                "1/1",
                read_ms,
            )
        }
    };
    let source =
        apply_orientation(DynamicImage::ImageRgb8(source), exif_orientation(path)).to_rgb8();
    let decode_ms = decode_started.elapsed().as_millis();
    thumb_trace!(
        "THUMB TRACE decoded path={} decoder={} scale={} source={}x{}",
        path,
        decoder,
        scale,
        source_width,
        source_height
    );

    let resize_started = Instant::now();
    let resized = resize(source)?;
    let resize_ms = resize_started.elapsed().as_millis();
    let output_width = resized.width();
    let output_height = resized.height();

    let encode_started = Instant::now();
    let mut encoded = Vec::new();
    JpegEncoder::new(&mut encoded).write_image(
        resized.as_raw(),
        output_width,
        output_height,
        ColorType::Rgb8.into(),
    )?;
    let encode_ms = encode_started.elapsed().as_millis();

    let write_started = Instant::now();
    fs::write(&destination, encoded)?;
    let write_ms = write_started.elapsed().as_millis();
    thumb_trace!(
        "THUMB PERF: path={} decoder={} scale={} source={}x{} output={}x{} read={}ms decode={}ms resize={}ms encode={}ms write={}ms total={}ms",
        path,
        decoder,
        scale,
        source_width,
        source_height,
        output_width,
        output_height,
        read_ms,
        decode_ms,
        resize_ms,
        encode_ms,
        write_ms,
        started.elapsed().as_millis()
    );
    Ok(destination.clone())
}
