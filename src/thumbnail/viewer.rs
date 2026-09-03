struct DecodedThumbnailSource {
    image: image::RgbImage,
    source_width: u32,
    source_height: u32,
    scale: &'static str,
}

static VIEWER_ORIENTATION_CACHE: OnceLock<Mutex<HashMap<(PathBuf, u128), u16>>> = OnceLock::new();

fn is_jpeg(path: &str) -> bool {
    crate::image_format::uses(path, crate::image_format::DecoderKind::TurboJpeg)
}

fn is_heif(path: &str) -> bool {
    crate::image_format::uses(path, crate::image_format::DecoderKind::Heif)
}

fn is_raw(path: &str) -> bool {
    crate::image_format::uses(path, crate::image_format::DecoderKind::Raw)
}

fn decode_heif(bytes: &[u8]) -> Result<DecodedThumbnailSource> {
    let decoded = heif_oxide::decode_bytes(bytes).context("HEIC/HEIF decode failed")?;
    let image = image::RgbaImage::from_raw(decoded.width, decoded.height, decoded.to_rgba8())
        .context("HEIC/HEIF decoder returned an invalid pixel buffer")?;
    Ok(DecodedThumbnailSource {
        source_width: decoded.width,
        source_height: decoded.height,
        image: DynamicImage::ImageRgba8(image).to_rgb8(),
        scale: "1/1",
    })
}

fn decode_nef(reference: &str) -> Result<DecodedThumbnailSource> {
    let local_path = crate::source::materialize(reference)?;
    if let Some(thumbnail) = nef_uncompressed_thumbnail(&local_path)? {
        return Ok(thumbnail);
    }
    if let Some(bytes) = nef_embedded_thumbnail(&local_path)? {
        // Nikon writes this tiny JPEG in IFD1. It is vastly faster than
        // decoding the full-size camera preview just to make a 320px tile.
        let decoded = decode_jpeg_turbo(&bytes).or_else(|_| decode_with_image(&bytes))?;
        return Ok(decoded);
    }

    thumb_trace!(
        "THUMB TRACE NEF thumbnail missing; falling back to preview decode path={reference}"
    );
    let image = rawler::analyze::extract_thumbnail_pixels(
        local_path,
        &rawler::decoders::RawDecodeParams::default(),
    )?;
    let source_width = image.width();
    let source_height = image.height();
    Ok(DecodedThumbnailSource {
        image: image.to_rgb8(),
        source_width,
        source_height,
        scale: "embedded preview",
    })
}

/// Decode a display-quality image for the lightbox. `viewport_width` and
/// `viewport_height` are physical display pixels, already adjusted for any
/// user rotation. RAW files prefer the largest embedded JPEG preview and JPEG
/// sources use TurboJPEG's native DCT scaling before the final small resize.
pub fn decode_for_viewer(
    reference: &str,
    viewport_width: u32,
    viewport_height: u32,
) -> Result<image::RgbaImage> {
    decode_for_viewer_with_cancel(reference, viewport_width, viewport_height, || false)
}

pub fn decode_for_viewer_with_cancel<F>(
    reference: &str,
    viewport_width: u32,
    viewport_height: u32,
    cancelled: F,
) -> Result<image::RgbaImage>
where
    F: Fn() -> bool,
{
    let started = Instant::now();
    thumb_trace!("VIEW TRACE decode_start path={reference}");

    check_viewer_cancelled(&cancelled, "before_orientation_metadata")?;
    let stage = Instant::now();
    // HEIF container transforms are applied by heif-oxide during decode.
    let orientation = if is_heif(reference) {
        1
    } else {
        exif_orientation(reference)
    };
    thumb_trace!(
        "VIEW PERF orientation_metadata_ms={} orientation={}",
        stage.elapsed().as_millis(),
        orientation
    );
    check_viewer_cancelled(&cancelled, "after_orientation_metadata")?;

    let (image, source_width, source_height, target_width, target_height) = if is_raw(reference) {
        let read_started = Instant::now();
        let stage = Instant::now();
        let local_path = crate::source::materialize(reference)?;
        check_viewer_cancelled(&cancelled, "after_materialize")?;
        thumb_trace!(
            "VIEW TRACE materialize_ms={} path={}",
            stage.elapsed().as_millis(),
            local_path.display()
        );
        let stage = Instant::now();
        if let Some(bytes) = nef_embedded_preview(&local_path)? {
            check_viewer_cancelled(&cancelled, "after_embedded_preview_read")?;
            let scan_ms = stage.elapsed().as_millis();
            thumb_trace!(
                "VIEW TRACE embedded_scan_ms={} bytes={}",
                scan_ms,
                bytes.len()
            );
            thumb_trace!(
                "VIEW PERF read_ms={} format=raw_embedded bytes={}",
                read_started.elapsed().as_millis(),
                bytes.len()
            );

            let (source_width, source_height) = jpeg_dimensions(&bytes)?;
            let target_started = Instant::now();
            let (target_width, target_height) = viewer_target_dimensions(
                source_width,
                source_height,
                orientation,
                viewport_width,
                viewport_height,
            );
            trace_viewer_target(
                target_started,
                source_width,
                source_height,
                orientation,
                viewport_width,
                viewport_height,
                target_width,
                target_height,
            );

            let stage = Instant::now();
            check_viewer_cancelled(&cancelled, "before_turbojpeg_decode")?;
            let (decoded, decoder) =
                match decode_jpeg_turbo_with_target(&bytes, target_width, target_height) {
                    Ok(decoded) => (decoded, "turbojpeg"),
                    Err(error) => {
                        thumb_trace!("VIEW TRACE turbojpeg_fallback reason={error}");
                        (decode_with_image(&bytes)?, "image")
                    }
                };
            check_viewer_cancelled(&cancelled, "after_turbojpeg_decode")?;
            thumb_trace!(
                "VIEW PERF decode_ms={} format=raw_embedded decoder={} dct_scale={} source={}x{} output={}x{}",
                stage.elapsed().as_millis(),
                decoder,
                decoded.scale,
                decoded.source_width,
                decoded.source_height,
                decoded.image.width(),
                decoded.image.height()
            );
            (
                DynamicImage::ImageRgb8(decoded.image),
                source_width,
                source_height,
                target_width,
                target_height,
            )
        } else {
            thumb_trace!(
                "VIEW TRACE embedded_scan_ms={} result=none",
                stage.elapsed().as_millis()
            );
            thumb_trace!(
                "VIEW PERF read_ms={} format=raw_preview",
                read_started.elapsed().as_millis()
            );
            let stage = Instant::now();
            check_viewer_cancelled(&cancelled, "before_raw_preview_decode")?;
            let image = rawler::analyze::extract_preview_pixels(
                local_path,
                &rawler::decoders::RawDecodeParams::default(),
            )?;
            check_viewer_cancelled(&cancelled, "after_raw_preview_decode")?;
            let source_width = image.width();
            let source_height = image.height();
            thumb_trace!(
                "VIEW PERF decode_ms={} format=raw_preview decoder=rawler source={}x{} output={}x{}",
                stage.elapsed().as_millis(),
                source_width,
                source_height,
                source_width,
                source_height
            );
            let target_started = Instant::now();
            let (target_width, target_height) = viewer_target_dimensions(
                source_width,
                source_height,
                orientation,
                viewport_width,
                viewport_height,
            );
            trace_viewer_target(
                target_started,
                source_width,
                source_height,
                orientation,
                viewport_width,
                viewport_height,
                target_width,
                target_height,
            );
            (
                image,
                source_width,
                source_height,
                target_width,
                target_height,
            )
        }
    } else if is_jpeg(reference) {
        let stage = Instant::now();
        let bytes = crate::source::read(reference)?;
        thumb_trace!(
            "VIEW PERF read_ms={} format=jpeg bytes={}",
            stage.elapsed().as_millis(),
            bytes.len()
        );

        let (source_width, source_height) = jpeg_dimensions(&bytes)?;
        let target_started = Instant::now();
        let (target_width, target_height) = viewer_target_dimensions(
            source_width,
            source_height,
            orientation,
            viewport_width,
            viewport_height,
        );
        trace_viewer_target(
            target_started,
            source_width,
            source_height,
            orientation,
            viewport_width,
            viewport_height,
            target_width,
            target_height,
        );

        let stage = Instant::now();
        check_viewer_cancelled(&cancelled, "before_jpeg_decode")?;
        let (decoded, decoder) =
            match decode_jpeg_turbo_with_target(&bytes, target_width, target_height) {
                Ok(decoded) => (decoded, "turbojpeg"),
                Err(error) => {
                    thumb_trace!("VIEW TRACE turbojpeg_fallback reason={error}");
                    (decode_with_image(&bytes)?, "image")
                }
            };
        check_viewer_cancelled(&cancelled, "after_jpeg_decode")?;
        thumb_trace!(
            "VIEW PERF decode_ms={} format=jpeg decoder={} dct_scale={} source={}x{} output={}x{}",
            stage.elapsed().as_millis(),
            decoder,
            decoded.scale,
            decoded.source_width,
            decoded.source_height,
            decoded.image.width(),
            decoded.image.height()
        );
        (
            DynamicImage::ImageRgb8(decoded.image),
            source_width,
            source_height,
            target_width,
            target_height,
        )
    } else if is_heif(reference) {
        let stage = Instant::now();
        let bytes = crate::source::read(reference)?;
        let read_ms = stage.elapsed().as_millis();
        let stage = Instant::now();
        check_viewer_cancelled(&cancelled, "before_heif_decode")?;
        let decoded = decode_heif(&bytes)?;
        check_viewer_cancelled(&cancelled, "after_heif_decode")?;
        let source_width = decoded.source_width;
        let source_height = decoded.source_height;
        let image = DynamicImage::ImageRgb8(decoded.image);
        let (target_width, target_height) = viewer_target_dimensions(
            source_width,
            source_height,
            orientation,
            viewport_width,
            viewport_height,
        );
        thumb_trace!(
            "VIEW PERF read_ms={} decode_ms={} format=heif decoder=heif-oxide source={}x{}",
            read_ms,
            stage.elapsed().as_millis(),
            source_width,
            source_height
        );
        (
            image,
            source_width,
            source_height,
            target_width,
            target_height,
        )
    } else {
        let stage = Instant::now();
        let bytes = crate::source::read(reference)?;
        thumb_trace!(
            "VIEW PERF read_ms={} format=generic bytes={}",
            stage.elapsed().as_millis(),
            bytes.len()
        );
        let reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
        let format = reader
            .format()
            .map(|format| {
                format
                    .extensions_str()
                    .first()
                    .copied()
                    .unwrap_or("unknown")
            })
            .unwrap_or("unknown");
        let stage = Instant::now();
        check_viewer_cancelled(&cancelled, "before_generic_decode")?;
        let image = reader.decode()?;
        check_viewer_cancelled(&cancelled, "after_generic_decode")?;
        let source_width = image.width();
        let source_height = image.height();
        thumb_trace!(
            "VIEW PERF decode_ms={} format={} decoder=image source={}x{} output={}x{}",
            stage.elapsed().as_millis(),
            format,
            source_width,
            source_height,
            source_width,
            source_height
        );
        let target_started = Instant::now();
        let (target_width, target_height) = viewer_target_dimensions(
            source_width,
            source_height,
            orientation,
            viewport_width,
            viewport_height,
        );
        trace_viewer_target(
            target_started,
            source_width,
            source_height,
            orientation,
            viewport_width,
            viewport_height,
            target_width,
            target_height,
        );
        (
            image,
            source_width,
            source_height,
            target_width,
            target_height,
        )
    };

    let stage = Instant::now();
    check_viewer_cancelled(&cancelled, "before_pixel_conversion")?;
    let image = image.into_rgba8();
    let resize_input_width = image.width();
    let resize_input_height = image.height();
    thumb_trace!(
        "VIEW PERF pixel_conversion_ms={} output={}x{}",
        stage.elapsed().as_millis(),
        image.width(),
        image.height()
    );

    // Resize before orientation. Rotating only display-sized pixels avoids a
    // large copy for orientations 5-8.
    let stage = Instant::now();
    check_viewer_cancelled(&cancelled, "before_resize")?;
    let skipped_resize = image.width() == target_width && image.height() == target_height;
    let image = resize_viewer_rgba(image, target_width, target_height)?;
    check_viewer_cancelled(&cancelled, "after_resize")?;
    thumb_trace!(
        "VIEW PERF resize_ms={} filter=hamming skipped={} full_source={}x{} input={}x{} output={}x{}",
        stage.elapsed().as_millis(),
        skipped_resize,
        source_width,
        source_height,
        resize_input_width,
        resize_input_height,
        image.width(),
        image.height()
    );

    let stage = Instant::now();
    check_viewer_cancelled(&cancelled, "before_final_orientation")?;
    let oriented = apply_orientation(DynamicImage::ImageRgba8(image), orientation).into_rgba8();
    check_viewer_cancelled(&cancelled, "after_final_orientation")?;
    thumb_trace!(
        "VIEW PERF orientation_ms={} orientation={} output={}x{}",
        stage.elapsed().as_millis(),
        orientation,
        oriented.width(),
        oriented.height()
    );
    thumb_trace!(
        "VIEW PERF viewer_pipeline_total_ms={} output={}x{}",
        started.elapsed().as_millis(),
        oriented.width(),
        oriented.height()
    );
    Ok(oriented)
}

fn check_viewer_cancelled<F>(cancelled: &F, stage: &str) -> Result<()>
where
    F: Fn() -> bool,
{
    if cancelled() {
        thumb_trace!("VIEW TRACE viewer_decode_cancelled stage={stage}");
        anyhow::bail!("cancelled at {stage}");
    }
    Ok(())
}

fn trace_viewer_target(
    started: Instant,
    source_width: u32,
    source_height: u32,
    orientation: u16,
    viewport_width: u32,
    viewport_height: u32,
    target_width: u32,
    target_height: u32,
) {
    thumb_trace!(
        "VIEW PERF target_calculation_ms={} viewport={}x{} source={}x{} orientation={} target_pre_orientation={}x{}",
        started.elapsed().as_millis(),
        viewport_width,
        viewport_height,
        source_width,
        source_height,
        orientation,
        target_width,
        target_height
    );
}

fn viewer_target_dimensions(
    source_width: u32,
    source_height: u32,
    orientation: u16,
    viewport_width: u32,
    viewport_height: u32,
) -> (u32, u32) {
    let swaps_axes = matches!(orientation, 5..=8);
    let (oriented_width, oriented_height) = if swaps_axes {
        (source_height, source_width)
    } else {
        (source_width, source_height)
    };
    let scale = (viewport_width.max(1) as f64 / oriented_width.max(1) as f64)
        .min(viewport_height.max(1) as f64 / oriented_height.max(1) as f64)
        .min(1.0);
    let target_oriented_width =
        ((oriented_width as f64 * scale).round() as u32).clamp(1, oriented_width.max(1));
    let target_oriented_height =
        ((oriented_height as f64 * scale).round() as u32).clamp(1, oriented_height.max(1));
    if swaps_axes {
        (target_oriented_height, target_oriented_width)
    } else {
        (target_oriented_width, target_oriented_height)
    }
}

fn resize_viewer_rgba(
    source: image::RgbaImage,
    destination_width: u32,
    destination_height: u32,
) -> Result<image::RgbaImage> {
    if source.width() == destination_width && source.height() == destination_height {
        return Ok(source);
    }
    let source_width = source.width();
    let source_height = source.height();
    let source_image = Image::from_vec_u8(
        source_width,
        source_height,
        source.into_raw(),
        PixelType::U8x4,
    )?;
    let mut destination_image = Image::new(destination_width, destination_height, PixelType::U8x4);
    // Hamming is the resizer's compact photographic downscale filter. It has
    // bilinear-class cost with sharper downscale quality and avoids the much
    // wider default Lanczos3 kernel used by ResizeOptions::default().
    let options = ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::Hamming));
    Resizer::new().resize(&source_image, &mut destination_image, &options)?;
    image::RgbaImage::from_raw(
        destination_width,
        destination_height,
        destination_image.into_vec(),
    )
    .context("viewer resizer returned an invalid buffer")
}

pub fn exif_orientation(reference: &str) -> u16 {
    let local = match crate::source::materialize(reference) {
        Ok(path) => path,
        Err(_) => return 1,
    };
    let modified = fs::metadata(&local)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let key = (local.clone(), modified);
    let cache = VIEWER_ORIENTATION_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(orientation) = cache.lock().unwrap().get(&key).copied() {
        thumb_trace!("VIEW PERF orientation_cache_hit path={}", reference);
        return orientation;
    }

    let orientation = fs::File::open(local)
        .ok()
        .and_then(|file| exif::Reader::new().read_from_container(&mut BufReader::new(file)).ok())
        .and_then(|exif| {
            exif.get_field(exif::Tag::Orientation, exif::In::PRIMARY)
        .and_then(|field| match &field.value {
            exif::Value::Short(values) => values.first().copied(),
            exif::Value::Long(values) => values.first().copied().map(|value| value as u16),
            _ => None,
        })
        .filter(|orientation| (1..=8).contains(orientation))
        })
        .unwrap_or(1);
    let mut cache = cache.lock().unwrap();
    if cache.len() >= 256 {
        if let Some(oldest) = cache.keys().next().cloned() {
            cache.remove(&oldest);
        }
    }
    cache.insert(key, orientation);
    orientation
}

pub fn apply_orientation(image: DynamicImage, orientation: u16) -> DynamicImage {
    match orientation {
        2 => DynamicImage::ImageRgba8(image::imageops::flip_horizontal(&image.to_rgba8())),
        3 => DynamicImage::ImageRgba8(image::imageops::rotate180(&image.to_rgba8())),
        4 => DynamicImage::ImageRgba8(image::imageops::flip_vertical(&image.to_rgba8())),
        5 => DynamicImage::ImageRgba8(image::imageops::rotate270(
            &image::imageops::flip_horizontal(&image.to_rgba8()),
        )),
        6 => DynamicImage::ImageRgba8(image::imageops::rotate90(&image.to_rgba8())),
        7 => DynamicImage::ImageRgba8(image::imageops::rotate90(
            &image::imageops::flip_horizontal(&image.to_rgba8()),
        )),
        8 => DynamicImage::ImageRgba8(image::imageops::rotate270(&image.to_rgba8())),
        _ => image,
    }
}
