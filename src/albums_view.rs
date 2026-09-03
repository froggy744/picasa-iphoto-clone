use std::rc::Rc;

use gtk::prelude::*;
use gtk4 as gtk;
use rusqlite::Connection;

use crate::db::{self, Album};

pub fn build(
    albums: &[Album],
    connection: &Connection,
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
    content.append(&count);

    let cards = gtk::FlowBox::new();
    cards.set_selection_mode(gtk::SelectionMode::None);
    cards.set_min_children_per_line(1);
    cards.set_max_children_per_line(6);
    cards.set_row_spacing(20);
    cards.set_column_spacing(20);
    cards.set_hexpand(true);
    cards.set_vexpand(true);
    cards.add_css_class("albums-home-grid");
    content.append(&cards);
    scrolled.set_child(Some(&content));

    populate(&cards, &count, albums, connection, on_album);
    scrolled
}

pub fn refresh(
    scrolled: &gtk::ScrolledWindow,
    albums: &[Album],
    connection: &Connection,
    on_album: Rc<dyn Fn(i64)>,
) {
    let Some(content) = scrolled.child().and_downcast::<gtk::Box>() else {
        return;
    };
    let Some(count) = content
        .first_child()
        .and_then(|child| child.next_sibling().and_downcast::<gtk::Label>())
    else {
        return;
    };
    let Some(cards) = content.last_child().and_downcast::<gtk::FlowBox>() else {
        return;
    };
    populate(&cards, &count, albums, connection, on_album);
}

fn populate(
    cards: &gtk::FlowBox,
    count: &gtk::Label,
    albums: &[Album],
    connection: &Connection,
    on_album: Rc<dyn Fn(i64)>,
) {
    while let Some(child) = cards.first_child() {
        cards.remove(&child);
    }
    count.set_text(&format!("{} albums", albums.len()));

    if albums.is_empty() {
        let empty = gtk::Label::new(Some(
            "No albums yet\nCreate an album with the + button in the sidebar.",
        ));
        empty.set_xalign(0.0);
        empty.add_css_class("dim-label");
        cards.insert(&empty, -1);
        return;
    }

    for album in albums {
        cards.insert(&album_card(album, connection, on_album.clone()), -1);
    }
}

fn album_card(album: &Album, connection: &Connection, on_album: Rc<dyn Fn(i64)>) -> gtk::Button {
    let card = gtk::Button::new();
    card.set_has_frame(false);
    card.add_css_class("album-card");

    let content = gtk::Box::new(gtk::Orientation::Vertical, 7);
    content.set_width_request(180);

    let cover = gtk::Overlay::new();
    cover.set_size_request(180, 180);
    cover.add_css_class("album-cover");

    let picture = gtk::Picture::new();
    picture.set_content_fit(gtk::ContentFit::Cover);
    picture.set_can_shrink(true);
    picture.set_size_request(180, 180);

    let photos = db::photos_in_album(connection, album.id, None).unwrap_or_default();
    let cover_path = photos.iter().find_map(|photo| {
        crate::thumbnail::existing_cache_path(&photo.path, photo.mtime, photo.size_bytes)
            .ok()
            .flatten()
            .map(|path| path.to_string_lossy().into_owned())
    });
    eprintln!(
        "ALBUM TRACE id={} name={} photos={} cover={:?}",
        album.id,
        album.name,
        photos.len(),
        cover_path
    );

    if let Some(path) = cover_path.as_deref() {
        picture.set_filename(Some(path));
    } else {
        picture.set_paintable(gtk::gdk::Paintable::NONE);
    }
    cover.set_child(Some(&picture));

    let placeholder = gtk::Image::from_icon_name("folder-pictures-symbolic");
    placeholder.set_pixel_size(48);
    placeholder.set_halign(gtk::Align::Center);
    placeholder.set_valign(gtk::Align::Center);
    placeholder.set_visible(cover_path.is_none());
    cover.add_overlay(&placeholder);
    content.append(&cover);

    let name = gtk::Label::new(Some(&album.name));
    name.set_xalign(0.0);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    content.append(&name);

    let photo_count = gtk::Label::new(Some(&format!("{} photos", album.photo_count)));
    photo_count.set_xalign(0.0);
    photo_count.add_css_class("dim-label");
    content.append(&photo_count);

    card.set_child(Some(&content));
    let album_id = album.id;
    card.connect_clicked(move |_| on_album(album_id));
    card
}
