use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::SystemTime;

use gtk::prelude::*;
use gtk4 as gtk;
use rusqlite::Connection;

use crate::db::{self, Album};
use crate::settings::{self, AlbumAppearance};

const DEFAULT_THUMBNAIL_WIDTH: i32 = 136;
const DEFAULT_THUMBNAIL_HEIGHT: i32 = 91;
const BOOKSHELF_ROW_HEIGHT: i32 = 288;
const BOOKSHELF_SURFACE_Y: i32 = 235;
const BOOKSHELF_COUNT_CSS: &str = ".albums-bookshelf-photo-count { color: #3a210f; }";
const ALBUM_COVER_THEME_DIRECTORY: &str = "images/theme/album-covers";
const BOOKSHELF_THEME_DIRECTORY: &str = "images/theme/bookshelf";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AlbumAppearanceAction {
    ToggleBookshelf,
    NextBackground,
    ToggleCovers,
    NextCovers,
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
            label: "Next Album Covers",
            action: AlbumAppearanceAction::NextCovers,
            checked: None,
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

// Alpha below this value counts as part of a frame's transparent opening.
const OPENING_ALPHA_THRESHOLD: u8 = 32;
// An opening smaller than this fraction of either axis is treated as noise.
const MIN_OPENING_FRACTION: f64 = 0.05;

/// The rectangular window a frame leaves transparent for the cover photo,
/// normalized against the frame's own size.
#[derive(Clone, Copy, Debug, PartialEq)]
struct PhotoOpening {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl PhotoOpening {
    /// Used when a frame's alpha channel has no readable opening (for example a
    /// JPEG without transparency). Matches the original supplied skins.
    const FALLBACK: Self = Self {
        x: 0.18,
        y: 53.0 / 500.0,
        width: 0.64,
        height: 273.0 / 500.0,
    };
}

/// A discovered cover frame with its decoded art and the opening that art
/// exposes, so every design keeps its own photo geometry.
struct FrameAsset {
    path: PathBuf,
    texture: gtk::gdk::Texture,
    opening: PhotoOpening,
}

thread_local! {
    static OPENING_CACHE: RefCell<HashMap<(PathBuf, Option<SystemTime>), PhotoOpening>> =
        RefCell::new(HashMap::new());
}

/// Find the transparent opening enclosed by the frame's opaque pixels.
///
/// Scanning out from the centre avoids the transparent margins around the
/// artwork, which also reach the image border.
fn opening_from_alpha(
    alpha: &[u8],
    width: usize,
    height: usize,
    threshold: u8,
) -> Option<PhotoOpening> {
    if width == 0 || height == 0 || alpha.len() < width * height {
        return None;
    }
    let at = |x: usize, y: usize| alpha[y * width + x];
    for (fx, fy) in [
        (0.5, 0.5),
        (0.5, 0.35),
        (0.5, 0.65),
        (0.35, 0.5),
        (0.65, 0.5),
    ] {
        let cx = ((width as f64 * fx) as usize).min(width - 1);
        let cy = ((height as f64 * fy) as usize).min(height - 1);
        if at(cx, cy) >= threshold {
            continue;
        }
        let mut top = cy;
        while top > 0 && at(cx, top - 1) < threshold {
            top -= 1;
        }
        let mut bottom = cy;
        while bottom + 1 < height && at(cx, bottom + 1) < threshold {
            bottom += 1;
        }
        let row = (top + bottom) / 2;
        let mut left = cx;
        while left > 0 && at(left - 1, row) < threshold {
            left -= 1;
        }
        let mut right = cx;
        while right + 1 < width && at(right + 1, row) < threshold {
            right += 1;
        }
        let opening = PhotoOpening {
            x: left as f64 / width as f64,
            y: top as f64 / height as f64,
            width: (right - left + 1) as f64 / width as f64,
            height: (bottom - top + 1) as f64 / height as f64,
        };
        let enclosed = opening.x > 0.0
            && opening.y > 0.0
            && opening.x + opening.width < 1.0
            && opening.y + opening.height < 1.0;
        if enclosed
            && opening.width >= MIN_OPENING_FRACTION
            && opening.height >= MIN_OPENING_FRACTION
        {
            return Some(opening);
        }
    }
    None
}

fn detect_photo_opening(path: &Path) -> Option<PhotoOpening> {
    let decoded = image::open(path).ok()?.to_rgba8();
    let (width, height) = decoded.dimensions();
    let alpha: Vec<u8> = decoded.pixels().map(|pixel| pixel.0[3]).collect();
    opening_from_alpha(
        &alpha,
        width as usize,
        height as usize,
        OPENING_ALPHA_THRESHOLD,
    )
}

/// Decoding every frame is only worth doing once per file revision, because
/// the album view repopulates whenever the library or the theme changes.
fn frame_photo_opening(path: &Path) -> PhotoOpening {
    let modified = std::fs::metadata(path)
        .ok()
        .and_then(|metadata| metadata.modified().ok());
    let key = (path.to_path_buf(), modified);
    if let Some(cached) = OPENING_CACHE.with(|cache| cache.borrow().get(&key).copied()) {
        return cached;
    }
    let opening = detect_photo_opening(path).unwrap_or(PhotoOpening::FALLBACK);
    OPENING_CACHE.with(|cache| {
        cache.borrow_mut().insert(key, opening);
    });
    opening
}

fn album_frame_paths_for_appearance(directory: &Path, appearance: AlbumAppearance) -> Vec<PathBuf> {
    if !appearance.covers_enabled {
        Vec::new()
    } else {
        album_frame_paths_in(directory)
    }
}

fn bookshelf_background_path(directory: &Path, index: usize) -> Option<PathBuf> {
    let backgrounds = bookshelf_background_paths_in(directory);
    (!backgrounds.is_empty()).then(|| backgrounds[index % backgrounds.len()].clone())
}

fn bookshelf_card_margin_top(cover_height: i32) -> i32 {
    (BOOKSHELF_SURFACE_Y - cover_height).max(0)
}

fn uses_responsive_bookshelf(appearance: AlbumAppearance) -> bool {
    appearance.bookshelf_enabled && appearance.background_index == 0
}

fn bookshelf_background_rules(directory: &Path) -> String {
    bookshelf_background_paths_in(directory)
        .into_iter()
        .enumerate()
        .map(|(index, path)| {
            let uri = gio::File::for_path(path).uri();
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
    let count = bookshelf_background_count();
    (appearance.bookshelf_enabled && count > 0)
        .then(|| format!("albums-bookshelf-{}", appearance.background_index % count))
}

fn album_frame_paths_in(directory: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(entries) = std::fs::read_dir(directory) {
        for path in entries.filter_map(Result::ok).map(|entry| entry.path()) {
            if path.is_dir() {
                collect_matching_files(&path, &mut paths, &|name| name.ends_with("-frame.png"));
            }
        }
    }
    paths.sort();
    paths
}

pub(crate) fn album_cover_theme_count() -> usize {
    let root = Path::new(ALBUM_COVER_THEME_DIRECTORY);
    cover_themes(root, &album_frame_paths_in(root)).len()
}

fn collect_matching_files(
    directory: &Path,
    paths: &mut Vec<PathBuf>,
    matches: &impl Fn(&str) -> bool,
) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            collect_matching_files(&path, paths, matches);
        } else if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(matches)
        {
            paths.push(path);
        }
    }
}

fn bookshelf_background_paths_in(directory: &Path) -> Vec<PathBuf> {
    let mut paths = png_or_jpg_paths_matching(directory, |name| {
        name.ends_with("-bookshelf.png") || name.ends_with("-bookshelf.jpg")
    });
    paths.sort_by_key(|path| {
        (
            path.file_name().and_then(|name| name.to_str()) != Some("single-row-bookshelf.png"),
            path.clone(),
        )
    });
    paths
}

pub(crate) fn bookshelf_background_count() -> usize {
    bookshelf_background_paths_in(Path::new(BOOKSHELF_THEME_DIRECTORY)).len()
}

fn png_or_jpg_paths_matching(directory: &Path, matches: impl Fn(&str) -> bool) -> Vec<PathBuf> {
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

/// Cover frames grouped by theme folder: the top-level directory under
/// `images/theme/album-covers`. A theme is one design family and may ship
/// several colour variants, so `Next Album Cover` walks folders, not files.
struct CoverTheme {
    name: String,
    frames: Vec<PathBuf>,
}

impl CoverTheme {
    /// The frame this album uses inside the theme.
    ///
    /// A theme with a single file always shows it; a theme with several variants
    /// gives the album a stable one chosen by hashing its id and name, so the
    /// page still draws only this theme.
    fn frame_for(&self, album: &Album) -> Option<&PathBuf> {
        self.frames
            .get(hash_slot(album, b"variant", self.frames.len()))
    }
}

fn cover_theme_name(root: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?;
    let component = relative.components().next()?;
    Some(component.as_os_str().to_string_lossy().into_owned())
}

/// `frames` arrives path-sorted, so themes come out alphabetically and every
/// theme's variants stay sorted too.
fn cover_themes(root: &Path, frames: &[PathBuf]) -> Vec<CoverTheme> {
    let mut themes: Vec<CoverTheme> = Vec::new();
    for path in frames {
        let Some(name) = cover_theme_name(root, path) else {
            continue;
        };
        match themes.iter_mut().find(|theme| theme.name == name) {
            Some(theme) => theme.frames.push(path.clone()),
            None => themes.push(CoverTheme {
                name,
                frames: vec![path.clone()],
            }),
        }
    }
    themes
}

/// A defined hash rather than a process-seeded or unspecified hasher, so an
/// album's design is stable across runs.
fn album_hash(album: &Album, salt: &[u8]) -> u64 {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&album.id.to_le_bytes());
    hasher.update(album.name.as_bytes());
    hasher.update(salt);
    let hash = hasher.finalize();
    u64::from_le_bytes(hash.as_bytes()[..8].try_into().unwrap())
}

fn hash_slot(album: &Album, salt: &[u8], count: usize) -> usize {
    debug_assert!(count > 0);
    (album_hash(album, salt) % count as u64) as usize
}

/// Albums that never picked a frame of their own follow the page's single
/// selected theme, so one page never mixes two themes. Only the variant inside
/// that theme differs between albums, and it stays stable across refreshes.
fn automatic_frame<'a>(
    album: &Album,
    themes: &'a [CoverTheme],
    cover_index: usize,
) -> Option<&'a PathBuf> {
    if themes.is_empty() {
        return None;
    }
    themes.get(cover_index % themes.len())?.frame_for(album)
}

fn frame_identifier(root: &Path, path: &Path) -> Option<String> {
    path.strip_prefix(root)
        .ok()
        .map(|relative| relative.to_string_lossy().into_owned())
}

/// The frame an album shows: its own choice while that file still exists,
/// otherwise the automatic theme assignment.
fn selected_frame_path(
    album: &Album,
    root: &Path,
    themes: &[CoverTheme],
    cover_index: usize,
) -> Option<PathBuf> {
    if let Some(selected) = album.cover_frame.as_deref() {
        let chosen = themes
            .iter()
            .flat_map(|theme| theme.frames.iter())
            .find(|path| frame_identifier(root, path).as_deref() == Some(selected));
        if let Some(path) = chosen {
            return Some(path.clone());
        }
    }
    automatic_frame(album, themes, cover_index).cloned()
}

fn selected_frame_index(
    album: &Album,
    root: &Path,
    frames: &[PathBuf],
    cover_index: usize,
) -> Option<usize> {
    if frames.is_empty() {
        return None;
    }
    let themes = cover_themes(root, frames);
    let selected = selected_frame_path(album, root, &themes, cover_index)?;
    frames.iter().position(|path| path == &selected)
}

/// `Next Album Cover` steps to the next theme folder while keeping the
/// album's stable variant inside it, so five themes cycle in five steps.
fn next_theme_identifier(
    album: &Album,
    root: &Path,
    frames: &[PathBuf],
    cover_index: usize,
) -> Option<String> {
    let themes = cover_themes(root, frames);
    let current = selected_frame_path(album, root, &themes, cover_index)?;
    let current_theme = themes
        .iter()
        .position(|theme| theme.frames.contains(&current))?;
    let next_theme = themes.get((current_theme + 1) % themes.len())?;
    frame_identifier(root, next_theme.frame_for(album)?)
}

fn change_to_next_album_cover(
    connection: &Connection,
    album: &Album,
    root: &Path,
    frames: &[PathBuf],
    cover_index: usize,
) -> anyhow::Result<Option<String>> {
    let Some(next) = next_theme_identifier(album, root, frames, cover_index) else {
        return Ok(None);
    };
    db::set_album_cover_frame(connection, album.id, &next)?;
    Ok(Some(next))
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
    let background_rules = bookshelf_background_rules(Path::new(BOOKSHELF_THEME_DIRECTORY));
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
        connection.clone(),
        thumbnail_width,
        on_album.clone(),
        on_appearance_changed.clone(),
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
    click.set_propagation_phase(gtk::PropagationPhase::Bubble);
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

fn install_album_context_menu(
    card: &gtk::Button,
    connection: Rc<RefCell<Connection>>,
    album: Album,
    on_appearance_changed: Rc<dyn Fn()>,
) {
    let click = gtk::GestureClick::new();
    click.set_button(gtk::gdk::BUTTON_SECONDARY);
    let anchor = card.clone();
    click.connect_pressed(move |gesture, _, x, y| {
        gesture.set_state(gtk::EventSequenceState::Claimed);
        show_album_context_menu(
            &anchor,
            x,
            y,
            connection.clone(),
            album.clone(),
            on_appearance_changed.clone(),
        );
    });
    card.add_controller(click);
}

fn show_album_context_menu(
    anchor: &gtk::Button,
    x: f64,
    y: f64,
    connection: Rc<RefCell<Connection>>,
    album: Album,
    on_appearance_changed: Rc<dyn Fn()>,
) {
    let frame_root = Path::new(ALBUM_COVER_THEME_DIRECTORY);
    let frames = album_frame_paths_in(frame_root);
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
    let heading = gtk::Label::new(Some(&album.name));
    heading.set_xalign(0.0);
    heading.set_margin_start(8);
    heading.set_margin_end(8);
    heading.set_margin_top(4);
    heading.set_margin_bottom(4);
    heading.add_css_class("heading");
    menu.append(&heading);

    let next_frame = gtk::Button::with_label("Next Album Cover");
    next_frame.set_halign(gtk::Align::Fill);
    next_frame.set_sensitive(!frames.is_empty());
    next_frame.add_css_class("flat");
    next_frame.add_css_class("next-album-frame-action");
    let popover_for_action = popover.clone();
    let reset_connection = connection.clone();
    let reset_album = album.clone();
    let reset_appearance_changed = on_appearance_changed.clone();
    let album_has_own_cover = album.cover_frame.is_some() || album.cover_photo_id.is_some();
    next_frame.connect_clicked(move |_| {
        popover_for_action.popdown();
        let result = (|| {
            settings::set_covers_enabled(&connection.borrow(), true)?;
            let cover_index = settings::album_appearance(&connection.borrow()).cover_index;
            change_to_next_album_cover(
                &connection.borrow(),
                &album,
                frame_root,
                &frames,
                cover_index,
            )?;
            anyhow::Ok(())
        })();
        if let Err(error) = result {
            eprintln!("Could not change album cover: {error}");
            return;
        }
        on_appearance_changed();
    });
    menu.append(&next_frame);

    // Only an album with a cover of its own can lose one, so the reset stays
    // disabled for albums that already follow the page-wide design and its
    // automatic thumbnail.
    let reset_frame = gtk::Button::with_label("Reset Album Cover");
    reset_frame.set_halign(gtk::Align::Fill);
    reset_frame.set_sensitive(album_has_own_cover);
    reset_frame.add_css_class("flat");
    reset_frame.add_css_class("reset-album-frame-action");
    let popover_for_reset = popover.clone();
    reset_frame.connect_clicked(move |_| {
        popover_for_reset.popdown();
        let result = (|| {
            let connection = reset_connection.borrow();
            db::clear_album_cover_frame(&connection, reset_album.id)?;
            db::clear_album_cover_photo(&connection, reset_album.id)?;
            anyhow::Ok(())
        })();
        if let Err(error) = result {
            eprintln!("Could not reset album cover: {error}");
            return;
        }
        reset_appearance_changed();
    });
    menu.append(&reset_frame);

    popover.set_child(Some(&menu));
    popover.set_parent(anchor);
    popover.connect_closed(|popover| popover.unparent());
    popover.popup();
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

    for item in album_context_menu_items(appearance) {
        if matches!(
            item.action,
            AlbumAppearanceAction::DisableAll | AlbumAppearanceAction::OpenThemeSettings
        ) {
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
                AlbumAppearanceAction::NextBackground => settings::next_bookshelf_background(
                    &connection.borrow(),
                    bookshelf_background_count(),
                )
                .map(|_| ()),
                AlbumAppearanceAction::ToggleCovers => settings::set_covers_enabled(
                    &connection.borrow(),
                    !settings::album_appearance(&connection.borrow()).covers_enabled,
                ),
                AlbumAppearanceAction::NextCovers => {
                    settings::next_album_covers(&connection.borrow(), album_cover_theme_count())
                        .map(|_| ())
                }
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
    connection: Rc<RefCell<Connection>>,
    thumbnail_width: i32,
    on_album: Rc<dyn Fn(i64)>,
    on_appearance_changed: Rc<dyn Fn()>,
) {
    refresh_presentation(scrolled, &connection.borrow());
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
        on_appearance_changed,
    );
}

pub fn refresh_presentation(scrolled: &gtk::ScrolledWindow, connection: &Connection) {
    let appearance = settings::album_appearance(connection);
    let cards = find_descendant_with_css_class(scrolled.upcast_ref(), "albums-home-grid");
    remove_bookshelf_background_classes(scrolled.upcast_ref());
    if let Some(cards) = cards.as_ref() {
        remove_bookshelf_background_classes(cards);
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

fn remove_bookshelf_background_classes(widget: &gtk::Widget) {
    for class_name in widget
        .css_classes()
        .into_iter()
        .filter(|name| name.starts_with("albums-bookshelf-"))
    {
        widget.remove_css_class(&class_name);
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
    connection: Rc<RefCell<Connection>>,
    thumbnail_width: i32,
    on_album: Rc<dyn Fn(i64)>,
    on_appearance_changed: Rc<dyn Fn()>,
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

    let appearance = settings::album_appearance(&connection.borrow());
    let responsive_bookshelf = uses_responsive_bookshelf(appearance);
    cards.set_vexpand(responsive_bookshelf);
    cards.set_valign(if responsive_bookshelf {
        gtk::Align::Fill
    } else {
        gtk::Align::Start
    });
    let frames: Vec<FrameAsset> =
        album_frame_paths_for_appearance(Path::new(ALBUM_COVER_THEME_DIRECTORY), appearance)
            .into_iter()
            .filter_map(|path| {
                gtk::gdk::Texture::from_filename(&path).ok().map(|texture| {
                    let opening = frame_photo_opening(&path);
                    FrameAsset {
                        path,
                        texture,
                        opening,
                    }
                })
            })
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
            (frame_height as f64 * frame.texture.width() as f64 / frame.texture.height() as f64)
                .round() as i32
        })
        .max()
        .unwrap_or_else(|| cover_width(thumbnail_width));
    let frame_paths: Vec<_> = frames.iter().map(|frame| frame.path.clone()).collect();
    for album in albums {
        let frame = selected_frame_index(
            album,
            Path::new(ALBUM_COVER_THEME_DIRECTORY),
            &frame_paths,
            appearance.cover_index,
        )
        .map(|index| &frames[index]);
        let actual_cover_height = if frame.is_some() {
            frame_height
        } else {
            cover_height(thumbnail_width)
        };
        let card = album_card(
            album,
            &connection.borrow(),
            thumbnail_width,
            on_album.clone(),
            frame,
            responsive_bookshelf,
            connection.clone(),
            on_appearance_changed.clone(),
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

/// The photo an album draws on its card. A cover chosen from the thumbnail
/// menu wins even when that photo is not a member of the album; otherwise the
/// album keeps its automatic first-thumbnail pick.
fn album_cover_photo(
    connection: &Connection,
    album: &Album,
    photos: &[db::Photo],
) -> Option<db::Photo> {
    if let Some(id) = album.cover_photo_id {
        let chosen = photos
            .iter()
            .find(|photo| photo.id == id)
            .cloned()
            .or_else(|| db::photo(connection, id).ok().flatten());
        if chosen.is_some() {
            return chosen;
        }
    }
    photos
        .iter()
        .find(|photo| {
            crate::thumbnail::existing_cache_path(&photo.path, photo.mtime, photo.size_bytes)
                .ok()
                .flatten()
                .is_some()
        })
        .cloned()
}

fn album_card(
    album: &Album,
    connection: &Connection,
    thumbnail_width: i32,
    on_album: Rc<dyn Fn(i64)>,
    frame: Option<&FrameAsset>,
    responsive_bookshelf: bool,
    menu_connection: Rc<RefCell<Connection>>,
    on_appearance_changed: Rc<dyn Fn()>,
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
        .map(|frame| {
            (height as f64 * frame.texture.width() as f64 / frame.texture.height() as f64).round()
                as i32
        })
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
    // The chosen cover wins over the automatic pick, even before its cached
    // thumbnail exists.
    let cover_photo = album_cover_photo(connection, album, &photos).map(|photo| {
        let cached =
            crate::thumbnail::existing_cache_path(&photo.path, photo.mtime, photo.size_bytes)
                .ok()
                .flatten();
        (photo, cached)
    });

    if std::env::var_os("PICASA_TRACE").is_some() {
        eprintln!(
            "ALBUM TRACE id={} name={} photos={} cover={:?}",
            album.id,
            album.name,
            photos.len(),
            cover_photo.as_ref().map(|(photo, path)| path
                .as_deref()
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or_else(|| photo.path.clone()))
        );
    }

    if let Some((photo, cached)) = cover_photo.as_ref() {
        // A cover without a cached thumbnail yet still draws from its original
        // file, so a fresh choice is visible straight away.
        let source = cached
            .as_deref()
            .unwrap_or_else(|| Path::new(photo.path.as_str()));
        let path_string = source.to_string_lossy();

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
        let (left, top, right, bottom) = photo_margins(frame.opening, width, height);
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
        let skin = gtk::Picture::for_paintable(&frame.texture);
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
    install_album_context_menu(&card, menu_connection, album.clone(), on_appearance_changed);

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

fn photo_margins(opening: PhotoOpening, width: i32, height: i32) -> (i32, i32, i32, i32) {
    // Round outward and add a pixel of underlap, so a frame's antialiased
    // opening edge cannot show a seam around the photo.
    (
        ((width as f64 * opening.x).floor() as i32 - 1).max(0),
        ((height as f64 * opening.y).floor() as i32 - 1).max(0),
        ((width as f64 * (1.0 - opening.x - opening.width)).floor() as i32 - 1).max(0),
        ((height as f64 * (1.0 - opening.y - opening.height)).floor() as i32 - 1).max(0),
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
            cover_frame: None,
            cover_photo_id: None,
        }
    }

    fn alpha_with_opening(
        width: usize,
        height: usize,
        opening: (usize, usize, usize, usize),
    ) -> Vec<u8> {
        let (x, y, opening_width, opening_height) = opening;
        let mut alpha = vec![255u8; width * height];
        for row in y..y + opening_height {
            for column in x..x + opening_width {
                alpha[row * width + column] = 0;
            }
        }
        alpha
    }

    #[test]
    fn discovers_cover_frames_recursively_across_theme_folders() {
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
        let standard = directory.join("standard");
        let vintage = directory.join("vintage");
        std::fs::create_dir(&standard).unwrap();
        std::fs::create_dir(&vintage).unwrap();
        for path in [
            standard.join("default-frame.png"),
            vintage.join("blue-frame.png"),
            vintage.join("green-frame.png"),
            vintage.join("green-frame.jpg"),
            vintage.join("notes.txt"),
            directory.join("loose-frame.png"),
        ] {
            std::fs::write(path, []).unwrap();
        }
        std::fs::create_dir(vintage.join("directory-frame.png")).unwrap();
        let expected = vec![
            standard.join("default-frame.png"),
            vintage.join("blue-frame.png"),
            vintage.join("green-frame.png"),
        ];
        assert_eq!(album_frame_paths_in(&directory), expected);

        std::fs::write(standard.join("new-frame.png"), []).unwrap();
        let mut expected = expected;
        expected.push(standard.join("new-frame.png"));
        expected.sort();
        assert_eq!(album_frame_paths_in(&directory), expected);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn discovers_only_named_bookshelf_png_and_jpg_files() {
        let directory = std::env::temp_dir().join(format!(
            "pic-bookshelf-backgrounds-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        for name in [
            "oak-bookshelf.png",
            "three-panel-bookshelf.jpg",
            "bookshelf.png",
            "oak-bookshelf.jpeg",
            "oak-frame.png",
            "oak-bookshelf.png.bak",
        ] {
            std::fs::write(directory.join(name), []).unwrap();
        }
        std::fs::create_dir(directory.join("nested-bookshelf.png")).unwrap();

        assert_eq!(
            bookshelf_background_paths_in(&directory),
            vec![
                directory.join("oak-bookshelf.png"),
                directory.join("three-panel-bookshelf.jpg"),
            ]
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn zero_skins_have_no_selection() {
        let root = Path::new("images/theme/album-covers");
        assert_eq!(selected_frame_index(&album(1), root, &[], 0), None);
    }

    #[test]
    fn album_cover_prefers_the_photo_chosen_from_the_menu() {
        let directory = std::env::temp_dir().join(format!(
            "pic-album-cover-photo-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let connection = db::open(&directory.join("library.db")).unwrap();
        connection
            .execute_batch(
                "INSERT INTO photos (id, path) VALUES
                   (1, '/tmp/one.jpg'),
                   (2, '/tmp/two.jpg'),
                   (3, '/tmp/three.jpg');",
            )
            .unwrap();
        let album = db::create_album(&connection, "Holiday").unwrap();
        let photos: Vec<_> = [1, 2]
            .into_iter()
            .filter_map(|id| db::photo(&connection, id).unwrap())
            .collect();

        // Nothing is cached in this test, so the automatic pick stays empty.
        assert_eq!(
            album_cover_photo(&connection, &album, &photos).map(|photo| photo.id),
            None
        );

        db::set_album_cover_photo(&connection, album.id, 2).unwrap();
        let album = db::albums(&connection).unwrap().remove(0);
        assert_eq!(
            album_cover_photo(&connection, &album, &photos).map(|photo| photo.id),
            Some(2)
        );

        // A chosen photo still resolves when it is not an album member.
        db::set_album_cover_photo(&connection, album.id, 3).unwrap();
        let album = db::albums(&connection).unwrap().remove(0);
        assert_eq!(
            album_cover_photo(&connection, &album, &photos).map(|photo| photo.id),
            Some(3)
        );

        // Deleting the photo clears the choice through the schema's
        // ON DELETE SET NULL, so nothing points at a missing photo.
        connection
            .execute("DELETE FROM photos WHERE id = 3", [])
            .unwrap();
        let album = db::albums(&connection).unwrap().remove(0);
        assert_eq!(album.cover_photo_id, None);
        assert_eq!(
            album_cover_photo(&connection, &album, &photos).map(|photo| photo.id),
            None
        );

        drop(connection);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn persisted_frame_is_selected_and_next_theme_wraps() {
        let root = Path::new("images/theme/album-covers");
        let frames = vec![
            root.join("standard/standard-frame.png"),
            root.join("vintage/blue-frame.png"),
            root.join("vintage/green-frame.png"),
        ];
        let mut album = album(7);
        album.cover_frame = Some("vintage/green-frame.png".to_string());

        assert_eq!(selected_frame_index(&album, root, &frames, 0), Some(2));
        // Themes are ordered alphabetically, so the last one wraps to the first.
        assert_eq!(
            next_theme_identifier(&album, root, &frames, 0).as_deref(),
            Some("standard/standard-frame.png")
        );
    }

    #[test]
    fn missing_persisted_frame_falls_back_to_stable_assignment() {
        let root = Path::new("images/theme/album-covers");
        let frames = vec![
            root.join("standard/standard-frame.png"),
            root.join("vintage/blue-frame.png"),
        ];
        let mut album = album(11);
        album.cover_frame = Some("removed/missing-frame.png".to_string());
        let mut without_stored_frame = album.clone();
        without_stored_frame.cover_frame = None;

        let selected = selected_frame_index(&album, root, &frames, 0);
        assert!(selected.is_some());
        // The disappeared frame leaves the album on the automatic design.
        assert_eq!(
            selected,
            selected_frame_index(&without_stored_frame, root, &frames, 0)
        );
    }

    #[test]
    fn changing_an_album_cover_persists_only_that_albums_next_theme() {
        let directory = std::env::temp_dir().join(format!(
            "pic-next-album-frame-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let connection = db::open(&directory.join("library.db")).unwrap();
        let first = db::create_album(&connection, "First").unwrap();
        let second = db::create_album(&connection, "Second").unwrap();
        db::set_album_cover_frame(&connection, first.id, "vintage/green-frame.png").unwrap();
        let first = db::albums(&connection).unwrap().remove(0);
        let root = Path::new("images/theme/album-covers");
        let frames = vec![
            root.join("standard/standard-frame.png"),
            root.join("vintage/blue-frame.png"),
            root.join("vintage/green-frame.png"),
        ];

        assert_eq!(
            change_to_next_album_cover(&connection, &first, root, &frames, 0)
                .unwrap()
                .as_deref(),
            Some("standard/standard-frame.png")
        );
        let albums = db::albums(&connection).unwrap();
        assert_eq!(
            albums
                .iter()
                .find(|album| album.id == first.id)
                .unwrap()
                .cover_frame
                .as_deref(),
            Some("standard/standard-frame.png")
        );
        assert_eq!(
            albums
                .iter()
                .find(|album| album.id == second.id)
                .unwrap()
                .cover_frame,
            None
        );
        drop(connection);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn all_cover_theme_folders_are_available_with_or_without_a_bookshelf() {
        let directory = Path::new("images/theme/album-covers");
        let with_bookshelf = album_frame_paths_for_appearance(
            directory,
            AlbumAppearance {
                bookshelf_enabled: true,
                covers_enabled: true,
                background_index: 0,
                cover_index: 0,
            },
        );
        let without_bookshelf = album_frame_paths_for_appearance(
            directory,
            AlbumAppearance {
                bookshelf_enabled: false,
                covers_enabled: true,
                background_index: 0,
                cover_index: 0,
            },
        );

        assert_eq!(with_bookshelf, without_bookshelf);
        assert!(with_bookshelf.contains(&directory.join("pink/pink-frame.png")));
        assert!(with_bookshelf.contains(&directory.join("vintage/blue-frame.png")));
        assert!(with_bookshelf.contains(&directory.join("wedding/wedding-frame.png")));
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
        let standard = directory.join("standard");
        let vintage = directory.join("vintage");
        std::fs::create_dir(&standard).unwrap();
        std::fs::create_dir(&vintage).unwrap();
        std::fs::write(standard.join("white-frame.png"), []).unwrap();
        std::fs::write(vintage.join("blue-frame.png"), []).unwrap();

        let bookshelf = AlbumAppearance {
            bookshelf_enabled: true,
            covers_enabled: true,
            background_index: 0,
            cover_index: 0,
        };
        assert_eq!(
            album_frame_paths_for_appearance(&directory, bookshelf),
            vec![
                standard.join("white-frame.png"),
                vintage.join("blue-frame.png")
            ]
        );

        let plain_covers = AlbumAppearance {
            bookshelf_enabled: false,
            covers_enabled: true,
            background_index: 1,
            cover_index: 0,
        };
        assert_eq!(
            album_frame_paths_for_appearance(&directory, plain_covers),
            vec![
                standard.join("white-frame.png"),
                vintage.join("blue-frame.png")
            ]
        );

        let disabled = AlbumAppearance {
            bookshelf_enabled: true,
            covers_enabled: false,
            background_index: 0,
            cover_index: 0,
        };
        assert!(album_frame_paths_for_appearance(&directory, disabled).is_empty());

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn page_cover_index_selects_one_theme_for_every_album() {
        let root = Path::new("images/theme/album-covers");
        let frames = album_frame_paths_in(root);
        let themes = cover_themes(root, &frames);
        assert!(themes.len() > 1);
        let albums: Vec<_> = (0..5).map(album).collect();

        let theme_for = |album: &Album, cover_index: usize| {
            let index = selected_frame_index(album, root, &frames, cover_index).unwrap();
            cover_theme_name(root, &frames[index]).unwrap()
        };
        for cover_index in 0..themes.len() + 1 {
            let names: std::collections::HashSet<_> = albums
                .iter()
                .map(|album| theme_for(album, cover_index))
                .collect();
            // One page shows one theme, no matter which albums it contains.
            assert_eq!(
                names.len(),
                1,
                "page mixed themes at cover index {cover_index}"
            );
            assert!(names.contains(&themes[cover_index % themes.len()].name));
        }

        let first_album = &albums[0];
        // A full turn of the page action restores the same theme and variant.
        assert_eq!(
            theme_for(first_album, themes.len()),
            theme_for(first_album, 0)
        );
        assert_eq!(
            selected_frame_index(first_album, root, &frames, themes.len()).unwrap(),
            selected_frame_index(first_album, root, &frames, 0).unwrap()
        );
    }

    #[test]
    fn next_album_cover_steps_between_theme_folders() {
        let root = Path::new("images/theme/album-covers");
        let frames = album_frame_paths_in(root);
        let names: Vec<_> = cover_themes(root, &frames)
            .into_iter()
            .map(|theme| theme.name)
            .collect();
        assert!(names.len() > 1);
        let album = album(7);

        let current = selected_frame_index(&album, root, &frames, 0).unwrap();
        let current_theme = cover_theme_name(root, &frames[current]).unwrap();
        let expected = names[(names
            .iter()
            .position(|name| name == &current_theme)
            .unwrap()
            + 1)
            % names.len()]
        .clone();

        let next = next_theme_identifier(&album, root, &frames, 0).unwrap();
        assert_eq!(cover_theme_name(root, &root.join(&next)).unwrap(), expected);
        // The album keeps the same variant whenever it returns to that theme.
        assert_eq!(
            next_theme_identifier(&album, root, &frames, 0),
            Some(next.clone())
        );
        // Clicking again moves on to the theme after that one.
        let mut chosen = album.clone();
        chosen.cover_frame = Some(next);
        let following = next_theme_identifier(&chosen, root, &frames, 0).unwrap();
        assert_ne!(
            cover_theme_name(root, &root.join(&following)).unwrap(),
            expected
        );
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
            cover_index: 0,
        }));
        assert!(!uses_responsive_bookshelf(AlbumAppearance {
            bookshelf_enabled: true,
            covers_enabled: false,
            background_index: 1,
            cover_index: 0,
        }));
        assert!(!uses_responsive_bookshelf(AlbumAppearance {
            bookshelf_enabled: false,
            covers_enabled: true,
            background_index: 0,
            cover_index: 0,
        }));
    }

    #[test]
    fn bookshelf_background_rules_use_fixed_row_height() {
        let directory = std::env::temp_dir().join(format!(
            "pic-bookshelf-css-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        for name in [
            "single-row-bookshelf.png",
            "three-panel-bookshelf.jpg",
            "four-panel-bookshelf.png",
        ] {
            std::fs::write(directory.join(name), []).unwrap();
        }
        assert_eq!(
            bookshelf_background_path(&directory, 0),
            Some(directory.join("single-row-bookshelf.png"))
        );

        let rules = bookshelf_background_rules(&directory);
        assert!(rules.contains(".albums-bookshelf-0 {"));
        assert_eq!(rules.matches("background-size: 100% 288px").count(), 1);
        assert_eq!(rules.matches("background-size: 100% auto").count(), 2);
        assert!(BOOKSHELF_COUNT_CSS.contains(".albums-bookshelf-photo-count"));
        assert!(BOOKSHELF_COUNT_CSS.contains("color: #3a210f"));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn context_menu_model_exposes_theme_controls_and_current_state() {
        use crate::settings::AlbumAppearance;

        let items = album_context_menu_items(AlbumAppearance {
            bookshelf_enabled: true,
            covers_enabled: false,
            background_index: 1,
            cover_index: 0,
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
                    label: "Next Album Covers",
                    action: AlbumAppearanceAction::NextCovers,
                    checked: None,
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
        assert!(
            !disabled
                .iter()
                .find(|item| item.action == AlbumAppearanceAction::DisableAll)
                .unwrap()
                .sensitive
        );
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
        assert_eq!(buttons.len(), 6);
        buttons[1].emit_clicked();
        assert_eq!(changed.get(), 1);
        assert_eq!(
            settings::album_appearance(&connection.borrow()),
            AlbumAppearance {
                bookshelf_enabled: true,
                covers_enabled: false,
                background_index: 1,
                cover_index: 0,
            }
        );

        buttons[3].emit_clicked();
        assert_eq!(changed.get(), 2);
        assert_eq!(
            settings::album_appearance(&connection.borrow()),
            AlbumAppearance {
                bookshelf_enabled: true,
                covers_enabled: true,
                background_index: 1,
                cover_index: 1,
            }
        );

        window.close();
    }

    #[test]
    #[ignore = "requires a GTK display; run with --ignored --test-threads=1"]
    fn album_context_menu_changes_and_persists_that_albums_frame() {
        gtk::init().unwrap();
        let directory = std::env::temp_dir().join(format!(
            "pic-album-frame-menu-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let connection = Rc::new(RefCell::new(
            db::open(&directory.join("library.db")).unwrap(),
        ));
        let album = db::create_album(&connection.borrow(), "Menu Album").unwrap();
        let changed = Rc::new(std::cell::Cell::new(0));
        let changed_for_callback = changed.clone();
        let anchor = gtk::Button::new();
        let window = gtk::Window::builder().child(&anchor).build();
        window.present();

        show_album_context_menu(
            &anchor,
            10.0,
            10.0,
            connection.clone(),
            album.clone(),
            Rc::new(move || changed_for_callback.set(changed_for_callback.get() + 1)),
        );
        let action = find_descendant_with_css_class(anchor.upcast_ref(), "next-album-frame-action")
            .unwrap()
            .downcast::<gtk::Button>()
            .unwrap();
        // The album has no frame of its own yet, so there is nothing to reset.
        let reset = find_descendant_with_css_class(anchor.upcast_ref(), "reset-album-frame-action")
            .unwrap()
            .downcast::<gtk::Button>()
            .unwrap();
        assert!(!reset.is_sensitive());
        action.emit_clicked();

        assert_eq!(changed.get(), 1);
        assert!(settings::album_appearance(&connection.borrow()).covers_enabled);
        assert!(db::albums(&connection.borrow()).unwrap()[0]
            .cover_frame
            .is_some());

        window.close();
        drop(connection);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    #[ignore = "requires a GTK display; run with --ignored --test-threads=1"]
    fn resetting_an_album_cover_restores_the_automatic_design() {
        gtk::init().unwrap();
        let directory = std::env::temp_dir().join(format!(
            "pic-reset-album-frame-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let connection = Rc::new(RefCell::new(
            db::open(&directory.join("library.db")).unwrap(),
        ));
        let album = db::create_album(&connection.borrow(), "Styled").unwrap();
        db::set_album_cover_frame(&connection.borrow(), album.id, "vintage/blue-frame.png")
            .unwrap();
        connection
            .borrow()
            .execute_batch("INSERT INTO photos (id, path) VALUES (7, '/tmp/seven.jpg');")
            .unwrap();
        db::set_album_cover_photo(&connection.borrow(), album.id, 7).unwrap();
        let album = db::albums(&connection.borrow()).unwrap().remove(0);
        let changed = Rc::new(std::cell::Cell::new(0));
        let changed_for_callback = changed.clone();
        let anchor = gtk::Button::new();
        let window = gtk::Window::builder().child(&anchor).build();
        window.present();

        show_album_context_menu(
            &anchor,
            10.0,
            10.0,
            connection.clone(),
            album,
            Rc::new(move || changed_for_callback.set(changed_for_callback.get() + 1)),
        );
        let reset = find_descendant_with_css_class(anchor.upcast_ref(), "reset-album-frame-action")
            .unwrap()
            .downcast::<gtk::Button>()
            .unwrap();
        assert!(reset.is_sensitive());
        reset.emit_clicked();

        assert_eq!(changed.get(), 1);
        let album = db::albums(&connection.borrow()).unwrap().remove(0);
        assert_eq!(album.cover_frame, None);
        assert_eq!(album.cover_photo_id, None);

        window.close();
        drop(connection);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    #[ignore = "requires a GTK display; run with --ignored --test-threads=1"]
    fn album_card_draws_the_photo_chosen_as_its_cover() {
        gtk::init().unwrap();
        let directory = std::env::temp_dir().join(format!(
            "pic-chosen-cover-{}-{}",
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
        connection
            .borrow()
            .execute_batch(
                "INSERT INTO photos (id, path) VALUES
                   (1, 'samples/01-Start Up.jpg'),
                   (2, 'samples/Single Photo.jpg');",
            )
            .unwrap();
        // Photo 2 is only the chosen cover; it never joined the album.
        db::set_album_cover_photo(&connection.borrow(), album.id, 2).unwrap();

        let albums = db::albums(&connection.borrow()).unwrap();
        let view = build(
            &albums,
            connection.clone(),
            136,
            Rc::new(|_| {}),
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

        let card = find_descendant_with_css_class(view.upcast_ref(), "album-card").unwrap();
        let picture = find_descendant_with_css_class(card.upcast_ref(), "thumbnail")
            .unwrap()
            .downcast::<gtk::Picture>()
            .unwrap();
        let drawn = picture.file().and_then(|file| file.path()).unwrap();
        assert!(
            drawn.ends_with("samples/Single Photo.jpg"),
            "card drew {drawn:?}"
        );

        window.close();
        drop(connection);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    #[ignore = "requires a GTK display; run with --ignored --test-threads=1"]
    fn framed_album_photo_uses_its_skin_opening() {
        gtk::init().unwrap();
        let directory = std::env::temp_dir().join(format!(
            "pic-frame-opening-{}-{}",
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
        settings::set_bookshelf_enabled(&connection.borrow(), true).unwrap();
        settings::set_covers_enabled(&connection.borrow(), true).unwrap();

        let mut selections = Vec::new();
        for skin in [
            "vintage/blue-frame.png",
            "white-panel/white-panel-frame.png",
        ] {
            db::set_album_cover_frame(&connection.borrow(), album.id, skin).unwrap();
            let albums = db::albums(&connection.borrow()).unwrap();
            let view = build(
                &albums,
                connection.clone(),
                136,
                Rc::new(|_| {}),
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

            let button = find_descendant_with_css_class(view.upcast_ref(), "album-card")
                .unwrap()
                .downcast::<gtk::Button>()
                .unwrap();
            let cover = find_descendant_with_css_class(button.upcast_ref(), "album-cover").unwrap();
            let picture = find_descendant_with_css_class(cover.upcast_ref(), "thumbnail").unwrap();
            let opening = frame_photo_opening(&Path::new(ALBUM_COVER_THEME_DIRECTORY).join(skin));
            let expected = photo_margins(opening, cover.width_request(), cover.height_request());
            assert_eq!(
                (
                    picture.margin_start(),
                    picture.margin_top(),
                    picture.margin_end(),
                    picture.margin_bottom(),
                ),
                expected,
                "{skin}"
            );
            selections.push(expected);
            window.close();
        }

        assert_ne!(
            selections[0], selections[1],
            "different frame designs must not share one shared opening"
        );
        drop(connection);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn detects_the_transparent_opening_inside_a_frame() {
        let alpha = alpha_with_opening(100, 100, (35, 27, 33, 43));
        assert_eq!(
            opening_from_alpha(&alpha, 100, 100, 32),
            Some(PhotoOpening {
                x: 0.35,
                y: 0.27,
                width: 0.33,
                height: 0.43,
            })
        );
    }

    #[test]
    fn ignores_transparency_that_leaves_the_frame_border() {
        // A transparent band along the border is the margin around the album,
        // not the opening that shows the cover photo.
        let mut border_band = vec![255u8; 100 * 100];
        for row in 0..100 {
            for column in 0..20 {
                border_band[row * 100 + column] = 0;
            }
        }
        assert_eq!(opening_from_alpha(&border_band, 100, 100, 32), None);

        // A photo opening must be enclosed by the opaque frame on all sides.
        let mut open_around = vec![0u8; 100 * 100];
        for row in 30..70 {
            for column in 30..70 {
                open_around[row * 100 + column] = 255;
            }
        }
        assert_eq!(opening_from_alpha(&open_around, 100, 100, 32), None);
    }

    #[test]
    fn shipped_frames_report_their_own_photo_openings() {
        let root = Path::new("images/theme/album-covers");
        let expected = [
            (
                "standard/standard-frame.png",
                (0.3569, 0.2715, 0.3157, 0.4160),
            ),
            ("vintage/blue-frame.png", (0.3569, 0.2715, 0.3157, 0.4160)),
            (
                "wedding/wedding-frame.png",
                (0.2524, 0.2461, 0.5269, 0.4775),
            ),
            (
                "white-panel/white-panel-frame.png",
                (0.1826, 0.1060, 0.6273, 0.5400),
            ),
            ("pink/pink-frame.png", (0.1995, 0.1060, 0.6167, 0.5460)),
        ];
        for (relative, (x, y, width, height)) in expected {
            let opening = frame_photo_opening(&root.join(relative));
            assert!(
                (opening.x - x).abs() < 0.002,
                "{relative} left {}",
                opening.x
            );
            assert!(
                (opening.y - y).abs() < 0.002,
                "{relative} top {}",
                opening.y
            );
            assert!(
                (opening.width - width).abs() < 0.002,
                "{relative} width {}",
                opening.width
            );
            assert!(
                (opening.height - height).abs() < 0.002,
                "{relative} height {}",
                opening.height
            );
        }
    }

    #[test]
    fn photo_margins_fill_the_opening_of_every_frame_family() {
        let root = Path::new("images/theme/album-covers");
        for relative in [
            "standard/standard-frame.png",
            "wedding/wedding-frame.png",
            "white-panel/white-panel-frame.png",
            "pink/pink-frame.png",
        ] {
            let opening = frame_photo_opening(&root.join(relative));
            for width in [100, 136, 220, 300] {
                let height = (width as f64 * 500.0 / 805.0).round() as i32;
                let (left, top, right, bottom) = photo_margins(opening, width, height);
                assert!(
                    left >= 0 && top >= 0 && right >= 0 && bottom >= 0,
                    "{relative}"
                );
                assert!(left + right < width && top + bottom < height, "{relative}");

                // The photo rect must cover the opening without spilling over
                // the opaque frame by more than a rounding pixel.
                let opening_left = (opening.x * width as f64).floor() as i32;
                let opening_top = (opening.y * height as f64).floor() as i32;
                let opening_right = (width as f64 * (opening.x + opening.width)).ceil() as i32;
                let opening_bottom = (height as f64 * (opening.y + opening.height)).ceil() as i32;
                assert!(
                    left <= opening_left && top <= opening_top,
                    "{relative} at {width}"
                );
                assert!(width - right >= opening_right, "{relative} at {width}");
                assert!(height - bottom >= opening_bottom, "{relative} at {width}");
                assert!(
                    opening_left - left <= 1 && opening_top - top <= 1,
                    "{relative} at {width}"
                );
                assert!(
                    width - right - opening_right <= 1 && height - bottom - opening_bottom <= 1,
                    "{relative} at {width}"
                );
            }
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
            refresh(
                &view,
                &albums,
                connection.clone(),
                136,
                on_album.clone(),
                Rc::new(|| {}),
            );
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
            if skin {
                // The cover photo must sit inside the selected frame's own
                // transparent opening, not a single shared rectangle.
                let root = Path::new(ALBUM_COVER_THEME_DIRECTORY);
                let frame_paths = album_frame_paths_in(root);
                let index = selected_frame_index(&album, root, &frame_paths, 0).unwrap();
                let opening = frame_photo_opening(&frame_paths[index]);
                let cover =
                    find_descendant_with_css_class(button.upcast_ref(), "album-cover").unwrap();
                let (left, top, right, bottom) =
                    photo_margins(opening, cover.width_request(), cover.height_request());
                let picture =
                    find_descendant_with_css_class(cover.upcast_ref(), "thumbnail").unwrap();
                assert_eq!(
                    (
                        picture.margin_start(),
                        picture.margin_top(),
                        picture.margin_end(),
                        picture.margin_bottom(),
                    ),
                    (left, top, right, bottom),
                    "{}",
                    frame_paths[index].display(),
                );
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
        refresh(
            &view,
            &albums,
            connection.clone(),
            136,
            on_album.clone(),
            Rc::new(|| {}),
        );
        settle_gtk_layout();
        let unframed_cards = find_descendant_with_css_class(view.upcast_ref(), "albums-home-grid")
            .unwrap()
            .downcast::<gtk::FlowBox>()
            .unwrap();
        assert!(unframed_cards.vexpands());
        assert_eq!(unframed_cards.valign(), gtk::Align::Fill);
        assert!(find_descendant_with_css_class(view.upcast_ref(), "album-skin").is_none());

        refresh(
            &view,
            &[],
            connection.clone(),
            136,
            on_album,
            Rc::new(|| {}),
        );
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
        refresh(
            &view,
            &[],
            connection.clone(),
            136,
            Rc::new(|_| {}),
            Rc::new(|| {}),
        );
        settle_gtk_layout();
        assert!(!empty_cards.vexpands());
        assert_eq!(empty_cards.valign(), gtk::Align::Start);
        assert!(!empty_cards.has_css_class("albums-bookshelf-0"));
        assert!(view.has_css_class("albums-bookshelf-1"));

        settings::disable_all_album_themes(&connection.borrow()).unwrap();
        refresh(
            &view,
            &[],
            connection.clone(),
            136,
            Rc::new(|_| {}),
            Rc::new(|| {}),
        );
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
        let root = Path::new("images/theme/album-covers");
        let frames = vec![root.join("only/only-frame.png")];
        for id in 0..100 {
            assert_eq!(selected_frame_index(&album(id), root, &frames, 0), Some(0));
        }
    }

    #[test]
    fn selection_is_stable_when_album_contents_change() {
        let root = Path::new("images/theme/album-covers");
        let frames = vec![
            root.join("pink/pink-frame.png"),
            root.join("standard/standard-frame.png"),
            root.join("vintage/blue-frame.png"),
            root.join("vintage/green-frame.png"),
        ];
        let mut album = album(42);
        let selected = selected_frame_index(&album, root, &frames, 0);
        for _ in 0..100 {
            assert_eq!(selected_frame_index(&album, root, &frames, 0), selected);
        }
        album.photo_count = 83;
        album.created_at = 12345;
        assert_eq!(selected_frame_index(&album, root, &frames, 0), selected);
    }

    #[test]
    fn theme_variants_stay_inside_the_theme() {
        let root = Path::new("images/theme/album-covers");
        // A theme with one file always shows that file.
        let single = CoverTheme {
            name: "pink".to_string(),
            frames: vec![root.join("pink/pink-frame.png")],
        };
        for id in -100..100 {
            assert_eq!(
                single.frame_for(&album(id)),
                Some(&root.join("pink/pink-frame.png"))
            );
        }

        // A theme with several files spreads them over the albums, but the
        // chosen file always belongs to that theme.
        let variants = CoverTheme {
            name: "vintage".to_string(),
            frames: (0..6)
                .map(|variant| root.join(format!("vintage/{variant}-frame.png")))
                .collect(),
        };
        let mut selected = std::collections::HashSet::new();
        for id in -100..100 {
            let frame = variants.frame_for(&album(id)).unwrap();
            let index = variants
                .frames
                .iter()
                .position(|path| path == frame)
                .unwrap();
            assert_eq!(cover_theme_name(root, frame).as_deref(), Some("vintage"));
            selected.insert(index);
        }
        assert!(selected.len() > 1);
    }
}
