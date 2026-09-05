pub fn create_many(
    items: &[(String, Option<i64>, Option<i64>)],
    completed: impl Fn(&str) + Sync,
) -> Vec<Result<PathBuf>> {
    use rayon::prelude::*;

    // A bounded pool avoids saturating the CPU and external disk at once.
    // The default Rayon pool was starting too many full-resolution decodes,
    // making each thumbnail slower instead of faster.
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(thumbnail_worker_threads(items))
        .build()
        .expect("thumbnail worker pool should be constructible");
    pool.install(|| {
        items
            .par_iter()
            .map(|(path, mtime, size)| {
                let result = create(path, *mtime, *size);
                completed(path);
                result
            })
            .collect()
    })
}

/// The current decode is allowed to complete, but no further source image is
/// opened after cancellation. This keeps Stop responsive without leaving a
/// half-written thumbnail (writes happen only at the end of `create`).
pub fn create_many_cancellable(
    items: &[(String, Option<i64>, Option<i64>)],
    cancelled: impl Fn() -> bool + Sync,
    completed: impl Fn(&str) + Sync,
) -> Vec<Option<Result<PathBuf>>> {
    use rayon::prelude::*;

    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(thumbnail_worker_threads(items))
        .build()
        .expect("thumbnail worker pool should be constructible");
    pool.install(|| {
        items
            .par_iter()
            .map(|(path, mtime, size)| {
                if cancelled() {
                    return None;
                }
                let result = create(path, *mtime, *size);
                completed(path);
                Some(result)
            })
            .collect()
    })
}

/// Size the thumbnail worker pool to the machine instead of a flat constant.
/// A hardcoded `4` leaves most cores idle on anything bigger than a quad-core
/// laptop, which is a large part of why bulk imports feel slow. The pool is
/// still capped (not `available_parallelism()` unbounded) because thumbnail
/// generation also does a fair amount of disk I/O per item; if profiling
/// with `PICASA_TRACE=1` shows the `read_ms`/`write_ms` fields dominating
/// `THUMB PERF` lines on your machine (e.g. a slow spinning disk or network
/// share), lower this cap — more threads won't help an I/O-bound workload
/// and can even hurt by causing seek contention.
fn thumbnail_worker_threads(items: &[(String, Option<i64>, Option<i64>)]) -> usize {
    let available = std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(4);

    // RAW preview decoding is substantially more CPU- and memory-intensive
    // than JPEG thumbnailing. Keep mixed refreshes responsive while still
    // allowing ordinary image batches to use the available cores.
    let has_raw = items.iter().any(|(path, _, _)| {
        crate::image_format::uses(path, crate::image_format::DecoderKind::Raw)
    });
    if has_raw {
        available.clamp(1, 2)
    } else {
        available.clamp(2, 8)
    }
}

pub fn clear_cache() -> Result<()> {
    let directory = cache_dir()?;
    if !directory.is_dir() {
        return Ok(());
    }

    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_file() {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

fn resize(source: image::RgbImage) -> Result<image::RgbImage> {
    let (width, height) = source.dimensions();
    let scale = THUMBNAIL_SIZE as f64 / width.max(height) as f64;
    let destination_width = ((width as f64 * scale).round() as u32).max(1);
    let destination_height = ((height as f64 * scale).round() as u32).max(1);
    let source_image = Image::from_vec_u8(width, height, source.into_raw(), PixelType::U8x3)?;
    let mut destination_image = Image::new(destination_width, destination_height, PixelType::U8x3);
    // Bilinear is substantially faster for small gallery thumbnails while
    // remaining smooth at the 320px cache size. Lanczos3 made refreshes
    // unnecessarily expensive, especially for 4K source photos.
    let options = ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::Box));
    Resizer::new().resize(&source_image, &mut destination_image, &options)?;
    let image = image::RgbImage::from_raw(
        destination_width,
        destination_height,
        destination_image.into_vec(),
    )
    .context("thumbnail resizer returned an invalid buffer")?;
    Ok(image)
}

#[cfg(test)]
mod viewer_tests {
    use super::*;

    #[test]
    fn viewer_target_fits_landscape_viewport() {
        assert_eq!(
            viewer_target_dimensions(6016, 4016, 1, 940, 698),
            (940, 628)
        );
    }

    #[test]
    fn viewer_target_accounts_for_exif_axis_swap() {
        assert_eq!(
            viewer_target_dimensions(6016, 4016, 8, 940, 698),
            (698, 466)
        );
    }

    #[test]
    fn viewer_target_never_upscales_small_images() {
        assert_eq!(viewer_target_dimensions(226, 320, 1, 940, 698), (226, 320));
    }

    #[test]
    fn unbounded_viewer_target_keeps_all_raw_preview_pixels() {
        assert_eq!(
            viewer_target_dimensions(6016, 4016, 1, u32::MAX, u32::MAX),
            (6016, 4016)
        );
    }

    #[test]
    fn viewer_jpeg_scale_uses_smallest_sufficient_dct_size() {
        let (scale, label) = native_scale_for_dimensions(6016, 4016, 698, 466);
        assert_eq!(label, "1/8");
        assert_eq!((scale.scale(6016), scale.scale(4016)), (752, 502));
    }

    #[test]
    fn decodes_requested_heif_fixture() {
        let Some(path) = std::env::var_os("PICASA_TEST_HEIF") else {
            return;
        };
        let bytes = std::fs::read(&path).unwrap();
        let decoded = decode_heif(&bytes).unwrap();
        assert!(decoded.source_width > 0);
        assert!(decoded.source_height > 0);
        assert_eq!(decoded.image.width(), decoded.source_width);
        assert_eq!(decoded.image.height(), decoded.source_height);
        assert_eq!(
            dimensions(path.to_string_lossy().as_ref(), &bytes).unwrap(),
            (decoded.source_width, decoded.source_height)
        );
        let viewer = decode_for_viewer(path.to_string_lossy().as_ref(), 32, 32).unwrap();
        assert!(viewer.width() <= 32);
        assert!(viewer.height() <= 32);
    }
}
