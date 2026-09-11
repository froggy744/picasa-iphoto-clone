use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::prelude::*;
use gtk4 as gtk;
use rusqlite::Connection;

use crate::db::{self, Album};
#[cfg(test)]
use crate::settings::AlbumViewStyle;
use crate::settings::{self, AlbumAppearance};

const DEFAULT_THUMBNAIL_WIDTH: i32 = 136;
const DEFAULT_THUMBNAIL_HEIGHT: i32 = 91;
const BOOKSHELF_ROW_HEIGHT: i32 = 288;
const BOOKSHELF_SURFACE_Y: i32 = 235;
const BOOKSHELF_COUNT_CSS: &str = ".albums-bookshelf-photo-count { color: #3a210f; }";
const BOOKSHELF_BACKGROUND_NAMES: [&str; 3] = ["bookshelf.png", "bookshelf2.png", "bookshel3f.png"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AlbumAppearanceAction {
    ToggleBookshelf,
    NextBackground,
    ToggleCovers,
    DisableAll,
    OpenThemeSettings,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct AlbumContextMenuItem {
    label: &'static str,
    action: AlbumAppearanceAction,
    checked: Option<bool>,
    sensitive: bool,
}

fn album_context_menu_items(appearance: AlbumAppearance) -> Vec<AlbumContextMenuItem> {
    vec![
        AlbumContextMenuItem {
            label: "Bookshelf",
            action: AlbumAppearanceAction::ToggleBookshelf,
            checked: Some(appearance.bookshelf_enabled),
            sensitive: true,
        },
        AlbumContextMenuItem {
            label: "Next Background",
            action: AlbumAppearanceAction::NextBackground,
            checked: None,
            sensitive: true,
        },
        AlbumContextMenuItem {
            label: "Album Covers",
            action: AlbumAppearanceAction::ToggleCovers,
            checked: Some(appearance.covers_enabled),
            sensitive: true,
        },
        AlbumContextMenuItem {
            label: "Disable All Themes",
            action: AlbumAppearanceAction::DisableAll,
            checked: None,
            sensitive: appearance.bookshelf_enabled || appearance.covers_enabled,
        },
        AlbumContextMenuItem {
            label: "Theme Settings…",
            action: AlbumAppearanceAction::OpenThemeSettings,
            checked: None,
            sensitive: true,
        },
    ]
}

// Common opening with a small underlap beneath both supplied skins' opaque edges.
const PHOTO_X_RATIO: f64 = 0.18;
const PHOTO_Y_RATIO: f64 = 53.0 / 500.0;
const PHOTO_WIDTH_RATIO: f64 = 0.64;
const PHOTO_HEIGHT_RATIO: f64 = 273.0 / 500.0;

#[cfg(test)]
fn album_frame_paths() -> Vec<PathBuf> {
    album_frame_paths_in(Path::new("images"))
}

#[cfg(test)]
fn album_cover_paths() -> Vec<PathBuf> {
    album_cover_paths_in(Path::new("images"))
}

#[cfg(test)]
fn album_frame_paths_for_style(style: AlbumViewStyle) -> Vec<PathBuf> {
    match style {
        AlbumViewStyle::Default => Vec::new(),
        AlbumViewStyle::Bookshelf => album_frame_paths(),
        AlbumViewStyle::AlbumCovers => album_cover_paths(),
    }
}

fn album_frame_paths_for_appearance(directory: &Path, appearance: AlbumAppearance) -> Vec<PathBuf> {
    if !appearance.covers_enabled {
        Vec::new()
    } else if appearance.bookshelf_enabled {
        album_frame_paths_in(directory)
    } else {
        album_cover_paths_in(directory)
    }
}

fn bookshelf_background_path(directory: &Path, index: usize) -> PathBuf {
    directory.join(BOOKSHELF_BACKGROUND_NAMES[index % BOOKSHELF_BACKGROUND_NAMES.len()])
}

fn bookshelf_card_margin_top(cover_height: i32) -> i32 {
    (BOOKSHELF_SURFACE_Y - cover_height).max(0)
}

fn uses_responsive_bookshelf(appearance: AlbumAppearance) -> bool {
    appearance.bookshelf_enabled && appearance.background_index == 0
}

fn bookshelf_background_rules(directory: &Path) -> String {
    BOOKSHELF_BACKGROUND_NAMES
        .iter()
        .enumerate()
        .map(|(index, _)| {
            let uri = gio::File::for_path(bookshelf_background_path(directory, index)).uri();
            if index == 0 {
                format!(
                    ".albums-bookshelf-{index} {{ \
                     background-image: url(\"{uri}\"); background-size: 100% {BOOKSHELF_ROW_HEIGHT}px; \
                     background-repeat: repeat-y; background-position: center top; }}"
                )
            } else {
                format!(
                    ".albums-bookshelf-{index} {{ background-image: url(\"{uri}\"); \
                     background-size: 100% auto; background-repeat: repeat-y; \
                     background-position: center top; }}"
                )
            }
        })
        .collect()
}

fn bookshelf_background_css_class(appearance: AlbumAppearance) -> Option<String> {
    appearance.bookshelf_enabled.then(|| {
        format!(
            "albums-bookshelf-{}",
            appearance.background_index % BOOKSHELF_BACKGROUND_NAMES.len()
        )
    })
}

#[cfg(test)]
fn uses_bookshelf_background(style: AlbumViewStyle) -> bool {
    style == AlbumViewStyle::Bookshelf
}

fn album_frame_paths_in(directory: &Path) -> Vec<PathBuf> {
    png_paths_matching(directory, |name| name.ends_with("-album.png"))
}

fn album_cover_paths_in(directory: &Path) -> Vec<PathBuf> {
    png_paths_matching(directory, |name| {
        name == "album-cover.png"
            || name
                .strip_prefix("album-cover")
                .is_some_and(|suffix| suffix.ends_with(".png"))
    })
}

fn png_paths_matching(directory: &Path, matches: impl Fn(&str) -> bool) -> Vec<PathBuf> {
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
                    .is_some_and(&matches)
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
    connection: Rc<RefCell<Connection>>,
    thumbnail_width: i32,
    on_album: Rc<dyn Fn(i64)>,
    on_appearance_changed: Rc<dyn Fn()>,
    open_theme_settings: Rc<dyn Fn()>,
) -> gtk::ScrolledWindow {
    let scrolled = gtk::ScrolledWindow::new();
    scrolled.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scrolled.set_hexpand(true);
    scrolled.set_vexpand(true);

    let background = gtk::CssProvider::new();
    let background_rules = bookshelf_background_rules(Path::new("images"));
    background.load_from_string(&background_rules);
    scrolled
        .style_context()
        .add_provider(&background, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 2);

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
    cards
        .style_context()
        .add_provider(&background, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 2);

    content.append(&cards);
    scrolled.set_child(Some(&content));
    refresh_presentation(&scrolled, &connection.borrow());

    populate(
        &cards,
        &count,
        albums,
        &connection.borrow(),
        thumbnail_width,
        on_album.clone(),
    );

    install_context_menu(
        &scrolled,
        connection,
        on_appearance_changed,
        open_theme_settings,
    );

    scrolled
}

fn install_context_menu(
    scrolled: &gtk::ScrolledWindow,
    connection: Rc<RefCell<Connection>>,
    on_appearance_changed: Rc<dyn Fn()>,
    open_theme_settings: Rc<dyn Fn()>,
) {
    let click = gtk::GestureClick::new();
    click.set_button(gtk::gdk::BUTTON_SECONDARY);
    click.set_propagation_phase(gtk::PropagationPhase::Capture);
    let anchor = scrolled.clone();
    click.connect_pressed(move |_, _, x, y| {
        show_context_menu(
            &anchor,
            x,
            y,
            connection.clone(),
            on_appearance_changed.clone(),
            open_theme_settings.clone(),
        );
    });
    scrolled.add_controller(click);
}

fn show_context_menu(
    anchor: &gtk::ScrolledWindow,
    x: f64,
    y: f64,
    connection: Rc<RefCell<Connection>>,
    on_appearance_changed: Rc<dyn Fn()>,
    open_theme_settings: Rc<dyn Fn()>,
) {
    let appearance = settings::album_appearance(&connection.borrow());
    let popover = gtk::Popover::new();
    popover.set_autohide(true);
    popover.set_has_arrow(false);
    popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(
        x.round() as i32,
        y.round() as i32,
        1,
        1,
    )));

    let menu = gtk::Box::new(gtk::Orientation::Vertical, 2);
    menu.set_margin_top(6);
    menu.set_margin_bottom(6);
    menu.set_margin_start(6);
    menu.set_margin_end(6);

    let heading = gtk::Label::new(Some("Album Appearance"));
    heading.set_xalign(0.0);
    heading.set_margin_start(8);
    heading.set_margin_end(8);
    heading.set_margin_top(4);
    heading.set_margin_bottom(4);
    heading.add_css_class("heading");
    menu.append(&heading);

    for (index, item) in album_context_menu_items(appearance).into_iter().enumerate() {
        if index == 3 || index == 4 {
            menu.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        }
        let button = gtk::Button::new();
        button.set_halign(gtk::Align::Fill);
        button.set_sensitive(item.sensitive);
        button.add_css_class("flat");

        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        if let Some(checked) = item.checked {
            let mark = gtk::Image::from_icon_name("object-select-symbolic");
            mark.set_visible(checked);
            mark.set_size_request(16, 16);
            row.append(&mark);
        } else {
            let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            spacer.set_size_request(16, 16);
            row.append(&spacer);
        }
        let label = gtk::Label::new(Some(item.label));
        label.set_xalign(0.0);
        label.set_hexpand(true);
        row.append(&label);
        button.set_child(Some(&row));

        let popover_for_action = popover.clone();
        let connection = connection.clone();
        let on_appearance_changed = on_appearance_changed.clone();
        let open_theme_settings = open_theme_settings.clone();
        button.connect_clicked(move |_| {
            popover_for_action.popdown();
            if item.action == AlbumAppearanceAction::OpenThemeSettings {
                open_theme_settings();
                return;
            }
            let result = match item.action {
                AlbumAppearanceAction::ToggleBookshelf => settings::set_bookshelf_enabled(
                    &connection.borrow(),
                    !settings::album_appearance(&connection.borrow()).bookshelf_enabled,
                ),
                AlbumAppearanceAction::NextBackground => {
                    settings::next_bookshelf_background(&connection.borrow()).map(|_| ())
                }
                AlbumAppearanceAction::ToggleCovers => settings::set_covers_enabled(
                    &connection.borrow(),
                    !settings::album_appearance(&connection.borrow()).covers_enabled,
                ),
                AlbumAppearanceAction::DisableAll => {
                    settings::disable_all_album_themes(&connection.borrow())
                }
                AlbumAppearanceAction::OpenThemeSettings => unreachable!(),
            };
            if let Err(error) = result {
                eprintln!("Could not update album appearance: {error}");
                return;
            }
            on_appearance_changed();
        });
        menu.append(&button);
    }

    popover.set_child(Some(&menu));
    popover.set_parent(anchor);
    popover.connect_closed(|popover| popover.unparent());
    popover.popup();
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
    let appearance = settings::album_appearance(connection);
    let cards = find_descendant_with_css_class(scrolled.upcast_ref(), "albums-home-grid");
    for index in 0..BOOKSHELF_BACKGROUND_NAMES.len() {
        scrolled.remove_css_class(&format!("albums-bookshelf-{index}"));
        if let Some(cards) = cards.as_ref() {
            cards.remove_css_class(&format!("albums-bookshelf-{index}"));
        }
    }
    if let Some(background_class) = bookshelf_background_css_class(appearance) {
        scrolled.add_css_class("albums-bookshelf");
        if uses_responsive_bookshelf(appearance) {
            if let Some(cards) = cards {
                cards.add_css_class(&background_class);
            }
        } else {
            scrolled.add_css_class(&background_class);
        }
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

    let appearance = settings::album_appearance(connection);
    let responsive_bookshelf = uses_responsive_bookshelf(appearance);
    cards.set_vexpand(responsive_bookshelf);
    cards.set_valign(if responsive_bookshelf {
        gtk::Align::Fill
    } else {
        gtk::Align::Start
    });
    let frames: Vec<_> = album_frame_paths_for_appearance(Path::new("images"), appearance)
        .iter()
        .filter_map(|path| gtk::gdk::Texture::from_filename(path).ok())
        .collect();
    let framed = !frames.is_empty();
    cards.set_row_spacing(if responsive_bookshelf {
        0
    } else if framed {
        28
    } else {
        20
    });
    cards.set_column_spacing(if framed { 24 } else { 20 });

    if albums.is_empty() {
        let empty = gtk::Label::new(Some(
            "No albums yet\nCreate an album with the + button in the sidebar.",
        ));
        empty.set_xalign(0.0);
        empty.add_css_class("dim-label");
        insert_child(cards, &empty, None, None);
        return;
    }

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
        let actual_cover_height = if frame.is_some() {
            frame_height
        } else {
            cover_height(thumbnail_width)
        };
        let card = album_card(
            album,
            connection,
            thumbnail_width,
            on_album.clone(),
            frame,
            responsive_bookshelf,
        );
        if framed {
            card.set_width_request(card_width);
        }
        if responsive_bookshelf {
            card.set_margin_top(bookshelf_card_margin_top(actual_cover_height));
        }
        insert_child(
            cards,
            &card,
            Some(card_width),
            responsive_bookshelf.then_some(BOOKSHELF_ROW_HEIGHT),
        );
    }
}

fn insert_child(
    cards: &gtk::FlowBox,
    child: &impl IsA<gtk::Widget>,
    width: Option<i32>,
    height: Option<i32>,
) {
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
        if let Some(height) = height {
            flow_child.set_height_request(height);
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
    responsive_bookshelf: bool,
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
    if responsive_bookshelf {
        photo_count.add_css_class("albums-bookshelf-photo-count");
        let style = gtk::CssProvider::new();
        style.load_from_string(BOOKSHELF_COUNT_CSS);
        photo_count
            .style_context()
            .add_provider(&style, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 2);
    }
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

    fn settle_gtk_layout() {
        let main_loop = gtk::glib::MainLoop::new(None, false);
        let loop_to_quit = main_loop.clone();
        gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(50), move || {
            loop_to_quit.quit();
        });
        main_loop.run();
    }

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
    fn discovers_album_cover_png_files_sorted_and_picks_up_new_covers() {
        let directory = std::env::temp_dir().join(format!(
            "pic-album-covers-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        assert!(album_cover_paths_in(&directory).is_empty());
        std::fs::create_dir(&directory).unwrap();
        assert!(album_cover_paths_in(&directory).is_empty());
        for name in [
            "album-cover.png",
            "album-cover-blue.png",
            "album-cover-Big.png",
            "album-cover-green.png",
            "pink-album.png",
            "cover.png",
            "album-cover.jpg",
            "album-cover.png.bak",
        ] {
            std::fs::write(directory.join(name), []).unwrap();
        }
        std::fs::create_dir(directory.join("album-cover-folder.png")).unwrap();
        let expected: Vec<_> = [
            "album-cover-Big.png",
            "album-cover-blue.png",
            "album-cover-green.png",
            "album-cover.png",
        ]
        .map(|name| directory.join(name))
        .into();
        assert_eq!(album_cover_paths_in(&directory), expected);

        std::fs::write(directory.join("album-cover-pink.png"), []).unwrap();
        let mut expected = expected;
        expected.push(directory.join("album-cover-pink.png"));
        expected.sort();
        assert_eq!(album_cover_paths_in(&directory), expected);
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
        let album_cover_paths = album_frame_paths_for_style(AlbumViewStyle::AlbumCovers);
        assert!(
            album_cover_paths
                .windows(2)
                .all(|paths| paths[0] <= paths[1])
        );
        assert!(album_cover_paths.contains(&PathBuf::from("images/album-cover.png")));
        assert!(album_cover_paths.contains(&PathBuf::from("images/album-cover-blue.png")));
        assert!(!album_cover_paths.contains(&PathBuf::from("images/pink-album.png")));
        assert!(!uses_bookshelf_background(AlbumViewStyle::Default));
        assert!(uses_bookshelf_background(AlbumViewStyle::Bookshelf));
        assert!(!uses_bookshelf_background(AlbumViewStyle::AlbumCovers));
    }

    #[test]
    fn independent_appearance_selects_background_and_cover_assets() {
        use crate::settings::AlbumAppearance;

        let directory = std::env::temp_dir().join(format!(
            "pic-album-appearance-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        for name in [
            "white-album.png",
            "album-cover-blue.png",
            "bookshelf.jpg",
            "bookshelf2.png",
            "bookshel3f.png",
        ] {
            std::fs::write(directory.join(name), []).unwrap();
        }

        let bookshelf = AlbumAppearance {
            bookshelf_enabled: true,
            covers_enabled: true,
            background_index: 2,
        };
        assert_eq!(
            album_frame_paths_for_appearance(&directory, bookshelf),
            vec![directory.join("white-album.png")]
        );
        assert_eq!(
            bookshelf_background_path(&directory, bookshelf.background_index),
            directory.join("bookshel3f.png")
        );
        assert_eq!(
            bookshelf_background_css_class(bookshelf).as_deref(),
            Some("albums-bookshelf-2")
        );

        let plain_covers = AlbumAppearance {
            bookshelf_enabled: false,
            covers_enabled: true,
            background_index: 1,
        };
        assert_eq!(
            album_frame_paths_for_appearance(&directory, plain_covers),
            vec![directory.join("album-cover-blue.png")]
        );
        assert_eq!(bookshelf_background_css_class(plain_covers), None);

        let disabled = AlbumAppearance {
            bookshelf_enabled: true,
            covers_enabled: false,
            background_index: 0,
        };
        assert!(album_frame_paths_for_appearance(&directory, disabled).is_empty());

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn responsive_bookshelf_geometry_keeps_cover_bottoms_on_the_surface() {
        for (cover_height, expected_top) in [(91, 144), (137, 98), (186, 49)] {
            let top = bookshelf_card_margin_top(cover_height);
            assert_eq!(top, expected_top);
            assert_eq!(top + cover_height, 235);
        }

        assert_eq!(BOOKSHELF_ROW_HEIGHT, 288);
        assert!(BOOKSHELF_SURFACE_Y < BOOKSHELF_ROW_HEIGHT);

        assert!(uses_responsive_bookshelf(AlbumAppearance {
            bookshelf_enabled: true,
            covers_enabled: false,
            background_index: 0,
        }));
        assert!(!uses_responsive_bookshelf(AlbumAppearance {
            bookshelf_enabled: true,
            covers_enabled: false,
            background_index: 1,
        }));
        assert!(!uses_responsive_bookshelf(AlbumAppearance {
            bookshelf_enabled: false,
            covers_enabled: true,
            background_index: 0,
        }));
    }

    #[test]
    fn bookshelf_background_rules_use_fixed_row_height() {
        let directory = Path::new("/tmp/pic-rs theme assets");
        assert_eq!(
            bookshelf_background_path(directory, 0),
            directory.join("bookshelf.png")
        );

        let rules = bookshelf_background_rules(directory);
        assert!(rules.contains(".albums-bookshelf-0 {"));
        assert_eq!(rules.matches("background-size: 100% 288px").count(), 1);
        assert_eq!(rules.matches("background-size: 100% auto").count(), 2);
        assert!(BOOKSHELF_COUNT_CSS.contains(".albums-bookshelf-photo-count"));
        assert!(BOOKSHELF_COUNT_CSS.contains("color: #3a210f"));
    }

    #[test]
    fn context_menu_model_exposes_theme_controls_and_current_state() {
        use crate::settings::AlbumAppearance;

        let items = album_context_menu_items(AlbumAppearance {
            bookshelf_enabled: true,
            covers_enabled: false,
            background_index: 1,
        });

        assert_eq!(
            items,
            vec![
                AlbumContextMenuItem {
                    label: "Bookshelf",
                    action: AlbumAppearanceAction::ToggleBookshelf,
                    checked: Some(true),
                    sensitive: true,
                },
                AlbumContextMenuItem {
                    label: "Next Background",
                    action: AlbumAppearanceAction::NextBackground,
                    checked: None,
                    sensitive: true,
                },
                AlbumContextMenuItem {
                    label: "Album Covers",
                    action: AlbumAppearanceAction::ToggleCovers,
                    checked: Some(false),
                    sensitive: true,
                },
                AlbumContextMenuItem {
                    label: "Disable All Themes",
                    action: AlbumAppearanceAction::DisableAll,
                    checked: None,
                    sensitive: true,
                },
                AlbumContextMenuItem {
                    label: "Theme Settings…",
                    action: AlbumAppearanceAction::OpenThemeSettings,
                    checked: None,
                    sensitive: true,
                },
            ]
        );

        let disabled = album_context_menu_items(AlbumAppearance::default());
        assert!(!disabled[3].sensitive);
    }

    #[test]
    #[ignore = "requires a GTK display; run with --ignored --test-threads=1"]
    fn context_menu_renders_and_applies_the_next_background_action() {
        fn find_buttons(widget: &gtk::Widget, buttons: &mut Vec<gtk::Button>) {
            if let Some(button) = widget.downcast_ref::<gtk::Button>() {
                buttons.push(button.clone());
            }
            let mut child = widget.first_child();
            while let Some(widget) = child {
                find_buttons(&widget, buttons);
                child = widget.next_sibling();
            }
        }

        gtk::init().unwrap();
        let connection = Rc::new(RefCell::new(Connection::open_in_memory().unwrap()));
        connection
            .borrow()
            .execute_batch("CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL)")
            .unwrap();
        let changed = Rc::new(std::cell::Cell::new(0));
        let changed_for_callback = changed.clone();
        let view = gtk::ScrolledWindow::new();
        let window = gtk::Window::new();
        window.set_child(Some(&view));
        window.present();

        show_context_menu(
            &view,
            20.0,
            20.0,
            connection.clone(),
            Rc::new(move || changed_for_callback.set(changed_for_callback.get() + 1)),
            Rc::new(|| {}),
        );
        let mut buttons = Vec::new();
        find_buttons(view.upcast_ref(), &mut buttons);
        assert_eq!(buttons.len(), 5);
        buttons[1].emit_clicked();
        assert_eq!(changed.get(), 1);
        assert_eq!(
            settings::album_appearance(&connection.borrow()),
            AlbumAppearance {
                bookshelf_enabled: true,
                covers_enabled: false,
                background_index: 1,
            }
        );

        window.close();
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
        let connection = Rc::new(RefCell::new(
            db::open(&directory.join("library.db")).unwrap(),
        ));
        let album = db::create_album(&connection.borrow(), "Holiday").unwrap();
        let albums = db::albums(&connection.borrow()).unwrap();
        let opened = Rc::new(std::cell::Cell::new(None));
        let opened_for_callback = opened.clone();
        let on_album: Rc<dyn Fn(i64)> = Rc::new(move |id| opened_for_callback.set(Some(id)));
        let view = build(
            &albums,
            connection.clone(),
            136,
            on_album.clone(),
            Rc::new(|| {}),
            Rc::new(|| {}),
        );
        let window = gtk::Window::builder()
            .default_width(900)
            .default_height(800)
            .child(&view)
            .build();
        window.present();
        settle_gtk_layout();
        assert!(!view.has_css_class("albums-bookshelf"));
        for (style, background, skin) in [
            ("default", false, false),
            ("bookshelf", true, true),
            ("album-covers", false, true),
            ("default", false, false),
        ] {
            db::set_setting(
                &connection.borrow(),
                crate::settings::ALBUM_VIEW_STYLE_SETTING_KEY,
                style,
            )
            .unwrap();
            refresh(&view, &albums, &connection.borrow(), 136, on_album.clone());
            settle_gtk_layout();
            assert_eq!(view.has_css_class("albums-bookshelf"), background);
            assert_eq!(
                find_descendant_with_css_class(view.upcast_ref(), "album-skin").is_some(),
                skin,
            );
            let cards = find_descendant_with_css_class(view.upcast_ref(), "albums-home-grid")
                .unwrap()
                .downcast::<gtk::FlowBox>()
                .unwrap();
            let flow_child = cards
                .first_child()
                .unwrap()
                .downcast::<gtk::FlowBoxChild>()
                .unwrap();
            assert_eq!(db::albums(&connection.borrow()).unwrap(), albums);
            let button = find_descendant_with_css_class(view.upcast_ref(), "album-card")
                .unwrap()
                .downcast::<gtk::Button>()
                .unwrap();
            if background {
                let cover =
                    find_descendant_with_css_class(button.upcast_ref(), "album-cover").unwrap();
                assert_eq!(cards.row_spacing(), 0);
                assert!(cards.vexpands());
                assert_eq!(cards.valign(), gtk::Align::Fill);
                assert!(
                    cards.allocated_height() > BOOKSHELF_ROW_HEIGHT,
                    "cards={} parent={} view={}",
                    cards.allocated_height(),
                    cards.parent().unwrap().allocated_height(),
                    view.allocated_height()
                );
                assert!(cards.has_css_class("albums-bookshelf-0"));
                let photo_count = find_descendant_with_css_class(
                    button.upcast_ref(),
                    "albums-bookshelf-photo-count",
                )
                .unwrap()
                .downcast::<gtk::Label>()
                .unwrap();
                let count_color = photo_count.style_context().color();
                assert!((count_color.red() - 58.0 / 255.0).abs() < 0.01);
                assert!((count_color.green() - 33.0 / 255.0).abs() < 0.01);
                assert!((count_color.blue() - 15.0 / 255.0).abs() < 0.01);
                assert_eq!(flow_child.height_request(), BOOKSHELF_ROW_HEIGHT);
                assert_eq!(
                    button.margin_top() + cover.height_request(),
                    BOOKSHELF_SURFACE_Y
                );
            } else if style == "default" {
                assert_eq!(cards.row_spacing(), 20);
                assert!(!cards.vexpands());
                assert_eq!(cards.valign(), gtk::Align::Start);
                assert!(!cards.has_css_class("albums-bookshelf-0"));
                assert_eq!(flow_child.height_request(), -1);
                assert_eq!(button.margin_top(), 0);
            }
            opened.set(None);
            button.emit_clicked();
            assert_eq!(opened.get(), Some(album.id));
        }

        settings::set_bookshelf_enabled(&connection.borrow(), true).unwrap();
        settings::set_covers_enabled(&connection.borrow(), false).unwrap();
        db::set_setting(
            &connection.borrow(),
            crate::settings::ALBUM_BOOKSHELF_BACKGROUND_SETTING_KEY,
            "0",
        )
        .unwrap();
        refresh(&view, &albums, &connection.borrow(), 136, on_album.clone());
        settle_gtk_layout();
        let unframed_cards = find_descendant_with_css_class(view.upcast_ref(), "albums-home-grid")
            .unwrap()
            .downcast::<gtk::FlowBox>()
            .unwrap();
        assert!(unframed_cards.vexpands());
        assert_eq!(unframed_cards.valign(), gtk::Align::Fill);
        assert!(find_descendant_with_css_class(view.upcast_ref(), "album-skin").is_none());

        refresh(&view, &[], &connection.borrow(), 136, on_album);
        settle_gtk_layout();
        let empty_cards = find_descendant_with_css_class(view.upcast_ref(), "albums-home-grid")
            .unwrap()
            .downcast::<gtk::FlowBox>()
            .unwrap();
        assert!(empty_cards.vexpands());
        assert_eq!(empty_cards.valign(), gtk::Align::Fill);
        assert!(empty_cards.allocated_height() > BOOKSHELF_ROW_HEIGHT);
        assert_eq!(empty_cards.row_spacing(), 0);
        assert!(empty_cards.has_css_class("albums-bookshelf-0"));

        db::set_setting(
            &connection.borrow(),
            crate::settings::ALBUM_BOOKSHELF_BACKGROUND_SETTING_KEY,
            "1",
        )
        .unwrap();
        refresh(&view, &[], &connection.borrow(), 136, Rc::new(|_| {}));
        settle_gtk_layout();
        assert!(!empty_cards.vexpands());
        assert_eq!(empty_cards.valign(), gtk::Align::Start);
        assert!(!empty_cards.has_css_class("albums-bookshelf-0"));
        assert!(view.has_css_class("albums-bookshelf-1"));

        settings::disable_all_album_themes(&connection.borrow()).unwrap();
        refresh(&view, &[], &connection.borrow(), 136, Rc::new(|_| {}));
        settle_gtk_layout();
        assert!(!empty_cards.vexpands());
        assert_eq!(empty_cards.valign(), gtk::Align::Start);
        assert_eq!(empty_cards.row_spacing(), 20);
        assert!(!view.has_css_class("albums-bookshelf"));

        window.close();
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
