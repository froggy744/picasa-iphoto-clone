use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::photo_object::PhotoObject;

#[derive(Clone, Debug)]
pub struct CollagePhoto {
    pub id: i64,
    pub folder_id: Option<i64>,
    pub path: String,
    pub filename: String,
    pub thumbnail_path: Option<String>,
    pub library_rotation: i32,
    pub edit_recipe: String,
    pub aspect_ratio: f32,
    /// On-device visual-interest estimate derived from the cached thumbnail.
    /// Smart layout uses this to give the clearest, most visually distinctive
    /// selections the strongest positions instead of trusting selection order.
    pub visual_weight: f32,
}

fn photo_aspect_ratio(photo: &PhotoObject) -> f32 {
    let mut width = photo.width().max(1) as u32;
    let mut height = photo.height().max(1) as u32;
    if photo.rotation().rem_euclid(180) == 90 {
        std::mem::swap(&mut width, &mut height);
    }
    let recipe = crate::edit::EditRecipe::decode(&photo.edit_recipe());
    let (width, height) = crate::edit::render::estimated_output_dimensions(width, height, &recipe);
    let ratio = width as f32 / height.max(1) as f32;
    if ratio.is_finite() && ratio > 0.0 {
        ratio
    } else {
        1.5
    }
}

/// Small, deterministic on-device vision model for collage composition.
///
/// A collage should not promote an arbitrary first-selected image.  The model
/// samples the already-generated thumbnail (never the full original) and
/// combines usable exposure, contrast, edge detail, colourfulness and centre
/// detail into a visual-interest score.  It deliberately has a neutral
/// fallback: unavailable/offline thumbnails remain valid collage candidates.
fn visual_interest(photo: &PhotoObject) -> f32 {
    let Some(path) = photo.cached_thumbnail_path() else {
        return 0.5;
    };
    let Ok(image) = image::open(path) else {
        return 0.5;
    };
    let image = image.to_rgb8();
    let (width, height) = image.dimensions();
    if width < 2 || height < 2 {
        return 0.5;
    }
    // Bound work for large cache entries while retaining a representative
    // image sample. Thumbnails are normally 320px, so this is inexpensive.
    let step_x = (width / 96).max(1);
    let step_y = (height / 96).max(1);
    let mut count = 0.0f32;
    let mut sum_luma = 0.0;
    let mut sum_luma_squared = 0.0;
    let mut saturation = 0.0;
    let mut detail = 0.0;
    let mut centre_detail = 0.0;
    let mut centre_count = 0.0f32;
    for y in (step_y..height).step_by(step_y as usize) {
        for x in (step_x..width).step_by(step_x as usize) {
            let pixel = image.get_pixel(x, y).0;
            let luma =
                (0.2126 * pixel[0] as f32 + 0.7152 * pixel[1] as f32 + 0.0722 * pixel[2] as f32)
                    / 255.0;
            let maximum = pixel[0].max(pixel[1]).max(pixel[2]) as f32;
            let minimum = pixel[0].min(pixel[1]).min(pixel[2]) as f32;
            let left = image.get_pixel(x - step_x, y).0;
            let above = image.get_pixel(x, y - step_y).0;
            let left_luma =
                (0.2126 * left[0] as f32 + 0.7152 * left[1] as f32 + 0.0722 * left[2] as f32)
                    / 255.0;
            let above_luma =
                (0.2126 * above[0] as f32 + 0.7152 * above[1] as f32 + 0.0722 * above[2] as f32)
                    / 255.0;
            let edge = (luma - left_luma).abs() + (luma - above_luma).abs();
            count += 1.0;
            sum_luma += luma;
            sum_luma_squared += luma * luma;
            saturation += (maximum - minimum) / 255.0;
            detail += edge;
            // Detail around the centre is a useful, privacy-preserving proxy
            // for a subject without attempting face recognition.
            if x * 4 >= width && x * 4 <= width * 3 && y * 4 >= height && y * 4 <= height * 3 {
                centre_detail += edge;
                centre_count += 1.0;
            }
        }
    }
    if count == 0.0 {
        return 0.5;
    }
    let mean = sum_luma / count;
    let contrast = ((sum_luma_squared / count - mean * mean).max(0.0).sqrt() / 0.24).min(1.0);
    let colour = (saturation / count / 0.45).min(1.0);
    let detail = (detail / count / 0.34).min(1.0);
    let centred =
        (centre_detail / centre_count.max(1.0) / (detail * 0.34).max(0.001)).min(1.5) / 1.5;
    let exposure = 1.0 - ((mean - 0.52).abs() / 0.52).min(1.0) * 0.45;
    (0.18 + contrast * 0.25 + detail * 0.30 + colour * 0.14 + centred * 0.08 + exposure * 0.05)
        .clamp(0.2, 1.0)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LayoutKind {
    Mosaic,
    SmartMosaic,
    Grid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AspectRatio {
    Square,
    FourThree,
    ThreeTwo,
    SixteenNine,
    Custom,
}

impl AspectRatio {
    pub fn value(self) -> f32 {
        match self {
            Self::Square => 1.0,
            Self::FourThree => 4.0 / 3.0,
            Self::ThreeTwo => 3.0 / 2.0,
            Self::SixteenNine => 16.0 / 9.0,
            Self::Custom => 1.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CollageOrientation {
    Landscape,
    Portrait,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Background {
    White,
    Black,
    LightGray,
}

#[derive(Clone, Debug)]
pub struct CollageItem {
    pub photo: CollagePhoto,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub rotation: f32,
    pub z: usize,
}

#[derive(Clone, Debug)]
pub struct CollageProject {
    pub aspect: AspectRatio,
    pub custom_aspect: f32,
    pub orientation: CollageOrientation,
    pub background: Background,
    pub round_corners: bool,
    pub corner_radius: f32,
    pub layout: LayoutKind,
    pub keep_photo_aspect: bool,
    pub spacing: f32,
    pub seed: u64,
    pub items: Vec<CollageItem>,
}

impl CollageProject {
    pub fn new(photos: Vec<PhotoObject>) -> Self {
        let mut project = Self {
            aspect: AspectRatio::SixteenNine,
            custom_aspect: 16.0 / 9.0,
            orientation: CollageOrientation::Landscape,
            background: Background::White,
            round_corners: false,
            corner_radius: 0.06,
            layout: LayoutKind::SmartMosaic,
            keep_photo_aspect: true,
            spacing: 0.018,
            seed: 1,
            items: photos
                .into_iter()
                .enumerate()
                .map(|(z, photo)| CollageItem {
                    photo: collage_photo_from_object(&photo),
                    x: 0.0,
                    y: 0.0,
                    width: 1.0,
                    height: 1.0,
                    rotation: 0.0,
                    z,
                })
                .collect(),
        };
        project.choose_smart_canvas();
        project.relayout();
        project
    }

    /// Pick the closest supported canvas shape for a new Smart AI collage.
    /// The weighted log-distance is stable across portrait and landscape
    /// photos, and using visual interest lets the strongest selections have
    /// more influence than incidental supporting shots.  Users can still
    /// choose any aspect manually after the initial recommendation.
    pub fn choose_smart_canvas(&mut self) {
        if self.items.is_empty() {
            return;
        }
        let candidates = [
            (AspectRatio::Square, CollageOrientation::Landscape),
            (AspectRatio::FourThree, CollageOrientation::Landscape),
            (AspectRatio::ThreeTwo, CollageOrientation::Landscape),
            (AspectRatio::SixteenNine, CollageOrientation::Landscape),
            (AspectRatio::Square, CollageOrientation::Portrait),
            (AspectRatio::FourThree, CollageOrientation::Portrait),
            (AspectRatio::ThreeTwo, CollageOrientation::Portrait),
            (AspectRatio::SixteenNine, CollageOrientation::Portrait),
        ];
        let (aspect, orientation) = candidates
            .into_iter()
            .min_by(
                |(left_aspect, left_orientation), (right_aspect, right_orientation)| {
                    let cost = |aspect: AspectRatio, orientation: CollageOrientation| {
                        let canvas = match orientation {
                            CollageOrientation::Landscape => aspect.value(),
                            CollageOrientation::Portrait => 1.0 / aspect.value(),
                        };
                        self.items
                            .iter()
                            .map(|item| {
                                let importance = item.photo.visual_weight.max(0.2);
                                importance
                                    * (canvas / item.photo.aspect_ratio.max(0.01)).ln().powi(2)
                            })
                            .sum::<f32>()
                    };
                    cost(*left_aspect, *left_orientation)
                        .total_cmp(&cost(*right_aspect, *right_orientation))
                },
            )
            .expect("Smart canvas candidates are never empty");
        self.aspect = aspect;
        self.orientation = orientation;
        self.custom_aspect = aspect.value();
    }

    pub fn relayout(&mut self) {
        crate::collage::layout::apply(self);
    }

    pub fn base_aspect_ratio(&self) -> f32 {
        if self.aspect == AspectRatio::Custom {
            self.custom_aspect.max(0.01)
        } else {
            self.aspect.value()
        }
    }

    pub fn effective_aspect_ratio(&self) -> f32 {
        let ratio = self.base_aspect_ratio();
        match self.orientation {
            CollageOrientation::Landscape => ratio.max(1.0 / ratio),
            CollageOrientation::Portrait => ratio.min(1.0 / ratio),
        }
    }

    pub fn add_photos(&mut self, photos: Vec<PhotoObject>) {
        let first_z = self.items.len();
        let mut ids = self
            .items
            .iter()
            .map(|item| item.photo.id)
            .collect::<HashSet<_>>();
        self.items.extend(
            photos
                .into_iter()
                .filter(|photo| ids.insert(photo.id()))
                .enumerate()
                .map(|(index, photo)| CollageItem {
                    photo: collage_photo_from_object(&photo),
                    x: 0.0,
                    y: 0.0,
                    width: 1.0,
                    height: 1.0,
                    rotation: 0.0,
                    z: first_z + index,
                }),
        );
        self.relayout();
    }

    pub fn set_photos(&mut self, photos: Vec<PhotoObject>) {
        let existing = self
            .items
            .iter()
            .cloned()
            .map(|item| (item.photo.id, item))
            .collect::<std::collections::HashMap<_, _>>();
        let mut seen = HashSet::new();
        self.items = photos
            .into_iter()
            .filter(|photo| seen.insert(photo.id()))
            .enumerate()
            .map(|(z, photo)| {
                existing
                    .get(&photo.id())
                    .cloned()
                    .unwrap_or_else(|| CollageItem {
                        photo: collage_photo_from_object(&photo),
                        x: 0.0,
                        y: 0.0,
                        width: 1.0,
                        height: 1.0,
                        rotation: 0.0,
                        z,
                    })
            })
            .collect();
        for (z, item) in self.items.iter_mut().enumerate() {
            item.z = z;
        }
        self.relayout();
    }

    /// Refresh embedded photo metadata (edits, rotations, thumbnails) for
    /// matching ids without touching the saved arrangement. Used when
    /// returning from the photo editor so tiles reflect latest edits.
    pub fn refresh_photo_metadata(&mut self, photos: &[PhotoObject]) {
        for item in self.items.iter_mut() {
            if let Some(photo) = photos.iter().find(|photo| photo.id() == item.photo.id) {
                item.photo = collage_photo_from_object(photo);
            }
        }
    }

    pub fn shuffle(&mut self) {
        self.seed = self.seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let mut random = self.seed;
        for index in (1..self.items.len()).rev() {
            random = random
                .wrapping_mul(2862933555777941757)
                .wrapping_add(3037000493);
            let swap_index = (random % (index as u64 + 1)) as usize;
            self.items.swap(index, swap_index);
        }
        self.relayout();
    }

    pub fn swap_item_positions(&mut self, first: usize, second: usize) {
        if first == second || first >= self.items.len() || second >= self.items.len() {
            return;
        }
        let (first_item, second_item) = if first < second {
            let (left, right) = self.items.split_at_mut(second);
            (&mut left[first], &mut right[0])
        } else {
            let (left, right) = self.items.split_at_mut(first);
            (&mut right[0], &mut left[second])
        };
        std::mem::swap(&mut first_item.x, &mut second_item.x);
        std::mem::swap(&mut first_item.y, &mut second_item.y);
        std::mem::swap(&mut first_item.width, &mut second_item.width);
        std::mem::swap(&mut first_item.height, &mut second_item.height);
        std::mem::swap(&mut first_item.rotation, &mut second_item.rotation);
        std::mem::swap(&mut first_item.z, &mut second_item.z);
    }

    /// Snapshot of every user-visible project decision, ready to persist.
    /// Geometry is stored per photo id; photos are re-fetched from the
    /// library when the draft is restored so edits/rotations stay current.
    pub fn to_draft(&self) -> CollageDraft {
        CollageDraft {
            version: COLLAGE_DRAFT_VERSION,
            layout: self.layout,
            orientation: self.orientation,
            aspect: self.aspect,
            custom_aspect: self.custom_aspect,
            background: self.background,
            round_corners: self.round_corners,
            corner_radius: self.corner_radius,
            spacing: self.spacing,
            keep_photo_aspect: self.keep_photo_aspect,
            seed: self.seed,
            items: self
                .items
                .iter()
                .map(|item| DraftItem {
                    id: item.photo.id,
                    x: item.x,
                    y: item.y,
                    width: item.width,
                    height: item.height,
                    rotation: item.rotation,
                    z: item.z,
                })
                .collect(),
        }
    }

    /// Restore settings and per-photo geometry from a draft. `photos` are
    /// freshly fetched from the library; draft entries whose photo is gone
    /// are dropped, and the remaining geometry is applied as saved (no
    /// relayout, the saved arrangement is the point of a draft).
    pub fn apply_draft(&mut self, draft: &CollageDraft, photos: &[PhotoObject]) {
        self.layout = draft.layout;
        self.orientation = draft.orientation;
        self.aspect = draft.aspect;
        self.custom_aspect = draft.custom_aspect;
        self.background = draft.background;
        self.round_corners = draft.round_corners;
        self.corner_radius = draft.corner_radius;
        self.spacing = draft.spacing;
        self.keep_photo_aspect = draft.keep_photo_aspect;
        self.seed = draft.seed;
        let mut items: Vec<CollageItem> = draft
            .items
            .iter()
            .filter_map(|entry| {
                let photo = photos.iter().find(|photo| photo.id() == entry.id)?;
                Some(CollageItem {
                    photo: collage_photo_from_object(photo),
                    x: entry.x,
                    y: entry.y,
                    width: entry.width,
                    height: entry.height,
                    rotation: entry.rotation,
                    z: entry.z,
                })
            })
            .collect();
        items.sort_by_key(|item| item.z);
        for (z, item) in items.iter_mut().enumerate() {
            item.z = z;
        }
        self.items = items;
    }
}

fn collage_photo_from_object(photo: &PhotoObject) -> CollagePhoto {
    CollagePhoto {
        id: photo.id(),
        folder_id: Some(photo.folder_id()),
        path: photo.path(),
        filename: photo.filename(),
        thumbnail_path: photo.cached_thumbnail_path(),
        library_rotation: photo.rotation(),
        edit_recipe: photo.edit_recipe(),
        aspect_ratio: photo_aspect_ratio(photo),
        visual_weight: visual_interest(photo),
    }
}

pub const COLLAGE_DRAFT_VERSION: u32 = 1;

/// Persisted snapshot of an in-progress collage; stored as JSON in the
/// library's settings table so work survives leaving the collage editor.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CollageDraft {
    pub version: u32,
    pub layout: LayoutKind,
    pub orientation: CollageOrientation,
    pub aspect: AspectRatio,
    pub custom_aspect: f32,
    pub background: Background,
    pub round_corners: bool,
    pub corner_radius: f32,
    pub spacing: f32,
    pub keep_photo_aspect: bool,
    pub seed: u64,
    pub items: Vec<DraftItem>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DraftItem {
    pub id: i64,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub rotation: f32,
    pub z: usize,
}

pub fn draft_to_json(project: &CollageProject) -> String {
    serde_json::to_string(&project.to_draft()).unwrap_or_else(|_| "null".to_string())
}

pub fn draft_from_json(json: &str) -> Option<CollageDraft> {
    let draft = serde_json::from_str::<CollageDraft>(json).ok()?;
    if draft.version != COLLAGE_DRAFT_VERSION {
        return None;
    }
    Some(draft)
}

#[cfg(test)]
mod draft_tests {
    use super::*;

    #[test]
    fn draft_json_round_trips_project_settings_and_rejects_versions() {
        let mut project = CollageProject {
            aspect: AspectRatio::Square,
            custom_aspect: 1.5,
            orientation: CollageOrientation::Portrait,
            background: Background::Black,
            round_corners: true,
            corner_radius: 0.15,
            layout: LayoutKind::Grid,
            keep_photo_aspect: false,
            spacing: 0.05,
            seed: 42,
            items: vec![CollageItem {
                photo: CollagePhoto {
                    id: 7,
                    folder_id: None,
                    path: String::new(),
                    filename: "a.jpg".to_string(),
                    thumbnail_path: None,
                    library_rotation: 0,
                    edit_recipe: String::new(),
                    aspect_ratio: 1.5,
                    visual_weight: 0.5,
                },
                x: 0.25,
                y: 0.5,
                width: 0.5,
                height: 0.5,
                rotation: 90.0,
                z: 0,
            }],
        };
        // No relayout here: it is allowed to reset per-item rotations, and
        // the draft must capture project state exactly as the user left it.
        let json = draft_to_json(&project);
        let draft = draft_from_json(&json).expect("round trip");
        assert_eq!(draft.version, COLLAGE_DRAFT_VERSION);
        assert_eq!(draft.layout, LayoutKind::Grid);
        assert_eq!(draft.orientation, CollageOrientation::Portrait);
        assert_eq!(draft.background, Background::Black);
        assert_eq!(draft.aspect, AspectRatio::Square);
        assert!((draft.custom_aspect - 1.5).abs() < 1e-6);
        assert!(draft.round_corners);
        assert!((draft.corner_radius - 0.15).abs() < 1e-6);
        assert!((draft.spacing - 0.05).abs() < 1e-6);
        assert_eq!(draft.seed, 42);
        assert!(!draft.keep_photo_aspect);
        assert_eq!(draft.items.len(), 1);
        assert_eq!(draft.items[0].id, 7);
        assert!((draft.items[0].rotation - 90.0).abs() < 1e-6);
        // Drafts from a different schema version must be refused.
        let mut foreign = draft_to_json(&project);
        foreign = foreign.replace("\"version\":1", "\"version\":99");
        assert!(draft_from_json(&foreign).is_none());
    }

    #[test]
    fn apply_draft_restores_settings_and_drops_missing_photos() {
        let json = r#"{
            "version": 1,
            "layout": "Grid",
            "orientation": "Portrait",
            "aspect": "Square",
            "custom_aspect": 1.0,
            "background": "LightGray",
            "round_corners": true,
            "corner_radius": 0.1,
            "spacing": 0.02,
            "keep_photo_aspect": false,
            "seed": 9,
            "items": [
                {"id": 11, "x": 0.0, "y": 0.0, "width": 0.5, "height": 1.0, "rotation": 0.0, "z": 0},
                {"id": 99, "x": 0.5, "y": 0.0, "width": 0.5, "height": 1.0, "rotation": 90.0, "z": 1}
            ]
        }"#;
        let draft = draft_from_json(json).expect("parse");
        let mut project = CollageProject {
            aspect: AspectRatio::SixteenNine,
            custom_aspect: 16.0 / 9.0,
            orientation: CollageOrientation::Landscape,
            background: Background::White,
            round_corners: false,
            corner_radius: 0.0,
            layout: LayoutKind::Mosaic,
            keep_photo_aspect: true,
            spacing: 0.0,
            seed: 1,
            items: Vec::new(),
        };
        // No photos supplied: every draft entry refers to a missing photo,
        // so the arrangement must end up empty instead of panicking.
        project.apply_draft(&draft, &[]);
        assert!(project.items.is_empty());
        assert_eq!(project.layout, LayoutKind::Grid);
        assert_eq!(project.orientation, CollageOrientation::Portrait);
        assert_eq!(project.background, Background::LightGray);
        assert!(project.round_corners);
        assert!((project.spacing - 0.02).abs() < 1e-6);
        assert_eq!(project.seed, 9);
    }
}
