//! Instagram/Picasa-style one-tap filters.
//!
//! Each preset is a table of tuning constants run through one shared pipeline.
//! The pipeline is deliberately independent from the editor UI and renderer so
//! the look of each preset can be tuned without changing recipe plumbing.

use image::{Rgba, RgbaImage};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FilterPreset {
    #[default]
    None,
    Clarendon,
    Gingham,
    Juno,
    Lark,
    Valencia,
    Walden,
    PencilSketch,
    DisposableFilm,
    DigicamFlash,
    DreamGlow,
    Cinematic,
    GoldenHour,
}

impl FilterPreset {
    pub const ALL: [FilterPreset; 12] = [
        FilterPreset::Clarendon,
        FilterPreset::Gingham,
        FilterPreset::Juno,
        FilterPreset::Lark,
        FilterPreset::Valencia,
        FilterPreset::Walden,
        Self::PencilSketch,
        Self::DisposableFilm,
        Self::DigicamFlash,
        Self::DreamGlow,
        Self::Cinematic,
        Self::GoldenHour,
    ];

    pub fn label(self) -> &'static str {
        match self {
            FilterPreset::None => "None",
            FilterPreset::Clarendon => "Clarendon",
            FilterPreset::Gingham => "Gingham",
            FilterPreset::Juno => "Juno",
            FilterPreset::Lark => "Lark",
            FilterPreset::Valencia => "Valencia",
            FilterPreset::Walden => "Walden",
            Self::PencilSketch => "Pencil Sketch",
            Self::DisposableFilm => "Disposable Film",
            Self::DigicamFlash => "Digicam Flash",
            Self::DreamGlow => "Dream Glow",
            Self::Cinematic => "Cinematic",
            Self::GoldenHour => "Golden Hour",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            FilterPreset::None => "none",
            FilterPreset::Clarendon => "clarendon",
            FilterPreset::Gingham => "gingham",
            FilterPreset::Juno => "juno",
            FilterPreset::Lark => "lark",
            FilterPreset::Valencia => "valencia",
            FilterPreset::Walden => "walden",
            Self::PencilSketch => "pencil_sketch",
            Self::DisposableFilm => "disposable_film",
            Self::DigicamFlash => "digicam_flash",
            Self::DreamGlow => "dream_glow",
            Self::Cinematic => "cinematic",
            Self::GoldenHour => "golden_hour",
        }
    }

    pub fn decode(key: &str) -> Self {
        match key {
            "clarendon" => Self::Clarendon,
            "gingham" => Self::Gingham,
            "juno" => Self::Juno,
            "lark" => Self::Lark,
            "valencia" => Self::Valencia,
            "walden" => Self::Walden,
            "pencil_sketch" => Self::PencilSketch,
            "disposable_film" => Self::DisposableFilm,
            "digicam_flash" => Self::DigicamFlash,
            "dream_glow" => Self::DreamGlow,
            "cinematic" => Self::Cinematic,
            "golden_hour" => Self::GoldenHour,
            _ => Self::None,
        }
    }
}

type Rgb = (f32, f32, f32);

struct FilterParams {
    exposure: f32,
    shadow_lift: f32,
    tint: Rgb,
    shadow_tone: Rgb,
    highlight_tone: Rgb,
    split_strength: f32,
    saturation: f32,
    contrast: f32,
    vignette: f32,
}

fn params(preset: FilterPreset) -> FilterParams {
    match preset {
        FilterPreset::Clarendon => FilterParams {
            exposure: 1.05,
            shadow_lift: 0.02,
            tint: (0.99, 1.01, 1.04),
            shadow_tone: (0.85, 0.92, 1.05),
            highlight_tone: (1.02, 1.02, 1.00),
            split_strength: 0.35,
            saturation: 1.25,
            contrast: 0.20,
            vignette: 0.10,
        },
        FilterPreset::Gingham => FilterParams {
            exposure: 1.02,
            shadow_lift: 0.12,
            tint: (1.03, 1.00, 0.94),
            shadow_tone: (1.05, 1.00, 0.92),
            highlight_tone: (1.00, 0.99, 0.95),
            split_strength: 0.25,
            saturation: 0.85,
            contrast: -0.10,
            vignette: 0.12,
        },
        FilterPreset::Juno => FilterParams {
            exposure: 1.03,
            shadow_lift: 0.03,
            tint: (1.08, 1.00, 0.92),
            shadow_tone: (1.02, 0.95, 0.85),
            highlight_tone: (1.05, 1.00, 0.90),
            split_strength: 0.30,
            saturation: 1.30,
            contrast: 0.15,
            vignette: 0.18,
        },
        FilterPreset::Lark => FilterParams {
            exposure: 1.08,
            shadow_lift: 0.05,
            tint: (0.97, 1.00, 1.03),
            shadow_tone: (0.90, 0.97, 1.05),
            highlight_tone: (0.98, 1.00, 1.02),
            split_strength: 0.20,
            saturation: 0.90,
            contrast: 0.05,
            vignette: 0.08,
        },
        FilterPreset::Valencia => FilterParams {
            exposure: 1.06,
            shadow_lift: 0.10,
            tint: (1.10, 1.00, 0.92),
            shadow_tone: (1.05, 0.95, 0.95),
            highlight_tone: (1.08, 1.00, 0.90),
            split_strength: 0.30,
            saturation: 1.10,
            contrast: -0.05,
            vignette: 0.15,
        },
        FilterPreset::Walden => FilterParams {
            exposure: 1.12,
            shadow_lift: 0.08,
            tint: (1.05, 1.02, 0.85),
            shadow_tone: (1.00, 0.98, 0.90),
            highlight_tone: (1.05, 1.02, 0.85),
            split_strength: 0.25,
            saturation: 1.05,
            contrast: -0.08,
            vignette: 0.20,
        },
        FilterPreset::DisposableFilm => FilterParams {
            exposure: 1.04,
            shadow_lift: 0.065,
            tint: (1.05, 1.015, 0.93),
            saturation: 0.88,
            contrast: 0.12,
            vignette: 0.28,
            ..params(FilterPreset::None)
        },
        FilterPreset::DigicamFlash => FilterParams {
            exposure: 1.16,
            tint: (0.98, 1.01, 1.055),
            saturation: 1.17,
            contrast: 0.38,
            vignette: 0.22,
            ..params(FilterPreset::None)
        },
        FilterPreset::DreamGlow => FilterParams {
            exposure: 1.035,
            shadow_lift: 0.035,
            tint: (1.025, 0.99, 1.025),
            saturation: 0.84,
            contrast: -0.18,
            ..params(FilterPreset::None)
        },
        FilterPreset::Cinematic => FilterParams {
            exposure: 0.96,
            shadow_lift: 0.025,
            shadow_tone: (0.72, 1.06, 1.12),
            highlight_tone: (1.12, 1.025, 0.86),
            split_strength: 0.65,
            saturation: 0.85,
            contrast: 0.28,
            vignette: 0.18,
            ..params(FilterPreset::None)
        },
        FilterPreset::GoldenHour => FilterParams {
            exposure: 1.055,
            shadow_lift: 0.025,
            tint: (1.10, 1.025, 0.88),
            shadow_tone: (1.03, 0.96, 1.025),
            highlight_tone: (1.10, 1.025, 0.86),
            split_strength: 0.40,
            saturation: 1.08,
            contrast: 0.06,
            ..params(FilterPreset::None)
        },
        FilterPreset::None | FilterPreset::PencilSketch => FilterParams {
            exposure: 1.0,
            shadow_lift: 0.0,
            tint: (1.0, 1.0, 1.0),
            shadow_tone: (1.0, 1.0, 1.0),
            highlight_tone: (1.0, 1.0, 1.0),
            split_strength: 0.0,
            saturation: 1.0,
            contrast: 0.0,
            vignette: 0.0,
        },
    }
}

pub fn apply_filter(image: &mut RgbaImage, preset: FilterPreset) {
    if preset == FilterPreset::None {
        return;
    }
    if image.width() == 0 || image.height() == 0 {
        return;
    }
    if preset == FilterPreset::PencilSketch {
        apply_pencil_sketch(image);
        return;
    }
    let p = params(preset);
    let (width, height) = image.dimensions();
    let cx = (width.max(1) - 1) as f32 * 0.5;
    let cy = (height.max(1) - 1) as f32 * 0.5;
    let max_dist = (cx * cx + cy * cy).sqrt().max(1.0);

    for (x, y, pixel) in image.enumerate_pixels_mut() {
        let alpha = pixel[3];
        let mut r = pixel[0] as f32 / 255.0;
        let mut g = pixel[1] as f32 / 255.0;
        let mut b = pixel[2] as f32 / 255.0;
        r *= p.exposure;
        g *= p.exposure;
        b *= p.exposure;
        r += p.shadow_lift * (1.0 - r);
        g += p.shadow_lift * (1.0 - g);
        b += p.shadow_lift * (1.0 - b);
        r *= p.tint.0;
        g *= p.tint.1;
        b *= p.tint.2;
        let luma = (0.2126 * r + 0.7152 * g + 0.0722 * b).clamp(0.0, 1.0);
        let shadow_weight = (1.0 - luma) * p.split_strength;
        let highlight_weight = luma * p.split_strength;
        r = r * (1.0 - shadow_weight) + r * p.shadow_tone.0 * shadow_weight;
        g = g * (1.0 - shadow_weight) + g * p.shadow_tone.1 * shadow_weight;
        b = b * (1.0 - shadow_weight) + b * p.shadow_tone.2 * shadow_weight;
        r = r * (1.0 - highlight_weight) + r * p.highlight_tone.0 * highlight_weight;
        g = g * (1.0 - highlight_weight) + g * p.highlight_tone.1 * highlight_weight;
        b = b * (1.0 - highlight_weight) + b * p.highlight_tone.2 * highlight_weight;
        let gray = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        r = gray + (r - gray) * p.saturation;
        g = gray + (g - gray) * p.saturation;
        b = gray + (b - gray) * p.saturation;
        r = contrast_curve(r, p.contrast);
        g = contrast_curve(g, p.contrast);
        b = contrast_curve(b, p.contrast);
        let dx = x as f32 - cx;
        let dy = y as f32 - cy;
        let falloff = 1.0 - p.vignette * ((dx * dx + dy * dy).sqrt() / max_dist).powi(2);
        r *= falloff;
        g *= falloff;
        b *= falloff;
        *pixel = Rgba([
            (r.clamp(0.0, 1.0) * 255.0).round() as u8,
            (g.clamp(0.0, 1.0) * 255.0).round() as u8,
            (b.clamp(0.0, 1.0) * 255.0).round() as u8,
            alpha,
        ]);
    }
    match preset {
        FilterPreset::DisposableFilm => apply_grain(image, 0.035),
        FilterPreset::DigicamFlash => apply_grain(image, 0.012),
        FilterPreset::DreamGlow => apply_glow(image),
        _ => {}
    }
}

/// Colour-dodge sketch: local luminance differences form graphite strokes,
/// with a little original shading retained on the paper. Scale the blur with
/// the image so previews and exports have comparable stroke widths.
fn apply_pencil_sketch(image: &mut RgbaImage) {
    let gray = image::GrayImage::from_fn(image.width(), image.height(), |x, y| {
        let p = image.get_pixel(x, y);
        let luma = 0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32;
        // Transparent pixels represent paper, avoiding dark hidden-RGB halos.
        image::Luma([(255.0 + (luma - 255.0) * p[3] as f32 / 255.0).round() as u8])
    });
    let sigma = (image.width().min(image.height()) as f32 * 0.004).max(0.8);
    let blurred = image::imageops::blur(&gray, sigma);
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        let value = gray.get_pixel(x, y)[0] as f32 / 255.0;
        let local = blurred.get_pixel(x, y)[0] as f32 / 255.0;
        let dodge = ((value + 0.015) / (local + 0.015)).min(1.0);
        let graphite = dodge.powf(3.0) * (0.88 + 0.12 * value);
        let shade = (graphite * 255.0).round() as u8;
        pixel[0] = shade;
        pixel[1] = shade;
        pixel[2] = shade;
    }
}

/// Fixed coordinate noise keeps redraws deterministic (no preview flicker).
fn apply_grain(image: &mut RgbaImage, strength: f32) {
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        let mut hash = x.wrapping_mul(374761393) ^ y.wrapping_mul(668265263) ^ 0x9e3779b9;
        hash = (hash ^ (hash >> 13)).wrapping_mul(1274126177);
        hash ^= hash >> 16;
        let noise = ((hash & 65535) as f32 / 65535.0 - 0.5) * strength * 255.0;
        for channel in 0..3 {
            pixel[channel] = (pixel[channel] as f32 + noise).round().clamp(0.0, 255.0) as u8;
        }
    }
}

fn apply_glow(image: &mut RgbaImage) {
    let highlights = image::GrayImage::from_fn(image.width(), image.height(), |x, y| {
        let p = image.get_pixel(x, y);
        let luma = (0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32) / 255.0;
        image::Luma([(((luma - 0.55) / 0.45).clamp(0.0, 1.0) * p[3] as f32).round() as u8])
    });
    let sigma = (image.width().min(image.height()) as f32 * 0.008).max(0.8);
    let bloom = image::imageops::blur(&highlights, sigma);
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        let glow = bloom.get_pixel(x, y)[0] as f32 / 255.0 * 0.45;
        for channel in 0..3 {
            let original = pixel[channel] as f32;
            pixel[channel] = (original + (255.0 - original) * glow).round() as u8;
        }
    }
}

fn contrast_curve(value: f32, contrast: f32) -> f32 {
    let x = value.clamp(0.0, 1.0);
    let amount = contrast.clamp(-1.0, 1.0) * 0.55;
    (x + amount * 4.0 * (x - 0.5) * x * (1.0 - x)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn none_preset_is_a_no_op() {
        let mut image = RgbaImage::from_pixel(4, 4, Rgba([120, 130, 140, 255]));
        let before = image.clone();
        apply_filter(&mut image, FilterPreset::None);
        assert_eq!(image.as_raw(), before.as_raw());
    }

    #[test]
    fn every_named_preset_changes_the_image() {
        for preset in FilterPreset::ALL {
            let mut image = RgbaImage::from_pixel(6, 6, Rgba([120, 130, 140, 255]));
            let before = image.clone();
            apply_filter(&mut image, preset);
            assert_ne!(image.as_raw(), before.as_raw(), "{preset:?} did nothing");
        }
    }

    #[test]
    fn alpha_is_preserved() {
        for preset in FilterPreset::ALL {
            let mut image =
                RgbaImage::from_fn(8, 8, |x, y| Rgba([10, 200, 90, (x * 31 + y) as u8]));
            let before = image.clone();
            apply_filter(&mut image, preset);
            assert!(
                image
                    .pixels()
                    .zip(before.pixels())
                    .all(|(a, b)| a[3] == b[3]),
                "{preset:?}"
            );
        }
    }

    #[test]
    fn pencil_draws_dark_edges_on_light_paper() {
        let mut image = RgbaImage::from_fn(64, 64, |x, _| {
            let v = if x < 32 { 60 } else { 210 };
            Rgba([v, v, v, 255])
        });
        apply_filter(&mut image, FilterPreset::PencilSketch);
        assert!(image.pixels().all(|p| p[0] == p[1] && p[1] == p[2]));
        assert!(
            image.get_pixel(8, 32)[0] > 220,
            "flat areas should look like paper"
        );
        assert!(
            image.get_pixel(31, 32)[0] + 40 < image.get_pixel(8, 32)[0],
            "boundary should form a pencil stroke"
        );
    }

    #[test]
    fn glow_spreads_highlights_into_neighbouring_pixels() {
        let mut image = RgbaImage::from_fn(64, 64, |x, y| {
            let v = if (28..36).contains(&x) && (28..36).contains(&y) {
                255
            } else {
                30
            };
            Rgba([v, v, v, 255])
        });
        apply_filter(&mut image, FilterPreset::DreamGlow);
        assert!(image.get_pixel(27, 32)[0] > image.get_pixel(8, 32)[0]);
    }

    #[test]
    fn presets_are_distinct_deterministic_and_persist_through_recipes() {
        let source = RgbaImage::from_fn(32, 32, |x, y| {
            Rgba([(x * 8) as u8, (y * 8) as u8, ((x + y) * 4) as u8, 255])
        });
        let mut outputs = Vec::new();
        for preset in FilterPreset::ALL {
            let recipe = crate::edit::EditRecipe {
                filter: preset,
                ..Default::default()
            };
            let saved = crate::edit::EditRecipe::decode(&recipe.encode());
            assert_eq!(saved.filter, preset);
            let output = crate::edit::render::apply_recipe(source.clone(), &saved);
            assert_eq!(
                output,
                crate::edit::render::apply_recipe(source.clone(), &saved)
            );
            assert!(!outputs.contains(&output), "duplicate look: {preset:?}");
            outputs.push(output);
        }
    }

    #[test]
    fn presets_handle_empty_and_single_pixel_images() {
        for preset in FilterPreset::ALL {
            for (width, height) in [(0, 0), (0, 2), (1, 1), (1, 8), (8, 1)] {
                let mut image = RgbaImage::from_pixel(width, height, Rgba([80, 120, 160, 255]));
                apply_filter(&mut image, preset);
                assert_eq!(image.dimensions(), (width, height));
            }
        }
    }

    #[test]
    fn key_round_trips_through_decode() {
        for preset in FilterPreset::ALL {
            assert_eq!(FilterPreset::decode(preset.key()), preset);
        }
        assert_eq!(
            FilterPreset::decode("not-a-real-filter"),
            FilterPreset::None
        );
    }
}
