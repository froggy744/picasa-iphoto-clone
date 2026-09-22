//! Content-addressed overlay assets and CPU compositing.
//!
//! Imported PNG/JPG overlays are copied into an application-managed directory
//! keyed by the blake3 content hash of their bytes. Edit recipes reference
//! only that hash, so:
//!
//! * the original file path never has to remain available,
//! * the same logo imported onto many photographs is stored exactly once,
//! * no photograph's edit record embeds overlay image data.
//!
//! Decoded assets are kept in a small process-wide cache so batch operations
//! (thumbnail rendering, batch export of 100 photos sharing one logo) decode
//! each asset at most once.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{Context, Result};
use image::RgbaImage;
use rayon::prelude::*;

use super::model::EditRecipe;

/// Overrides the managed overlay directory. Used by tests.
pub const OVERLAY_DIR_ENV_VAR: &str = "PIC_OVERLAY_DIR";

const ASSET_CACHE_CAPACITY: usize = 6;

type AssetCache = Mutex<Vec<(String, Arc<RgbaImage>)>>;

static ASSET_CACHE: OnceLock<AssetCache> = OnceLock::new();

fn asset_cache() -> &'static AssetCache {
    ASSET_CACHE.get_or_init(|| Mutex::new(Vec::new()))
}

/// Application-managed directory holding every imported overlay asset.
pub fn overlays_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os(OVERLAY_DIR_ENV_VAR) {
        return PathBuf::from(dir);
    }
    dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("picasa-rs")
        .join("overlays")
}

/// An overlay image decoded and hashed, ready to be stored.
#[derive(Debug)]
pub struct PreparedOverlay {
    pub hash: String,
    pub width: u32,
    pub height: u32,
    /// "png" or "jpg"
    pub format: String,
    pub original_name: String,
    pub bytes: Vec<u8>,
}

/// Read, validate and hash an overlay file chosen by the user. Pure filesystem
/// work: safe to run off the GTK thread.
pub fn prepare_overlay_asset(source: &Path) -> Result<PreparedOverlay> {
    let bytes = std::fs::read(source)
        .with_context(|| format!("could not read overlay image {}", source.display()))?;
    let original_name = source
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("overlay")
        .to_string();
    prepare_overlay_bytes(&bytes, original_name)
}

/// Validate and hash overlay image bytes. Only PNG and JPG/JPEG are accepted,
/// matching the file picker's filters.
pub fn prepare_overlay_bytes(
    bytes: &[u8],
    original_name: impl Into<String>,
) -> Result<PreparedOverlay> {
    let format = match image::guess_format(bytes) {
        Ok(image::ImageFormat::Png) => "png",
        Ok(image::ImageFormat::Jpeg) => "jpg",
        _ => anyhow::bail!("Only PNG and JPG overlay images are supported"),
    };
    let decoded = image::load_from_memory(bytes).context("could not decode the overlay image")?;
    let (width, height) = (decoded.width(), decoded.height());
    if width == 0 || height == 0 {
        anyhow::bail!("the overlay image is empty");
    }
    Ok(PreparedOverlay {
        hash: blake3::hash(bytes).to_hex().to_string(),
        width,
        height,
        format: format.to_string(),
        original_name: original_name.into(),
        bytes: bytes.to_vec(),
    })
}

/// Write the asset into the managed directory (idempotent for identical
/// content) and record it in the library database.
pub fn store_overlay_asset(
    connection: &rusqlite::Connection,
    prepared: &PreparedOverlay,
) -> Result<PathBuf> {
    let directory = overlays_dir();
    std::fs::create_dir_all(&directory)
        .with_context(|| format!("could not create overlay directory {}", directory.display()))?;
    let path = directory.join(format!("{}.{}", prepared.hash, prepared.format));
    if !path.exists() {
        // Write to a temporary name then rename, so a render thread can never
        // observe a partially written asset.
        let temporary = directory.join(format!(".{}.tmp", prepared.hash));
        std::fs::write(&temporary, &prepared.bytes)
            .with_context(|| format!("could not write overlay asset {}", temporary.display()))?;
        std::fs::rename(&temporary, &path)
            .with_context(|| format!("could not store overlay asset {}", path.display()))?;
    }
    crate::db::upsert_overlay_asset(
        connection,
        &prepared.hash,
        prepared.width as i64,
        prepared.height as i64,
        &prepared.format,
        &prepared.original_name,
    )?;
    Ok(path)
}

/// Locate a stored asset file by content hash, trying known extensions.
pub fn asset_file_path(hash: &str) -> Option<PathBuf> {
    if hash.is_empty() {
        return None;
    }
    let directory = overlays_dir();
    for extension in ["png", "jpg", "jpeg"] {
        let candidate = directory.join(format!("{hash}.{extension}"));
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Decode (or fetch from the process-wide cache) an overlay asset.
/// Returns `None` for missing or corrupted files; callers skip the overlay
/// instead of failing the whole render.
pub fn load_asset(hash: &str) -> Option<Arc<RgbaImage>> {
    if hash.is_empty() {
        return None;
    }
    {
        let mut cache = asset_cache().lock().ok()?;
        if let Some(position) = cache.iter().position(|(key, _)| key == hash) {
            let entry = cache.remove(position);
            cache.push(entry);
            return cache.last().map(|(_, image)| image.clone());
        }
    }
    let path = asset_file_path(hash)?;
    let decoded = image::ImageReader::open(&path)
        .ok()?
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()?
        .to_rgba8();
    let image = Arc::new(decoded);
    if let Ok(mut cache) = asset_cache().lock() {
        cache.push((hash.to_string(), image.clone()));
        while cache.len() > ASSET_CACHE_CAPACITY {
            cache.remove(0);
        }
    }
    Some(image)
}

/// Drop every cached overlay decode. Test helper.
#[cfg(test)]
pub fn clear_asset_cache() {
    if let Ok(mut cache) = asset_cache().lock() {
        cache.clear();
    }
}

/// Composite every visible overlay of `recipe` onto `image`.
///
/// This runs strictly after crop, rotation and tone, so overlays stay upright
/// and keep their own colours no matter how the photograph was edited. Missing
/// or corrupted assets are skipped without failing the render.
pub fn composite_overlays(mut image: RgbaImage, recipe: &EditRecipe) -> RgbaImage {
    if (recipe.overlays.is_empty() && recipe.text_layers.is_empty())
        || image.width() == 0
        || image.height() == 0
    {
        return image;
    }
    let photo_width = image.width() as f32;
    let photo_height = image.height() as f32;
    for overlay in &recipe.overlays {
        if !overlay.visible || overlay.opacity <= 0.001 {
            continue;
        }
        let Some(asset) = load_asset(&overlay.asset) else {
            eprintln!(
                "overlay asset {} is missing or unreadable; skipping it",
                overlay.asset
            );
            continue;
        };
        let aspect = asset.width() as f32 / asset.height().max(1) as f32;
        let rect = overlay.rect(photo_width, photo_height, aspect);
        blend_overlay(&mut image, &asset, rect, overlay.opacity);
    }
    for layer in &recipe.text_layers {
        if !layer.visible || layer.opacity <= 0.001 {
            continue;
        }
        let Some(raster) = super::text_render::render_text_rgba(
            layer,
            f64::from(photo_width),
            f64::from(photo_height),
        ) else {
            continue;
        };
        let rect = layer.rect_with_size(
            raster.width() as f32 / photo_width,
            raster.height() as f32 / photo_height,
        );
        blend_overlay(&mut image, &raster, rect, layer.opacity);
    }
    image
}

/// Source-over blend of `overlay` into `base` inside the normalized `rect`,
/// honouring the overlay's own alpha channel and the requested opacity.
fn blend_overlay(
    base: &mut RgbaImage,
    overlay: &RgbaImage,
    rect: super::model::NormRect,
    opacity: f32,
) {
    let base_width = base.width();
    let base_height = base.height();
    if base_width == 0 || base_height == 0 || rect.width <= 0.0 || rect.height <= 0.0 {
        return;
    }
    let opacity = opacity.clamp(0.0, 1.0);
    if opacity <= 0.0 {
        return;
    }

    // Full (possibly overflowing) destination rectangle in base pixels.
    let full_x = rect.left * base_width as f32;
    let full_y = rect.top * base_height as f32;
    let full_w = rect.width * base_width as f32;
    let full_h = rect.height * base_height as f32;
    if full_w < 0.5 || full_h < 0.5 {
        return;
    }

    // Clip to the photo; parts of the overlay outside the photo are dropped.
    let x0 = full_x.floor().max(0.0) as u32;
    let y0 = full_y.floor().max(0.0) as u32;
    let x1 = (full_x + full_w).ceil().min(base_width as f32) as u32;
    let y1 = (full_y + full_h).ceil().min(base_height as f32) as u32;
    if x1 <= x0 || y1 <= y0 {
        return;
    }

    // High-quality resize of the asset to the on-photo size once, then blend.
    let scaled_width = full_w.round().max(1.0) as u32;
    let scaled_height = full_h.round().max(1.0) as u32;
    let scaled: RgbaImage = if overlay.width() == scaled_width && overlay.height() == scaled_height
    {
        overlay.clone()
    } else {
        image::imageops::resize(
            overlay,
            scaled_width,
            scaled_height,
            image::imageops::FilterType::Lanczos3,
        )
    };

    let scale_u = scaled.width() as f32 / full_w;
    let scale_v = scaled.height() as f32 / full_h;
    let row_bytes = base_width as usize * 4;

    base.as_mut()
        .par_chunks_mut(row_bytes)
        .enumerate()
        .for_each(|(row_y, row)| {
            let y = row_y as u32;
            if y < y0 || y >= y1 {
                return;
            }
            for x in x0..x1 {
                let u = ((x as f32 + 0.5 - full_x) * scale_u - 0.5)
                    .floor()
                    .clamp(0.0, scaled.width() as f32 - 1.0) as u32;
                let v = ((y as f32 + 0.5 - full_y) * scale_v - 0.5)
                    .floor()
                    .clamp(0.0, scaled.height() as f32 - 1.0) as u32;
                let source = scaled.get_pixel(u, v).0;
                let alpha = (source[3] as f32 / 255.0) * opacity;
                if alpha <= 0.0 {
                    continue;
                }
                let inverse = 1.0 - alpha;
                let offset = x as usize * 4;
                let dest = &mut row[offset..offset + 4];
                for channel in 0..3 {
                    dest[channel] = (source[channel] as f32 * alpha
                        + dest[channel] as f32 * inverse)
                        .round()
                        .clamp(0.0, 255.0) as u8;
                }
                dest[3] = (255.0 * alpha + dest[3] as f32 * inverse)
                    .round()
                    .clamp(0.0, 255.0) as u8;
            }
        });
}

#[cfg(test)]
mod tests {
    use super::super::model::{OverlayAnchor, OverlaySpec, TextLayerSpec};
    use super::*;
    use image::Rgba;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Serializes every test that touches the managed overlay directory or the
    /// process-wide asset cache, since both are process-global state.
    static FS_LOCK: Mutex<()> = Mutex::new(());

    fn unique_overlay_dir(tag: &str) -> PathBuf {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let directory = std::env::temp_dir().join(format!(
            "pic-overlays-{}-{}-{}-{}",
            tag,
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
            // Distinguish parallel test binaries sharing the same pid.
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    fn png_bytes(width: u32, height: u32, pixel: [u8; 4]) -> Vec<u8> {
        let image = RgbaImage::from_pixel(width, height, Rgba(pixel));
        let mut bytes = Vec::new();
        let encoder = image::codecs::png::PngEncoder::new(&mut bytes);
        use image::ImageEncoder;
        encoder
            .write_image(
                image.as_raw(),
                width,
                height,
                image::ExtendedColorType::Rgba8,
            )
            .unwrap();
        bytes
    }

    #[test]
    fn prepare_accepts_png_and_rejects_other_formats() {
        let prepared = prepare_overlay_bytes(&png_bytes(4, 4, [10, 20, 30, 255]), "logo.png")
            .expect("png accepted");
        assert_eq!(prepared.format, "png");
        assert_eq!(prepared.width, 4);
        assert_eq!(prepared.height, 4);
        assert_eq!(prepared.hash.len(), 64);

        let error = prepare_overlay_bytes(b"not an image", "logo.png").unwrap_err();
        assert!(error.to_string().contains("PNG and JPG"));
    }

    #[test]
    fn identical_content_shares_one_asset_record_and_file() {
        let _guard = FS_LOCK.lock().unwrap();
        let directory = unique_overlay_dir("dedup");
        unsafe { std::env::set_var(OVERLAY_DIR_ENV_VAR, &directory) };
        clear_asset_cache();

        let connection = rusqlite::Connection::open_in_memory().unwrap();
        connection.execute_batch(crate::db::SCHEMA).unwrap();
        let bytes = png_bytes(8, 6, [200, 0, 0, 255]);

        let first = prepare_overlay_bytes(&bytes, "first.png").unwrap();
        let second = prepare_overlay_bytes(&bytes, "elsewhere/first.png").unwrap();
        assert_eq!(first.hash, second.hash);

        let path_a = store_overlay_asset(&connection, &first).unwrap();
        let path_b = store_overlay_asset(&connection, &second).unwrap();
        assert_eq!(path_a, path_b);
        assert!(path_a.is_file());
        let records = crate::db::overlay_assets(&connection).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].hash, first.hash);
        // Re-importing identical content refreshes the recorded filename but
        // never creates a second row.
        assert_eq!(records[0].original_name, "elsewhere/first.png");
        assert_eq!(records[0].width, 8);

        unsafe { std::env::remove_var(OVERLAY_DIR_ENV_VAR) };
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn stored_asset_round_trips_through_the_decode_cache() {
        let _guard = FS_LOCK.lock().unwrap();
        let directory = unique_overlay_dir("roundtrip");
        unsafe { std::env::set_var(OVERLAY_DIR_ENV_VAR, &directory) };
        clear_asset_cache();

        let connection = rusqlite::Connection::open_in_memory().unwrap();
        connection.execute_batch(crate::db::SCHEMA).unwrap();
        let prepared = prepare_overlay_bytes(&png_bytes(5, 3, [0, 255, 0, 128]), "g.png").unwrap();
        store_overlay_asset(&connection, &prepared).unwrap();

        let loaded = load_asset(&prepared.hash).expect("asset loads");
        assert_eq!((loaded.width(), loaded.height()), (5, 3));
        // Second call is served from cache and returns the same allocation.
        let cached = load_asset(&prepared.hash).unwrap();
        assert!(Arc::ptr_eq(&loaded, &cached));

        unsafe { std::env::remove_var(OVERLAY_DIR_ENV_VAR) };
        let _ = std::fs::remove_dir_all(directory);
        clear_asset_cache();
    }

    #[test]
    fn missing_asset_is_skipped_and_the_photo_is_untouched() {
        let _guard = FS_LOCK.lock().unwrap();
        let directory = unique_overlay_dir("missing");
        unsafe { std::env::set_var(OVERLAY_DIR_ENV_VAR, &directory) };
        clear_asset_cache();

        let base = RgbaImage::from_pixel(16, 16, Rgba([30, 60, 90, 255]));
        let mut recipe = EditRecipe::default();
        recipe.overlays.push(OverlaySpec::new_centered(
            "0000000000000000000000000000000000000000000000000000000000000000",
        ));

        let output = composite_overlays(base.clone(), &recipe);

        assert_eq!(output.as_raw(), base.as_raw());

        unsafe { std::env::remove_var(OVERLAY_DIR_ENV_VAR) };
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn corrupted_asset_file_is_skipped() {
        let _guard = FS_LOCK.lock().unwrap();
        let directory = unique_overlay_dir("corrupt");
        unsafe { std::env::set_var(OVERLAY_DIR_ENV_VAR, &directory) };
        clear_asset_cache();

        let hash = "1111111111111111111111111111111111111111111111111111111111111111";
        std::fs::write(directory.join(format!("{hash}.png")), b"garbage").unwrap();

        let base = RgbaImage::from_pixel(8, 8, Rgba([1, 2, 3, 255]));
        let mut recipe = EditRecipe::default();
        recipe.overlays.push(OverlaySpec::new_centered(hash));

        let output = composite_overlays(base.clone(), &recipe);

        assert_eq!(output.as_raw(), base.as_raw());

        unsafe { std::env::remove_var(OVERLAY_DIR_ENV_VAR) };
        let _ = std::fs::remove_dir_all(directory);
        clear_asset_cache();
    }

    #[test]
    fn transparent_png_blends_with_source_over_alpha() {
        let _guard = FS_LOCK.lock().unwrap();
        let directory = unique_overlay_dir("alpha");
        unsafe { std::env::set_var(OVERLAY_DIR_ENV_VAR, &directory) };
        clear_asset_cache();

        let connection = rusqlite::Connection::open_in_memory().unwrap();
        connection.execute_batch(crate::db::SCHEMA).unwrap();
        // Fully opaque red 4x4 asset.
        let prepared = prepare_overlay_bytes(&png_bytes(4, 4, [255, 0, 0, 255]), "r.png").unwrap();
        store_overlay_asset(&connection, &prepared).unwrap();

        let mut recipe = EditRecipe::default();
        let mut overlay = OverlaySpec::new_centered(&prepared.hash);
        overlay.anchor = OverlayAnchor::TopLeft;
        overlay.x = 0.0;
        overlay.y = 0.0;
        overlay.width = 0.5;
        overlay.opacity = 0.5;
        recipe.overlays.push(overlay);

        let base = RgbaImage::from_pixel(8, 8, Rgba([0, 0, 255, 255]));
        let output = composite_overlays(base.clone(), &recipe);

        // 50% red over opaque blue.
        let inside = output.get_pixel(1, 1).0;
        assert!((inside[0] as i16 - 128).abs() <= 1, "red {}", inside[0]);
        assert!(inside[1] <= 1);
        assert!((inside[2] as i16 - 128).abs() <= 1, "blue {}", inside[2]);
        assert_eq!(inside[3], 255);
        // The uncovered half of the photo is untouched.
        assert_eq!(output.get_pixel(7, 7).0, [0, 0, 255, 255]);

        unsafe { std::env::remove_var(OVERLAY_DIR_ENV_VAR) };
        let _ = std::fs::remove_dir_all(directory);
        clear_asset_cache();
    }

    #[test]
    fn semi_transitive_asset_alpha_and_opacity_multiply() {
        let _guard = FS_LOCK.lock().unwrap();
        let directory = unique_overlay_dir("mulalpha");
        unsafe { std::env::set_var(OVERLAY_DIR_ENV_VAR, &directory) };
        clear_asset_cache();

        let connection = rusqlite::Connection::open_in_memory().unwrap();
        connection.execute_batch(crate::db::SCHEMA).unwrap();
        // Asset alpha 128 (~0.502).
        let prepared =
            prepare_overlay_bytes(&png_bytes(4, 4, [255, 255, 255, 128]), "w.png").unwrap();
        store_overlay_asset(&connection, &prepared).unwrap();

        let mut recipe = EditRecipe::default();
        let mut overlay = OverlaySpec::new_centered(&prepared.hash);
        overlay.anchor = OverlayAnchor::TopLeft;
        overlay.x = 0.0;
        overlay.y = 0.0;
        overlay.width = 1.0;
        overlay.opacity = 0.5;
        recipe.overlays.push(overlay);

        let base = RgbaImage::from_pixel(4, 4, Rgba([0, 0, 0, 255]));
        let output = composite_overlays(base, &recipe);
        let pixel = output.get_pixel(0, 0).0;
        // Effective alpha = 0.502 * 0.5 ≈ 0.251 → white blend ≈ 64.
        assert!(
            (pixel[0] as i16 - 64).abs() <= 2,
            "blended red {}",
            pixel[0]
        );
        assert_eq!(pixel[3], 255);

        unsafe { std::env::remove_var(OVERLAY_DIR_ENV_VAR) };
        let _ = std::fs::remove_dir_all(directory);
        clear_asset_cache();
    }

    #[test]
    fn hidden_and_zero_opacity_overlays_do_not_composite() {
        let _guard = FS_LOCK.lock().unwrap();
        let directory = unique_overlay_dir("invisible");
        unsafe { std::env::set_var(OVERLAY_DIR_ENV_VAR, &directory) };
        clear_asset_cache();

        let connection = rusqlite::Connection::open_in_memory().unwrap();
        connection.execute_batch(crate::db::SCHEMA).unwrap();
        let prepared = prepare_overlay_bytes(&png_bytes(4, 4, [255, 0, 0, 255]), "r.png").unwrap();
        store_overlay_asset(&connection, &prepared).unwrap();

        let base = RgbaImage::from_pixel(8, 8, Rgba([0, 255, 0, 255]));

        let mut hidden = EditRecipe::default();
        let mut overlay = OverlaySpec::new_centered(&prepared.hash);
        overlay.visible = false;
        hidden.overlays.push(overlay);
        assert_eq!(
            composite_overlays(base.clone(), &hidden).as_raw(),
            base.as_raw()
        );

        let mut transparent = EditRecipe::default();
        let mut overlay = OverlaySpec::new_centered(&prepared.hash);
        overlay.opacity = 0.0;
        transparent.overlays.push(overlay);
        assert_eq!(
            composite_overlays(base.clone(), &transparent).as_raw(),
            base.as_raw()
        );

        unsafe { std::env::remove_var(OVERLAY_DIR_ENV_VAR) };
        let _ = std::fs::remove_dir_all(directory);
        clear_asset_cache();
    }

    #[test]
    fn overlay_lands_in_the_expected_region_of_the_final_photo() {
        let _guard = FS_LOCK.lock().unwrap();
        let directory = unique_overlay_dir("placement");
        unsafe { std::env::set_var(OVERLAY_DIR_ENV_VAR, &directory) };
        clear_asset_cache();

        let connection = rusqlite::Connection::open_in_memory().unwrap();
        connection.execute_batch(crate::db::SCHEMA).unwrap();
        let prepared = prepare_overlay_bytes(&png_bytes(4, 4, [255, 0, 0, 255]), "r.png").unwrap();
        store_overlay_asset(&connection, &prepared).unwrap();

        let mut recipe = EditRecipe::default();
        let mut overlay = OverlaySpec::new_centered(&prepared.hash);
        overlay.anchor = OverlayAnchor::BottomRight;
        overlay.x = 1.0;
        overlay.y = 1.0;
        overlay.width = 0.25;
        recipe.overlays.push(overlay);

        // A landscape photo: the overlay hugs the bottom-right corner.
        let output = composite_overlays(
            RgbaImage::from_pixel(40, 20, Rgba([0, 0, 255, 255])),
            &recipe,
        );
        assert_eq!(output.get_pixel(39, 19).0[0], 255, "corner is covered");
        assert_eq!(output.get_pixel(39, 19).0[2], 0);
        assert_eq!(output.get_pixel(0, 0).0, [0, 0, 255, 255]);

        // A portrait photo keeps relative corner placement and pixel aspect.
        let portrait = composite_overlays(
            RgbaImage::from_pixel(20, 40, Rgba([0, 0, 255, 255])),
            &recipe,
        );
        assert_eq!(portrait.get_pixel(19, 39).0[0], 255, "corner is covered");
        assert_eq!(portrait.get_pixel(0, 0).0, [0, 0, 255, 255]);

        unsafe { std::env::remove_var(OVERLAY_DIR_ENV_VAR) };
        let _ = std::fs::remove_dir_all(directory);
        clear_asset_cache();
    }

    #[test]
    fn text_layers_composite_without_any_overlay_assets() {
        let base = RgbaImage::from_pixel(64, 48, Rgba([0, 0, 0, 255]));
        let mut recipe = EditRecipe::default();
        let mut layer = TextLayerSpec::new_default();
        layer.text = "Hi".to_string();
        layer.size = 0.4;
        layer.set_color_rgba(1.0, 1.0, 1.0, 1.0);
        layer.anchor = OverlayAnchor::Center;
        layer.x = 0.5;
        layer.y = 0.5;
        recipe.text_layers.push(layer.clone());

        let output = composite_overlays(base.clone(), &recipe);
        let changed = output
            .pixels()
            .zip(base.pixels())
            .filter(|(a, b)| a != b)
            .count();
        assert!(changed > 0, "white glyphs must alter black pixels");
        let lit = output
            .pixels()
            .any(|pixel| pixel.0[0] > 200 && pixel.0[1] > 200 && pixel.0[2] > 200);
        assert!(lit, "glyph core is near-white");
        assert_eq!(output.get_pixel(0, 0).0, [0, 0, 0, 255], "corner untouched");

        let mut hidden = EditRecipe::default();
        let mut hidden_layer = layer.clone();
        hidden_layer.visible = false;
        hidden.text_layers.push(hidden_layer);
        assert_eq!(
            composite_overlays(base.clone(), &hidden).as_raw(),
            base.as_raw()
        );

        let mut transparent = EditRecipe::default();
        let mut faded = layer;
        faded.opacity = 0.0;
        transparent.text_layers.push(faded);
        assert_eq!(
            composite_overlays(base.clone(), &transparent).as_raw(),
            base.as_raw()
        );
    }

    #[test]
    fn text_paints_above_image_overlays() {
        let _guard = FS_LOCK.lock().unwrap();
        let directory = unique_overlay_dir("text-above");
        unsafe { std::env::set_var(OVERLAY_DIR_ENV_VAR, &directory) };
        clear_asset_cache();

        let connection = rusqlite::Connection::open_in_memory().unwrap();
        connection.execute_batch(crate::db::SCHEMA).unwrap();
        let prepared =
            prepare_overlay_bytes(&png_bytes(8, 8, [255, 0, 0, 255]), "red.png").unwrap();
        store_overlay_asset(&connection, &prepared).unwrap();

        let mut recipe = EditRecipe::default();
        let mut overlay = OverlaySpec::new_centered(&prepared.hash);
        overlay.anchor = OverlayAnchor::Center;
        overlay.x = 0.5;
        overlay.y = 0.5;
        overlay.width = 1.0;
        recipe.overlays.push(overlay);
        let mut layer = TextLayerSpec::new_default();
        layer.text = "W".to_string();
        layer.size = 0.5;
        layer.set_color_rgba(1.0, 1.0, 1.0, 1.0);
        recipe.text_layers.push(layer);

        let output = composite_overlays(
            RgbaImage::from_pixel(48, 48, Rgba([0, 0, 255, 255])),
            &recipe,
        );
        assert_eq!(output.get_pixel(0, 0).0[1], 0, "red covers the photo");
        assert_eq!(output.get_pixel(0, 0).0[0], 255);
        let whitish = output
            .pixels()
            .any(|pixel| pixel.0[0] > 220 && pixel.0[1] > 220 && pixel.0[2] > 220);
        assert!(whitish, "text sits on top of the red overlay");

        unsafe { std::env::remove_var(OVERLAY_DIR_ENV_VAR) };
        let _ = std::fs::remove_dir_all(directory);
        clear_asset_cache();
    }
}
