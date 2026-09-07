use std::collections::HashSet;

use crate::photo_object::PhotoObject;

#[derive(Clone, Debug)]
pub struct CollagePhoto {
    pub id: i64,
    pub path: String,
    pub filename: String,
    pub thumbnail_path: Option<String>,
    pub library_rotation: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutKind {
    Mosaic,
    Grid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AspectRatio {
    Square,
    FourThree,
    ThreeTwo,
    SixteenNine,
}

impl AspectRatio {
    pub fn value(self) -> f32 {
        match self {
            Self::Square => 1.0,
            Self::FourThree => 4.0 / 3.0,
            Self::ThreeTwo => 3.0 / 2.0,
            Self::SixteenNine => 16.0 / 9.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    pub background: Background,
    pub round_corners: bool,
    pub corner_radius: f32,
    pub layout: LayoutKind,
    pub spacing: f32,
    pub seed: u64,
    pub items: Vec<CollageItem>,
}

impl CollageProject {
    pub fn new(photos: Vec<PhotoObject>) -> Self {
        if std::env::var_os("PICASA_TRACE").is_some() {
            eprintln!("COLLAGE TRACE project photos={}", photos.len());
        }
        let mut project = Self {
            aspect: AspectRatio::SixteenNine,
            background: Background::White,
            round_corners: false,
            corner_radius: 0.06,
            layout: LayoutKind::Grid,
            spacing: 0.018,
            seed: 1,
            items: photos
                .into_iter()
                .enumerate()
                .map(|(z, photo)| CollageItem {
                    photo: CollagePhoto {
                        id: photo.id(),
                        path: photo.path(),
                        filename: photo.filename(),
                        thumbnail_path: photo.cached_thumbnail_path(),
                        library_rotation: photo.rotation(),
                    },
                    x: 0.0,
                    y: 0.0,
                    width: 1.0,
                    height: 1.0,
                    rotation: 0.0,
                    z,
                })
                .collect(),
        };
        project.relayout();
        project
    }

    pub fn relayout(&mut self) {
        crate::collage::layout::apply(self);
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
                    photo: CollagePhoto {
                        id: photo.id(),
                        path: photo.path(),
                        filename: photo.filename(),
                        thumbnail_path: photo.cached_thumbnail_path(),
                        library_rotation: photo.rotation(),
                    },
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
                        photo: CollagePhoto {
                            id: photo.id(),
                            path: photo.path(),
                            filename: photo.filename(),
                            thumbnail_path: photo.cached_thumbnail_path(),
                            library_rotation: photo.rotation(),
                        },
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
}
