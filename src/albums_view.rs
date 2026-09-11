use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::prelude::*;
use gtk4 as gtk;
use rusqlite::Connection;

use crate::db::{self, Album};
use crate::settings::{self, AlbumViewStyle};

const DEFAULT_THUMBNAIL_WIDTH: i32 = 136;
const DEFAULT_THUMBNAIL_HEIGHT: i32 = 91;

// Common opening with a small underlap beneath both supplied skins' opaque edges.
const PHOTO_X_RATIO: f64 = 0.18;
const PHOTO_Y_RATIO: f64 = 53.0 / 500.0;
const PHOTO_WIDTH_RATIO: f64 = 0.64;
const PHOTO_HEIGHT_RATIO: f64 = 273.0 / 500.0;

fn album_frame_paths() -> Vec<PathBuf> {
    album_frame_paths_in(Path::new("images"))
}

fn album_frame_paths_for_style(style: AlbumViewStyle) -> Vec<PathBuf> {
    match style {
        AlbumViewStyle::Default => Vec::new(),
        AlbumViewStyle::Bookshelf => album_frame_paths(),
        AlbumViewStyle::AlbumCovers => vec![PathBuf::from("images/album-cover.png")],
    }
}

fn uses_bookshelf_background(style: AlbumViewStyle) -> bool {
    style == AlbumViewStyle::Bookshelf
}

fn bookshelf_background_path() -> PathBuf {
    bookshelf_background_path_in(Path::new("images"))
}

fn bookshelf_background_path_in(directory: &Path) -> PathBuf {
    for name in ["bookshelf.jpg", "bookshelf.jpeg", "bookshelf.png"] {
        let path = directory.join(name);
        if path.is_file() {
            return path;
        }
    }
    directory.join("bookshelf.png")
}

fn album_frame_paths_in(directory: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut paths: Vec<_> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.ends_with("-album.png"))
        })
        .collect();
    paths.sort();
    paths
}

fn frame_index(album: &Album, frame_count: usize) -> Option<usize> {
    if frame_count == 0 {
        return None;
    }
    // Use a defined hash rather than a process-seeded or unspecified hasher.
    let mut hasher = blake3::Hasher::new();
    hasher.update(&album.id.to_le_bytes());
    hasher.update(album.name.as_bytes());
    let hash = hasher.finalize();
    let value = u64::from_le_bytes(hash.as_bytes()[..8].try_into().unwrap());
    Some((value % frame_count as u64) as usize)
}

pub fn build(
    albums: &[Album],
    connection: &Connection,
    thumbnail_width: i32,
    on_album: Rc<dyn Fn(i64)>,
) -> gtk::ScrolledWindow {
    let scrolled = gtk::ScrolledWindow::new();
    scrolled.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scrolled.set_hexpand(true);
    scrolled.set_vexpand(true);

    let background = gtk::CssProvider::new();
    let uri = gio::File::for_path(bookshelf_background_path()).uri();
    background.load_from_string(&format!(
        ".albums-bookshelf {{ background-image: url(\"{uri}\"); \
         background-size: 100% auto; background-repeat: repeat-y; \
         background-position: center top; }}"
    ));
    scrolled
        .style_context()
        .add_provider(&background, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 2);
    refresh_presentation(&scrolled, connection);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 18);
    content.set_margin_start(28);
    content.set_margin_end(28);
    content.set_margin_top(24);
    content.set_margin_bottom(28);

    let title = gtk::Label::new(Some("Albums"));
    title.set_xalign(0.0);
    title.add_css_class("section-heading");
    content.append(&title);

    let count = gtk::Label::new(None);
    count.set_xalign(0.0);
    count.add_css_class("dim-label");
    count.add_css_class("albums-home-count");
    content.append(&count);

    let cards = gtk::FlowBox::new();
    cards.set_selection_mode(gtk::SelectionMode::None);
    cards.set_min_children_per_line(1);
    cards.set_max_children_per_line(6);
    cards.set_row_spacing(20);
    cards.set_column_spacing(20);
    cards.set_homogeneous(false);
    cards.set_hexpand(true);
    cards.set_vexpand(false);
    cards.set_valign(gtk::Align::Start);
    cards.add_css_class("albums-home-grid");

    content.append(&cards);
    scrolled.set_child(Some(&content));

    populate(
        &cards,
        &count,
        albums,
        connection,
        thumbnail_width,
        on_album,
    );

    scrolled
}

pub fn refresh(
    scrolled: &gtk::ScrolledWindow,
    albums: &[Album],
    connection: &Connection,
    thumbnail_width: i32,
    on_album: Rc<dyn Fn(i64)>,
) {
    refresh_presentation(scrolled, connection);
    // The ScrolledWindow may expose a GTK viewport around its content. Find
    // the existing widgets by their local CSS markers instead of assuming
    // that the content box is the direct child.
    let root = scrolled.clone().upcast::<gtk::Widget>();
    let Some(count) = find_descendant_with_css_class(&root, "albums-home-count")
        .and_then(|widget| widget.downcast::<gtk::Label>().ok())
    else {
        return;
    };
    let Some(cards) = find_descendant_with_css_class(&root, "albums-home-grid")
        .and_then(|widget| widget.downcast::<gtk::FlowBox>().ok())
    else {
        return;
    };

    populate(
        &cards,
        &count,
        albums,
        connection,
        thumbnail_width,
        on_album,
    );
}

pub fn refresh_presentation(scrolled: &gtk::ScrolledWindow, connection: &Connection) {
    if uses_bookshelf_background(settings::album_view_style(connection)) {
        scrolled.add_css_class("albums-bookshelf");
    } else {
        scrolled.remove_css_class("albums-bookshelf");
    }
}

fn find_descendant_with_css_class(root: &gtk::Widget, class_name: &str) -> Option<gtk::Widget> {
    if root.has_css_class(class_name) {
        return Some(root.clone());
    }

    let mut child = root.first_child();
    while let Some(candidate) = child {
        let next = candidate.next_sibling();
        if let Some(found) = find_descendant_with_css_class(&candidate, class_name) {
            return Some(found);
        }
        child = next;
    }

    None
}

fn populate(
    cards: &gtk::FlowBox,
    count: &gtk::Label,
    albums: &[Album],
    connection: &Connection,
    thumbnail_width: i32,
    on_album: Rc<dyn Fn(i64)>,
) {
    while let Some(child) = cards.first_child() {
        cards.remove(&child);
    }

    count.set_text(&format!(
        "{} album{}",
        albums.len(),
        if albums.len() == 1 { "" } else { "s" }
    ));

    if std::env::var_os("PICASA_TRACE").is_some() {
        eprintln!(
            "ALBUM VIEW TRACE albums={} cover_size={}x{}",
            albums.len(),
            cover_width(thumbnail_width),
            cover_height(thumbnail_width)
        );
    }

    if albums.is_empty() {
        let empty = gtk::Label::new(Some(
            "No albums yet\nCreate an album with the + button in the sidebar.",
        ));
        empty.set_xalign(0.0);
        empty.add_css_class("dim-label");
        insert_child(cards, &empty, None);
        return;
    }

    let frames: Vec<_> = album_frame_paths_for_style(settings::album_view_style(connection))
        .iter()
        .filter_map(|path| gtk::gdk::Texture::from_filename(path).ok())
        .collect();
    let framed = !frames.is_empty();
    cards.set_row_spacing(if framed { 28 } else { 20 });
    cards.set_column_spacing(if framed { 24 } else { 20 });
    let thumbnail_width = if framed {
        thumbnail_width.clamp(220, 300)
    } else {
        thumbnail_width
    };
    let frame_height = (thumbnail_width as f64 * 500.0 / 805.0).round() as i32;
    let card_width = frames
        .iter()
        .map(|frame| {
            (frame_height as f64 * frame.width() as f64 / frame.height() as f64).round() as i32
        })
        .max()
        .unwrap_or_else(|| cover_width(thumbnail_width));
    for album in albums {
        let frame = frame_index(album, frames.len()).map(|index| &frames[index]);
        let card = album_card(album, connection, thumbnail_width, on_album.clone(), frame);
        if framed {
            card.set_width_request(card_width);
        }
        insert_child(cards, &card, Some(card_width));
    }
}

fn insert_child(cards: &gtk::FlowBox, child: &impl IsA<gtk::Widget>, width: Option<i32>) {
    cards.insert(child, -1);

    // GtkFlowBox automatically wraps every inserted widget in a
    // GtkFlowBoxChild. Prevent that wrapper from stretching the album card.
    if let Some(flow_child) = cards
        .last_child()
        .and_then(|child| child.downcast::<gtk::FlowBoxChild>().ok())
    {
        if let Some(width) = width {
            flow_child.set_width_request(width);
        }
        flow_child.set_hexpand(false);
        flow_child.set_vexpand(false);
        flow_child.set_halign(gtk::Align::Start);
        flow_child.set_valign(gtk::Align::Start);
    }
}

fn cover_width(thumbnail_width: i32) -> i32 {
    thumbnail_width.clamp(100, 300)
}

fn cover_height(thumbnail_width: i32) -> i32 {
    ((cover_width(thumbnail_width) as f64 * DEFAULT_THUMBNAIL_HEIGHT as f64
        / DEFAULT_THUMBNAIL_WIDTH as f64)
        .round()
        .max(1.0)) as i32
}

fn album_card(
    album: &Album,
    connection: &Connection,
    thumbnail_width: i32,
    on_album: Rc<dyn Fn(i64)>,
    frame: Option<&gtk::gdk::Texture>,
) -> gtk::Button {
    let card = gtk::Button::new();
    card.set_has_frame(false);
    card.set_hexpand(false);
    card.set_vexpand(false);
    card.set_halign(gtk::Align::Start);
    card.set_valign(gtk::Align::Start);
    card.add_css_class("album-card");

    let height = if frame.is_some() {
        (thumbnail_width as f64 * 500.0 / 805.0).round() as i32
    } else {
        cover_height(thumbnail_width)
    };
    let width = frame
        .map(|frame| (height as f64 * frame.width() as f64 / frame.height() as f64).round() as i32)
        .unwrap_or_else(|| cover_width(thumbnail_width));
    card.set_width_request(width);
    let content = gtk::Box::new(gtk::Orientation::Vertical, 7);
    content.set_size_request(width, -1);
    content.set_hexpand(false);
    content.set_vexpand(false);
    content.set_halign(gtk::Align::Start);
    if frame.is_some() {
        content.set_halign(gtk::Align::Center);
        let style = gtk::CssProvider::new();
        style.load_from_string(
            "button.album-card { color: #302014; transition: opacity 120ms ease; } \
             button.album-card:hover { opacity: 0.90; }",
        );
        card.style_context()
            .add_provider(&style, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 2);
    }

    // Match the normal photo-grid thumbnail allocation.
    let cover = gtk::Overlay::new();
    cover.set_width_request(width);
    cover.set_height_request(height);
    cover.set_size_request(width, height);
    cover.set_hexpand(false);
    cover.set_vexpand(false);
    cover.set_halign(gtk::Align::Start);
    cover.set_valign(gtk::Align::Start);
    cover.set_overflow(gtk::Overflow::Hidden);
    cover.add_css_class("album-cover");
    if frame.is_none() {
        cover.add_css_class("photo-frame");
        cover.add_css_class("photo-tile");
    }

    let picture = gtk::Picture::new();

    // Fill the standard thumbnail rectangle while preserving aspect ratio.
    picture.set_content_fit(gtk::ContentFit::Cover);

    picture.set_can_shrink(true);
    picture.set_size_request(1, 1);
    picture.set_hexpand(true);
    picture.set_vexpand(true);
    picture.set_halign(gtk::Align::Fill);
    picture.set_valign(gtk::Align::Fill);
    picture.set_overflow(gtk::Overflow::Hidden);
    picture.add_css_class("thumbnail");

    let photos = db::photos_in_album(connection, album.id, None).unwrap_or_default();

    // Use the first album photo that already has a cached thumbnail.
    let cover_photo = photos.iter().find_map(|photo| {
        crate::thumbnail::existing_cache_path(&photo.path, photo.mtime, photo.size_bytes)
            .ok()
            .flatten()
            .map(|path| (photo, path))
    });

    if std::env::var_os("PICASA_TRACE").is_some() {
        eprintln!(
            "ALBUM TRACE id={} name={} photos={} cover={:?}",
            album.id,
            album.name,
            photos.len(),
            cover_photo
                .as_ref()
                .map(|(_, path)| path.to_string_lossy().into_owned())
        );
    }

    if let Some((photo, path)) = cover_photo.as_ref() {
        let path_string = path.to_string_lossy();

        // Use the same RAW/NEF crop as the normal gallery. Some embedded RAW
        // previews contain side bars that GTK's Cover mode cannot remove.
        let photo_object = crate::photo_object::PhotoObject::from_photo(photo);
        if let Some(cropped) = crate::grid::raw_cached_thumbnail(&photo_object, &path_string) {
            picture.set_paintable(Some(&cropped));
        // Respect user rotation without regenerating the cached thumbnail.
        } else if let Some(rotated) =
            crate::photo_texture::edited_thumbnail(&path_string, photo.rotation, &photo.edit_recipe)
        {
            picture.set_paintable(Some(&rotated));
        } else {
            picture.set_filename(Some(path_string.as_ref()));
        }
    } else {
        picture.set_paintable(gtk::gdk::Paintable::NONE);
    }

    cover.set_child(Some(&picture));

    let placeholder = gtk::Image::from_icon_name("folder-pictures-symbolic");
    placeholder.set_pixel_size(48);
    placeholder.set_halign(gtk::Align::Center);
    placeholder.set_valign(gtk::Align::Center);
    placeholder.add_css_class("dim-label");
    placeholder.set_visible(cover_photo.is_none());
    cover.add_overlay(&placeholder);

    if let Some(frame) = frame {
        // The cached photo stays below the transparent opening; the PNG is the top layer.
        let (left, top, right, bottom) = photo_margins(width, height);
        for widget in [
            picture.upcast_ref::<gtk::Widget>(),
            placeholder.upcast_ref(),
        ] {
            widget.set_margin_start(left);
            widget.set_margin_top(top);
            widget.set_margin_end(right);
            widget.set_margin_bottom(bottom);
        }
        placeholder.set_halign(gtk::Align::Fill);
        placeholder.set_valign(gtk::Align::Fill);
        placeholder.set_pixel_size(24);
        let skin = gtk::Picture::for_paintable(frame);
        skin.set_content_fit(gtk::ContentFit::Contain);
        skin.set_can_shrink(true);
        skin.set_size_request(1, 1);
        skin.set_halign(gtk::Align::Fill);
        skin.set_valign(gtk::Align::Fill);
        skin.set_can_target(false);
        skin.add_css_class("album-skin");
        cover.add_overlay(&skin);
    }

    // GtkPicture's natural size must not determine the album card height.
    // SquareTile manually allocates the cover to the same fixed rectangle as
    // the normal photo grid, even when the cached image is portrait.
    let cover_tile = crate::grid::SquareTile::new(width, height, &cover);
    cover_tile.set_hexpand(false);
    cover_tile.set_vexpand(false);
    cover_tile.set_halign(gtk::Align::Start);
    cover_tile.set_valign(gtk::Align::Start);
    content.append(&cover_tile);

    // Album name below cover.
    let name = gtk::Label::new(Some(&album.name));
    name.set_xalign(0.0);
    name.set_width_chars(1);
    name.set_max_width_chars(24);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    name.set_tooltip_text(Some(&album.name));
    if frame.is_some() {
        name.set_xalign(0.5);
    }
    content.append(&name);

    // Photo count below album name.
    let count_text = if album.photo_count == 1 {
        "1 photo".to_string()
    } else {
        format!("{} photos", album.photo_count)
    };

    let photo_count = gtk::Label::new(Some(&count_text));
    photo_count.set_xalign(0.0);
    photo_count.add_css_class("dim-label");
    if frame.is_some() {
        photo_count.set_xalign(0.5);
    }
    content.append(&photo_count);

    card.set_child(Some(&content));

    if std::env::var_os("PICASA_TRACE").is_some() {
        let cover_for_trace = cover.clone();
        let picture_for_trace = picture.clone();
        card.add_tick_callback(move |card, _| {
            if card.allocated_width() <= 0 || card.allocated_height() <= 0 {
                return glib::ControlFlow::Continue;
            }

            eprintln!(
                "ALBUM COVER TRACE card={}x{} cover={}x{} picture={}x{}",
                card.allocated_width(),
                card.allocated_height(),
                cover_for_trace.allocated_width(),
                cover_for_trace.allocated_height(),
                picture_for_trace.allocated_width(),
                picture_for_trace.allocated_height()
            );
            glib::ControlFlow::Break
        });
    }

    let album_id = album.id;
    card.connect_clicked(move |_| {
        on_album(album_id);
    });

    card
}

fn photo_margins(width: i32, height: i32) -> (i32, i32, i32, i32) {
    // Round outward so there are no background seams at the opening's edges.
    (
        (width as f64 * PHOTO_X_RATIO).floor() as i32,
        (height as f64 * PHOTO_Y_RATIO).floor() as i32,
        (width as f64 * (1.0 - PHOTO_X_RATIO - PHOTO_WIDTH_RATIO)).floor() as i32,
        (height as f64 * (1.0 - PHOTO_Y_RATIO - PHOTO_HEIGHT_RATIO)).floor() as i32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn album(id: i64) -> Album {
        Album {
            id,
            name: format!("Album {id}"),
            created_at: 0,
            photo_count: 0,
        }
    }

    #[test]
    fn discovers_only_album_png_files_sorted_and_picks_up_new_skins() {
        let directory = std::env::temp_dir().join(format!(
            "pic-album-skins-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        assert!(album_frame_paths_in(&directory).is_empty());
        std::fs::create_dir(&directory).unwrap();
        assert!(album_frame_paths_in(&directory).is_empty());
        for name in [
            "white-album.png",
            "pink-album.png",
            "blue-album.png",
            "leather-album.png",
            "bookshelf.png",
            "album.png",
            "cover.png",
            "white-album.jpg",
            "white-album.png.bak",
        ] {
            std::fs::write(directory.join(name), []).unwrap();
        }
        std::fs::create_dir(directory.join("directory-album.png")).unwrap();
        let expected: Vec<_> = [
            "blue-album.png",
            "leather-album.png",
            "pink-album.png",
            "white-album.png",
        ]
        .map(|name| directory.join(name))
        .into();
        assert_eq!(album_frame_paths_in(&directory), expected);

        std::fs::write(directory.join("green-album.png"), []).unwrap();
        let mut expected = expected;
        expected.push(directory.join("green-album.png"));
        expected.sort();
        assert_eq!(album_frame_paths_in(&directory), expected);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn bookshelf_background_prefers_jpg_and_falls_back_to_png() {
        let directory = std::env::temp_dir().join(format!(
            "pic-bookshelf-background-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));

        assert_eq!(
            bookshelf_background_path_in(&directory),
            directory.join("bookshelf.png")
        );

        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join("bookshelf.png"), []).unwrap();
        assert_eq!(
            bookshelf_background_path_in(&directory),
            directory.join("bookshelf.png")
        );

        std::fs::write(directory.join("bookshelf.jpg"), []).unwrap();
        assert_eq!(
            bookshelf_background_path_in(&directory),
            directory.join("bookshelf.jpg")
        );

        std::fs::remove_file(directory.join("bookshelf.jpg")).unwrap();
        std::fs::write(directory.join("bookshelf.jpeg"), []).unwrap();
        assert_eq!(
            bookshelf_background_path_in(&directory),
            directory.join("bookshelf.jpeg")
        );

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn zero_skins_have_no_selection() {
        assert_eq!(frame_index(&album(1), 0), None);
    }

    #[test]
    fn each_album_view_style_selects_its_own_assets() {
        use crate::settings::AlbumViewStyle;

        assert!(album_frame_paths_for_style(AlbumViewStyle::Default).is_empty());
        let bookshelf_paths = album_frame_paths_for_style(AlbumViewStyle::Bookshelf);
        assert!(bookshelf_paths.windows(2).all(|paths| paths[0] <= paths[1]));
        assert!(bookshelf_paths.contains(&PathBuf::from("images/pink-album.png")));
        assert!(bookshelf_paths.contains(&PathBuf::from("images/white-album.png")));
        assert!(!bookshelf_paths.contains(&PathBuf::from("images/album-cover.png")));
        assert_eq!(
            album_frame_paths_for_style(AlbumViewStyle::AlbumCovers),
            [PathBuf::from("images/album-cover.png")],
        );
        assert!(!uses_bookshelf_background(AlbumViewStyle::Default));
        assert!(uses_bookshelf_background(AlbumViewStyle::Bookshelf));
        assert!(!uses_bookshelf_background(AlbumViewStyle::AlbumCovers));
    }

    #[test]
    fn shared_photo_geometry_covers_both_skin_openings() {
        for (width, height, x, y, opening_width, opening_height) in
            [(805, 500, 147, 53, 505, 270), (827, 500, 165, 53, 510, 273)]
        {
            let (left, top, right, bottom) = photo_margins(width, height);
            assert!(left <= x && top <= y);
            assert!(width - right >= x + opening_width);
            assert!(height - bottom >= y + opening_height);
        }
        for width in [100, 136, 200, 300] {
            let height = (width as f64 * 500.0 / 805.0).round() as i32;
            let (left, top, right, bottom) = photo_margins(width, height);
            assert!(left >= 0 && top >= 0 && right >= 0 && bottom >= 0);
            assert!(left + right < width && top + bottom < height);
        }
    }

    #[test]
    #[ignore = "requires a GTK display; run with --ignored --test-threads=1"]
    fn album_view_modes_preserve_albums_and_click_navigation() {
        gtk::init().unwrap();
        let directory = std::env::temp_dir().join(format!(
            "pic-bookshelf-modes-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let connection = db::open(&directory.join("library.db")).unwrap();
        let album = db::create_album(&connection, "Holiday").unwrap();
        let albums = db::albums(&connection).unwrap();
        let opened = Rc::new(std::cell::Cell::new(None));
        let opened_for_callback = opened.clone();
        let on_album: Rc<dyn Fn(i64)> = Rc::new(move |id| opened_for_callback.set(Some(id)));
        let view = build(&albums, &connection, 136, on_album.clone());
        assert!(!view.has_css_class("albums-bookshelf"));
        for (style, background, skin) in [
            ("default", false, false),
            ("bookshelf", true, true),
            ("album-covers", false, true),
            ("default", false, false),
        ] {
            db::set_setting(
                &connection,
                crate::settings::ALBUM_VIEW_STYLE_SETTING_KEY,
                style,
            )
            .unwrap();
            refresh(&view, &albums, &connection, 136, on_album.clone());
            assert_eq!(view.has_css_class("albums-bookshelf"), background);
            assert_eq!(
                find_descendant_with_css_class(view.upcast_ref(), "album-skin").is_some(),
                skin,
            );
            assert_eq!(db::albums(&connection).unwrap(), albums);
            let button = find_descendant_with_css_class(view.upcast_ref(), "album-card")
                .unwrap()
                .downcast::<gtk::Button>()
                .unwrap();
            opened.set(None);
            button.emit_clicked();
            assert_eq!(opened.get(), Some(album.id));
        }
        drop(view);
        drop(connection);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn one_skin_always_selects_zero() {
        for id in 0..100 {
            assert_eq!(frame_index(&album(id), 1), Some(0));
        }
    }

    #[test]
    fn selection_is_stable_when_album_contents_change() {
        let mut album = album(42);
        let selected = frame_index(&album, 7);
        for _ in 0..100 {
            assert_eq!(frame_index(&album, 7), selected);
        }
        album.photo_count = 83;
        album.created_at = 12345;
        assert_eq!(frame_index(&album, 7), selected);
    }

    #[test]
    fn selection_stays_in_bounds_and_uses_multiple_skins() {
        for frame_count in [2, 3, 7, 32, usize::MAX] {
            let mut selected = std::collections::HashSet::new();
            for id in -100..100 {
                let index = frame_index(&album(id), frame_count).unwrap();
                assert!(index < frame_count);
                selected.insert(index);
            }
            assert!(selected.len() > 1);
        }
    }
}
