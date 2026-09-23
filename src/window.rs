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

pub(crate) mod theme;

const THEME_SETTING_KEY: &str = "appearance-theme";

/// Structural edit-mode styling shared by every visual theme.
pub(crate) const EDIT_PANEL_CSS: &str = crate::css::BASE;
const SORT_FIELD_SETTING_KEY: &str = "photo-sort-field";
const SORT_DIRECTION_SETTING_KEY: &str = "photo-sort-direction";
const GROUP_MODE_SETTING_KEY: &str = "photo-group-mode";
const GRID_THUMBNAIL_SIZE_SETTING_KEY: &str = "grid-thumbnail-size";
const LAST_VIEW_SETTING_KEY: &str = "last-view";
const LAST_VIEW_PHOTO_ID_SETTING_KEY: &str = "last-view-photo-id";
const LAST_ACTIVATED_PHOTO_ID_SETTING_KEY: &str = "last-activated-photo-id";
const LAST_VIEW_SCROLL_SETTING_KEY: &str = "last-view-scroll";
const LAST_ALBUMS_SCROLL_SETTING_KEY: &str = "last-albums-scroll";
const WINDOW_WIDTH_SETTING_KEY: &str = "window-width";
const WINDOW_HEIGHT_SETTING_KEY: &str = "window-height";
const WINDOW_MAXIMIZED_SETTING_KEY: &str = "window-maximized";
const SIDEBAR_WIDTH_FRACTION_SETTING_KEY: &str = "sidebar-width-fraction";
// Fallback before the first real layout when no thumbnail size is stored.
// A ladder level, so +/- from it stays on the canonical sizes.
const DEFAULT_GRID_THUMBNAIL_SIZE: i32 = 160;
// The historical fixed default. A stored value equal to this means the user
// never picked a size themselves, so they get the new 4-per-row default.
const LEGACY_GRID_THUMBNAIL_SIZE: i32 = 136;

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

fn grid_thumbnail_size_from_setting(connection: &Connection) -> Option<i32> {
    db::setting(connection, GRID_THUMBNAIL_SIZE_SETTING_KEY)
        .ok()
        .flatten()
        .and_then(|value| value.parse::<i32>().ok())
        .map(|size| size.clamp(100, 300))
        // The untouched legacy default is not a real preference.
        .filter(|size| *size != LEGACY_GRID_THUMBNAIL_SIZE)
}

fn sidebar_filter_setting(filter: sidebar::SidebarFilter) -> String {
    match filter {
        sidebar::SidebarFilter::All => "all".into(),
        sidebar::SidebarFilter::Favorites => "favorites".into(),
        sidebar::SidebarFilter::RecentlyAdded => "recently-added".into(),
        sidebar::SidebarFilter::Albums => "albums".into(),
        sidebar::SidebarFilter::Folder(id) => format!("folder:{id}"),
        sidebar::SidebarFilter::Album(id) => format!("album:{id}"),
    }
}

/// Decode the last destination and reject references that disappeared since
/// the previous session. A stale album/folder must never reopen as an empty,
/// unselectable page.
fn sidebar_filter_from_setting(
    value: Option<&str>,
    folders: &[db::Folder],
    albums: &[db::Album],
) -> sidebar::SidebarFilter {
    match value.unwrap_or_default() {
        "all" => sidebar::SidebarFilter::All,
        "favorites" => sidebar::SidebarFilter::Favorites,
        "albums" => sidebar::SidebarFilter::Albums,
        "recently-added" => sidebar::SidebarFilter::RecentlyAdded,
        value if value.starts_with("folder:") => value[7..]
            .parse::<i64>()
            .ok()
            .filter(|id| folders.iter().any(|folder| folder.id == *id))
            .map(sidebar::SidebarFilter::Folder)
            .unwrap_or(sidebar::SidebarFilter::RecentlyAdded),
        value if value.starts_with("album:") => value[6..]
            .parse::<i64>()
            .ok()
            .filter(|id| albums.iter().any(|album| album.id == *id))
            .map(sidebar::SidebarFilter::Album)
            .unwrap_or(sidebar::SidebarFilter::RecentlyAdded),
        _ => sidebar::SidebarFilter::RecentlyAdded,
    }
}

fn numeric_setting<T: std::str::FromStr>(connection: &Connection, key: &str) -> Option<T> {
    db::setting(connection, key).ok().flatten()?.parse().ok()
}

#[cfg(test)]
mod session_restore_tests {
    use super::{sidebar_filter_from_setting, sidebar_filter_setting};
    use crate::{db, sidebar::SidebarFilter};

    #[test]
    fn every_sidebar_destination_has_a_stable_setting() {
        assert_eq!(sidebar_filter_setting(SidebarFilter::All), "all");
        assert_eq!(sidebar_filter_setting(SidebarFilter::Favorites), "favorites");
        assert_eq!(
            sidebar_filter_setting(SidebarFilter::RecentlyAdded),
            "recently-added"
        );
        assert_eq!(sidebar_filter_setting(SidebarFilter::Albums), "albums");
        assert_eq!(sidebar_filter_setting(SidebarFilter::Folder(42)), "folder:42");
        assert_eq!(sidebar_filter_setting(SidebarFilter::Album(17)), "album:17");
    }

    #[test]
    fn existing_album_and_folder_are_restored() {
        let folders = [db::Folder {
            id: 42,
            path: "/photos".into(),
            name: "Photos".into(),
            parent_id: None,
            imported_root: true,
            watched: false,
            photo_count: 0,
            subfolder_count: 0,
            available: true,
        }];
        let albums = [db::Album {
            id: 17,
            name: "Holiday".into(),
            created_at: 0,
            photo_count: 0,
            cover_frame: None,
            cover_photo_id: None,
        }];

        assert_eq!(
            sidebar_filter_from_setting(Some("folder:42"), &folders, &albums),
            SidebarFilter::Folder(42)
        );
        assert_eq!(
            sidebar_filter_from_setting(Some("album:17"), &folders, &albums),
            SidebarFilter::Album(17)
        );
    }

    #[test]
    fn missing_or_invalid_destination_falls_back_safely() {
        for value in [None, Some("album:17"), Some("folder:nope"), Some("unknown")] {
            assert_eq!(
                sidebar_filter_from_setting(value, &[], &[]),
                SidebarFilter::RecentlyAdded
            );
        }
    }
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
    },
    FolderReady {
        generation: u64,
        path: String,
        imported_root: Result<bool, String>,
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
    operation_progress: Rc<OperationProgressUi>,
}

include!("window/operation_progress.rs");
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
include!("window/print.rs");
