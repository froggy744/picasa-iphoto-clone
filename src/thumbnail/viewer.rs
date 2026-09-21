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

fn is_nikon_raw(path: &str) -> bool {
    crate::image_format::for_path(path).is_some_and(|format| format.id == "nikon_raw")
}

fn is_dng(path: &str) -> bool {
    crate::image_format::for_path(path).is_some_and(|format| format.id == "dng")
}

fn decode_raw_thumbnail(reference: &str) -> Result<DecodedThumbnailSource> {
    std::panic::catch_unwind(|| decode_raw_thumbnail_inner(reference))
        .map_err(|_| anyhow::anyhow!("RAW thumbnail decoder panicked"))?
}

fn decode_raw_thumbnail_inner(reference: &str) -> Result<DecodedThumbnailSource> {
    #[cfg(target_os="linux")]
    if crate::network_shares::private(reference){anyhow::ensure!(is_nikon_raw(reference),"Remote RAW preview is unsupported for this format; original was not downloaded");let bytes=remote_nef_embedded_jpeg(reference,false)?.ok_or_else(||anyhow::anyhow!("Remote NEF has no embedded JPEG thumbnail; original was not downloaded"))?;return decode_jpeg_turbo(&bytes).or_else(|_|decode_with_image(&bytes));}
    let local_path = crate::source::materialize(reference)?;
    let mut failures = Vec::new();

    // Prefer rawler's larger decoded preview for the cached thumbnail. This is
    // the generic path used by DNG and every other supported RAW format.
    match rawler::analyze::extract_preview_pixels(
        local_path.clone(),
        &rawler::decoders::RawDecodeParams::default(),
    ) {
        Ok(image) => {
            let source_width = image.width();
            let source_height = image.height();

            return Ok(DecodedThumbnailSource {
                image: image.to_rgb8(),
                source_width,
                source_height,
                scale: "raw preview",
            });
        }
        Err(error) => {
            failures.push(format!("preview extraction: {error}"));

        }
    }

    // These fallbacks understand Nikon's unusual embedded thumbnail layout;
    // applying them to DNG/CR2/ARW/etc. can misinterpret unrelated TIFF data.
    if is_nikon_raw(reference) {
        match nef_uncompressed_thumbnail(&local_path) {
            Ok(Some(thumbnail)) => return Ok(thumbnail),
            Ok(None) => failures.push("Nikon uncompressed thumbnail: not found".into()),
            Err(error) => failures.push(format!("Nikon uncompressed thumbnail: {error}")),
        }
        match nef_embedded_thumbnail(&local_path) {
            Ok(Some(bytes)) => {
                // Nikon writes this tiny JPEG in IFD1. It is vastly faster
                // than decoding the full-size camera preview for a tile.
                match decode_jpeg_turbo(&bytes).or_else(|_| decode_with_image(&bytes)) {
                    Ok(decoded) => return Ok(decoded),
                    Err(error) => failures.push(format!("Nikon JPEG thumbnail decode: {error}")),
                }
            }
            Ok(None) => failures.push("Nikon JPEG thumbnail: not found".into()),
            Err(error) => failures.push(format!("Nikon JPEG thumbnail: {error}")),
        }
    }

    match rawler::analyze::extract_thumbnail_pixels(
        &local_path,
        &rawler::decoders::RawDecodeParams::default(),
    ) {
        Ok(image) => {
            let source_width = image.width();
            let source_height = image.height();
            return Ok(DecodedThumbnailSource {
                image: image.to_rgb8(),
                source_width,
                source_height,
                scale: "raw thumbnail",
            });
        }
        Err(error) => {
            failures.push(format!("thumbnail extraction: {error}"));
        }
    }

    // Samsung DNGs can have an incomplete preview after intact sensor data.
    // Only try the expensive recovery after both embedded strategies fail.
    if is_dng(reference) {
        match decode_dng_sensor_thumbnail(&local_path) {
            Ok(decoded) => return Ok(decoded),
            Err(error) => failures.push(format!("full RAW recovery: {error:#}")),
        }
    }
    Err(anyhow::anyhow!(
        "RAW thumbnail strategies failed: {}",
        failures.join("; ")
    ))
}

fn decode_dng_sensor_thumbnail(path: &Path) -> Result<DecodedThumbnailSource> {
    // Serialize full-sensor recovery across bulk and visible workers, and
    // keep rawler's internal Rayon work off the unbounded global pool.
    static POOL: OnceLock<std::result::Result<Mutex<rayon::ThreadPool>, rayon::ThreadPoolBuildError>> =
        OnceLock::new();
    let pool = POOL.get_or_init(|| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .build()
            .map(Mutex::new)
    });
    let pool = pool.as_ref().map_err(|error| anyhow::anyhow!("DNG recovery pool: {error}"))?;
    let pool = pool.lock().map_err(|_| anyhow::anyhow!("DNG recovery pool poisoned"))?;
    let path = path.to_owned();
    let (send, receive) = std::sync::mpsc::sync_channel(1);
    pool.spawn(move || {
        let result = std::panic::catch_unwind(|| -> Result<DecodedThumbnailSource> {
            let image = rawler::analyze::extract_full_pixels(
                &path,
                &rawler::decoders::RawDecodeParams::default(),
            )?;
            let (source_width, source_height) = (image.width(), image.height());
            Ok(DecodedThumbnailSource {
                image: resize(image.into_rgb8())?,
                source_width,
                source_height,
                scale: "full RAW recovery",
            })
        })
        .unwrap_or_else(|_| Err(anyhow::anyhow!("DNG full RAW decoder panicked")));
        let _ = send.send(result);
    });
    // Blocking receive deliberately avoids install(): a caller in a Rayon
    // batch must not steal another recovery task while holding this mutex.
    receive.recv().context("DNG recovery worker disconnected")?
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
    decode_for_viewer_with_cancel(reference, viewport_width, viewport_height, None, || false)
}

pub fn decode_for_viewer_with_cancel<F>(
    reference: &str,
    viewport_width: u32,
    viewport_height: u32,
    read_context: Option<&crate::source::ViewerReadContext>,
    cancelled: F,
) -> Result<image::RgbaImage>
where
    F: Fn() -> bool,
{
    let decode_started = std::time::Instant::now();
    let mut source_read_ms = 0;
    check_viewer_cancelled(&cancelled, "before_orientation_metadata")?;
    // HEIF container transforms are applied by heif-oxide during decode.
    let orientation = if is_heif(reference) {
        1
    } else {
        exif_orientation(reference)
    };
    check_viewer_cancelled(&cancelled, "after_orientation_metadata")?;

    let (image, target_width, target_height) = if is_raw(reference) {
        #[cfg(target_os="linux")]
        if crate::network_shares::private(reference){anyhow::ensure!(is_nikon_raw(reference),"Remote RAW preview is unsupported for this format; original was not downloaded");check_viewer_cancelled(&cancelled,"before_remote_embedded_preview")?;let bytes=remote_nef_embedded_jpeg(reference,true)?.ok_or_else(||anyhow::anyhow!("Remote NEF has no embedded JPEG preview; original was not downloaded"))?;check_viewer_cancelled(&cancelled,"after_remote_embedded_preview")?;let(source_width,source_height)=jpeg_dimensions(&bytes)?;let(target_width,target_height)=viewer_target_dimensions(source_width,source_height,orientation,viewport_width,viewport_height);let decoded=decode_jpeg_turbo_with_target(&bytes,target_width,target_height).or_else(|_|decode_with_image(&bytes))?;(DynamicImage::ImageRgb8(decoded.image),target_width,target_height)}else {
        let local_path = crate::source::materialize(reference)?;
        check_viewer_cancelled(&cancelled, "after_materialize")?;

        if let Some(bytes) = nef_embedded_preview(&local_path)? {
            check_viewer_cancelled(&cancelled, "after_embedded_preview_read")?;
            let (source_width, source_height) = jpeg_dimensions(&bytes)?;
            let (target_width, target_height) = viewer_target_dimensions(
                source_width,
                source_height,
                orientation,
                viewport_width,
                viewport_height,
            );

            check_viewer_cancelled(&cancelled, "before_turbojpeg_decode")?;
            let decoded = match decode_jpeg_turbo_with_target(&bytes, target_width, target_height) {
                Ok(decoded) => decoded,
                Err(_) => decode_with_image(&bytes)?,
            };
            check_viewer_cancelled(&cancelled, "after_turbojpeg_decode")?;
            (
                DynamicImage::ImageRgb8(decoded.image),
                target_width,
                target_height,
            )
        } else {
            check_viewer_cancelled(&cancelled, "before_raw_preview_decode")?;
            let raw_params = rawler::decoders::RawDecodeParams::default();
            let image = match rawler::analyze::extract_preview_pixels(&local_path, &raw_params) {
                Ok(image) => image,
                Err(preview_error) => {
                    // Some DNG files have a truncated embedded preview while
                    // their sensor data is still readable. Develop the RAW
                    // image as a viewer fallback. DNG thumbnails have a
                    // separate bounded recovery path after preview failure.
                    check_viewer_cancelled(&cancelled, "before_full_raw_decode")?;
                    let full = rawler::analyze::extract_full_pixels(&local_path, &raw_params)
                        .map_err(|full_error| {
                            anyhow::anyhow!(
                                "RAW viewer strategies failed: preview: {preview_error}; full RAW: {full_error}"
                            )
                        })?;
                    check_viewer_cancelled(&cancelled, "after_full_raw_decode")?;
                    full
                }
            };
            check_viewer_cancelled(&cancelled, "after_raw_preview_decode")?;
            let source_width = image.width();
            let source_height = image.height();
            let (target_width, target_height) = viewer_target_dimensions(
                source_width,
                source_height,
                orientation,
                viewport_width,
                viewport_height,
            );
            (image, target_width, target_height)
        }}
    } else if is_jpeg(reference) {
        // A network read is synchronous and the SMB/NFS backends serialize
        // their sessions. Bail out before entering that lock when navigation
        // has already made this request obsolete.
        check_viewer_cancelled(&cancelled, "before_source_read")?;
        let source_started = std::time::Instant::now();
        let bytes = read_viewer_source(reference, read_context)?;
        source_read_ms += source_started.elapsed().as_millis();
        let (source_width, source_height) = jpeg_dimensions(&bytes)?;
        let (target_width, target_height) = viewer_target_dimensions(
            source_width,
            source_height,
            orientation,
            viewport_width,
            viewport_height,
        );

        check_viewer_cancelled(&cancelled, "before_jpeg_decode")?;
        let decoded = match decode_jpeg_turbo_with_target(&bytes, target_width, target_height) {
            Ok(decoded) => decoded,
            Err(_) => decode_with_image(&bytes)?,
        };
        check_viewer_cancelled(&cancelled, "after_jpeg_decode")?;
        (
            DynamicImage::ImageRgb8(decoded.image),
            target_width,
            target_height,
        )
    } else if is_heif(reference) {
        check_viewer_cancelled(&cancelled, "before_source_read")?;
        let source_started = std::time::Instant::now();
        let bytes = read_viewer_source(reference, read_context)?;
        source_read_ms += source_started.elapsed().as_millis();
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
        (image, target_width, target_height)
    } else {
        check_viewer_cancelled(&cancelled, "before_source_read")?;
        let source_started = std::time::Instant::now();
        let bytes = read_viewer_source(reference, read_context)?;
        source_read_ms += source_started.elapsed().as_millis();
        let reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
        check_viewer_cancelled(&cancelled, "before_generic_decode")?;
        let image = reader.decode()?;
        check_viewer_cancelled(&cancelled, "after_generic_decode")?;
        let source_width = image.width();
        let source_height = image.height();
        let (target_width, target_height) = viewer_target_dimensions(
            source_width,
            source_height,
            orientation,
            viewport_width,
            viewport_height,
        );
        (image, target_width, target_height)
    };

    check_viewer_cancelled(&cancelled, "before_pixel_conversion")?;
    let image = image.into_rgba8();

    // Resize before orientation. Rotating only display-sized pixels avoids a
    // large copy for orientations 5-8.
    check_viewer_cancelled(&cancelled, "before_resize")?;
    let image = resize_viewer_rgba(image, target_width, target_height)?;
    check_viewer_cancelled(&cancelled, "after_resize")?;

    check_viewer_cancelled(&cancelled, "before_final_orientation")?;
    let oriented = apply_orientation(DynamicImage::ImageRgba8(image), orientation).into_rgba8();
    check_viewer_cancelled(&cancelled, "after_final_orientation")?;
    if std::env::var_os("PICASA_TRACE").is_some() {
        let total_ms = decode_started.elapsed().as_millis();
        eprintln!(
            "PIC_VIEWER decode_stage source_ms={} cpu_ms={} uri={}",
            source_read_ms,
            total_ms.saturating_sub(source_read_ms),
            viewer_trace_reference(reference),
        );
    }
    Ok(oriented)
}

fn read_viewer_source(
    reference: &str,
    context: Option<&crate::source::ViewerReadContext>,
) -> Result<std::sync::Arc<[u8]>> {
    match context {
        Some(context) => Ok(crate::source::read_for_viewer(reference, context)?.bytes),
        None => Ok(crate::source::read(reference)?.into()),
    }
}

fn viewer_trace_reference(reference: &str) -> String {
    let Some((scheme, rest)) = reference.split_once("://") else {
        return reference.to_owned();
    };
    let safe_rest = rest.rsplit_once('@').map(|(_, value)| value).unwrap_or(rest);
    format!("{scheme}://{safe_rest}")
}

fn check_viewer_cancelled<F>(cancelled: &F, stage: &str) -> Result<()>
where
    F: Fn() -> bool,
{
    if cancelled() {

        anyhow::bail!("cancelled at {stage}");
    }
    Ok(())
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
    #[cfg(target_os="linux")]
    if crate::network_shares::private(reference) && is_nikon_raw(reference) {
        return remote_nef_orientation(reference).unwrap_or(1);
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn cancelled_remote_jpeg_stops_before_source_read() {
        let checks = Cell::new(0);
        let error = decode_for_viewer_with_cancel(
            "smb://example.invalid/share/photo.jpg",
            1124,
            794,
            None,
            || {
                let next = checks.get() + 1;
                checks.set(next);
                next >= 3
            },
        )
        .expect_err("cancelled request must not attempt a network read");
        assert!(error.to_string().contains("cancelled at before_source_read"));
        assert_eq!(checks.get(), 3);
    }
}

#[cfg(test)]
mod raw_thumbnail_tests {
    use super::*;

    #[test]
    fn non_nikon_raw_never_uses_nikon_thumbnail_fallbacks() {
        for path in ["photo.dng", "photo.DNG", "photo.CR2", "photo.ARW", "photo.RAF"] {
            assert!(is_raw(path));
            assert!(!is_nikon_raw(path));
        }
        assert!(is_nikon_raw("photo.NEF"));
        assert!(is_nikon_raw("photo.nrw"));
        assert!(is_dng("photo.DNG"));
        assert!(!is_dng("photo.NEF"));
    }

    #[test]
    fn broken_dng_returns_all_strategy_errors_and_releases_recovery_worker() {
        use rayon::prelude::*;

        let directory = std::env::temp_dir().join(format!(
            "picasa-broken-dng-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        fs::create_dir(&directory).unwrap();
        let path = directory.join("broken.DNG");
        fs::write(&path, b"II\x2a\x00\x08\x00\x00\x00").unwrap();
        let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap();
        // Exercise callers from a parallel bulk pool as well as repeated
        // failures, so an error cannot leave the recovery mutex held.
        pool.install(|| {
            (0..4).into_par_iter().for_each(|_| {
                let error = decode_raw_thumbnail(path.to_str().unwrap()).err().unwrap().to_string();
                assert!(error.contains("preview extraction:"), "{error}");
                assert!(error.contains("thumbnail extraction:"), "{error}");
                assert!(error.contains("full RAW recovery:"), "{error}");
                assert!(!error.contains("Nikon"), "{error}");
            });
        });
        fs::remove_file(path).unwrap();
        fs::remove_dir(directory).unwrap();
    }

    #[test]
    #[ignore = "set PICASA_TEST_DNG to a DNG with a broken preview and readable sensor data"]
    fn recovers_requested_dng_with_broken_preview() {
        let path = std::env::var("PICASA_TEST_DNG").expect("PICASA_TEST_DNG must name a fixture");
        let before = fs::metadata(&path).unwrap();
        let params = rawler::decoders::RawDecodeParams::default();
        assert!(rawler::analyze::extract_preview_pixels(&path, &params).is_err());
        assert!(rawler::analyze::extract_thumbnail_pixels(&path, &params).is_err());
        let decoded = decode_raw_thumbnail(&path).unwrap();
        assert_eq!(decoded.scale, "full RAW recovery");
        assert!(decoded.source_width > THUMBNAIL_SIZE);
        assert!(decoded.source_height > THUMBNAIL_SIZE);
        assert_eq!(decoded.image.width().max(decoded.image.height()), THUMBNAIL_SIZE);

        let destination = std::env::temp_dir().join(format!("picasa-dng-test-{}.jpg", std::process::id()));
        create_uncached(&path, &destination).unwrap();
        let cached = image::open(&destination).unwrap();
        assert_eq!(cached.width().max(cached.height()), THUMBNAIL_SIZE);
        fs::remove_file(destination).unwrap();
        let after = fs::metadata(&path).unwrap();
        assert_eq!(before.len(), after.len());
        assert_eq!(before.modified().unwrap(), after.modified().unwrap());
    }
}
