use anyhow::{Context, Result};
use image::{DynamicImage, ImageEncoder, RgbaImage};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use super::model::{Background, CollageProject};

pub fn export(project: &CollageProject, destination: &Path) -> Result<()> {
    let offline = project
        .items
        .iter()
        .filter(|item| !crate::source::file_available(&item.photo.path))
        .map(|item| item.photo.filename.as_str())
        .collect::<Vec<_>>();
    if !offline.is_empty() {
        let files = offline
            .iter()
            .map(|filename| format!("• {filename}"))
            .collect::<Vec<_>>()
            .join("\n");
        return Err(anyhow::anyhow!(
            "Cannot export collage because these original files are offline:\n{files}"
        ));
    }

    let width = 3840u32;
    let height = (width as f32 / project.aspect.value()).round() as u32;
    trace_export(&format!("destination={}", destination.display()));
    trace_export(&format!(
        "canvas={}x{} items={}",
        width,
        height,
        project.items.len()
    ));
    let background = match project.background {
        Background::White => image::Rgba([255, 255, 255, 255]),
        Background::Black => image::Rgba([0, 0, 0, 255]),
        Background::LightGray => image::Rgba([238, 238, 238, 255]),
    };
    let mut canvas = RgbaImage::from_pixel(width, height, background);
    let mut items = project.items.iter().collect::<Vec<_>>();
    items.sort_by_key(|item| item.z);
    for item in items {
        let target_width = (item.width.max(0.01) * width as f32).ceil() as u32;
        let target_height = (item.height.max(0.01) * height as f32).ceil() as u32;
        let decoded = match crate::thumbnail::decode_for_viewer(
            &item.photo.path,
            target_width.max(1),
            target_height.max(1),
        ) {
            Ok(decoded) => decoded,
            Err(error) => {
                trace_export(&format!(
                    "decode_failed id={} path={} error={error:#}",
                    item.photo.id, item.photo.path
                ));
                trace_export(&format!("failed stage=decode error={error:#}"));
                return Err(error).with_context(|| {
                    format!(
                        "could not decode original {} for collage export",
                        item.photo.filename
                    )
                });
            }
        };
        // decode_for_viewer has already applied source EXIF orientation. The
        // database rotation is PIC presentation metadata and is applied once
        // here, before the collage's own placement rotation.
        let source = apply_library_rotation(
            DynamicImage::ImageRgba8(decoded),
            item.photo.library_rotation,
        );
        let mut image = cover(source, target_width, target_height);
        if item.rotation.abs() > 0.1 {
            image = rotate(&image, item.rotation.to_radians(), background);
        }
        if project.round_corners {
            round_corners(&mut image, project.corner_radius);
        }
        let x = (item.x * width as f32) as i64;
        let y = (item.y * height as f32) as i64;
        image::imageops::overlay(&mut canvas, &image, x, y);
    }
    let parent = destination
        .parent()
        .filter(|path| path.as_os_str().len() > 0)
        .unwrap_or_else(|| Path::new("."));
    if !parent.is_dir() {
        let error = anyhow::anyhow!("parent directory does not exist: {}", parent.display());
        trace_export(&format!("failed stage=parent_directory error={error}"));
        return Err(error);
    }

    let file = match File::create(destination) {
        Ok(file) => file,
        Err(error) => {
            trace_export(&format!("failed stage=file_create error={error}"));
            return Err(error).with_context(|| {
                format!("could not create JPEG collage at {}", destination.display())
            });
        }
    };
    trace_export(&format!("encoding destination={}", destination.display()));
    let mut writer = BufWriter::new(file);
    let rgb = DynamicImage::ImageRgba8(canvas).to_rgb8();
    let encode_result = {
        let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut writer, 90);
        encoder.write_image(&rgb, width, height, image::ExtendedColorType::Rgb8)
    };
    if let Err(error) = encode_result {
        trace_export(&format!("failed stage=jpeg_encode error={error}"));
        return Err(error).context("could not encode JPEG collage");
    }
    if let Err(error) = writer.flush() {
        trace_export(&format!("failed stage=flush error={error}"));
        return Err(error)
            .with_context(|| format!("could not flush JPEG collage at {}", destination.display()));
    }
    trace_export(&format!("success destination={}", destination.display()));
    Ok(())
}

fn round_corners(image: &mut RgbaImage, radius_ratio: f32) {
    let radius = (image.width().min(image.height()) as f32 * radius_ratio)
        .round()
        .max(1.0);
    let radius_squared = radius * radius;
    let width = image.width() as f32;
    let height = image.height() as f32;

    for y in 0..image.height() {
        for x in 0..image.width() {
            let x = x as f32;
            let y = y as f32;
            let distance_squared = if x < radius && y < radius {
                (radius - x) * (radius - x) + (radius - y) * (radius - y)
            } else if x >= width - radius && y < radius {
                (x - (width - radius - 1.0)).powi(2) + (radius - y).powi(2)
            } else if x < radius && y >= height - radius {
                (radius - x).powi(2) + (y - (height - radius - 1.0)).powi(2)
            } else if x >= width - radius && y >= height - radius {
                (x - (width - radius - 1.0)).powi(2) + (y - (height - radius - 1.0)).powi(2)
            } else {
                0.0
            };
            if distance_squared > radius_squared {
                image.get_pixel_mut(x as u32, y as u32).0[3] = 0;
            }
        }
    }
}

fn trace_export(message: &str) {
    if std::env::var_os("PICASA_TRACE").is_some() {
        eprintln!("COLLAGE EXPORT {message}");
    }
}

fn apply_library_rotation(source: DynamicImage, rotation: i32) -> DynamicImage {
    match rotation.rem_euclid(360) {
        90 => DynamicImage::ImageRgba8(image::imageops::rotate90(&source.to_rgba8())),
        180 => DynamicImage::ImageRgba8(image::imageops::rotate180(&source.to_rgba8())),
        270 => DynamicImage::ImageRgba8(image::imageops::rotate270(&source.to_rgba8())),
        _ => source,
    }
}

fn rotate(source: &RgbaImage, angle: f32, background: image::Rgba<u8>) -> RgbaImage {
    let (width, height) = source.dimensions();
    let sin = angle.sin().abs();
    let cos = angle.cos().abs();
    let output_width = (width as f32 * cos + height as f32 * sin).ceil() as u32;
    let output_height = (width as f32 * sin + height as f32 * cos).ceil() as u32;
    let mut output = RgbaImage::from_pixel(output_width.max(1), output_height.max(1), background);
    let center_x = width as f32 / 2.0;
    let center_y = height as f32 / 2.0;
    let output_center_x = output.width() as f32 / 2.0;
    let output_center_y = output.height() as f32 / 2.0;
    let (sin, cos) = angle.sin_cos();
    for y in 0..output.height() {
        for x in 0..output.width() {
            let dx = x as f32 - output_center_x;
            let dy = y as f32 - output_center_y;
            let source_x = dx * cos + dy * sin + center_x;
            let source_y = -dx * sin + dy * cos + center_y;
            if source_x >= 0.0
                && source_y >= 0.0
                && source_x < width as f32
                && source_y < height as f32
            {
                output.put_pixel(x, y, *source.get_pixel(source_x as u32, source_y as u32));
            }
        }
    }
    output
}

fn cover(source: DynamicImage, width: u32, height: u32) -> RgbaImage {
    let width = width.max(1);
    let height = height.max(1);
    let source_ratio = source.width() as f32 / source.height().max(1) as f32;
    let target_ratio = width as f32 / height as f32;
    let image = if source_ratio > target_ratio {
        let crop_width = (source.height() as f32 * target_ratio) as u32;
        let left = source.width().saturating_sub(crop_width) / 2;
        source.crop_imm(left, 0, crop_width.max(1), source.height())
    } else {
        let crop_height = (source.width() as f32 / target_ratio) as u32;
        let top = source.height().saturating_sub(crop_height) / 2;
        source.crop_imm(0, top, source.width(), crop_height.max(1))
    };
    image
        .resize_to_fill(width, height, image::imageops::FilterType::Lanczos3)
        .to_rgba8()
}
