use anyhow::{Context, Result};
use image::{DynamicImage, Rgba, RgbaImage};

use super::model::{CropRect, EditRecipe};

pub fn apply_library_rotation(image: RgbaImage, rotation: i32) -> RgbaImage {
    match rotation.rem_euclid(360) {
        90 => image::imageops::rotate90(&image),
        180 => image::imageops::rotate180(&image),
        270 => image::imageops::rotate270(&image),
        _ => image,
    }
}

pub fn apply_recipe(mut image: RgbaImage, recipe: &EditRecipe) -> RgbaImage {
    if recipe.straighten.abs() >= 0.01 {
        image = rotate_autocrop(&image, recipe.straighten.to_radians());
    }
    if !recipe.crop.is_full() {
        image = crop_normalized(&image, recipe.crop);
    }
    apply_tone(image, recipe)
}

pub fn apply_recipe_without_crop(mut image: RgbaImage, recipe: &EditRecipe) -> RgbaImage {
    if recipe.straighten.abs() >= 0.01 {
        image = rotate_autocrop(&image, recipe.straighten.to_radians());
    }
    apply_tone(image, recipe)
}

pub fn estimated_output_dimensions(width: u32, height: u32, recipe: &EditRecipe) -> (u32, u32) {
    let (mut width, mut height) = (width.max(1) as f32, height.max(1) as f32);
    if recipe.straighten.abs() >= 0.01 {
        let rotated = largest_rotated_rect(width, height, recipe.straighten.to_radians());
        width = rotated.0.max(1.0);
        height = rotated.1.max(1.0);
    }
    let crop = recipe.crop.normalized();
    width *= (crop.right - crop.left).max(0.01);
    height *= (crop.bottom - crop.top).max(0.01);
    (
        width.round().max(1.0) as u32,
        height.round().max(1.0) as u32,
    )
}

pub fn render_thumbnail(path: &str, rotation: i32, recipe: &EditRecipe) -> Result<RgbaImage> {
    let image = image::open(path)
        .with_context(|| format!("could not open cached thumbnail {path}"))?
        .to_rgba8();
    Ok(apply_recipe(
        apply_library_rotation(image, rotation),
        recipe,
    ))
}

pub fn render_for_viewer(
    reference: &str,
    rotation: i32,
    recipe: &EditRecipe,
    width: u32,
    height: u32,
) -> Result<RgbaImage> {
    let image = crate::thumbnail::decode_for_viewer(reference, width.max(1), height.max(1))?;
    Ok(apply_recipe(
        apply_library_rotation(image, rotation),
        recipe,
    ))
}

/// Render a full-quality export from plain Rust values.
///
/// Keep GTK/GLib objects out of this API so callers can safely run the heavy
/// decode/render work on a `std::thread`. `PhotoObject` is a GTK object and is
/// intentionally neither `Send` nor `Sync`.
pub fn render_for_export(
    reference: &str,
    rotation: i32,
    edit_recipe: &str,
    source_width: i64,
    source_height: i64,
) -> Result<RgbaImage> {
    let (mut width, mut height) = if source_width > 0 && source_height > 0 {
        (source_width as u32, source_height as u32)
    } else {
        // RAW/remote records can lack catalog dimensions. An unbounded request
        // lets the viewer decoder clamp to the largest source/embedded preview
        // it can actually decode instead of exporting a 1x1 image.
        (u32::MAX, u32::MAX)
    };
    if matches!(rotation.rem_euclid(360), 90 | 270) {
        std::mem::swap(&mut width, &mut height);
    }
    render_for_viewer(
        reference,
        rotation,
        &EditRecipe::decode(edit_recipe),
        width,
        height,
    )
}

fn crop_normalized(image: &RgbaImage, crop: CropRect) -> RgbaImage {
    let crop = crop.normalized();
    let width = image.width().max(1);
    let height = image.height().max(1);
    let x = (crop.left * width as f32).round() as u32;
    let y = (crop.top * height as f32).round() as u32;
    let right = (crop.right * width as f32).round() as u32;
    let bottom = (crop.bottom * height as f32).round() as u32;
    let x = x.min(width - 1);
    let y = y.min(height - 1);
    let crop_width = right.saturating_sub(x).clamp(1, width - x);
    let crop_height = bottom.saturating_sub(y).clamp(1, height - y);
    image::imageops::crop_imm(image, x, y, crop_width, crop_height).to_image()
}

fn apply_tone(mut image: RgbaImage, recipe: &EditRecipe) -> RgbaImage {
    if recipe.auto_color {
        apply_auto_color(&mut image);
    }
    if recipe.auto_contrast {
        apply_auto_contrast(&mut image);
    }

    let exposure = 2.0_f32.powf(recipe.exposure);
    let temperature = recipe.temperature;
    let saturation = 1.0 + recipe.saturation;
    for pixel in image.pixels_mut() {
        let alpha = pixel[3];
        let mut r = pixel[0] as f32 / 255.0;
        let mut g = pixel[1] as f32 / 255.0;
        let mut b = pixel[2] as f32 / 255.0;

        r *= exposure;
        g *= exposure;
        b *= exposure;

        let luma = (0.2126 * r + 0.7152 * g + 0.0722 * b).clamp(0.0, 1.0);
        let shadow_weight = (1.0 - luma).powi(2);
        let highlight_weight = luma.powi(2);
        let fill = recipe.fill_light * 0.45 * shadow_weight;
        let shadows = recipe.shadows * 0.35 * shadow_weight;
        let highlights = recipe.highlights * 0.35 * highlight_weight;
        r += fill + shadows + highlights;
        g += fill + shadows + highlights;
        b += fill + shadows + highlights;

        r *= 1.0 + temperature * 0.18;
        b *= 1.0 - temperature * 0.18;
        g *= 1.0 + temperature.abs() * 0.015;

        let gray = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        r = gray + (r - gray) * saturation;
        g = gray + (g - gray) * saturation;
        b = gray + (b - gray) * saturation;

        if recipe.contrast.abs() > 0.0001 {
            r = apply_contrast_curve(r, recipe.contrast);
            g = apply_contrast_curve(g, recipe.contrast);
            b = apply_contrast_curve(b, recipe.contrast);
        }

        if recipe.black_white {
            let y = rgb_luma(r, g, b).clamp(0.0, 1.0);
            r = y;
            g = y;
            b = y;
        }
        if recipe.sepia {
            // Photographic sepia is a toned monochrome print, not a full-strength
            // RGB matrix. Preserve luminance, then add a restrained warm tint.
            let y = rgb_luma(r, g, b).clamp(0.0, 1.0);
            let strength = 0.75 * (1.0 - 0.20 * (2.0 * y - 1.0).abs());
            r = y * (1.0 + 0.080 * strength);
            g = y * (1.0 - 0.006 * strength);
            b = y * (1.0 - 0.180 * strength);
        }
        *pixel = Rgba([
            (r.clamp(0.0, 1.0) * 255.0).round() as u8,
            (g.clamp(0.0, 1.0) * 255.0).round() as u8,
            (b.clamp(0.0, 1.0) * 255.0).round() as u8,
            alpha,
        ]);
    }

    if recipe.sharpen > 0.001 && image.width() > 2 && image.height() > 2 {
        let blurred = image::imageops::blur(&image, 1.1);
        let amount = recipe.sharpen * 1.6;
        for (pixel, blurred) in image.pixels_mut().zip(blurred.pixels()) {
            for channel in 0..3 {
                let original = pixel[channel] as f32;
                let soft = blurred[channel] as f32;
                pixel[channel] = (original + (original - soft) * amount)
                    .clamp(0.0, 255.0)
                    .round() as u8;
            }
        }
    }
    image
}

fn rgb_luma(r: f32, g: f32, b: f32) -> f32 {
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

fn apply_contrast_curve(value: f32, contrast: f32) -> f32 {
    let x = value.clamp(0.0, 1.0);
    // Keep the visible slider range expressive without letting +1.0 collapse
    // ordinary near-black/near-white detail to pure clipping.
    let amount = contrast.clamp(-1.0, 1.0) * 0.55;
    // Smooth S-curve with fixed black, midpoint and white anchors. Positive
    // values deepen shadows and lift highlights; negative values compress them.
    let curve = 4.0 * (x - 0.5) * x * (1.0 - x);
    (x + amount * curve).clamp(0.0, 1.0)
}

fn apply_auto_color(image: &mut RgbaImage) {
    let mut sums = [0.0f64; 3];
    let mut count = 0.0f64;
    let step = ((image.width() as u64 * image.height() as u64) / 200_000).max(1) as usize;
    for pixel in image.pixels().step_by(step) {
        if pixel[3] < 16 {
            continue;
        }
        let r = pixel[0] as f64;
        let g = pixel[1] as f64;
        let b = pixel[2] as f64;
        let luma = (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255.0;
        if !(0.06..=0.94).contains(&luma) {
            continue;
        }
        sums[0] += r;
        sums[1] += g;
        sums[2] += b;
        count += 1.0;
    }
    if count <= 0.0 {
        return;
    }

    let means = [sums[0] / count, sums[1] / count, sums[2] / count];
    let gray = (means[0] + means[1] + means[2]) / 3.0;
    let raw_gains = [
        (gray / means[0].max(1.0)).clamp(0.80, 1.20),
        (gray / means[1].max(1.0)).clamp(0.80, 1.20),
        (gray / means[2].max(1.0)).clamp(0.80, 1.20),
    ];
    // Blend only part-way toward gray-world neutrality. This removes obvious
    // casts without erasing deliberate warm/cool lighting.
    let strength = 0.35;
    let gains = [
        1.0 + (raw_gains[0] - 1.0) * strength,
        1.0 + (raw_gains[1] - 1.0) * strength,
        1.0 + (raw_gains[2] - 1.0) * strength,
    ];
    for pixel in image.pixels_mut() {
        if pixel[3] < 16 {
            continue;
        }
        for channel in 0..3 {
            pixel[channel] = (pixel[channel] as f64 * gains[channel])
                .clamp(0.0, 255.0)
                .round() as u8;
        }
    }
}

fn apply_auto_contrast(image: &mut RgbaImage) {
    let mut histogram = [0u64; 256];
    let step = ((image.width() as u64 * image.height() as u64) / 250_000).max(1) as usize;
    let mut samples = 0u64;
    for pixel in image.pixels().step_by(step) {
        if pixel[3] < 16 {
            continue;
        }
        let luma = rgb_luma(
            pixel[0] as f32 / 255.0,
            pixel[1] as f32 / 255.0,
            pixel[2] as f32 / 255.0,
        );
        let bucket = (luma.clamp(0.0, 1.0) * 255.0).round() as usize;
        histogram[bucket] += 1;
        samples += 1;
    }
    if samples == 0 {
        return;
    }

    // Ignore roughly the outer 1% of luminance samples so a single specular
    // highlight or black pixel does not determine the entire correction.
    let clip = if samples >= 100 {
        ((samples as f64 * 0.01).round() as u64).max(1)
    } else {
        0
    };
    let mut cumulative = 0u64;
    let mut low = 0usize;
    for (index, count) in histogram.iter().enumerate() {
        cumulative += *count;
        if cumulative > clip {
            low = index;
            break;
        }
    }
    cumulative = 0;
    let mut high = 255usize;
    for (index, count) in histogram.iter().enumerate().rev() {
        cumulative += *count;
        if cumulative > clip {
            high = index;
            break;
        }
    }
    if high <= low + 6 {
        return;
    }

    let low = low as f32 / 255.0;
    let high = high as f32 / 255.0;
    let range = high - low;
    for pixel in image.pixels_mut() {
        if pixel[3] < 16 {
            continue;
        }
        let r = pixel[0] as f32 / 255.0;
        let g = pixel[1] as f32 / 255.0;
        let b = pixel[2] as f32 / 255.0;
        let original_luma = rgb_luma(r, g, b);
        // Preserve a small amount of headroom at both ends instead of forcing
        // the selected percentile range to absolute black and white.
        let normalized = ((original_luma - low) / range).clamp(0.0, 1.0);
        let stretched = 0.02 + normalized * 0.96;
        let target_luma = original_luma + (stretched - original_luma) * 0.80;
        let delta = target_luma - original_luma;
        pixel[0] = ((r + delta).clamp(0.0, 1.0) * 255.0).round() as u8;
        pixel[1] = ((g + delta).clamp(0.0, 1.0) * 255.0).round() as u8;
        pixel[2] = ((b + delta).clamp(0.0, 1.0) * 255.0).round() as u8;
    }
}

fn rotate_autocrop(source: &RgbaImage, angle: f32) -> RgbaImage {
    if angle.abs() < 0.0001 || source.width() < 2 || source.height() < 2 {
        return source.clone();
    }
    let (output_width, output_height) =
        largest_rotated_rect(source.width() as f32, source.height() as f32, angle);
    let output_width = output_width.floor().max(1.0) as u32;
    let output_height = output_height.floor().max(1.0) as u32;
    let mut output = RgbaImage::new(output_width, output_height);
    let cos = angle.cos();
    let sin = angle.sin();
    let src_cx = (source.width() as f32 - 1.0) * 0.5;
    let src_cy = (source.height() as f32 - 1.0) * 0.5;
    let dst_cx = (output_width as f32 - 1.0) * 0.5;
    let dst_cy = (output_height as f32 - 1.0) * 0.5;

    for y in 0..output_height {
        for x in 0..output_width {
            let dx = x as f32 - dst_cx;
            let dy = y as f32 - dst_cy;
            let sx = cos * dx + sin * dy + src_cx;
            let sy = -sin * dx + cos * dy + src_cy;
            output.put_pixel(x, y, bilinear(source, sx, sy));
        }
    }
    output
}

fn bilinear(image: &RgbaImage, x: f32, y: f32) -> Rgba<u8> {
    let max_x = image.width().saturating_sub(1) as f32;
    let max_y = image.height().saturating_sub(1) as f32;
    let x = x.clamp(0.0, max_x);
    let y = y.clamp(0.0, max_y);
    let x0 = x.floor() as u32;
    let y0 = y.floor() as u32;
    let x1 = (x0 + 1).min(image.width() - 1);
    let y1 = (y0 + 1).min(image.height() - 1);
    let tx = x - x0 as f32;
    let ty = y - y0 as f32;
    let p00 = image.get_pixel(x0, y0);
    let p10 = image.get_pixel(x1, y0);
    let p01 = image.get_pixel(x0, y1);
    let p11 = image.get_pixel(x1, y1);
    let mut out = [0u8; 4];
    for channel in 0..4 {
        let top = p00[channel] as f32 * (1.0 - tx) + p10[channel] as f32 * tx;
        let bottom = p01[channel] as f32 * (1.0 - tx) + p11[channel] as f32 * tx;
        out[channel] = (top * (1.0 - ty) + bottom * ty).clamp(0.0, 255.0).round() as u8;
    }
    Rgba(out)
}

fn largest_rotated_rect(width: f32, height: f32, angle: f32) -> (f32, f32) {
    let width = width.abs();
    let height = height.abs();
    if width <= 0.0 || height <= 0.0 {
        return (1.0, 1.0);
    }
    let sin_a = angle.sin().abs();
    let cos_a = angle.cos().abs();
    let width_is_longer = width >= height;
    let side_long = if width_is_longer { width } else { height };
    let side_short = if width_is_longer { height } else { width };
    if side_short <= 2.0 * sin_a * cos_a * side_long || (sin_a - cos_a).abs() < 0.00001 {
        let x = 0.5 * side_short;
        if width_is_longer {
            (x / sin_a.max(0.00001), x / cos_a.max(0.00001))
        } else {
            (x / cos_a.max(0.00001), x / sin_a.max(0.00001))
        }
    } else {
        let cos_2a = cos_a * cos_a - sin_a * sin_a;
        (
            (width * cos_a - height * sin_a) / cos_2a,
            (height * cos_a - width * sin_a) / cos_2a,
        )
    }
}

pub fn save_jpeg(image: &RgbaImage, destination: &std::path::Path, quality: u8) -> Result<()> {
    use image::ImageEncoder;
    let file = std::fs::File::create(destination)
        .with_context(|| format!("could not create {}", destination.display()))?;
    let writer = std::io::BufWriter::new(file);
    let rgb = DynamicImage::ImageRgba8(image.clone()).to_rgb8();
    image::codecs::jpeg::JpegEncoder::new_with_quality(writer, quality).write_image(
        &rgb,
        rgb.width(),
        rgb.height(),
        image::ExtendedColorType::Rgb8,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crop_reduces_dimensions() {
        let image = RgbaImage::new(100, 80);
        let mut recipe = EditRecipe::default();
        recipe.crop = CropRect {
            left: 0.25,
            top: 0.25,
            right: 0.75,
            bottom: 0.75,
        };
        let output = apply_recipe(image, &recipe);
        assert_eq!(output.dimensions(), (50, 40));
    }

    #[test]
    fn straightening_autocrops_black_corners() {
        let image = RgbaImage::from_pixel(100, 80, Rgba([200, 200, 200, 255]));
        let mut recipe = EditRecipe::default();
        recipe.straighten = 5.0;
        let output = apply_recipe(image, &recipe);
        assert!(output.width() < 100);
        assert!(output.height() < 80);
        assert!(output.pixels().all(|pixel| pixel[3] == 255));
    }

    #[test]
    fn sepia_is_warm_monochrome_and_bw_plus_sepia_is_equivalent() {
        let source = RgbaImage::from_pixel(2, 2, Rgba([200, 100, 50, 255]));
        let mut combined_recipe = EditRecipe::default();
        combined_recipe.black_white = true;
        combined_recipe.sepia = true;
        let combined = apply_recipe(source.clone(), &combined_recipe);

        let mut bw_recipe = EditRecipe::default();
        bw_recipe.black_white = true;
        let black_white = apply_recipe(source.clone(), &bw_recipe);

        let mut sepia_recipe = EditRecipe::default();
        sepia_recipe.sepia = true;
        let sepia = apply_recipe(source, &sepia_recipe);

        assert_ne!(sepia.as_raw(), black_white.as_raw());
        assert_eq!(combined.as_raw(), sepia.as_raw());
        let pixel = sepia.get_pixel(0, 0);
        assert!(pixel[0] > pixel[1] && pixel[1] > pixel[2]);
    }

    #[test]
    fn positive_contrast_expands_midtones() {
        let mut source = RgbaImage::new(2, 1);
        source.put_pixel(0, 0, Rgba([96, 96, 96, 255]));
        source.put_pixel(1, 0, Rgba([160, 160, 160, 255]));
        let mut recipe = EditRecipe::default();
        recipe.contrast = 0.75;
        let output = apply_recipe(source, &recipe);
        let delta = output.get_pixel(1, 0)[0] as i16 - output.get_pixel(0, 0)[0] as i16;
        assert!(delta > 64);
    }

    #[test]
    fn sepia_preserves_midtone_luminance() {
        let source = RgbaImage::from_pixel(1, 1, Rgba([128, 128, 128, 255]));
        let mut recipe = EditRecipe::default();
        recipe.sepia = true;
        let output = apply_recipe(source, &recipe);
        let pixel = output.get_pixel(0, 0);
        let luma = 0.2126 * pixel[0] as f32 + 0.7152 * pixel[1] as f32 + 0.0722 * pixel[2] as f32;
        assert!((luma - 128.0).abs() < 12.0);
    }

    #[test]
    fn auto_contrast_ignores_single_black_and_white_outliers() {
        let mut source = RgbaImage::new(100, 1);
        source.put_pixel(0, 0, Rgba([0, 0, 0, 255]));
        source.put_pixel(99, 0, Rgba([255, 255, 255, 255]));
        for x in 1..99 {
            let value = if x % 2 == 0 { 100 } else { 120 };
            source.put_pixel(x, 0, Rgba([value, value, value, 255]));
        }
        let mut recipe = EditRecipe::default();
        recipe.auto_contrast = true;
        let output = apply_recipe(source, &recipe);
        let dark = output.get_pixel(2, 0)[0] as i16;
        let light = output.get_pixel(1, 0)[0] as i16;
        assert!(light - dark > 80);
    }

    #[test]
    fn auto_color_keeps_intentional_warmth() {
        let source = RgbaImage::from_pixel(20, 20, Rgba([160, 120, 90, 255]));
        let mut recipe = EditRecipe::default();
        recipe.auto_color = true;
        let output = apply_recipe(source, &recipe);
        let pixel = output.get_pixel(0, 0);
        assert!(pixel[0] > pixel[1] && pixel[1] > pixel[2]);
    }
}
