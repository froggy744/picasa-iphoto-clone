use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{mpsc, Arc};
use std::time::Duration;

use gtk::prelude::*;
use gtk4 as gtk;

use crate::{db, photo_object::PhotoObject, sidebar::SidebarFilter, thumbnail_display};

type OpenPhoto = Rc<dyn Fn(Vec<PhotoObject>, usize, bool)>;
type Navigate = Rc<dyn Fn(SidebarFilter)>;

/// Square thumbnail edge length for every Home preview card.
const CARD_THUMB: i32 = 180;
/// Fixed gap between cards in a horizontal row (spec: 12–16 px).
const CARD_GAP: i32 = 14;

/// Home cards need exact geometry independent of theme button padding, so the
/// rules ship with the widget and load once per display (tests skip base.css).
const HOME_CSS: &str = r#"
button.home-card {
  padding: 0;
  min-width: 180px;
}
.home-thumb {
  min-width: 180px;
  min-height: 180px;
}
.home-card-caption { font-size: 12px; }
.home-card-caption-secondary { font-size: 11px; }
button.home-section-pan {
  min-width: 28px;
  min-height: 28px;
  padding: 2px;
}
"#;

fn install_home_css(display: &gtk::gdk::Display) {
    use std::sync::OnceLock;
    static LOADED: OnceLock<()> = OnceLock::new();
    if LOADED.get().is_some() {
        return;
    }
    let provider = gtk::CssProvider::new();
    provider.load_from_data(HOME_CSS);
    gtk::style_context_add_provider_for_display(
        display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 5,
    );
    let _ = LOADED.set(());
}

struct PreviewImage {
    width: i32,
    height: i32,
    pixels: Vec<u8>,
}

struct Snapshot {
    data: db::LibraryHomeData,
    images: HashMap<i64, Arc<PreviewImage>>,
}

struct HomeSection {
    scroller: gtk::ScrolledWindow,
    track: gtk::Box,
    prev: gtk::Button,
    next: gtk::Button,
}

pub struct LibraryHome {
    pub root: gtk::ScrolledWindow,
}

#[cfg(test)]
#[path = "library_home_tests.rs"]
mod tests;

impl LibraryHome {
    pub fn new(database: std::path::PathBuf, navigate: Navigate, open: OpenPhoto) -> Self {
        if let Some(display) = gtk::gdk::Display::default() {
            install_home_css(&display);
        }
        let root = gtk::ScrolledWindow::new();
        root.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        root.set_hexpand(true);
        root.set_vexpand(true);
        let content = gtk::Box::new(gtk::Orientation::Vertical, 32);
        content.set_margin_top(24);
        content.set_margin_bottom(24);
        content.set_margin_start(28);
        content.set_margin_end(28);
        let title = gtk::Label::new(Some("Library"));
        title.set_xalign(0.0);
        title.add_css_class("title-1");
        content.append(&title);
        let mut sections = Vec::new();
        for (title, action, destination) in [
            ("Recently Added", "View All", SidebarFilter::RecentlyAdded),
            ("Recently Edited", "View History", SidebarFilter::History),
            ("Favourites", "View All", SidebarFilter::Favorites),
            ("Albums", "View All", SidebarFilter::Albums),
        ] {
            let section = gtk::Box::new(gtk::Orientation::Vertical, 14);
            let heading = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            heading.set_valign(gtk::Align::Center);
            let label = gtk::Label::new(Some(title));
            label.add_css_class("title-2");
            label.set_xalign(0.0);
            label.set_hexpand(true);
            label.set_valign(gtk::Align::Center);
            heading.append(&label);

            let scroller = gtk::ScrolledWindow::new();
            scroller.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Never);
            scroller.set_hexpand(true);
            scroller.set_kinetic_scrolling(true);
            scroller.add_css_class("home-section-row");
            let track = gtk::Box::new(gtk::Orientation::Horizontal, CARD_GAP);
            track.add_css_class("home-section-track");
            track.set_valign(gtk::Align::Start);
            track.set_halign(gtk::Align::Start);
            scroller.set_child(Some(&track));

            let prev = gtk::Button::from_icon_name("pan-start-symbolic");
            prev.add_css_class("flat");
            prev.add_css_class("home-section-pan");
            prev.set_valign(gtk::Align::Center);
            prev.set_tooltip_text(Some("Scroll left"));
            prev.set_visible(false);
            let next = gtk::Button::from_icon_name("pan-end-symbolic");
            next.add_css_class("flat");
            next.add_css_class("home-section-pan");
            next.set_valign(gtk::Align::Center);
            next.set_tooltip_text(Some("Scroll right"));
            next.set_visible(false);
            wire_scroll_buttons(&scroller, &prev, &next);
            heading.append(&prev);
            heading.append(&next);

            let button = gtk::Button::with_label(action);
            button.add_css_class("flat");
            let navigate = navigate.clone();
            button.connect_clicked(move |_| navigate(destination));
            heading.append(&button);
            section.append(&heading);
            section.append(&scroller);
            content.append(&section);
            sections.push(HomeSection {
                scroller,
                track,
                prev,
                next,
            });
        }
        let status = gtk::Label::new(Some("Loading library…"));
        status.add_css_class("dim-label");
        content.append(&status);
        root.set_child(Some(&content));

        let (requests, receiver) = mpsc::sync_channel::<bool>(1);
        let (sender, snapshots) = mpsc::channel();
        std::thread::spawn(move || worker(database, receiver, sender));
        let pending = Rc::new(Cell::new(false));
        {
            let pending = pending.clone();
            let requests = requests.clone();
            root.connect_map(move |_| {
                if !pending.replace(true) && requests.try_send(true).is_err() {
                    pending.set(false);
                }
            });
        }
        let weak_root = root.downgrade();
        let mut ticks = 0;
        glib::timeout_add_local(Duration::from_millis(100), move || {
            let Some(root) = weak_root.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if let Ok(result) = snapshots.try_recv() {
                pending.set(false);
                match result {
                    Ok(Some(snapshot)) => {
                        populate(&sections, &snapshot, &navigate, &open);
                        status.set_visible(false);
                    }
                    Ok(None) => {}
                    Err(error) => {
                        status.set_text(&format!("Could not load library: {error}"));
                        status.set_visible(true);
                    }
                }
            }
            ticks += 1;
            if ticks >= 20 {
                ticks = 0;
                if root.is_mapped() && !pending.replace(true) && requests.try_send(false).is_err() {
                    pending.set(false);
                }
            }
            glib::ControlFlow::Continue
        });
        Self { root }
    }
}

fn wire_scroll_buttons(scroller: &gtk::ScrolledWindow, prev: &gtk::Button, next: &gtk::Button) {
    {
        let scroller = scroller.clone();
        prev.connect_clicked(move |_| scroll_row(&scroller, -1.0));
    }
    {
        let scroller = scroller.clone();
        next.connect_clicked(move |_| scroll_row(&scroller, 1.0));
    }
    let hadj = scroller.hadjustment();
    let prev = prev.clone();
    let next = next.clone();
    let update = {
        let hadj = hadj.clone();
        let prev = prev.clone();
        let next = next.clone();
        Rc::new(move || {
            let max = (hadj.upper() - hadj.page_size()).max(hadj.lower());
            let scrollable = max > hadj.lower() + 1.0;
            let value = hadj.value();
            prev.set_visible(scrollable);
            next.set_visible(scrollable);
            prev.set_sensitive(scrollable && value > hadj.lower() + 0.5);
            next.set_sensitive(scrollable && value < max - 0.5);
        })
    };
    update();
    hadj.connect_value_changed({
        let update = update.clone();
        move |_| update()
    });
    hadj.connect_changed({
        let update = update.clone();
        move |_| update()
    });
}

fn scroll_row(scroller: &gtk::ScrolledWindow, direction: f64) {
    let hadj = scroller.hadjustment();
    let step = (hadj.page_size() * 0.75).max(160.0);
    let max = (hadj.upper() - hadj.page_size()).max(hadj.lower());
    let target = (hadj.value() + direction * step).clamp(hadj.lower(), max);
    hadj.set_value(target);
}

fn worker(
    database: std::path::PathBuf,
    requests: mpsc::Receiver<bool>,
    sender: mpsc::Sender<Result<Option<Snapshot>, String>>,
) {
    // Open read-only: no schema migration, folder probing or startup scans.
    let connection =
        rusqlite::Connection::open_with_flags(database, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY);
    let mut version = None;
    let mut cache = HashMap::<String, Arc<PreviewImage>>::new();
    let mut missing = false;
    while let Ok(force) = requests.recv() {
        let result = (|| -> anyhow::Result<Option<Snapshot>> {
            let connection = connection
                .as_ref()
                .map_err(|error| anyhow::anyhow!(error.to_string()))?;
            let current: i64 = connection.query_row("PRAGMA data_version", [], |row| row.get(0))?;
            if !force && version == Some(current) && !missing {
                return Ok(None);
            }
            let data = db::library_home_data(connection)?;
            let mut images = HashMap::new();
            let mut used = std::collections::HashSet::new();
            let mut loaded = false;
            missing = false;
            for photo in data
                .added
                .iter()
                .chain(&data.edited)
                .chain(&data.favorites)
                .chain(data.albums.iter().filter_map(|(_, cover)| cover.as_ref()))
            {
                let path =
                    crate::thumbnail::cache_path(&photo.path, photo.mtime, photo.size_bytes)?;
                let request = thumbnail_display::request_for(
                    path.to_string_lossy().into_owned(),
                    photo.path.clone(),
                    photo.mtime.unwrap_or_default(),
                    photo.size_bytes.unwrap_or_default(),
                    photo.rotation,
                    photo.edit_recipe.clone(),
                    photo.width.unwrap_or_default(),
                    photo.height.unwrap_or_default(),
                    false,
                );
                used.insert(request.key.clone());
                if !cache.contains_key(&request.key) {
                    if let thumbnail_display::DisplayOutcome::Loaded {
                        width,
                        height,
                        pixels,
                    } = thumbnail_display::load_cached_display_thumbnail(&request)
                    {
                        cache.insert(
                            request.key.clone(),
                            Arc::new(PreviewImage {
                                width,
                                height,
                                pixels,
                            }),
                        );
                        loaded = true;
                    } else {
                        missing = true;
                    }
                }
                if let Some(image) = cache.get(&request.key) {
                    images.insert(photo.id, image.clone());
                }
            }
            cache.retain(|key, _| used.contains(key));
            let changed = force || version != Some(current) || loaded;
            version = Some(current);
            Ok(changed.then_some(Snapshot { data, images }))
        })()
        .map_err(|error| error.to_string());
        if sender.send(result).is_err() {
            break;
        }
    }
}

fn populate(sections: &[HomeSection], snapshot: &Snapshot, navigate: &Navigate, open: &OpenPhoto) {
    let positions: Vec<f64> = sections
        .iter()
        .map(|section| section.scroller.hadjustment().value())
        .collect();
    for section in sections {
        while let Some(child) = section.track.first_child() {
            section.track.remove(&child);
        }
    }
    for (index, photos) in [
        &snapshot.data.added,
        &snapshot.data.edited,
        &snapshot.data.favorites,
    ]
    .into_iter()
    .enumerate()
    {
        let objects: Vec<_> = photos.iter().map(PhotoObject::from_photo).collect();
        for (position, photo) in photos.iter().enumerate() {
            let filename = crate::source::filename(&photo.path);
            let image = snapshot.images.get(&photo.id);
            // Recently Edited keeps the filename as the primary caption and the
            // history line ("Photo · …" / "Collage · …") as smaller secondary
            // text so saved collages stay distinguishable from photographs.
            let button = if index == 1 {
                let secondary = photo.history_caption.as_deref().map(short_edit_line);
                let collage = secondary
                    .as_deref()
                    .map(|line| line.starts_with("Collage"))
                    .unwrap_or(false);
                let button = card(&filename, secondary.as_deref(), image);
                if collage {
                    button.add_css_class("home-collage");
                }
                button
            } else {
                card(&filename, None, image)
            };
            button.set_tooltip_text(Some(&filename));
            let open = open.clone();
            let objects = objects.clone();
            button.connect_clicked(move |_| open(objects.clone(), position, index == 1));
            sections[index].track.append(&button);
        }
    }
    for (album, cover) in &snapshot.data.albums {
        let button = card(
            &album.name,
            Some(&format!("{} photos", album.photo_count)),
            cover
                .as_ref()
                .and_then(|photo| snapshot.images.get(&photo.id)),
        );
        button.set_tooltip_text(Some(&album.name));
        let navigate = navigate.clone();
        let id = album.id;
        button.connect_clicked(move |_| navigate(SidebarFilter::Album(id)));
        sections[3].track.append(&button);
    }
    for (section, message) in sections.iter().zip([
        "No photos added yet",
        "No editing activity yet",
        "No favourites yet",
        "No albums yet",
    ]) {
        if section.track.first_child().is_none() {
            let label = gtk::Label::new(Some(message));
            label.add_css_class("dim-label");
            label.set_margin_top(24);
            label.set_margin_bottom(24);
            section.track.append(&label);
        }
    }
    restore_row_positions(sections, &positions);
}

/// Reapply row scroll offsets after children are rebuilt. The adjustment upper
/// is only correct once the new cards have been measured, so retry shortly
/// after the rebuild instead of assuming the value sticks immediately.
fn restore_row_positions(sections: &[HomeSection], positions: &[f64]) {
    for (section, position) in sections.iter().zip(positions) {
        let position = *position;
        if position <= 0.0 {
            continue;
        }
        let hadj = section.scroller.hadjustment();
        apply_position(&hadj, position);
        let apply = {
            let hadj = hadj.clone();
            Rc::new(move || apply_position(&hadj, position))
        };
        // One idle tick plus a later tick cover measure/allocate after rebuild.
        glib::timeout_add_local_once(Duration::from_millis(16), {
            let apply = apply.clone();
            move || apply()
        });
        glib::timeout_add_local_once(Duration::from_millis(50), move || apply());
    }
}

fn apply_position(hadj: &gtk::Adjustment, position: f64) {
    let max = (hadj.upper() - hadj.page_size()).max(hadj.lower());
    if position <= max {
        hadj.set_value(position);
    }
}

fn card(title: &str, subtitle: Option<&str>, image: Option<&Arc<PreviewImage>>) -> gtk::Button {
    let button = gtk::Button::new();
    button.add_css_class("flat");
    button.add_css_class("home-card");
    // Left-align in the track; never expand or distribute across the window.
    button.set_hexpand(false);
    button.set_vexpand(false);
    button.set_halign(gtk::Align::Start);
    button.set_valign(gtk::Align::Start);
    let content = gtk::Box::new(gtk::Orientation::Vertical, 6);
    content.set_halign(gtk::Align::Start);
    content.set_hexpand(false);
    content.set_size_request(CARD_THUMB, -1);
    let frame = gtk::Overlay::new();
    frame.add_css_class("card");
    frame.add_css_class("home-thumb");
    // Square thumbnail so every preview card shares one size, regardless of
    // the cached image's natural dimensions or the caption length.
    frame.set_size_request(CARD_THUMB, CARD_THUMB);
    frame.set_hexpand(false);
    frame.set_vexpand(false);
    frame.set_halign(gtk::Align::Fill);
    frame.set_valign(gtk::Align::Start);
    frame.set_overflow(gtk::Overflow::Hidden);
    let picture = gtk::Picture::new();
    picture.set_can_shrink(true);
    picture.set_content_fit(gtk::ContentFit::Cover);
    picture.set_size_request(1, 1);
    picture.set_hexpand(true);
    picture.set_vexpand(true);
    picture.set_halign(gtk::Align::Fill);
    picture.set_valign(gtk::Align::Fill);
    picture.add_css_class("thumbnail");
    if let Some(image) = image {
        let texture = gtk::gdk::MemoryTexture::new(
            image.width,
            image.height,
            gtk::gdk::MemoryFormat::R8g8b8a8,
            &glib::Bytes::from_owned(image.pixels.clone()),
            image.width as usize * 4,
        );
        picture.set_paintable(Some(&texture));
    }
    // Keep card dimensions independent of the cache image's natural size.
    let backing = gtk::Box::new(gtk::Orientation::Vertical, 0);
    frame.set_child(Some(&backing));
    frame.add_overlay(&picture);
    if image.is_none() {
        let placeholder = gtk::Image::from_icon_name("image-x-generic-symbolic");
        placeholder.set_pixel_size(32);
        placeholder.add_css_class("dim-label");
        frame.add_overlay(&placeholder);
    }
    content.append(&frame);
    content.append(&caption_label(title, false));
    if let Some(text) = subtitle {
        content.append(&caption_label(text, true));
    }
    button.set_child(Some(&content));
    button
}

/// Compact caption under a thumbnail. Ellipsizes only when the parent width
/// forces it; the button tooltip carries the complete text.
fn caption_label(text: &str, secondary: bool) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label.set_xalign(0.0);
    label.set_halign(gtk::Align::Fill);
    label.set_hexpand(true);
    // Cap natural width so long filenames cannot stretch the card past 180px.
    label.set_max_width_chars(if secondary { 26 } else { 22 });
    if secondary {
        label.add_css_class("dim-label");
        label.add_css_class("home-card-caption-secondary");
    } else {
        label.add_css_class("home-card-caption");
    }
    label
}

/// History lines arrive as "Photo · YYYY-MM-DD HH:MM:SS". Drop the seconds so
/// the secondary caption fits the fixed card width while keeping the kind
/// (Photo/Collage) that distinguishes saved collages from photographs.
fn short_edit_line(line: &str) -> String {
    if line.len() >= 3 && line.as_bytes()[line.len() - 3] == b':' {
        line[..line.len() - 3].to_string()
    } else {
        line.to_string()
    }
}
