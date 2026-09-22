#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CropRect {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Default for CropRect {
    fn default() -> Self {
        Self {
            left: 0.0,
            top: 0.0,
            right: 1.0,
            bottom: 1.0,
        }
    }
}

impl CropRect {
    pub fn normalized(self) -> Self {
        let mut left = self.left.clamp(0.0, 1.0);
        let mut right = self.right.clamp(0.0, 1.0);
        let mut top = self.top.clamp(0.0, 1.0);
        let mut bottom = self.bottom.clamp(0.0, 1.0);
        if right < left {
            std::mem::swap(&mut left, &mut right);
        }
        if bottom < top {
            std::mem::swap(&mut top, &mut bottom);
        }
        if right - left < 0.01 {
            right = (left + 0.01).min(1.0);
            left = (right - 0.01).max(0.0);
        }
        if bottom - top < 0.01 {
            bottom = (top + 0.01).min(1.0);
            top = (bottom - 0.01).max(0.0);
        }
        Self {
            left,
            top,
            right,
            bottom,
        }
    }

    pub fn is_full(self) -> bool {
        (self.left.abs() < 0.0001)
            && (self.top.abs() < 0.0001)
            && ((self.right - 1.0).abs() < 0.0001)
            && ((self.bottom - 1.0).abs() < 0.0001)
    }

    /// Compose a crop selected inside the already-cropped image with the
    /// persisted crop. This makes repeated crop operations predictable while
    /// keeping every crop normalized to the original edited frame.
    pub fn compose(self, child: CropRect) -> CropRect {
        let parent = self.normalized();
        let child = child.normalized();
        let width = parent.right - parent.left;
        let height = parent.bottom - parent.top;
        CropRect {
            left: parent.left + child.left * width,
            top: parent.top + child.top * height,
            right: parent.left + child.right * width,
            bottom: parent.top + child.bottom * height,
        }
        .normalized()
    }
}

/// Which point of an overlay's bounding box is pinned at `(x, y)`.
///
/// The anchor makes a copied overlay land in the same visual corner of a
/// destination photo regardless of that photo's aspect ratio: a logo stored
/// with `BottomRight` at (0.95, 0.95) keeps its bottom-right corner near the
/// bottom-right of every photo it is pasted onto.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverlayAnchor {
    TopLeft,
    TopRight,
    Center,
    BottomLeft,
    BottomRight,
}

impl OverlayAnchor {
    #[cfg(test)]
    pub const ALL: [OverlayAnchor; 5] = [
        OverlayAnchor::TopLeft,
        OverlayAnchor::TopRight,
        OverlayAnchor::Center,
        OverlayAnchor::BottomLeft,
        OverlayAnchor::BottomRight,
    ];

    /// Compact wire code used inside the edit-recipe string.
    pub fn code(self) -> &'static str {
        match self {
            Self::TopLeft => "tl",
            Self::TopRight => "tr",
            Self::Center => "c",
            Self::BottomLeft => "bl",
            Self::BottomRight => "br",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        match code {
            "tl" => Some(Self::TopLeft),
            "tr" => Some(Self::TopRight),
            "c" => Some(Self::Center),
            "bl" => Some(Self::BottomLeft),
            "br" => Some(Self::BottomRight),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::TopLeft => "Top Left",
            Self::TopRight => "Top Right",
            Self::Center => "Centre",
            Self::BottomLeft => "Bottom Left",
            Self::BottomRight => "Bottom Right",
        }
    }
}

/// Horizontal alignment of the lines inside a text layer's bounding box.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextAlign {
    Left,
    Center,
    Right,
}

impl TextAlign {
    pub fn code(self) -> &'static str {
        match self {
            Self::Left => "l",
            Self::Center => "c",
            Self::Right => "r",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        match code {
            "l" => Some(Self::Left),
            "c" => Some(Self::Center),
            "r" => Some(Self::Right),
            _ => None,
        }
    }
}

/// Axis-aligned rectangle in normalized final-photo space (0..1 per axis).
///
/// Overlay geometry is stored and rendered relative to the final photograph
/// dimensions, never in screen or canvas pixels, so an overlay placed on a
/// 6000x4000 photo keeps the same relative position and size when pasted onto
/// a 4000x6000 photo.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NormRect {
    pub left: f32,
    pub top: f32,
    pub width: f32,
    pub height: f32,
}

impl NormRect {
    pub fn right(self) -> f32 {
        self.left + self.width
    }

    pub fn bottom(self) -> f32 {
        self.top + self.height
    }

    /// Translate-only clamp that keeps the rectangle inside the photo when it
    /// fits, and centres it on axes where it is larger than the photo.
    /// Size is never altered, so an overlay's aspect ratio survives dragging.
    pub fn clamp_inside(self) -> Self {
        let width = self.width.max(0.0);
        let height = self.height.max(0.0);
        let left = if width >= 1.0 {
            (1.0 - width) * 0.5
        } else {
            self.left.clamp(0.0, 1.0 - width)
        };
        let top = if height >= 1.0 {
            (1.0 - height) * 0.5
        } else {
            self.top.clamp(0.0, 1.0 - height)
        };
        Self {
            left,
            top,
            width,
            height,
        }
    }
}

/// One image overlay placed on a photograph.
///
/// Positions and sizes are resolution-independent: `(x, y)` is the normalized
/// photo position of `anchor`, and `width` is the overlay's width as a fraction
/// of the photo's width. Height is derived from the asset's intrinsic aspect
/// ratio and the photo's aspect ratio, which preserves the overlay's pixel
/// aspect ratio across any destination dimensions.
#[derive(Clone, Debug, PartialEq)]
pub struct OverlaySpec {
    /// Content hash (blake3 hex) of the asset in the application-managed
    /// overlay directory. The original file path is never relied upon.
    pub asset: String,
    pub anchor: OverlayAnchor,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    /// 0.0 ..= 1.0
    pub opacity: f32,
    /// Degrees. Reserved for future rotation controls; 0.0 for now.
    pub rotation: f32,
    pub visible: bool,
}

impl OverlaySpec {
    pub const MIN_WIDTH: f32 = 0.02;
    pub const MAX_WIDTH: f32 = 1.0;
    pub const DEFAULT_WIDTH: f32 = 0.25;
    pub const POSITION_MARGIN: f32 = 0.03;

    /// A centered overlay covering a quarter of the photo's width.
    pub fn new_centered(asset: impl Into<String>) -> Self {
        Self {
            asset: asset.into(),
            anchor: OverlayAnchor::Center,
            x: 0.5,
            y: 0.5,
            width: Self::DEFAULT_WIDTH,
            opacity: 1.0,
            rotation: 0.0,
            visible: true,
        }
    }

    /// Normalized rectangle for this overlay on a photo of the given
    /// dimensions. `asset_aspect` is the overlay image's width / height.
    pub fn rect(&self, photo_width: f32, photo_height: f32, asset_aspect: f32) -> NormRect {
        let photo_width = photo_width.max(1.0e-6);
        let photo_height = photo_height.max(1.0e-6);
        let aspect = if asset_aspect.is_finite() && asset_aspect > 1.0e-6 {
            asset_aspect
        } else {
            1.0
        };
        let width = self.width.clamp(Self::MIN_WIDTH, Self::MAX_WIDTH);
        // Pixel height = pixel width / asset aspect; normalized height divides
        // that by the photo height.
        let height = width * photo_width / photo_height / aspect;
        let (left, top) = match self.anchor {
            OverlayAnchor::TopLeft => (self.x, self.y),
            OverlayAnchor::TopRight => (self.x - width, self.y),
            OverlayAnchor::Center => (self.x - width * 0.5, self.y - height * 0.5),
            OverlayAnchor::BottomLeft => (self.x, self.y - height),
            OverlayAnchor::BottomRight => (self.x - width, self.y - height),
        };
        NormRect {
            left,
            top,
            width,
            height,
        }
    }

    /// Re-express position and size as a stored anchor point plus width
    /// fraction. `photo_*` and `asset_aspect` are accepted so callers can pass
    /// them uniformly; only the anchor mapping needs them indirectly through
    /// the rectangle itself.
    pub fn set_rect(
        &mut self,
        rect: NormRect,
        _photo_width: f32,
        _photo_height: f32,
        _asset_aspect: f32,
    ) {
        let rect = NormRect {
            left: rect.left,
            top: rect.top,
            width: rect.width.clamp(Self::MIN_WIDTH, Self::MAX_WIDTH),
            height: rect.height,
        };
        self.width = rect.width;
        let (x, y) = match self.anchor {
            OverlayAnchor::TopLeft => (rect.left, rect.top),
            OverlayAnchor::TopRight => (rect.right(), rect.top),
            OverlayAnchor::Center => (rect.left + rect.width * 0.5, rect.top + rect.height * 0.5),
            OverlayAnchor::BottomLeft => (rect.left, rect.bottom()),
            OverlayAnchor::BottomRight => (rect.right(), rect.bottom()),
        };
        self.x = x;
        self.y = y;
    }

    /// Change the anchor while keeping the overlay exactly where it is.
    #[allow(dead_code)]
    pub fn reanchor(
        &mut self,
        anchor: OverlayAnchor,
        photo_width: f32,
        photo_height: f32,
        asset_aspect: f32,
    ) {
        let rect = self.rect(photo_width, photo_height, asset_aspect);
        self.anchor = anchor;
        self.set_rect(rect, photo_width, photo_height, asset_aspect);
    }

    /// Restore the default placement used when an overlay is first added.
    pub fn position_at(&mut self, anchor: OverlayAnchor) {
        let margin = Self::POSITION_MARGIN;
        self.anchor = anchor;
        let (x, y) = match anchor {
            OverlayAnchor::TopLeft => (margin, margin),
            OverlayAnchor::TopRight => (1.0 - margin, margin),
            OverlayAnchor::Center => (0.5, 0.5),
            OverlayAnchor::BottomLeft => (margin, 1.0 - margin),
            OverlayAnchor::BottomRight => (1.0 - margin, 1.0 - margin),
        };
        self.x = x;
        self.y = y;
    }

    pub fn reset_placement(&mut self) {
        self.anchor = OverlayAnchor::Center;
        self.x = 0.5;
        self.y = 0.5;
        self.width = Self::DEFAULT_WIDTH;
        self.opacity = 1.0;
        self.rotation = 0.0;
        self.visible = true;
    }

    /// Widest fraction this overlay may occupy while still fitting inside the
    /// photo both horizontally and vertically, preserving aspect ratio.
    pub fn max_fitting_width(photo_width: f32, photo_height: f32, asset_aspect: f32) -> f32 {
        let photo_width = photo_width.max(1.0);
        let photo_height = photo_height.max(1.0);
        let aspect = if asset_aspect.is_finite() && asset_aspect > 1.0e-6 {
            asset_aspect
        } else {
            1.0
        };
        let by_height = photo_height * aspect / photo_width;
        Self::MAX_WIDTH.min(by_height).max(Self::MIN_WIDTH)
    }

    pub fn fit_to_width(&mut self, photo_width: f32, photo_height: f32, asset_aspect: f32) {
        let photo_width = photo_width.max(1.0);
        let photo_height = photo_height.max(1.0);
        let aspect = if asset_aspect.is_finite() && asset_aspect > 1.0e-6 {
            asset_aspect
        } else {
            1.0
        };
        let current = self.rect(photo_width, photo_height, aspect);
        let centre_y = current.top + current.height * 0.5;
        let height = Self::MAX_WIDTH * photo_width / photo_height / aspect;
        let top = if height >= 1.0 {
            (1.0 - height) * 0.5
        } else {
            (centre_y - height * 0.5).clamp(0.0, 1.0 - height)
        };
        self.set_rect(
            NormRect {
                left: 0.0,
                top,
                width: Self::MAX_WIDTH,
                height,
            },
            photo_width,
            photo_height,
            asset_aspect,
        );
    }

    pub fn fit_to_screen(&mut self, photo_width: f32, photo_height: f32, asset_aspect: f32) {
        let photo_width = photo_width.max(1.0);
        let photo_height = photo_height.max(1.0);
        let aspect = if asset_aspect.is_finite() && asset_aspect > 1.0e-6 {
            asset_aspect
        } else {
            1.0
        };
        let width = Self::max_fitting_width(photo_width, photo_height, aspect);
        let height = width * photo_width / photo_height / aspect;
        self.set_rect(
            NormRect {
                left: (1.0 - width) * 0.5,
                top: (1.0 - height) * 0.5,
                width,
                height,
            },
            photo_width,
            photo_height,
            asset_aspect,
        );
    }
}

fn next_text_layer_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!("t{nanos:x}-{n:x}")
}

/// One editable text layer placed on a photograph. Geometry follows the same
/// normalized anchor model as image overlays: `(x, y)` is the photo position
/// of `anchor`, and the bounding box is derived at render time from the font
/// size, content and photo dimensions (it is not stored).
#[derive(Clone, Debug, PartialEq)]
pub struct TextLayerSpec {
    pub id: String,
    /// Multiline content; `\n` separates lines.
    pub text: String,
    pub font_family: String,
    /// Font height as a fraction of the photo height, so text stays sharp at
    /// any export resolution.
    pub size: f32,
    pub bold: bool,
    pub italic: bool,
    /// 0xRRGGBBAA
    pub color: u32,
    pub align: TextAlign,
    pub anchor: OverlayAnchor,
    pub x: f32,
    pub y: f32,
    /// 0.0 ..= 1.0
    pub opacity: f32,
    pub visible: bool,
}

impl TextLayerSpec {
    pub const MIN_SIZE: f32 = 0.004;
    pub const MAX_SIZE: f32 = 0.5;
    pub const DEFAULT_SIZE: f32 = 0.045;
    pub const POSITION_MARGIN: f32 = OverlaySpec::POSITION_MARGIN;

    /// A centred layer with placeholder content, matching the defaults the
    /// Text panel shows on first use.
    pub fn new_default() -> Self {
        Self {
            id: next_text_layer_id(),
            text: "Your text".to_string(),
            font_family: "Sans".to_string(),
            size: Self::DEFAULT_SIZE,
            bold: false,
            italic: false,
            color: 0xFFFF_FFFF,
            align: TextAlign::Left,
            anchor: OverlayAnchor::Center,
            x: 0.5,
            y: 0.5,
            opacity: 1.0,
            visible: true,
        }
    }

    /// Normalized bounding box for a measured box already expressed as photo
    /// fractions, anchored like `OverlaySpec::rect`.
    pub fn rect_with_size(&self, width: f32, height: f32) -> NormRect {
        let (left, top) = match self.anchor {
            OverlayAnchor::TopLeft => (self.x, self.y),
            OverlayAnchor::TopRight => (self.x - width, self.y),
            OverlayAnchor::Center => (self.x - width * 0.5, self.y - height * 0.5),
            OverlayAnchor::BottomLeft => (self.x, self.y - height),
            OverlayAnchor::BottomRight => (self.x - width, self.y - height),
        };
        NormRect {
            left,
            top,
            width,
            height,
        }
    }

    /// Store the layer's anchor point so its bounding box lands exactly on
    /// `rect` (size is re-measured at render time, never stored here).
    pub fn set_position_from_rect(&mut self, rect: NormRect) {
        let (x, y) = match self.anchor {
            OverlayAnchor::TopLeft => (rect.left, rect.top),
            OverlayAnchor::TopRight => (rect.right(), rect.top),
            OverlayAnchor::Center => (rect.left + rect.width * 0.5, rect.top + rect.height * 0.5),
            OverlayAnchor::BottomLeft => (rect.left, rect.bottom()),
            OverlayAnchor::BottomRight => (rect.right(), rect.bottom()),
        };
        self.x = x;
        self.y = y;
    }

    /// Move the layer to a named photo position with the standard 3% margin,
    /// keeping content, formatting, size and opacity untouched.
    pub fn position_at(&mut self, anchor: OverlayAnchor) {
        let margin = Self::POSITION_MARGIN;
        self.anchor = anchor;
        let (x, y) = match anchor {
            OverlayAnchor::TopLeft => (margin, margin),
            OverlayAnchor::TopRight => (1.0 - margin, margin),
            OverlayAnchor::Center => (0.5, 0.5),
            OverlayAnchor::BottomLeft => (margin, 1.0 - margin),
            OverlayAnchor::BottomRight => (1.0 - margin, 1.0 - margin),
        };
        self.x = x;
        self.y = y;
    }

    pub fn reset_placement(&mut self) {
        self.anchor = OverlayAnchor::Center;
        self.x = 0.5;
        self.y = 0.5;
        self.opacity = 1.0;
        self.visible = true;
    }

    pub fn color_rgba(&self) -> (f32, f32, f32, f32) {
        (
            ((self.color >> 24) & 0xFF) as f32 / 255.0,
            ((self.color >> 16) & 0xFF) as f32 / 255.0,
            ((self.color >> 8) & 0xFF) as f32 / 255.0,
            (self.color & 0xFF) as f32 / 255.0,
        )
    }

    pub fn set_color_rgba(&mut self, red: f32, green: f32, blue: f32, alpha: f32) {
        let channel = |value: f32| ((value.clamp(0.0, 1.0) * 255.0).round() as u32) & 0xFF;
        self.color =
            (channel(red) << 24) | (channel(green) << 16) | (channel(blue) << 8) | channel(alpha);
    }
}

/// Compact JSON wire form for one overlay inside the edit-recipe string.
/// Field names stay short and are strictly typed (no free-form user text) so
/// the recipe's `key=value|key=value` framing is never ambiguous.
#[derive(serde::Serialize, serde::Deserialize)]
struct OverlayWire {
    /// asset content hash
    a: String,
    /// anchor code
    n: String,
    x: f32,
    y: f32,
    /// width as fraction of photo width
    w: f32,
    /// opacity 0..1
    o: f32,
    /// rotation degrees
    r: f32,
    /// visible
    v: bool,
}

fn decode_overlays(value: &str) -> Vec<OverlaySpec> {
    let Ok(wires) = serde_json::from_str::<Vec<OverlayWire>>(value) else {
        return Vec::new();
    };
    wires
        .into_iter()
        .filter_map(|wire| {
            if wire.a.trim().is_empty() {
                return None;
            }
            let anchor = OverlayAnchor::from_code(&wire.n)?;
            Some(OverlaySpec {
                asset: wire.a,
                anchor,
                x: wire.x,
                y: wire.y,
                width: wire.w.clamp(OverlaySpec::MIN_WIDTH, OverlaySpec::MAX_WIDTH),
                opacity: wire.o.clamp(0.0, 1.0),
                rotation: wire.r,
                visible: wire.v,
            })
        })
        .collect()
}

fn encode_overlays(overlays: &[OverlaySpec]) -> String {
    let wires = overlays
        .iter()
        .map(|overlay| OverlayWire {
            a: overlay.asset.clone(),
            n: overlay.anchor.code().to_string(),
            x: overlay.x,
            y: overlay.y,
            w: overlay.width,
            o: overlay.opacity,
            r: overlay.rotation,
            v: overlay.visible,
        })
        .collect::<Vec<_>>();
    serde_json::to_string(&wires).unwrap_or_else(|_| "[]".to_string())
}

/// Compact JSON wire form for one text layer inside the edit-recipe string.
/// Field names stay short and strictly typed so the recipe's
/// `key=value|key=value` framing is never ambiguous.
#[derive(serde::Serialize, serde::Deserialize)]
struct TextLayerWire {
    /// layer identifier
    i: String,
    /// text content
    t: String,
    /// font family
    f: String,
    /// size as fraction of photo height
    s: f32,
    b: bool,
    /// italic
    k: bool,
    /// colour 0xRRGGBBAA
    c: u32,
    /// alignment code
    a: String,
    /// anchor code
    n: String,
    x: f32,
    y: f32,
    /// opacity
    o: f32,
    v: bool,
}

fn decode_text_layers(value: &str) -> Vec<TextLayerSpec> {
    let Ok(wires) = serde_json::from_str::<Vec<TextLayerWire>>(value) else {
        return Vec::new();
    };
    wires
        .into_iter()
        .filter_map(|wire| {
            let anchor = OverlayAnchor::from_code(&wire.n)?;
            Some(TextLayerSpec {
                id: if wire.i.trim().is_empty() {
                    next_text_layer_id()
                } else {
                    wire.i
                },
                text: wire.t,
                font_family: if wire.f.trim().is_empty() {
                    "Sans".to_string()
                } else {
                    wire.f
                },
                size: wire
                    .s
                    .clamp(TextLayerSpec::MIN_SIZE, TextLayerSpec::MAX_SIZE),
                bold: wire.b,
                italic: wire.k,
                color: wire.c,
                align: TextAlign::from_code(&wire.a).unwrap_or(TextAlign::Left),
                anchor,
                x: wire.x,
                y: wire.y,
                opacity: wire.o.clamp(0.0, 1.0),
                visible: wire.v,
            })
        })
        .collect()
}

fn encode_text_layers(layers: &[TextLayerSpec]) -> String {
    let wires = layers
        .iter()
        .map(|layer| TextLayerWire {
            i: layer.id.clone(),
            t: layer.text.clone(),
            f: layer.font_family.clone(),
            s: layer.size,
            b: layer.bold,
            k: layer.italic,
            c: layer.color,
            a: layer.align.code().to_string(),
            n: layer.anchor.code().to_string(),
            x: layer.x,
            y: layer.y,
            o: layer.opacity,
            v: layer.visible,
        })
        .collect::<Vec<_>>();
    let json = serde_json::to_string(&wires).unwrap_or_else(|_| "[]".to_string());
    // A raw `|` inside user text would split the recipe's key=value framing;
    // JSON allows it escaped, and serde decodes the escape transparently.
    json.replace('|', "\\u007c")
}

/// Replace only the image overlays and text layers of `destination` with the
/// clipboard's, leaving exposure, contrast, saturation, crop and rotation of
/// the destination untouched.
///
/// Paste Text & Overlays Only **replaces** the destination's layers (it never
/// appends), so pasting the same clipboard twice is idempotent and can never
/// silently accumulate duplicates.
pub fn paste_layers_only(destination: &str, clipboard: &str) -> String {
    let mut merged = EditRecipe::decode(destination);
    let source = EditRecipe::decode(clipboard);
    merged.overlays = source.overlays;
    merged.text_layers = source.text_layers;
    merged.encode()
}

#[derive(Clone, Debug, PartialEq)]
pub struct EditRecipe {
    pub crop: CropRect,
    pub straighten: f32,
    pub exposure: f32,
    pub contrast: f32,
    pub fill_light: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub temperature: f32,
    pub saturation: f32,
    pub auto_contrast: bool,
    pub auto_color: bool,
    pub black_white: bool,
    pub sepia: bool,
    pub filter: super::filters::FilterPreset,
    pub sharpen: f32,
    /// Non-destructive image overlays, painted after crop, rotation and tone.
    /// Paint order follows vector order (later entries stack on top).
    pub overlays: Vec<OverlaySpec>,
    /// Non-destructive editable text layers, painted above the image
    /// overlays. Vector order is the stacking order among text layers.
    pub text_layers: Vec<TextLayerSpec>,
}

impl Default for EditRecipe {
    fn default() -> Self {
        Self {
            crop: CropRect::default(),
            straighten: 0.0,
            exposure: 0.0,
            contrast: 0.0,
            fill_light: 0.0,
            highlights: 0.0,
            shadows: 0.0,
            temperature: 0.0,
            saturation: 0.0,
            auto_contrast: false,
            auto_color: false,
            black_white: false,
            sepia: false,
            filter: super::filters::FilterPreset::None,
            sharpen: 0.0,
            overlays: Vec::new(),
            text_layers: Vec::new(),
        }
    }
}

impl EditRecipe {
    pub fn decode(value: &str) -> Self {
        if value.trim().is_empty() {
            return Self::default();
        }
        let mut recipe = Self::default();
        for part in value.split('|') {
            let Some((key, value)) = part.split_once('=') else {
                continue;
            };
            let number = || value.parse::<f32>().ok();
            let flag = || matches!(value, "1" | "true" | "yes" | "on");
            match key {
                "crop" => {
                    let values = value
                        .split(',')
                        .filter_map(|item| item.parse::<f32>().ok())
                        .collect::<Vec<_>>();
                    if values.len() == 4 {
                        recipe.crop = CropRect {
                            left: values[0],
                            top: values[1],
                            right: values[2],
                            bottom: values[3],
                        }
                        .normalized();
                    }
                }
                "straighten" => recipe.straighten = number().unwrap_or(0.0).clamp(-10.0, 10.0),
                "exposure" => recipe.exposure = number().unwrap_or(0.0).clamp(-2.0, 2.0),
                "contrast" => recipe.contrast = number().unwrap_or(0.0).clamp(-1.0, 1.0),
                "fill" => recipe.fill_light = number().unwrap_or(0.0).clamp(-1.0, 1.0),
                "highlights" => recipe.highlights = number().unwrap_or(0.0).clamp(-1.0, 1.0),
                "shadows" => recipe.shadows = number().unwrap_or(0.0).clamp(-1.0, 1.0),
                "temp" => recipe.temperature = number().unwrap_or(0.0).clamp(-1.0, 1.0),
                "sat" => recipe.saturation = number().unwrap_or(0.0).clamp(-1.0, 1.0),
                "autocontrast" => recipe.auto_contrast = flag(),
                "autocolor" => recipe.auto_color = flag(),
                "bw" => recipe.black_white = flag(),
                "sepia" => recipe.sepia = flag(),
                "filter" => recipe.filter = super::filters::FilterPreset::decode(value),
                "sharpen" => recipe.sharpen = number().unwrap_or(0.0).clamp(0.0, 1.0),
                "ov" => recipe.overlays = decode_overlays(value),
                "tl" => recipe.text_layers = decode_text_layers(value),
                _ => {}
            }
        }
        recipe
    }

    pub fn encode(&self) -> String {
        if self.is_default() {
            return String::new();
        }
        let mut encoded = format!(
            "v=1|crop={:.5},{:.5},{:.5},{:.5}|straighten={:.4}|exposure={:.4}|contrast={:.4}|fill={:.4}|highlights={:.4}|shadows={:.4}|temp={:.4}|sat={:.4}|autocontrast={}|autocolor={}|bw={}|sepia={}|filter={}|sharpen={:.4}",
            self.crop.left,
            self.crop.top,
            self.crop.right,
            self.crop.bottom,
            self.straighten,
            self.exposure,
            self.contrast,
            self.fill_light,
            self.highlights,
            self.shadows,
            self.temperature,
            self.saturation,
            self.auto_contrast as u8,
            self.auto_color as u8,
            self.black_white as u8,
            self.sepia as u8,
            self.filter.key(),
            self.sharpen,
        );
        if !self.overlays.is_empty() {
            encoded.push_str("|ov=");
            encoded.push_str(&encode_overlays(&self.overlays));
        }
        if !self.text_layers.is_empty() {
            encoded.push_str("|tl=");
            encoded.push_str(&encode_text_layers(&self.text_layers));
        }
        encoded
    }

    pub fn is_default(&self) -> bool {
        self.crop.is_full()
            && self.straighten.abs() < 0.0001
            && self.exposure.abs() < 0.0001
            && self.contrast.abs() < 0.0001
            && self.fill_light.abs() < 0.0001
            && self.highlights.abs() < 0.0001
            && self.shadows.abs() < 0.0001
            && self.temperature.abs() < 0.0001
            && self.saturation.abs() < 0.0001
            && !self.auto_contrast
            && !self.auto_color
            && !self.black_white
            && !self.sepia
            && self.filter == super::filters::FilterPreset::None
            && self.sharpen.abs() < 0.0001
            && self.overlays.is_empty()
            && self.text_layers.is_empty()
    }
}

#[derive(Default)]
pub struct EditSession {
    pub recipe: EditRecipe,
    undo: Vec<EditRecipe>,
    redo: Vec<EditRecipe>,
    active_action: Option<EditRecipe>,
}

impl EditSession {
    pub fn new(recipe: EditRecipe) -> Self {
        Self {
            recipe,
            undo: Vec::new(),
            redo: Vec::new(),
            active_action: None,
        }
    }

    pub fn replace(&mut self, next: EditRecipe) {
        self.end_action();
        if self.recipe == next {
            return;
        }
        self.undo.push(self.recipe.clone());
        if self.undo.len() > 100 {
            self.undo.remove(0);
        }
        self.recipe = next;
        self.redo.clear();
    }

    pub fn mutate(&mut self, update: impl FnOnce(&mut EditRecipe)) {
        let mut next = self.recipe.clone();
        update(&mut next);
        self.replace(next);
    }

    /// Begin one continuous UI action, such as dragging a slider. Intermediate
    /// values update the preview immediately, but only the recipe that existed
    /// before the drag is added to Undo when the action finishes. Any pending
    /// continuous action is closed first so every action stays its own step.
    pub fn begin_action(&mut self) {
        self.end_action();
        self.active_action = Some(self.recipe.clone());
    }

    pub fn mutate_active(&mut self, update: impl FnOnce(&mut EditRecipe)) {
        update(&mut self.recipe);
        self.redo.clear();
    }

    /// True while a continuous UI action (a slider drag) is in progress.
    pub fn action_active(&self) -> bool {
        self.active_action.is_some()
    }

    pub fn end_action(&mut self) {
        let Some(previous) = self.active_action.take() else {
            return;
        };
        if previous == self.recipe {
            return;
        }
        self.undo.push(previous);
        if self.undo.len() > 100 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    pub fn undo(&mut self) -> bool {
        self.end_action();
        let Some(previous) = self.undo.pop() else {
            return false;
        };
        self.redo.push(self.recipe.clone());
        self.recipe = previous;
        true
    }

    pub fn redo(&mut self) -> bool {
        self.end_action();
        let Some(next) = self.redo.pop() else {
            return false;
        };
        self.undo.push(self.recipe.clone());
        self.recipe = next;
        true
    }

    pub fn reset(&mut self) {
        self.replace(EditRecipe::default());
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::super::filters::FilterPreset;
    use super::*;

    #[test]
    fn recipe_round_trip() {
        let mut recipe = EditRecipe::default();
        recipe.crop = CropRect {
            left: 0.1,
            top: 0.2,
            right: 0.8,
            bottom: 0.9,
        };
        recipe.exposure = 0.7;
        recipe.contrast = 0.45;
        recipe.auto_color = true;
        recipe.sepia = true;
        recipe.filter = FilterPreset::Valencia;
        recipe.sharpen = 0.4;
        let decoded = EditRecipe::decode(&recipe.encode());
        assert!((decoded.crop.left - 0.1).abs() < 0.001);
        assert!((decoded.exposure - 0.7).abs() < 0.001);
        assert!((decoded.contrast - 0.45).abs() < 0.001);
        assert!(decoded.auto_color);
        assert!(decoded.sepia);
        assert_eq!(decoded.filter, FilterPreset::Valencia);
        assert!((decoded.sharpen - 0.4).abs() < 0.001);
    }

    #[test]
    fn filter_is_persisted_and_makes_recipe_non_default() {
        let mut recipe = EditRecipe::default();
        assert!(recipe.is_default());
        recipe.filter = FilterPreset::Clarendon;
        assert!(!recipe.is_default());
        assert_eq!(
            EditRecipe::decode(&recipe.encode()).filter,
            FilterPreset::Clarendon
        );
    }

    #[test]
    fn legacy_bw_and_sepia_recipes_still_decode_without_a_named_filter() {
        let bw = EditRecipe::decode("v=1|bw=1");
        let sepia = EditRecipe::decode("v=1|sepia=1");
        assert_eq!(bw.filter, FilterPreset::None);
        assert_eq!(sepia.filter, FilterPreset::None);
        assert!(bw.black_white);
        assert!(sepia.sepia);
    }

    #[test]
    fn old_recipe_without_contrast_defaults_to_zero() {
        let decoded = EditRecipe::decode("v=1|exposure=0.2500|sat=0.1000|bw=1");
        assert!((decoded.exposure - 0.25).abs() < 0.001);
        assert!(decoded.contrast.abs() < 0.001);
        assert!((decoded.saturation - 0.1).abs() < 0.001);
        assert!(decoded.black_white);
    }

    #[test]
    fn session_keeps_bw_and_sepia_independently() {
        let mut session = EditSession::new(EditRecipe::default());
        session.mutate(|recipe| recipe.black_white = true);
        session.mutate(|recipe| recipe.sepia = true);
        assert!(session.recipe.black_white);
        assert!(session.recipe.sepia);
        let decoded = EditRecipe::decode(&session.recipe.encode());
        assert!(decoded.black_white);
        assert!(decoded.sepia);
    }

    #[test]
    fn composed_crop_stays_normalized() {
        let outer = CropRect {
            left: 0.1,
            top: 0.1,
            right: 0.9,
            bottom: 0.9,
        };
        let inner = CropRect {
            left: 0.25,
            top: 0.25,
            right: 0.75,
            bottom: 0.75,
        };
        let result = outer.compose(inner);
        assert!((result.left - 0.3).abs() < 0.001);
        assert!((result.right - 0.7).abs() < 0.001);
    }

    #[test]
    fn continuous_slider_drag_is_one_undo_action() {
        let mut session = EditSession::new(EditRecipe::default());
        session.begin_action();
        session.mutate_active(|recipe| recipe.exposure = 0.2);
        session.mutate_active(|recipe| recipe.exposure = 0.7);
        session.mutate_active(|recipe| recipe.exposure = 1.1);
        session.end_action();

        assert!((session.recipe.exposure - 1.1).abs() < 0.001);
        assert!(session.undo());
        assert!(session.recipe.exposure.abs() < 0.001);
        assert!(!session.undo());
        assert!(session.redo());
        assert!((session.recipe.exposure - 1.1).abs() < 0.001);
    }

    #[test]
    fn removing_filter_preserves_tool_adjustments() {
        let mut recipe = EditRecipe::default();
        recipe.filter = FilterPreset::Clarendon;
        recipe.exposure = 0.65;
        recipe.saturation = 0.25;
        let mut session = EditSession::new(recipe);

        session.mutate(|recipe| recipe.filter = FilterPreset::None);

        assert_eq!(session.recipe.filter, FilterPreset::None);
        assert!((session.recipe.exposure - 0.65).abs() < 0.001);
        assert!((session.recipe.saturation - 0.25).abs() < 0.001);
    }

    #[test]
    fn reset_clears_filter_and_tool_adjustments() {
        let mut recipe = EditRecipe::default();
        recipe.filter = FilterPreset::Clarendon;
        recipe.exposure = 0.65;
        recipe.saturation = 0.25;
        let mut session = EditSession::new(recipe);

        session.reset();

        assert_eq!(session.recipe, EditRecipe::default());
    }

    #[test]
    fn overlay_recipe_round_trips_every_field() {
        let mut recipe = EditRecipe::default();
        recipe.exposure = 0.25;
        recipe.overlays.push(OverlaySpec {
            asset: "deadbeefcafe0123456789abcdef".to_string(),
            anchor: OverlayAnchor::BottomRight,
            x: 0.93,
            y: 0.95,
            width: 0.2,
            opacity: 0.65,
            rotation: 0.0,
            visible: true,
        });
        recipe.overlays.push(OverlaySpec {
            asset: "0123456789abcdefdeadbeefcafe".to_string(),
            anchor: OverlayAnchor::TopLeft,
            x: 0.02,
            y: 0.03,
            width: 0.4,
            opacity: 1.0,
            rotation: 0.0,
            visible: false,
        });

        let decoded = EditRecipe::decode(&recipe.encode());

        assert!((decoded.exposure - 0.25).abs() < 0.001);
        assert_eq!(decoded.overlays.len(), 2);
        assert_eq!(decoded.overlays[0].asset, recipe.overlays[0].asset);
        assert_eq!(decoded.overlays[0].anchor, OverlayAnchor::BottomRight);
        assert!((decoded.overlays[0].x - 0.93).abs() < 0.001);
        assert!((decoded.overlays[0].y - 0.95).abs() < 0.001);
        assert!((decoded.overlays[0].width - 0.2).abs() < 0.001);
        assert!((decoded.overlays[0].opacity - 0.65).abs() < 0.001);
        assert!(decoded.overlays[0].visible);
        assert_eq!(decoded.overlays[1].anchor, OverlayAnchor::TopLeft);
        assert!(!decoded.overlays[1].visible);
        assert_eq!(decoded, recipe);
    }

    #[test]
    fn legacy_recipes_decode_with_no_overlays() {
        let decoded = EditRecipe::decode("v=1|exposure=0.2500|sat=0.1000");
        assert!(decoded.overlays.is_empty());
        assert!(EditRecipe::default().is_default());
    }

    #[test]
    fn overlay_makes_a_recipe_non_default_and_encode_is_non_empty() {
        let mut recipe = EditRecipe::default();
        assert!(recipe.is_default());
        assert_eq!(recipe.encode(), "");

        recipe.overlays.push(OverlaySpec::new_centered("aabbcc"));

        assert!(!recipe.is_default());
        let encoded = recipe.encode();
        assert!(encoded.contains("|ov="));
        assert_eq!(EditRecipe::decode(&encoded).overlays.len(), 1);
    }

    #[test]
    fn malformed_overlay_json_is_dropped_without_losing_other_edits() {
        let decoded = EditRecipe::decode("v=1|exposure=0.5000|ov=not-json");
        assert!((decoded.exposure - 0.5).abs() < 0.001);
        assert!(decoded.overlays.is_empty());
    }

    #[test]
    fn overlay_stays_in_the_bottom_right_corner_across_photo_orientations() {
        // Brand a 6000x4000 landscape, then paste onto a 4000x6000 portrait.
        let mut overlay = OverlaySpec::new_centered("logo");
        overlay.anchor = OverlayAnchor::BottomRight;
        let source_rect = {
            let mut candidate = overlay.clone();
            candidate.width = 0.2;
            candidate.x = 0.95;
            candidate.y = 0.95;
            candidate.rect(6000.0, 4000.0, 2.0)
        };

        overlay.x = 0.95;
        overlay.y = 0.95;
        overlay.width = 0.2;
        let landscape = overlay.rect(6000.0, 4000.0, 2.0);
        let portrait = overlay.rect(4000.0, 6000.0, 2.0);

        // Same relative width on both photos…
        assert!((landscape.width - 0.2).abs() < 1e-6);
        assert!((portrait.width - 0.2).abs() < 1e-6);
        // …and the pixel aspect ratio of the overlay is preserved.
        let landscape_pixel_aspect = (landscape.width * 6000.0) / (landscape.height * 4000.0);
        let portrait_pixel_aspect = (portrait.width * 4000.0) / (portrait.height * 6000.0);
        assert!((landscape_pixel_aspect - 2.0).abs() < 1e-4);
        assert!((portrait_pixel_aspect - 2.0).abs() < 1e-4);
        // The bottom-right corner of the overlay stays in the bottom-right.
        assert!((landscape.right() - 0.95).abs() < 1e-6);
        assert!((landscape.bottom() - 0.95).abs() < 1e-6);
        assert!((portrait.right() - 0.95).abs() < 1e-6);
        assert!((portrait.bottom() - 0.95).abs() < 1e-6);
        assert_eq!(overlay.rect(6000.0, 4000.0, 2.0), source_rect);
    }

    #[test]
    fn center_anchor_places_the_overlay_in_the_middle() {
        let overlay = OverlaySpec::new_centered("badge");
        let rect = overlay.rect(800.0, 600.0, 1.0);
        assert!((rect.left + rect.width * 0.5 - 0.5).abs() < 1e-6);
        assert!((rect.top + rect.height * 0.5 - 0.5).abs() < 1e-6);
        // A square asset on a 4:3 photo is wider than tall in normalized
        // units, but square in pixels.
        let pixel_aspect = (rect.width * 800.0) / (rect.height * 600.0);
        assert!((pixel_aspect - 1.0).abs() < 1e-4);
    }

    #[test]
    fn every_anchor_round_trips_through_rect_and_set_rect() {
        for anchor in OverlayAnchor::ALL {
            let mut overlay = OverlaySpec::new_centered("asset");
            overlay.anchor = anchor;
            overlay.x = 0.4;
            overlay.y = 0.6;
            overlay.width = 0.3;
            let rect = overlay.rect(1000.0, 500.0, 1.5);
            let restored = {
                let mut copy = overlay.clone();
                copy.set_rect(rect, 1000.0, 500.0, 1.5);
                copy.rect(1000.0, 500.0, 1.5)
            };
            assert!(
                (restored.left - rect.left).abs() < 1e-5,
                "{anchor:?} left drift"
            );
            assert!(
                (restored.top - rect.top).abs() < 1e-5,
                "{anchor:?} top drift"
            );
            assert!((restored.width - rect.width).abs() < 1e-5);
            assert!((restored.height - rect.height).abs() < 1e-5);
        }
    }

    #[test]
    fn position_at_pins_corners_with_a_uniform_margin() {
        let mut overlay = OverlaySpec::new_centered("logo");
        overlay.width = 0.25;
        overlay.opacity = 0.6;
        overlay.position_at(OverlayAnchor::BottomRight);
        assert_eq!(overlay.anchor, OverlayAnchor::BottomRight);
        let rect = overlay.rect(1600.0, 900.0, 1.0);
        assert!((rect.right() - 0.97).abs() < 1e-6);
        assert!((rect.bottom() - 0.97).abs() < 1e-6);
        assert!((rect.width - 0.25).abs() < 1e-6);
        assert!((overlay.opacity - 0.6).abs() < 1e-6);

        overlay.position_at(OverlayAnchor::TopLeft);
        let rect = overlay.rect(1600.0, 900.0, 1.0);
        assert!((rect.left - 0.03).abs() < 1e-6);
        assert!((rect.top - 0.03).abs() < 1e-6);
        assert!((rect.width - 0.25).abs() < 1e-6);

        overlay.position_at(OverlayAnchor::Center);
        let rect = overlay.rect(1600.0, 900.0, 1.0);
        assert!((rect.left + rect.width * 0.5 - 0.5).abs() < 1e-6);
        assert!((rect.top + rect.height * 0.5 - 0.5).abs() < 1e-6);
        assert!((rect.width - 0.25).abs() < 1e-6);
        assert!((overlay.opacity - 0.6).abs() < 1e-6);
    }

    #[test]
    fn reanchoring_keeps_the_overlay_where_it_is() {
        let mut overlay = OverlaySpec::new_centered("logo");
        overlay.anchor = OverlayAnchor::BottomRight;
        overlay.x = 0.9;
        overlay.y = 0.9;
        overlay.width = 0.25;
        let before = overlay.rect(1600.0, 900.0, 1.0);

        overlay.reanchor(OverlayAnchor::Center, 1600.0, 900.0, 1.0);

        assert_eq!(overlay.anchor, OverlayAnchor::Center);
        let after = overlay.rect(1600.0, 900.0, 1.0);
        assert!((after.left - before.left).abs() < 1e-5);
        assert!((after.top - before.top).abs() < 1e-5);
        assert!((after.width - before.width).abs() < 1e-6);
    }

    #[test]
    fn clamp_inside_translates_without_changing_size() {
        let rect = NormRect {
            left: 0.9,
            top: -0.2,
            width: 0.2,
            height: 0.4,
        }
        .clamp_inside();
        assert!((rect.left - 0.8).abs() < 1e-6);
        assert!((rect.top - 0.0).abs() < 1e-6);
        assert!((rect.width - 0.2).abs() < 1e-6);
        assert!((rect.height - 0.4).abs() < 1e-6);
    }

    #[test]
    fn paste_layers_only_replaces_overlays_and_preserves_tone() {
        let source = "v=1|exposure=1.0000|ov=[{\"a\":\"hash1\",\"n\":\"br\",\"x\":0.9,\"y\":0.9,\"w\":0.2,\"o\":1,\"r\":0,\"v\":true}]";
        let destination = "v=1|exposure=-0.5000|sat=0.3000|crop=0.10000,0.10000,0.90000,0.90000|ov=[{\"a\":\"old\",\"n\":\"c\",\"x\":0.5,\"y\":0.5,\"w\":0.5,\"o\":1,\"r\":0,\"v\":true}]";

        let merged = EditRecipe::decode(&paste_layers_only(destination, source));

        // Destination tone and crop untouched…
        assert!((merged.exposure + 0.5).abs() < 0.001);
        assert!((merged.saturation - 0.3).abs() < 0.001);
        assert!((merged.crop.left - 0.1).abs() < 0.001);
        // …overlays replaced by the clipboard's, never appended.
        assert_eq!(merged.overlays.len(), 1);
        assert_eq!(merged.overlays[0].asset, "hash1");
        assert_eq!(merged.overlays[0].anchor, OverlayAnchor::BottomRight);
    }

    #[test]
    fn paste_layers_only_is_idempotent() {
        let source = "v=1|ov=[{\"a\":\"hash1\",\"n\":\"br\",\"x\":0.9,\"y\":0.9,\"w\":0.2,\"o\":1,\"r\":0,\"v\":true}]";
        let destination = "v=1|exposure=0.2500";

        let once = paste_layers_only(destination, source);
        let twice = paste_layers_only(&once, source);

        assert_eq!(once, twice);
        assert_eq!(EditRecipe::decode(&twice).overlays.len(), 1);
    }

    #[test]
    fn paste_layers_only_replaces_text_layers_without_accumulating() {
        let mut clipboard = EditRecipe::default();
        clipboard.text_layers.push(TextLayerSpec::new_default());
        let mut destination = EditRecipe::default();
        destination.exposure = 0.4;
        let destination = destination.encode();
        let clipboard = clipboard.encode();

        let once = paste_layers_only(&destination, &clipboard);
        let twice = paste_layers_only(&once, &clipboard);

        assert_eq!(EditRecipe::decode(&once).text_layers.len(), 1);
        assert_eq!(EditRecipe::decode(&twice).text_layers.len(), 1);
        assert!((EditRecipe::decode(&twice).exposure - 0.4).abs() < 0.001);
        assert_eq!(once, twice);
    }

    #[test]
    fn pasted_text_and_overlays_land_in_the_same_relative_place_on_a_differently_shaped_destination() {
        // Brand a landscape photo at the bottom-right with the 3% margin,
        // paste onto a portrait destination, and require the same relative
        // placement with the pixel aspect of both layer kinds preserved.
        let mut clipboard = EditRecipe::default();
        let mut overlay = OverlaySpec::new_centered("logo");
        overlay.anchor = OverlayAnchor::BottomRight;
        overlay.x = 1.0 - OverlaySpec::POSITION_MARGIN;
        overlay.y = 1.0 - OverlaySpec::POSITION_MARGIN;
        overlay.width = 0.2;
        clipboard.overlays.push(overlay);
        let mut text = TextLayerSpec::new_default();
        text.anchor = OverlayAnchor::BottomRight;
        text.x = 1.0 - TextLayerSpec::POSITION_MARGIN;
        text.y = 1.0 - TextLayerSpec::POSITION_MARGIN;
        text.size = 0.05;
        clipboard.text_layers.push(text);

        let destination = EditRecipe {
            exposure: 0.3,
            ..EditRecipe::default()
        }
        .encode();
        let pasted = EditRecipe::decode(&paste_layers_only(&destination, &clipboard.encode()));

        // Destination tone survives; layers are replaced, not accumulated.
        assert!((pasted.exposure - 0.3).abs() < 0.001);
        assert_eq!(pasted.overlays.len(), 1);
        assert_eq!(pasted.text_layers.len(), 1);

        let overlay = &pasted.overlays[0];
        let text = &pasted.text_layers[0];
        let landscape = overlay.rect(6000.0, 4000.0, 2.0);
        let portrait = overlay.rect(4000.0, 6000.0, 2.0);
        assert!((landscape.right() - portrait.right()).abs() < 1e-6);
        assert!((landscape.bottom() - portrait.bottom()).abs() < 1e-6);
        assert!((landscape.right() - (1.0 - OverlaySpec::POSITION_MARGIN)).abs() < 1e-6);
        let landscape_aspect = (landscape.width * 6000.0) / (landscape.height * 4000.0);
        let portrait_aspect = (portrait.width * 4000.0) / (portrait.height * 6000.0);
        assert!((landscape_aspect - 2.0).abs() < 1e-4);
        assert!((portrait_aspect - 2.0).abs() < 1e-4);

        // Text size is a height fraction, so its normalized box is
        // identical across destinations; its bottom-right corner keeps
        // the same relative margin.
        let landscape_text = text.rect_with_size(0.2, text.size);
        let portrait_text = text.rect_with_size(0.2, text.size);
        assert!((landscape_text.right() - portrait_text.right()).abs() < 1e-6);
        assert!((landscape_text.bottom() - portrait_text.bottom()).abs() < 1e-6);
        assert!(
            (landscape_text.right() - (1.0 - TextLayerSpec::POSITION_MARGIN)).abs() < 1e-6
        );
        assert!(
            (landscape_text.bottom() - (1.0 - TextLayerSpec::POSITION_MARGIN)).abs() < 1e-6
        );
    }

    #[test]
    fn paste_all_edits_replaces_the_whole_recipe_including_overlays() {
        let source = EditRecipe {
            exposure: 0.75,
            overlays: vec![OverlaySpec::new_centered("brand")],
            ..EditRecipe::default()
        };
        let destination = EditRecipe {
            sepia: true,
            overlays: vec![OverlaySpec::new_centered("other")],
            ..EditRecipe::default()
        };

        let pasted = EditRecipe::decode(&source.encode());

        assert!((pasted.exposure - 0.75).abs() < 0.001);
        assert!(!pasted.sepia);
        assert_eq!(pasted.overlays.len(), 1);
        assert_eq!(pasted.overlays[0].asset, "brand");
        let _ = destination;
    }

    #[test]
    fn one_canvas_gesture_is_a_single_undo_step() {
        let mut session = EditSession::new(EditRecipe::default());
        session.mutate(|recipe| recipe.overlays.push(OverlaySpec::new_centered("logo")));
        let after_add = session.recipe.clone();

        // A drag reports many intermediate positions but commits once.
        session.begin_action();
        for step in 1..=10 {
            session.mutate_active(|recipe| {
                let overlay = recipe.overlays.first_mut().expect("overlay");
                overlay.x = 0.5 + step as f32 * 0.01;
            });
        }
        session.end_action();

        // First undo reverts only the drag, not the add that preceded it.
        assert!(session.undo());
        assert_eq!(session.recipe, after_add);
        // The drag itself was a single step: undoing again reaches the add.
        assert!(session.undo());
        assert!(session.recipe.overlays.is_empty());
        assert!(!session.undo(), "add and drag are two distinct steps");
        // Redo walks back: add, then the full drag at its final position.
        assert!(session.redo());
        assert!(session.redo());
        assert!((session.recipe.overlays[0].x - 0.6).abs() < 0.001);
    }

    #[test]
    fn reset_all_edits_removes_overlays() {
        let mut session = EditSession::new(EditRecipe::default());
        session.mutate(|recipe| recipe.overlays.push(OverlaySpec::new_centered("logo")));

        session.reset();

        assert!(session.recipe.overlays.is_empty());
        assert!(session.recipe.is_default());
    }

    #[test]
    fn max_fitting_width_keeps_tall_overlays_inside_portrait_photos() {
        // A square asset (aspect 1) on a 4000x800 portrait: fitting vertically
        // allows at most a fifth of the width.
        let max = OverlaySpec::max_fitting_width(4000.0, 800.0, 1.0);
        assert!((max - 0.2).abs() < 1e-5);
        // A wide logo (aspect 2) on a 4:3 landscape: height still binds, but
        // less tightly than for a square.
        let max = OverlaySpec::max_fitting_width(8000.0, 6000.0, 2.0);
        assert!((max - 1.0).abs() < 1e-5, "width bound applies: {max}");
        // A very wide logo on a square photo is limited by photo width.
        let max = OverlaySpec::max_fitting_width(4000.0, 4000.0, 4.0);
        assert!((max - 1.0).abs() < 1e-5, "width bound applies: {max}");
    }

    #[test]
    fn text_layer_recipe_round_trips_every_field() {
        let mut recipe = EditRecipe::default();
        recipe.exposure = 0.25;
        let mut layer = TextLayerSpec::new_default();
        layer.text = "Line one\nLine two".to_string();
        layer.font_family = "DejaVu Sans".to_string();
        layer.size = 0.07;
        layer.bold = true;
        layer.italic = true;
        layer.set_color_rgba(0.2, 0.4, 0.6, 0.8);
        layer.align = TextAlign::Right;
        layer.anchor = OverlayAnchor::BottomRight;
        layer.x = 0.97;
        layer.y = 0.97;
        layer.opacity = 0.55;
        layer.visible = false;
        recipe.text_layers.push(layer.clone());

        let decoded = EditRecipe::decode(&recipe.encode());

        assert!((decoded.exposure - 0.25).abs() < 0.001);
        assert_eq!(decoded.text_layers.len(), 1);
        assert_eq!(decoded, recipe);
        let restored = &decoded.text_layers[0];
        assert_eq!(restored.id, layer.id);
        assert_eq!(restored.text, "Line one\nLine two");
        assert_eq!(restored.font_family, "DejaVu Sans");
        assert!((restored.size - 0.07).abs() < 1e-6);
        assert!(restored.bold);
        assert!(restored.italic);
        assert_eq!(restored.align, TextAlign::Right);
        assert_eq!(restored.anchor, OverlayAnchor::BottomRight);
        assert!((restored.opacity - 0.55).abs() < 1e-6);
        assert!(!restored.visible);
        let (r, g, b, a) = restored.color_rgba();
        assert!((r - 0.2).abs() < 0.01);
        assert!((g - 0.4).abs() < 0.01);
        assert!((b - 0.6).abs() < 0.01);
        assert!((a - 0.8).abs() < 0.01);
    }

    #[test]
    fn multiline_text_with_recipe_metacharacters_survives_round_trip() {
        let mut recipe = EditRecipe::default();
        let mut layer = TextLayerSpec::new_default();
        layer.text = "a|b=c\nd=e|f\n\"quoted\" \\ backslash".to_string();
        recipe.text_layers.push(layer);

        let encoded = recipe.encode();
        assert!(!encoded.is_empty());
        let decoded = EditRecipe::decode(&encoded);

        assert_eq!(decoded.text_layers.len(), 1);
        assert_eq!(
            decoded.text_layers[0].text,
            "a|b=c\nd=e|f\n\"quoted\" \\ backslash"
        );
        assert_eq!(decoded, recipe);
    }

    #[test]
    fn old_recipes_without_text_layers_decode_to_an_empty_list() {
        let old_overlay_recipe =
            EditRecipe::decode("v=1|exposure=0.2500|ov=[{\"a\":\"h\",\"n\":\"c\",\"x\":0.5,\"y\":0.5,\"w\":0.2,\"o\":1,\"r\":0,\"v\":true}]");
        assert!(old_overlay_recipe.text_layers.is_empty());
        assert_eq!(old_overlay_recipe.overlays.len(), 1);

        let legacy = EditRecipe::decode("v=1|bw=1");
        assert!(legacy.text_layers.is_empty());
        assert!(!legacy.is_default());
    }

    #[test]
    fn text_layers_make_a_recipe_non_default_and_encode_is_non_empty() {
        let mut recipe = EditRecipe::default();
        assert!(recipe.is_default());
        assert_eq!(recipe.encode(), "");
        recipe.text_layers.push(TextLayerSpec::new_default());
        assert!(!recipe.is_default());
        assert!(!recipe.encode().is_empty());
        assert!(EditRecipe::decode(&recipe.encode()).text_layers.len() == 1);
    }

    #[test]
    fn text_position_at_uses_a_uniform_three_percent_margin() {
        let mut layer = TextLayerSpec::new_default();

        layer.position_at(OverlayAnchor::TopLeft);
        assert!((layer.x - 0.03).abs() < 1e-6);
        assert!((layer.y - 0.03).abs() < 1e-6);

        layer.position_at(OverlayAnchor::BottomRight);
        assert!((layer.x - 0.97).abs() < 1e-6);
        assert!((layer.y - 0.97).abs() < 1e-6);

        layer.position_at(OverlayAnchor::Center);
        assert!((layer.x - 0.5).abs() < 1e-6);
        assert!((layer.y - 0.5).abs() < 1e-6);
        assert_eq!(layer.text, "Your text");
        assert!((layer.size - TextLayerSpec::DEFAULT_SIZE).abs() < 1e-6);
    }

    #[test]
    fn text_rect_mapping_matches_the_shared_anchor_model() {
        let mut layer = TextLayerSpec::new_default();
        layer.anchor = OverlayAnchor::TopRight;
        layer.x = 0.9;
        layer.y = 0.2;
        let rect = layer.rect_with_size(0.3, 0.1);
        assert!((rect.right() - 0.9).abs() < 1e-6);
        assert!((rect.top - 0.2).abs() < 1e-6);

        layer.set_position_from_rect(rect);
        assert!((layer.x - 0.9).abs() < 1e-6);
        assert!((layer.y - 0.2).abs() < 1e-6);
    }

    #[test]
    fn text_editing_flow_is_one_undo_step_per_operation() {
        let mut session = EditSession::new(EditRecipe::default());

        session.mutate(|recipe| recipe.text_layers.push(TextLayerSpec::new_default()));
        assert_eq!(session.recipe.text_layers.len(), 1);
        let after_add = session.recipe.clone();

        session.begin_action();
        session.mutate_active(|recipe| recipe.text_layers[0].text = "Y".to_string());
        session.mutate_active(|recipe| recipe.text_layers[0].text = "Your".to_string());
        session.mutate_active(|recipe| recipe.text_layers[0].text = "Your words".to_string());
        session.end_action();

        session.mutate(|recipe| {
            recipe.text_layers[0].font_family = "Serif".to_string();
        });

        assert_eq!(session.recipe.text_layers[0].font_family, "Serif");
        assert!(session.undo());
        assert_eq!(session.recipe.text_layers[0].text, "Your words");
        assert_eq!(session.recipe.text_layers[0].font_family, "Sans");
        assert!(session.undo());
        assert_eq!(session.recipe, after_add);
        assert!(session.undo());
        assert!(session.recipe.text_layers.is_empty());
        assert!(!session.undo());

        assert!(session.redo());
        assert!(session.redo());
        assert!(session.redo());
        assert_eq!(session.recipe.text_layers[0].font_family, "Serif");
    }

    #[test]
    fn pending_text_action_flushes_before_a_new_discrete_mutation() {
        let mut session = EditSession::new(EditRecipe::default());
        session.mutate(|recipe| recipe.text_layers.push(TextLayerSpec::new_default()));

        session.begin_action();
        session.mutate_active(|recipe| recipe.text_layers[0].text = "typed".to_string());
        session.mutate(|recipe| recipe.text_layers[0].bold = true);

        assert!(session.recipe.text_layers[0].bold);
        assert_eq!(session.recipe.text_layers[0].text, "typed");
        assert!(session.undo(), "bold toggle is its own step");
        assert!(!session.recipe.text_layers[0].bold);
        assert_eq!(session.recipe.text_layers[0].text, "typed");
        assert!(session.undo());
        assert_eq!(session.recipe.text_layers[0].text, "Your text");
        assert!(session.undo());
        assert!(session.recipe.text_layers.is_empty());
    }

    #[test]
    fn reset_all_edits_removes_text_layers() {
        let mut session = EditSession::new(EditRecipe::default());
        session.mutate(|recipe| recipe.text_layers.push(TextLayerSpec::new_default()));
        session.reset();
        assert!(session.recipe.text_layers.is_empty());
        assert_eq!(session.recipe, EditRecipe::default());
    }

    #[test]
    fn fit_to_width_spans_the_photo_and_keeps_vertical_centre() {
        let mut overlay = OverlaySpec::new_centered("banner");
        overlay.width = 0.2;
        overlay.x = 0.5;
        overlay.y = 0.75;
        overlay.fit_to_width(1600.0, 900.0, 4.0);
        let rect = overlay.rect(1600.0, 900.0, 4.0);
        assert!(rect.left.abs() < 1e-5);
        assert!((rect.right() - 1.0).abs() < 1e-5);
        let centre_y = rect.top + rect.height * 0.5;
        assert!((centre_y - 0.75).abs() < 1e-5);
    }

    #[test]
    fn fit_to_screen_contains_the_overlay_and_centres_it() {
        let mut overlay = OverlaySpec::new_centered("badge");
        overlay.width = 0.9;
        overlay.x = 0.2;
        overlay.y = 0.2;
        overlay.fit_to_screen(1600.0, 900.0, 0.5);
        let rect = overlay.rect(1600.0, 900.0, 0.5);
        assert!((rect.width - 0.28125).abs() < 1e-5);
        assert!((rect.height - 1.0).abs() < 1e-5);
        assert!((rect.left - (1.0 - 0.28125) * 0.5).abs() < 1e-5);
        assert!(rect.top.abs() < 1e-5);
    }
}
