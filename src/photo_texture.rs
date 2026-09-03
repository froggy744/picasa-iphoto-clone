use gtk::prelude::*;
use gtk4 as gtk;

/// Load the small cached thumbnail and apply the user's persisted rotation.
/// EXIF orientation is already baked into the thumbnail cache, so this only
/// handles the additional 0/90/180/270-degree database value.
pub fn rotated_thumbnail(path: &str, rotation: i32) -> Option<gtk::gdk::Paintable> {
    let rotation = rotation.rem_euclid(360);
    if rotation == 0 {
        return None;
    }

    let image = rotate_pixels(image::open(path).ok()?.to_rgba8(), rotation);
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
    Some(texture.upcast())
}

fn rotate_pixels(image: image::RgbaImage, rotation: i32) -> image::RgbaImage {
    match rotation.rem_euclid(360) {
        90 => image::imageops::rotate90(&image),
        180 => image::imageops::rotate180(&image),
        270 => image::imageops::rotate270(&image),
        _ => image,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quarter_turn_swaps_thumbnail_axes() {
        let image = image::RgbaImage::new(3, 2);
        assert_eq!(rotate_pixels(image.clone(), 90).dimensions(), (2, 3));
        assert_eq!(rotate_pixels(image, 270).dimensions(), (2, 3));
    }

    #[test]
    fn half_turn_keeps_thumbnail_axes() {
        let image = image::RgbaImage::new(3, 2);
        assert_eq!(rotate_pixels(image, 180).dimensions(), (3, 2));
    }
}
