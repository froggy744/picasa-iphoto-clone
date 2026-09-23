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

struct PreviewImage {
    width: i32,
    height: i32,
    pixels: Vec<u8>,
}

struct Snapshot {
    data: db::LibraryHomeData,
    images: HashMap<i64, Arc<PreviewImage>>,
}

pub struct LibraryHome {
    pub root: gtk::ScrolledWindow,
}

#[cfg(test)]
#[path = "library_home_tests.rs"]
mod tests;

impl LibraryHome {
    pub fn new(database: std::path::PathBuf, navigate: Navigate, open: OpenPhoto) -> Self {
        let root = gtk::ScrolledWindow::new();
        root.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        root.set_hexpand(true);
        root.set_vexpand(true);
        let content = gtk::Box::new(gtk::Orientation::Vertical, 28);
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
            let section = gtk::Box::new(gtk::Orientation::Vertical, 12);
            let heading = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            let label = gtk::Label::new(Some(title));
            label.add_css_class("title-2");
            label.set_xalign(0.0);
            label.set_hexpand(true);
            heading.append(&label);
            let button = gtk::Button::with_label(action);
            button.add_css_class("flat");
            let navigate = navigate.clone();
            button.connect_clicked(move |_| navigate(destination));
            heading.append(&button);
            section.append(&heading);
            let row = gtk::FlowBox::new();
            row.set_selection_mode(gtk::SelectionMode::None);
            row.set_homogeneous(true);
            row.set_min_children_per_line(1);
            row.set_max_children_per_line(6);
            row.set_column_spacing(12);
            row.set_row_spacing(12);
            section.append(&row);
            content.append(&section);
            sections.push(row);
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

fn populate(sections: &[gtk::FlowBox], snapshot: &Snapshot, navigate: &Navigate, open: &OpenPhoto) {
    for section in sections {
        while let Some(child) = section.first_child() {
            section.remove(&child);
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
            let caption = photo
                .history_caption
                .clone()
                .unwrap_or_else(|| crate::source::filename(&photo.path));
            let button = card(&caption, None, snapshot.images.get(&photo.id));
            button.set_tooltip_text(Some(&format!(
                "{}\n{}",
                crate::source::filename(&photo.path),
                caption
            )));
            let open = open.clone();
            let objects = objects.clone();
            button.connect_clicked(move |_| open(objects.clone(), position, index == 1));
            sections[index].insert(&button, -1);
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
        let navigate = navigate.clone();
        let id = album.id;
        button.connect_clicked(move |_| navigate(SidebarFilter::Album(id)));
        sections[3].insert(&button, -1);
    }
    for (section, message) in sections.iter().zip([
        "No photos added yet",
        "No editing activity yet",
        "No favourites yet",
        "No albums yet",
    ]) {
        if section.first_child().is_none() {
            let label = gtk::Label::new(Some(message));
            label.add_css_class("dim-label");
            label.set_margin_top(24);
            label.set_margin_bottom(24);
            section.insert(&label, -1);
        }
    }
}

fn card(title: &str, subtitle: Option<&str>, image: Option<&Arc<PreviewImage>>) -> gtk::Button {
    let button = gtk::Button::new();
    button.add_css_class("flat");
    let content = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let frame = gtk::Overlay::new();
    frame.add_css_class("card");
    frame.set_size_request(140, 100);
    let picture = gtk::Picture::new();
    picture.set_can_shrink(true);
    picture.set_content_fit(gtk::ContentFit::Cover);
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
    let label = gtk::Label::new(Some(title));
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label.set_width_chars(18);
    label.set_max_width_chars(18);
    label.set_xalign(0.0);
    content.append(&label);
    if let Some(text) = subtitle {
        let label = gtk::Label::new(Some(text));
        label.add_css_class("dim-label");
        label.set_xalign(0.0);
        content.append(&label);
    }
    button.set_child(Some(&content));
    button
}
