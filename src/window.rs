use std::cell::{Cell, RefCell};
use std::cmp::Ordering;
use std::collections::VecDeque;
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use adw::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;
use rusqlite::Connection;

use crate::albums_view;
use crate::{db, grid, infobar::InfoBar, lightbox::Lightbox, scanner, sidebar};

// This intentionally mirrors the original, system-colour-aware GTK4 look.
// It is installed above the iPhone stylesheet only while selected from the
// gear menu, so switching back reveals the dark gallery without rebuilding UI.
const STANDARD_GTK4_CSS: &str = r#"
    window, .layout-left-column, .navigation-sidebar { background: @window_bg_color; color: @theme_fg_color; }
    .layout-left-column { border-right: 1px solid alpha(@theme_fg_color, 0.12); }
    .layout-left-header, .layout-right-header { background: @headerbar_bg_color; border-bottom: none; color: @theme_fg_color; }
    .app-title { color: @theme_fg_color; text-shadow: none; }
    .layout-left-header button, .layout-right-header button { color: @theme_fg_color; }
    .layout-left-header .sidebar-toggle-button, .layout-left-header .sidebar-toggle-button image { color: @theme_fg_color; opacity: 1; }
    .layout-left-header button:hover, .layout-right-header button:hover { background: alpha(@theme_fg_color, 0.08); }
    .search-field { min-height: 34px; border-radius: 9999px; background: alpha(@theme_fg_color, 0.10); border: none; color: @theme_fg_color; box-shadow: none; }
    .search-field image { color: alpha(@theme_fg_color, 0.70); }
    .search-field entry { color: @theme_fg_color; }
    .photo-grid { background: @view_bg_color; }
    /* GtkGridView uses CSS child nodes for its cells. Keep the old `item`
       selector too so this stays harmless across GTK minor-version styling. */
    gridview.section-grid > child,
    gridview.section-grid > item {
        padding: 6px;
        margin: 0;
        background: transparent;
        box-shadow: none;
    }
    gridview.section-grid > child:hover,
    gridview.section-grid > child:selected,
    gridview.section-grid > child:focus,
    gridview.section-grid > child:active,
    gridview.section-grid > item:hover,
    gridview.section-grid > item:selected,
    gridview.section-grid > item:focus,
    gridview.section-grid > item:active {
        background: transparent;
        background-image: none;
        outline: none;
        box-shadow: none;
    }
    .group-heading-bar { background: transparent; padding: 0; }
    .photo-frame { box-shadow: none; }
    .photo-tile { border-radius: 10px; border: 2px solid transparent; background: alpha(@theme_fg_color, 0.05); box-shadow: none; }
    .photo-tile:hover { border-color: alpha(@accent_bg_color, 0.40); box-shadow: none; }
    gridview.section-grid > child:selected .photo-tile,
    gridview.section-grid > item:selected .photo-tile {
        border-color: @accent_bg_color;
        box-shadow: none;
    }
    .selection-badge { background: @accent_bg_color; box-shadow: 0 1px 3px rgba(0,0,0,0.25); }
    .offline-badge { min-width: 20px; min-height: 20px; padding: 0; border-radius: 9999px; color: #2a1a00; background: #f2c14e; font-weight: 700; }
    .sidebar-offline-badge { min-width: 16px; min-height: 16px; padding: 0; border-radius: 9999px; color: #2a1a00; background: #f2c14e; font-weight: 700; font-size: 10px; }
    .thumbnail { border-radius: 8px; }
    .missing-thumbnail { background: alpha(@theme_fg_color, 0.06); }
    .section-heading-box { margin-top: 16px; margin-bottom: 8px; }
    .section-heading { color: @theme_fg_color; font-size: 15px; text-shadow: none; }
    .section-more-btn { color: @theme_fg_color; }
    .section-more-btn:hover { background: alpha(@theme_fg_color, 0.08); }
    .photo-info-bar { background: @window_bg_color; border-top: 1px solid alpha(@theme_fg_color, 0.12); color: @theme_fg_color; }
    .info-preview { border-radius: 8px; background: alpha(@theme_fg_color, 0.06); border: 1px solid alpha(@theme_fg_color, 0.10); }
    .info-title, .metric-val { color: @theme_fg_color; }
    .metric-key, .navigation-sidebar .dim-label, .photo-info-bar .dim-label { color: alpha(@theme_fg_color, 0.55); }
    .photo-action-button { border-radius: 8px; color: @theme_fg_color; background: alpha(@theme_fg_color, 0.05); border: 1px solid alpha(@theme_fg_color, 0.10); box-shadow: none; }
    .photo-action-button:hover { background: alpha(@theme_fg_color, 0.12); }
    .favorite-btn.active, .favorite-btn.active image { color: #e01b24; }
    .one-to-one-btn:checked { color: @accent_fg_color; background: @accent_bg_color; }
    .sidebar-count { min-width: 38px; font-variant-numeric: tabular-nums; }
    .section-count { font-size: 13px; }
    .navigation-sidebar row { color: @theme_fg_color; }
    .navigation-sidebar row:hover { background: alpha(@theme_fg_color, 0.06); }
    .navigation-sidebar .sidebar-section-heading { margin-top: 8px; padding-top: 0; }
    .navigation-sidebar .sidebar-section-heading-title { color: alpha(@theme_fg_color, 0.88); font-size: inherit; font-weight: 700; }
    .navigation-sidebar row:selected { background: alpha(@accent_bg_color, 0.15); color: @theme_fg_color; }
    .navigation-sidebar .heading { color: alpha(@theme_fg_color, 0.55); }
"#;

const THEME_SETTING_KEY: &str = "appearance-theme";
const SORT_FIELD_SETTING_KEY: &str = "photo-sort-field";
const SORT_DIRECTION_SETTING_KEY: &str = "photo-sort-direction";
const GROUP_MODE_SETTING_KEY: &str = "photo-group-mode";
const GRID_THUMBNAIL_SIZE_SETTING_KEY: &str = "grid-thumbnail-size";
const DEFAULT_GRID_THUMBNAIL_SIZE: i32 = 136;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SortField {
    DateTaken,
    Name,
    FileSize,
    Dimensions,
    DateAdded,
}

impl SortField {
    fn key(self) -> &'static str {
        match self {
            Self::DateTaken => "date-taken",
            Self::Name => "name",
            Self::FileSize => "file-size",
            Self::Dimensions => "dimensions",
            Self::DateAdded => "date-added",
        }
    }

    fn from_key(key: &str) -> Self {
        match key {
            "name" => Self::Name,
            "file-size" => Self::FileSize,
            "dimensions" => Self::Dimensions,
            "date-added" => Self::DateAdded,
            _ => Self::DateTaken,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SortDirection {
    Ascending,
    Descending,
}

impl SortDirection {
    fn key(self) -> &'static str {
        match self {
            Self::Ascending => "ascending",
            Self::Descending => "descending",
        }
    }

    fn from_key(key: &str) -> Self {
        if key == "ascending" {
            Self::Ascending
        } else {
            Self::Descending
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct PhotoSort {
    field: SortField,
    direction: SortDirection,
}

fn group_mode_key(mode: grid::GroupMode) -> &'static str {
    match mode {
        grid::GroupMode::None => "none",
        grid::GroupMode::Day => "day",
        grid::GroupMode::Month => "month",
    }
}

fn group_mode_from_key(key: &str) -> grid::GroupMode {
    match key {
        "day" => grid::GroupMode::Day,
        "month" => grid::GroupMode::Month,
        _ => grid::GroupMode::None,
    }
}

fn group_date_for_sort(sort: PhotoSort) -> Option<grid::GroupDate> {
    match sort.field {
        SortField::DateTaken => Some(grid::GroupDate::Taken),
        SortField::DateAdded => Some(grid::GroupDate::Added),
        _ => None,
    }
}

fn is_library_filter(filter: sidebar::SidebarFilter) -> bool {
    matches!(
        filter,
        sidebar::SidebarFilter::All
            | sidebar::SidebarFilter::Favorites
            | sidebar::SidebarFilter::RecentlyAdded
    )
}

fn apply_gallery_grouping(
    gallery: &grid::Gallery,
    filter: sidebar::SidebarFilter,
    sort: PhotoSort,
    mode: grid::GroupMode,
) {
    // Grouping is a Library feature: Photos, Favourites and Recently Added.
    // Albums and individual folders deliberately keep the ordinary flat grid.
    if is_library_filter(filter) && mode != grid::GroupMode::None {
        let date = group_date_for_sort(sort).unwrap_or(grid::GroupDate::Taken);
        gallery.set_grouping(mode, date);
        return;
    }
    gallery.set_grouping(grid::GroupMode::None, grid::GroupDate::Taken);
}

fn grid_thumbnail_size_from_setting(connection: &Connection) -> i32 {
    db::setting(connection, GRID_THUMBNAIL_SIZE_SETTING_KEY)
        .ok()
        .flatten()
        .and_then(|value| value.parse::<i32>().ok())
        .map(|size| size.clamp(100, 300))
        .unwrap_or(DEFAULT_GRID_THUMBNAIL_SIZE)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScanJobKind {
    Import,
    Refresh,
    Maintenance,
}

#[derive(Debug)]
struct ScanUiEvent {
    generation: u64,
    event: scanner::ScanEvent,
}

#[derive(Default)]
struct ScanJobState {
    generation: u64,
    kind: Option<ScanJobKind>,
    pending: VecDeque<String>,
    active: Option<scanner::ScanControl>,
    imported_total: usize,
    failed_total: usize,
}

fn spawn_tagged_scan(
    root: String,
    generation: u64,
    ui_sender: std::sync::mpsc::Sender<ScanUiEvent>,
) -> scanner::ScanControl {
    let (scan_sender, scan_receiver) = std::sync::mpsc::channel();
    let control = scanner::spawn_scan(root, scan_sender);
    std::thread::spawn(move || {
        let mut terminal_seen = false;
        let mut indexed = 0usize;
        let mut failures = 0usize;
        while let Ok(event) = scan_receiver.recv() {
            match &event {
                scanner::ScanEvent::PhotoIndexed { .. } => indexed += 1,
                scanner::ScanEvent::Failed { .. } => failures += 1,
                scanner::ScanEvent::Finished { .. } | scanner::ScanEvent::Cancelled { .. } => {
                    terminal_seen = true;
                }
                _ => {}
            }
            if ui_sender.send(ScanUiEvent { generation, event }).is_err() {
                return;
            }
        }

        // spawn_scan reports a fatal folder-level error as Failed and then
        // closes its event channel without a Finished event. Synthesize a
        // terminal event so a multi-folder Refresh cannot stall forever.
        if !terminal_seen {
            let _ = ui_sender.send(ScanUiEvent {
                generation,
                event: scanner::ScanEvent::Finished {
                    imported: indexed,
                    failed: failures.max(1),
                },
            });
        }
    });
    control
}

#[derive(Clone)]
struct PhotoActionContext {
    connection: Rc<RefCell<Connection>>,
    gallery: Rc<RefCell<Weak<grid::Gallery>>>,
    filter: Rc<Cell<sidebar::SidebarFilter>>,
    search: Rc<RefCell<String>>,
    sort: Rc<Cell<PhotoSort>>,
    info: Rc<InfoBar>,
    selected_photo: Rc<RefCell<Option<crate::photo_object::PhotoObject>>>,
    lightbox: Weak<Lightbox>,
    sidebar: Rc<RefCell<Option<gtk::ScrolledWindow>>>,
    create_album: Rc<dyn Fn()>,
    import_folder: Rc<dyn Fn()>,
    delete_album: Rc<dyn Fn(i64)>,
    on_unavailable: Rc<dyn Fn()>,
    refresh_albums_home: Rc<dyn Fn()>,
}

include!("window/build.rs");
include!("window/search.rs");
include!("window/availability.rs");
include!("window/albums.rs");
include!("window/photo_actions.rs");
include!("window/library.rs");
