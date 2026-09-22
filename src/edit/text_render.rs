use gtk4::cairo::{Context, Format, ImageSurface};
use gtk4::pango;
use image::RgbaImage;
use pangocairo::functions::{create_layout, show_layout};

use super::model::{NormRect, TextAlign, TextLayerSpec};

#[derive(Clone, Copy, Debug)]
pub struct TextMetrics {
    pub width: i32,
    pub height: i32,
    pub off_x: i32,
    pub off_y: i32,
}

fn font_px(layer: &TextLayerSpec, photo_height: f64) -> f64 {
    let size = f64::from(
        layer
            .size
            .clamp(TextLayerSpec::MIN_SIZE, TextLayerSpec::MAX_SIZE),
    );
    size * photo_height * f64::from(pango::SCALE)
}

fn configure_layout(layout: &pango::Layout, layer: &TextLayerSpec, size: f64) {
    layout.set_text(&layer.text);
    let mut description = pango::FontDescription::new();
    description.set_family(&layer.font_family);
    description.set_absolute_size(size);
    if layer.bold {
        description.set_weight(pango::Weight::Bold);
    }
    if layer.italic {
        description.set_style(pango::Style::Italic);
    }
    layout.set_font_description(Some(&description));
    layout.set_alignment(match layer.align {
        TextAlign::Left => pango::Alignment::Left,
        TextAlign::Center => pango::Alignment::Center,
        TextAlign::Right => pango::Alignment::Right,
    });
}

pub fn measure_text_px(layer: &TextLayerSpec, photo_height: f64) -> Option<TextMetrics> {
    if layer.text.is_empty() || photo_height <= 0.0 {
        return None;
    }
    let size = font_px(layer, photo_height);
    if size <= 0.0 {
        return None;
    }
    let surface = ImageSurface::create(Format::ARgb32, 1, 1).ok()?;
    let cr = Context::new(&surface).ok()?;
    let layout = create_layout(&cr);
    configure_layout(&layout, layer, size);
    let (ink, logical) = layout.pixel_extents();
    drop(cr);
    drop(surface);

    let left = ink.x().min(logical.x());
    let top = ink.y().min(logical.y());
    let right = (ink.x() + ink.width()).max(logical.x() + logical.width());
    let bottom = (ink.y() + ink.height()).max(logical.y() + logical.height());
    let width = right - left;
    let height = bottom - top;
    if width <= 0 || height <= 0 {
        return None;
    }
    Some(TextMetrics {
        width,
        height,
        off_x: left,
        off_y: top,
    })
}

pub fn text_rect(layer: &TextLayerSpec, photo_width: f64, photo_height: f64) -> Option<NormRect> {
    if photo_width <= 0.0 || photo_height <= 0.0 {
        return None;
    }
    let metrics = measure_text_px(layer, photo_height)?;
    Some(layer.rect_with_size(
        metrics.width as f32 / photo_width as f32,
        metrics.height as f32 / photo_height as f32,
    ))
}

pub fn render_text_rgba(
    layer: &TextLayerSpec,
    photo_width: f64,
    photo_height: f64,
) -> Option<RgbaImage> {
    if photo_width <= 0.0 || photo_height <= 0.0 {
        return None;
    }
    let metrics = measure_text_px(layer, photo_height)?;
    let mut surface = ImageSurface::create(Format::ARgb32, metrics.width, metrics.height).ok()?;
    {
        let cr = Context::new(&surface).ok()?;
        let (red, green, blue, alpha) = layer.color_rgba();
        cr.set_source_rgba(
            f64::from(red),
            f64::from(green),
            f64::from(blue),
            f64::from(alpha),
        );
        cr.move_to(f64::from(-metrics.off_x), f64::from(-metrics.off_y));
        let layout = create_layout(&cr);
        configure_layout(&layout, layer, font_px(layer, photo_height));
        show_layout(&cr, &layout);
    }

    let width = metrics.width as usize;
    let height = metrics.height as usize;
    let stride = surface.stride() as usize;
    let mut rgba = vec![0u8; width * height * 4];
    {
        let data = surface.data().ok()?;
        for y in 0..height {
            let row = &data[y * stride..y * stride + width * 4];
            for x in 0..width {
                let source = &row[x * 4..x * 4 + 4];
                let offset = (y * width + x) * 4;
                let out = &mut rgba[offset..offset + 4];
                // Cairo ARGB32 as a native u32 is always A<<24|R<<16|G<<8|B
                // (premultiplied); byte order in memory is BGRA on LE.
                let pixel = u32::from_ne_bytes([source[0], source[1], source[2], source[3]]);
                let source_alpha = (pixel >> 24) & 0xFF;
                if source_alpha == 0 {
                    out.fill(0);
                } else {
                    for (channel, shift) in [(0usize, 16u32), (1, 8), (2, 0)] {
                        let premultiplied = (pixel >> shift) & 0xFF;
                        out[channel] = ((premultiplied * 255 + source_alpha / 2) / source_alpha)
                            .min(255) as u8;
                    }
                    out[3] = source_alpha as u8;
                }
            }
        }
    }
    RgbaImage::from_raw(width as u32, height as u32, rgba)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layer_with(text: &str) -> TextLayerSpec {
        let mut layer = TextLayerSpec::new_default();
        layer.text = text.to_string();
        layer.size = 0.08;
        layer
    }

    #[test]
    fn measure_returns_a_non_empty_box_in_device_pixels() {
        let layer = layer_with("Hello");
        let metrics = measure_text_px(&layer, 1000.0).expect("non-empty text measures");
        assert!(metrics.width > 10, "width {}", metrics.width);
        assert!(
            metrics.height > 20 && metrics.height < 400,
            "absolute font size is device pixels: height {}",
            metrics.height
        );
    }

    #[test]
    fn empty_text_has_no_box() {
        assert!(measure_text_px(&layer_with(""), 1000.0).is_none());
        assert!(text_rect(&layer_with(""), 1000.0, 800.0).is_none());
        assert!(render_text_rgba(&layer_with(""), 1000.0, 800.0).is_none());
    }

    #[test]
    fn multiline_text_is_taller_than_a_single_line() {
        let single = measure_text_px(&layer_with("One"), 1000.0).unwrap();
        let multi = measure_text_px(&layer_with("One\nTwo\nThree"), 1000.0).unwrap();
        assert!(multi.height > single.height * 2);
        assert_eq!(multi.width >= single.width, true);
    }

    #[test]
    fn render_matches_measure_and_contains_ink() {
        let layer = layer_with("Ink test");
        let metrics = measure_text_px(&layer, 800.0).unwrap();
        let raster = render_text_rgba(&layer, 800.0, 800.0).unwrap();
        assert_eq!(raster.width(), metrics.width as u32);
        assert_eq!(raster.height(), metrics.height as u32);
        let inked = raster.pixels().filter(|pixel| pixel.0[3] > 0).count();
        assert!(inked > 0, "glyphs must paint at least one pixel");
        assert!(
            inked < (raster.width() * raster.height()) as usize,
            "not every pixel is ink"
        );
    }

    #[test]
    fn white_text_lands_where_the_anchor_says() {
        let mut layer = layer_with("X");
        layer.set_color_rgba(1.0, 1.0, 1.0, 1.0);
        layer.anchor = super::super::model::OverlayAnchor::TopLeft;
        layer.x = 0.0;
        layer.y = 0.0;
        let rect = text_rect(&layer, 1000.0, 1000.0).unwrap();
        assert!(rect.left.abs() < 1e-5);
        assert!(rect.top.abs() < 1e-5);
        assert!(rect.width > 0.001);
        assert!(rect.height > 0.001);

        let raster = render_text_rgba(&layer, 1000.0, 1000.0).unwrap();
        let alpha_sum: u32 = raster.pixels().map(|pixel| u32::from(pixel.0[3])).sum();
        assert!(alpha_sum > 0);
        let lit = raster.pixels().any(|pixel| {
            pixel.0[3] > 0 && pixel.0[0] > 200 && pixel.0[1] > 200 && pixel.0[2] > 200
        });
        assert!(lit, "unpremultiplied white glyphs recover near 255 rgb");
    }

    #[test]
    fn saturated_picker_colours_keep_their_channel_order() {
        for (red, green, blue, expect) in [
            (1.0, 0.0, 0.0, (255u8, 0u8, 0u8)),
            (0.0, 1.0, 0.0, (0u8, 255u8, 0u8)),
            (0.0, 0.0, 1.0, (0u8, 0u8, 255u8)),
        ] {
            let mut layer = layer_with("RGB");
            layer.set_color_rgba(red, green, blue, 1.0);
            let raster = render_text_rgba(&layer, 800.0, 800.0).unwrap();
            let lit: Vec<_> = raster.pixels().filter(|pixel| pixel.0[3] > 200).collect();
            assert!(!lit.is_empty(), "glyphs must paint opaque pixels");
            let dominant = lit
                .iter()
                .filter(|pixel| {
                    pixel.0[0] == expect.0 && pixel.0[1] == expect.1 && pixel.0[2] == expect.2
                })
                .count();
            assert!(
                dominant > lit.len() / 2,
                "picked rgba({red},{green},{blue}) must render as {:?}, got sample {:?}",
                expect,
                lit[0].0
            );
        }
    }
}
