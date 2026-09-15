use std::cell::{Cell, RefCell};
use std::cmp::Ordering;
use std::collections::VecDeque;
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use adw::prelude::*;
use gio::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;
use rusqlite::Connection;

use crate::albums_view;
use crate::{db, grid, infobar::InfoBar, lightbox::Lightbox, scanner, sidebar};

mod layout_settle;
use layout_settle::{should_observe_width, WidthSettleGate};

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
    listview.folder-stream { background: transparent; padding: 0 20px 24px 20px; }
    listview.folder-stream > row {
        padding: 0;
        margin: 0;
        background: transparent;
        background-image: none;
        box-shadow: none;
    }
    listview.folder-stream > row:hover,
    listview.folder-stream > row:selected,
    listview.folder-stream > row:focus,
    listview.folder-stream > row:active {
        background: transparent;
        background-image: none;
        outline: none;
        box-shadow: none;
    }
    .folder-section-header { background: transparent; }
    .folder-section-title { color: @theme_fg_color; font-weight: 700; font-size: 14px; text-shadow: none; }
    .folder-section-count { color: alpha(@theme_fg_color, 0.58); font-size: 12px; }
    .folder-section-icon { color: alpha(@theme_fg_color, 0.75); }
    .folder-section-separator { margin-top: 7px; opacity: 0.35; }
    .folder-photo-row { background: transparent; }
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
    .folder-photo-flow > flowboxchild { padding: 6px; margin: 0; min-height: 0; background: transparent; background-image: none; box-shadow: none; }
    .folder-photo-flow > flowboxchild:hover,
    .folder-photo-flow > flowboxchild:focus,
    .folder-photo-flow > flowboxchild:active,
    .folder-photo-flow > flowboxchild:selected { background: transparent; background-image: none; outline: none; box-shadow: none; }
    .folder-photo-selected.photo-tile { border-color: @accent_bg_color; box-shadow: none; }
    .folder-photo-selected .selection-badge { opacity: 1; }
    .albums-home-grid > flowboxchild { padding: 0; margin: 0; min-height: 0; }
    button.album-card { min-width: 0; padding: 0; margin: 0; background: transparent; background-image: none; border: none; box-shadow: none; }
    button.album-card:hover, button.album-card:focus, button.album-card:active { background: transparent; background-image: none; box-shadow: none; }
    gridview.section-grid > child:selected .photo-tile,
    gridview.section-grid > item:selected .photo-tile {
        border-color: @accent_bg_color;
        box-shadow: none;
    }
    .selection-badge { background: @accent_bg_color; box-shadow: 0 1px 3px rgba(0,0,0,0.25); }
    .offline-badge { min-width: 20px; min-height: 20px; padding: 0; border-radius: 9999px; color: #2a1a00; background: #f2c14e; font-weight: 700; }
    .sidebar-offline-badge { min-width: 16px; min-height: 16px; padding: 0; border-radius: 9999px; color: #2a1a00; background: #f2c14e; font-weight: 700; font-size: 10px; }
    .folder-disclosure { opacity: 0.68; }
    .navigation-sidebar .folder-count { color: alpha(@theme_fg_color, 0.68); }
    .thumbnail { border-radius: 8px; }
    .missing-thumbnail { background: alpha(@theme_fg_color, 0.06); }
    .section-heading-box { margin-top: 16px; margin-bottom: 8px; }
    .section-heading { color: @theme_fg_color; font-size: 15px; text-shadow: none; }
    .section-more-btn { color: @theme_fg_color; }
    .section-more-btn:hover { background: alpha(@theme_fg_color, 0.08); }
    .photo-info-bar { background: @window_bg_color; border-top: 1px solid alpha(@theme_fg_color, 0.12); color: @theme_fg_color; }
    .info-preview { border-radius: 8px; background: alpha(@theme_fg_color, 0.06); border: 1px solid alpha(@theme_fg_color, 0.10); }
    .edit-panel-tabs button { min-height: 34px; }
    .filter-grid > flowboxchild { padding: 0; margin: 0; min-width: 0; background: transparent; }
    button.filter-tile, button.filter-tile:hover, button.filter-tile:active, button.filter-tile:checked { padding: 0; border: none; background: transparent; box-shadow: none; outline: none; }
    .filter-tile-preview { min-width: 67px; min-height: 67px; padding: 0; border: 2px solid transparent; border-radius: 12px; background: transparent; box-shadow: none; overflow: hidden; }
    .filter-tile-preview > border { border: none; background: transparent; }
    button.filter-tile:checked .filter-tile-preview { border-color: @accent_bg_color; }
    .filter-tile-label { font-size: 11px; }
    .info-title, .metric-val { color: @theme_fg_color; }
    .metric-key, .navigation-sidebar .dim-label, .photo-info-bar .dim-label { color: alpha(@theme_fg_color, 0.55); }
    .photo-action-button { border-radius: 8px; color: @theme_fg_color; background: alpha(@theme_fg_color, 0.05); border: 1px solid alpha(@theme_fg_color, 0.10); box-shadow: none; }
    .photo-action-button:hover { background: alpha(@theme_fg_color, 0.12); }
    .photo-context-menu, .photo-context-menu viewport { background: @window_bg_color; }
    .photo-context-menu { border: 1px solid alpha(@theme_fg_color, 0.18); border-radius: 8px; box-shadow: 0 5px 18px alpha(#000000, 0.40); }
    .photo-context-menu button { color: @theme_fg_color; min-height: 32px; padding: 6px 10px; }
    .photo-context-menu button:hover { background: alpha(@theme_fg_color, 0.10); }
    button.clear-action-button { color: #2e3436; background: #e6e6e6; border: 1px solid #9a9a9a; }
    button.clear-action-button:hover { color: #1f2325; background: #f0f0f0; border-color: #777777; }
    button.clear-action-button:active { background: #d2d2d2; }
    .favorite-btn.active, .favorite-btn.active image { color: #e01b24; }
    .favorite-badge { color: #e01b24; }
    .edited-badge { color: @theme_fg_color; }
    .one-to-one-btn:checked { color: @accent_fg_color; background: @accent_bg_color; }
    .sidebar-count { min-width: 38px; font-variant-numeric: tabular-nums; }
    .section-count { font-size: 13px; }
    .scroll-scrub-date {
        padding: 7px 10px;
        border-radius: 8px;
        background: alpha(@window_bg_color, 0.88);
        color: @theme_fg_color;
        font-weight: 700;
        box-shadow: 0 1px 4px alpha(black, 0.22);
    }
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
        // Folder is an internal presentation mode and is never persisted as
        // the user's Library grouping preference.
        grid::GroupMode::Folder => "none",
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
    gallery.set_favorite_indicators_visible(filter != sidebar::SidebarFilter::Favorites);

    // Folder browsing is one continuous photo stream. Folder headers are real
    // rows inside that stream, so they scroll away naturally with the photos.
    if matches!(filter, sidebar::SidebarFilter::Folder(_)) {
        gallery.set_grouping(grid::GroupMode::Folder, grid::GroupDate::Taken);
        return;
    }

    // User-selectable date grouping remains a Library-only feature.
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
    FolderRefresh,
    Maintenance,
}

/// Only direct user actions may authorize filesystem discovery/photo scanning.
/// Passive UI and filesystem signals are represented here so they cannot be
/// accidentally mapped onto a scan job later.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PhotoScanRequestReason {
    UserFolderRefresh,
    ManualLibraryRefresh,
    ImportFolder,
    FilesystemNotification,
    DebouncedWatchRefresh,
    AvailabilityUpdate,
    ProgrammaticSidebarSelection,
}

impl PhotoScanRequestReason {
    fn scan_kind(self) -> Option<ScanJobKind> {
        match self {
            Self::UserFolderRefresh => Some(ScanJobKind::FolderRefresh),
            Self::ManualLibraryRefresh => Some(ScanJobKind::Refresh),
            Self::ImportFolder => Some(ScanJobKind::Import),
            // Raw filesystem signals never cross the scan boundary directly.
            // Only the coalesced/debounced watch request may start a targeted scan.
            Self::DebouncedWatchRefresh => Some(ScanJobKind::FolderRefresh),
            Self::FilesystemNotification
            | Self::AvailabilityUpdate
            | Self::ProgrammaticSidebarSelection => None,
        }
    }

    fn trace_label(self) -> &'static str {
        match self {
            Self::UserFolderRefresh => "user_click",
            Self::ManualLibraryRefresh => "manual_refresh",
            Self::ImportFolder => "import",
            Self::FilesystemNotification => "filesystem_notification",
            Self::DebouncedWatchRefresh => "watch_refresh",
            Self::AvailabilityUpdate => "availability_update",
            Self::ProgrammaticSidebarSelection => "programmatic_sidebar_selection",
        }
    }
}

/// Resolve a watched library folder to the imported root that owns its scan.
/// A discovered subfolder can be watched independently, but scanning still
/// uses the existing imported-root boundary so folder ownership cannot be
/// rewritten by an automatic refresh.
fn watch_scan_root(folders: &[db::Folder], watched_id: i64) -> Option<String> {
    let by_id = folders
        .iter()
        .map(|folder| (folder.id, folder))
        .collect::<std::collections::HashMap<_, _>>();
    let mut current = by_id.get(&watched_id).copied();
    let mut seen = std::collections::HashSet::new();
    while let Some(folder) = current {
        if !seen.insert(folder.id) {
            return None;
        }
        if folder.imported_root {
            return Some(folder.path.clone());
        }
        current = folder.parent_id.and_then(|id| by_id.get(&id).copied());
    }
    None
}

#[derive(Debug)]
struct ScanUiEvent {
    generation: u64,
    event: scanner::ScanEvent,
}

#[derive(Debug)]
enum RefreshPrepareEvent {
    LibraryReady {
        generation: u64,
        roots: Result<Vec<String>, String>,
        elapsed_ms: u128,
    },
    FolderReady {
        generation: u64,
        path: String,
        imported_root: Result<bool, String>,
        elapsed_ms: u128,
    },
}

#[derive(Default)]
struct ScanJobState {
    generation: u64,
    kind: Option<ScanJobKind>,
    pending: VecDeque<String>,
    active: Option<scanner::ScanControl>,
    imported_total: usize,
    failed_total: usize,
    stop_requested: bool,
}

impl ScanJobState {
    fn preempt_maintenance(&mut self) -> bool {
        if self.kind != Some(ScanJobKind::Maintenance) {
            return false;
        }
        if let Some(active) = self.active.take() {
            active.cancel();
        }
        self.generation = self.generation.wrapping_add(1);
        self.kind = None;
        self.pending.clear();
        self.stop_requested = false;
        true
    }

    /// Crosses the authorization boundary for photo discovery/scanning.
    /// Passive reasons leave every job field untouched.
    fn authorize_photo_scan(&mut self, reason: PhotoScanRequestReason) -> Option<u64> {
        let kind = reason.scan_kind()?;
        if let Some(previous) = self.active.take() {
            previous.cancel();
        }
        self.generation = self.generation.wrapping_add(1);
        self.kind = Some(kind);
        self.pending.clear();
        self.imported_total = 0;
        self.failed_total = 0;
        self.stop_requested = false;
        Some(self.generation)
    }
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
    refresh_albums_home: Rc<dyn Fn(&[db::Album])>,
    navigate_to_folder: Rc<dyn Fn(i64, i64)>,
    open_collage: Rc<dyn Fn(Vec<i64>)>,
    open_edit: Rc<dyn Fn(i64)>,
    edit_clipboard: Rc<RefCell<Option<String>>>,
    window: glib::WeakRef<gtk::Window>,
    context_menu_host: Rc<RefCell<Option<glib::WeakRef<gtk::Overlay>>>>,
}

include!("window/build.rs");

/// Folder scrollbar-scrub sampling state.
///
/// A direct scrub samples its decode target immediately in the frame the jump
/// happens, then at most once per 50 ms while the drag continues, and never
/// after it ends. GtkListView emits tiny ±1 px anchor corrections during
/// drags; coalescing samples keeps the decode queue owned by one destination
/// at a time instead of one per correction.
#[derive(Default)]
struct FolderScrollbarScrub {
    active: bool,
    last_sample: Option<Instant>,
}

impl FolderScrollbarScrub {
    const SAMPLE_INTERVAL: Duration = Duration::from_millis(50);

    fn begin(&mut self) {
        self.active = true;
    }

    fn end(&mut self) {
        self.active = false;
        self.last_sample = None;
    }

    /// Returns true (and records the sample time) when a new scrub sample
    /// should be queued now.
    fn sample_due(&mut self, now: Instant) -> bool {
        if !self.active {
            return false;
        }
        let due = self.last_sample.map_or(true, |last| {
            now.duration_since(last) >= Self::SAMPLE_INTERVAL
        });
        if due {
            self.last_sample = Some(now);
        }
        due
    }
}

#[cfg(test)]
mod folder_scroll_tests {
    use super::{Duration, FolderScrollbarScrub, Instant};

    #[test]
    fn thumb_drag_samples_immediately_then_waits_for_the_interval() {
        let started = Instant::now();
        let mut scrub = FolderScrollbarScrub::default();

        assert!(!scrub.sample_due(started));

        scrub.begin();
        assert!(scrub.sample_due(started));
        assert!(!scrub.sample_due(started + Duration::from_millis(49)));
        assert!(scrub.sample_due(started + Duration::from_millis(50)));

        scrub.end();
        assert!(!scrub.sample_due(started + Duration::from_millis(100)));
    }
}

#[cfg(test)]
mod photo_scan_authorization_tests {
    use super::{watch_scan_root, PhotoScanRequestReason, ScanJobKind, ScanJobState};
    use crate::db;

    fn authorized_kind(reason: PhotoScanRequestReason) -> Option<ScanJobKind> {
        let mut job = ScanJobState::default();
        let generation = job.authorize_photo_scan(reason);
        assert_eq!(generation.is_some(), job.kind.is_some());
        assert_eq!(generation.unwrap_or_default(), job.generation);
        job.kind
    }

    fn assert_denied_without_job_mutation(reason: PhotoScanRequestReason) {
        let mut job = ScanJobState {
            generation: 41,
            pending: std::collections::VecDeque::from(["keep".to_string()]),
            imported_total: 7,
            failed_total: 3,
            stop_requested: true,
            ..Default::default()
        };

        assert_eq!(job.authorize_photo_scan(reason), None);
        assert_eq!(job.generation, 41);
        assert_eq!(job.kind, None);
        assert_eq!(
            job.pending,
            std::collections::VecDeque::from(["keep".to_string()])
        );
        assert_eq!(job.imported_total, 7);
        assert_eq!(job.failed_total, 3);
        assert!(job.stop_requested);
    }

    #[test]
    fn filesystem_notifications_cannot_authorize_a_refresh() {
        assert_denied_without_job_mutation(PhotoScanRequestReason::FilesystemNotification);
    }

    #[test]
    fn watched_subfolder_scans_through_its_imported_root() {
        let folders = vec![
            db::Folder {
                id: 1,
                path: "/photos".to_string(),
                name: "photos".to_string(),
                parent_id: None,
                imported_root: true,
                watched: false,
                photo_count: 0,
                subfolder_count: 1,
                available: true,
            },
            db::Folder {
                id: 2,
                path: "/photos/screenshots".to_string(),
                name: "screenshots".to_string(),
                parent_id: Some(1),
                imported_root: false,
                watched: true,
                photo_count: 0,
                subfolder_count: 0,
                available: true,
            },
        ];

        assert_eq!(watch_scan_root(&folders, 2).as_deref(), Some("/photos"));
    }

    #[test]
    fn debounced_watch_refresh_authorizes_targeted_scanning() {
        assert_eq!(
            authorized_kind(PhotoScanRequestReason::DebouncedWatchRefresh),
            Some(ScanJobKind::FolderRefresh)
        );
    }

    #[test]
    fn availability_updates_cannot_authorize_a_refresh() {
        assert_denied_without_job_mutation(PhotoScanRequestReason::AvailabilityUpdate);
    }

    #[test]
    fn programmatic_sidebar_selection_cannot_authorize_a_refresh() {
        assert_denied_without_job_mutation(PhotoScanRequestReason::ProgrammaticSidebarSelection);
    }

    #[test]
    fn manual_library_refresh_authorizes_scanning() {
        assert_eq!(
            authorized_kind(PhotoScanRequestReason::ManualLibraryRefresh),
            Some(ScanJobKind::Refresh)
        );
    }

    #[test]
    fn manual_refresh_can_preempt_startup_thumbnail_recovery() {
        let control = crate::scanner::ScanControl::default();
        let mut job = ScanJobState {
            generation: 7,
            kind: Some(ScanJobKind::Maintenance),
            active: Some(control.clone()),
            ..Default::default()
        };

        assert!(job.preempt_maintenance());
        assert_eq!(job.kind, None);
        assert!(job.active.is_none());
        assert_eq!(job.generation, 8);
        assert!(control.is_cancelled());
    }

    #[test]
    fn explicit_folder_refresh_authorizes_scanning() {
        assert_eq!(
            authorized_kind(PhotoScanRequestReason::UserFolderRefresh),
            Some(ScanJobKind::FolderRefresh)
        );
    }

    #[test]
    fn import_authorization_remains_unchanged() {
        assert_eq!(
            authorized_kind(PhotoScanRequestReason::ImportFolder),
            Some(ScanJobKind::Import)
        );
    }
}
include!("window/search.rs");
include!("window/availability.rs");
include!("window/albums.rs");
include!("window/photo_actions.rs");
include!("window/library.rs");
