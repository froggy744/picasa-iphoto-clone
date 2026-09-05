use std::rc::Rc;

use gtk::prelude::*;
use gtk4 as gtk;
use rusqlite::Connection;

use crate::db::{self, Album};

const DEFAULT_THUMBNAIL_WIDTH: i32 = 136;
const DEFAULT_THUMBNAIL_HEIGHT: i32 = 91;

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
    // The ScrolledWindow may expose a GTK viewport around its content. Find
    // the existing widgets by their local CSS markers instead of assuming
    // that the content box is the direct child.
    let root = scrolled.clone().upcast::<gtk::Widget>();
    let Some(count) = find_descendant_with_css_class(&root, "albums-home-count")
        .and_then(|widget| widget.downcast::<gtk::Label>().ok())
    else {
        eprintln!("ALBUM VIEW TRACE refresh_failed reason=count_missing");
        return;
    };
    let Some(cards) = find_descendant_with_css_class(&root, "albums-home-grid")
        .and_then(|widget| widget.downcast::<gtk::FlowBox>().ok())
    else {
        eprintln!("ALBUM VIEW TRACE refresh_failed reason=grid_missing");
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

    for album in albums {
        let card = album_card(album, connection, thumbnail_width, on_album.clone());
        insert_child(cards, &card, Some(cover_width(thumbnail_width)));
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
) -> gtk::Button {
    let card = gtk::Button::new();
    card.set_has_frame(false);
    card.set_hexpand(false);
    card.set_vexpand(false);
    card.set_halign(gtk::Align::Start);
    card.set_valign(gtk::Align::Start);
    card.add_css_class("album-card");

    let width = cover_width(thumbnail_width);
    let height = cover_height(thumbnail_width);
    card.set_width_request(width);
    let content = gtk::Box::new(gtk::Orientation::Vertical, 7);
    content.set_size_request(width, -1);
    content.set_hexpand(false);
    content.set_vexpand(false);
    content.set_halign(gtk::Align::Start);

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
    cover.add_css_class("photo-frame");
    cover.add_css_class("photo-tile");

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
            crate::photo_texture::rotated_thumbnail(&path_string, photo.rotation)
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
