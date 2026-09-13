use std::cell::RefCell;
use std::collections::VecDeque;

use gtk::prelude::*;
use gtk4 as gtk;

thread_local! {
    static EDITED_THUMBNAIL_CACHE: RefCell<VecDeque<(String, i32, String, gtk::gdk::Paintable)>> =
        RefCell::new(VecDeque::new());
}
const EDITED_THUMBNAIL_CACHE_CAPACITY: usize = 96;

/// Load a cached thumbnail and apply PIC's non-destructive presentation edits.
/// EXIF orientation is already baked into the thumbnail cache. The database
/// rotation and edit recipe are presentation metadata applied here without
/// touching the original file.
pub fn edited_thumbnail(
    path: &str,
    rotation: i32,
    edit_recipe: &str,
) -> Option<gtk::gdk::Paintable> {
    let rotation = rotation.rem_euclid(360);
    let recipe = crate::edit::EditRecipe::decode(edit_recipe);
    if rotation == 0 && recipe.is_default() {
        return None;
    }
    if let Some(cached) = cache_get(path, rotation, edit_recipe) {
        return Some(cached);
    }
    let image = crate::edit::render::render_thumbnail(path, rotation, &recipe).ok()?;
    let width = image.width() as i32;
    let height = image.height() as i32;
    let bytes = glib::Bytes::from_owned(image.into_raw());
    let texture = gtk::gdk::MemoryTexture::new(
        width,
        height,
        gtk::gdk::MemoryFormat::R8g8b8a8,
        &bytes,
        width as usize * 4,
    );
    let paintable: gtk::gdk::Paintable = texture.upcast();
    cache_insert(
        path.to_string(),
        rotation,
        edit_recipe.to_string(),
        paintable.clone(),
    );
    Some(paintable)
}

/// Compatibility helper used by collage code that only has rotation metadata.
pub fn rotated_thumbnail(path: &str, rotation: i32) -> Option<gtk::gdk::Paintable> {
    edited_thumbnail(path, rotation, "")
}

fn cache_get(path: &str, rotation: i32, recipe: &str) -> Option<gtk::gdk::Paintable> {
    EDITED_THUMBNAIL_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        let position =
            cache
                .iter()
                .position(|(cached_path, cached_rotation, cached_recipe, _)| {
                    cached_path == path && *cached_rotation == rotation && cached_recipe == recipe
                })?;
        let entry = cache.remove(position)?;
        let paintable = entry.3.clone();
        cache.push_back(entry);
        Some(paintable)
    })
}

fn cache_insert(path: String, rotation: i32, recipe: String, paintable: gtk::gdk::Paintable) {
    EDITED_THUMBNAIL_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        cache.retain(|(cached_path, cached_rotation, cached_recipe, _)| {
            cached_path != &path || *cached_rotation != rotation || cached_recipe != &recipe
        });
        cache.push_back((path, rotation, recipe, paintable));
        while cache.len() > EDITED_THUMBNAIL_CACHE_CAPACITY {
            cache.pop_front();
        }
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn quarter_turn_swaps_thumbnail_axes() {
        let image = image::RgbaImage::new(3, 2);
        assert_eq!(
            crate::edit::render::apply_library_rotation(image.clone(), 90).dimensions(),
            (2, 3)
        );
        assert_eq!(
            crate::edit::render::apply_library_rotation(image, 270).dimensions(),
            (2, 3)
        );
    }

    #[test]
    fn half_turn_keeps_thumbnail_axes() {
        let image = image::RgbaImage::new(3, 2);
        assert_eq!(
            crate::edit::render::apply_library_rotation(image, 180).dimensions(),
            (3, 2)
        );
    }
}
