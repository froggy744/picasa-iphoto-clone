use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use gtk::prelude::*;
use gtk4 as gtk;

use crate::db::{Album, Folder, SidebarCounts};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarFilter {
    All,
    Favorites,
    RecentlyAdded,
    Albums,
    Folder(i64),
    Album(i64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderDisplayMode {
    Tree,
    ImportedOnly,
}

impl FolderDisplayMode {
    pub fn from_setting(value: Option<&str>) -> Self {
        match value {
            Some("imported-only") => Self::ImportedOnly,
            _ => Self::Tree,
        }
    }

    pub fn setting_value(self) -> &'static str {
        match self {
            Self::Tree => "tree",
            Self::ImportedOnly => "imported-only",
        }
    }
}

pub const FOLDER_DISPLAY_MODE_SETTING_KEY: &str = "folder-display-mode";

/// Photo/source routing for the context menu. A GVfs FUSE path is a network
/// source even though it is an absolute path; regular /mnt and /run/media
/// mounts remain local.
pub(crate) fn is_network_photo_path(path: &str) -> bool {
    if crate::db::is_remote_path(path) {
        return true;
    }
    let parts = path.split('/').collect::<Vec<_>>();
    parts.len() >= 5
        && parts[0].is_empty()
        && parts[1] == "run"
        && parts[2] == "user"
        && parts[3].parse::<u32>().is_ok()
        && parts[4] == "gvfs"
}

/// Only physical/local filesystem folders belong in Folders. Network share
/// photos indexed via GVfs have absolute FUSE paths, but these are still
/// network-backed and must not create a second folder tree in the local pane.
/// Ordinary mounts such as /mnt/4TBP and /run/media/peet/... remain local.
fn is_local_sidebar_folder(folder: &Folder) -> bool {
    let path = folder.path.as_str();
    if crate::db::is_remote_path(path) {
        return false;
    }
    // GVfs's FUSE root is /run/user/UID/gvfs; exclude both the root and
    // its children (including smb-share:, sftp:, etc.). Filter by mount
    // location, not an SMB-specific name, to cover every network protocol.
    let parts = path.split('/').collect::<Vec<_>>();
    if parts.len() >= 5
        && parts[0].is_empty()
        && parts[1] == "run"
        && parts[2] == "user"
        && parts[3].parse::<u32>().is_ok()
        && parts[4] == "gvfs"
    {
        return false;
    }
    // A scan may have created synthetic /run/user/UID ancestors purely for
    // GVfs photos. They are not useful local photo sources. Explicitly
    // imported local roots are still allowed (but never GVfs itself).
    if !folder.imported_root
        && (path == "/run"
            || path == "/run/user"
            || (parts.len() == 4
                && parts[0].is_empty()
                && parts[1] == "run"
                && parts[2] == "user"
                && parts[3].parse::<u32>().is_ok()))
    {
        return false;
    }
    true
}

#[derive(Debug)]
struct SidebarState {
    library_expanded: bool,
    albums_expanded: bool,
    folders_expanded: bool,
    shares_expanded: bool,
    expanded_folders: HashSet<i64>,
    folder_display_mode: FolderDisplayMode,
    pinned: Cell<bool>,
    hover_open: Cell<bool>,
}

impl Default for SidebarState {
    fn default() -> Self {
        Self {
            library_expanded: true,
            albums_expanded: true,
            folders_expanded: true,
            shares_expanded: true,
            expanded_folders: HashSet::new(),
            folder_display_mode: FolderDisplayMode::Tree,
            pinned: Cell::new(true),
            hover_open: Cell::new(false),
        }
    }
}

const STATE_KEY: &str = "picasa-sidebar-state";
const LIBRARY_LIST_KEY: &str = "picasa-sidebar-library-list";
const LIBRARY_REVEALER_KEY: &str = "picasa-sidebar-library-revealer";
const LIBRARY_INDICATOR_KEY: &str = "picasa-sidebar-library-indicator";
const ALBUM_LIST_KEY: &str = "picasa-sidebar-album-list";
const ALBUM_REVEALER_KEY: &str = "picasa-sidebar-album-revealer";
const ALBUM_INDICATOR_KEY: &str = "picasa-sidebar-album-indicator";
const FOLDER_LIST_KEY: &str = "picasa-sidebar-folder-list";
const FOLDER_SCROLL_KEY: &str = "picasa-sidebar-folder-scroll";
const FOLDER_REVEALER_KEY: &str = "picasa-sidebar-folder-revealer";
const FOLDER_INDICATOR_KEY: &str = "picasa-sidebar-folder-indicator";
const CURRENT_FILTER_KEY: &str = "picasa-sidebar-current-filter";
const FILTER_SYNCING_KEY: &str = "picasa-sidebar-filter-syncing";
const FOLDER_REFRESH_KEY: &str = "picasa-sidebar-folder-refresh";
const REFRESH_GATE_KEY: &str = "picasa-sidebar-refresh-gate";
const ALBUM_PANE_ANIMATION_MS: u32 = 250;
const STARTUP_VISIBLE_ALBUM_ROWS: usize = 5;

/// Share the window's refresh sensitivity with existing and future menus.
pub fn bind_refresh_gate(scrolled: &gtk::ScrolledWindow, gate: &gtk::Button) {
    if let Some(list) = stored_widget::<gtk::ListBox>(scrolled, FOLDER_LIST_KEY) {
        unsafe {
            list.set_data(REFRESH_GATE_KEY, gate.downgrade());
        }
    }
}
const FOLDER_STATISTICS_KEY: &str = "picasa-sidebar-folder-statistics";
const FOLDER_REMOVE_KEY: &str = "picasa-sidebar-folder-remove";
const FOLDER_FAVORITE_KEY: &str = "picasa-sidebar-folder-favorite";
const FOLDER_WATCH_KEY: &str = "picasa-sidebar-folder-watch";
const SHARE_LIST_KEY: &str = "picasa-sidebar-share-list";
const SHARE_SCROLL_KEY: &str = "picasa-sidebar-share-scroll";
// A newer refresh must never be overwritten by an older worker's count result.
static SHARE_COUNT_GENERATION: AtomicU64 = AtomicU64::new(0);
thread_local! {
    // Preserve previously calculated counts while share rows are rebuilt.
    static SHARE_COUNT_CACHE: RefCell<HashMap<i64, (String, i64)>> = RefCell::new(HashMap::new());
}
const SHARE_OPEN_KEY: &str = "picasa-sidebar-share-open";
const SHARE_RETRY_KEY: &str = "picasa-sidebar-share-retry";
const FOLDER_MODE_TOGGLE_KEY: &str = "picasa-sidebar-folder-mode-toggle";
const FOLDER_MODE_CHANGED_KEY: &str = "picasa-sidebar-folder-mode-changed";
const KEYBOARD_GRID_TARGET_KEY: &str = "picasa-sidebar-keyboard-grid-target";
const SCROLL_LOCATION_FOLDER_KEY: &str = "picasa-sidebar-scroll-location-folder";

thread_local! {
    // Guards the coalesced idle pass that follows the Folder scroll position.
    static SCROLL_LOCATION_SCHEDULED: Cell<bool> = const { Cell::new(false) };
}

pub fn build(
    folders: &[Folder],
    albums: &[Album],
    counts: SidebarCounts,
    on_filter: impl Fn(SidebarFilter) + 'static,
    on_create_album: Rc<dyn Fn()>,
    on_import_folder: Rc<dyn Fn()>,
    on_delete_album: Rc<dyn Fn(i64)>,
    on_unavailable: Rc<dyn Fn()>,
    on_refresh_folder: Rc<dyn Fn(String)>,
    on_folder_statistics: Rc<dyn Fn(Folder)>,
    on_remove_folder: Rc<dyn Fn(Folder)>,
    on_folder_favorite: Rc<dyn Fn(Folder, bool)>,
    on_folder_watch: Rc<dyn Fn(Folder, bool)>,
    on_add_share: Rc<dyn Fn()>,
    on_open_share: Rc<dyn Fn(Folder)>,
    on_retry_share: Rc<dyn Fn(Folder)>,
    folder_display_mode: FolderDisplayMode,
    on_folder_display_mode_changed: Rc<dyn Fn(FolderDisplayMode)>,
) -> gtk::ScrolledWindow {
    let on_filter: Rc<dyn Fn(SidebarFilter)> = Rc::new(on_filter);
    let mut initial_state = SidebarState::default();
    initial_state.folder_display_mode = folder_display_mode;
    let state = Rc::new(RefCell::new(initial_state));
    let filter_syncing = Rc::new(Cell::new(false));

    // Keep the public return type exactly as before because window/build.rs,
    // window/availability.rs and window/albums.rs all store this as a
    // GtkScrolledWindow. The outer scroller itself never scrolls; only the
    // dedicated folder scroller below is allowed to scroll.
    let outer = gtk::ScrolledWindow::new();
    outer.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Never);
    outer.set_hexpand(true);
    outer.set_vexpand(true);
    outer.add_css_class("navigation-sidebar");

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.set_hexpand(true);
    root.set_vexpand(true);
    root.set_margin_start(10);
    root.set_margin_end(10);
    root.set_margin_top(4);
    root.set_margin_bottom(12);

    // LIBRARY: collapsible.
    let (library_heading, library_indicator) = collapsible_heading("Library", None, "", true, None);
    root.append(&library_heading);

    let library_list = section_list();
    populate_library(&library_list, counts, &on_unavailable);
    connect_filter_list(&library_list, on_filter.clone(), filter_syncing.clone());
    let library_revealer = gtk::Revealer::new();
    library_revealer.set_transition_type(gtk::RevealerTransitionType::SlideDown);
    library_revealer.set_reveal_child(true);
    library_revealer.set_child(Some(&library_list));
    root.append(&library_revealer);

    {
        let state = state.clone();
        let revealer = library_revealer.clone();
        let indicator = library_indicator.clone();
        library_indicator.connect_clicked(move |_| {
            let expanded = !state.borrow().library_expanded;
            state.borrow_mut().library_expanded = expanded;
            revealer.set_reveal_child(expanded);
            indicator.set_icon_name(if expanded {
                "pan-down-symbolic"
            } else {
                "pan-end-symbolic"
            });
        });
    }

    // ALBUMS + FOLDERS: use GTK's native vertical split control. GtkPaned
    // owns pointer tracking and allocation while the divider is dragged, which
    // avoids the feedback loop caused by resizing a widget underneath a
    // GestureDrag attached to that same moving divider.
    //
    // The Albums heading stays outside the paned. The Folders heading lives at
    // the top of the end pane, so it always follows Albums directly instead of
    // becoming bottom-anchored when the folder rows are collapsed.
    let (album_heading, album_indicator) = collapsible_heading(
        "Albums",
        Some(on_create_album.clone()),
        "Create album",
        true,
        Some({
            let on_filter = on_filter.clone();
            Rc::new(move || on_filter(SidebarFilter::Albums))
        }),
    );
    album_heading.add_css_class("sidebar-sticky-heading");
    root.append(&album_heading);

    let album_list = section_list();
    connect_filter_list(&album_list, on_filter.clone(), filter_syncing.clone());

    let album_scroll = gtk::ScrolledWindow::new();
    album_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    album_scroll.set_hexpand(true);
    album_scroll.set_vexpand(true);
    // This caps only the initial/natural request. GtkPaned may allocate more
    // when the user drags its native handle, while a short album list still
    // opens at its natural height without a large empty block.
    album_scroll.set_propagate_natural_height(true);
    album_scroll.set_max_content_height(260);
    album_scroll.set_child(Some(&album_list));

    let album_revealer = gtk::Revealer::new();
    album_revealer.set_transition_type(gtk::RevealerTransitionType::SlideDown);
    album_revealer.set_reveal_child(true);
    album_revealer.set_hexpand(true);
    album_revealer.set_vexpand(true);
    album_revealer.set_child(Some(&album_scroll));

    let (folder_heading, folder_indicator) = collapsible_heading(
        "Folders",
        Some(on_import_folder.clone()),
        "Import Folder",
        true,
        None,
    );
    folder_heading.add_css_class("sidebar-sticky-heading");

    let folder_mode_toggle = gtk::Button::from_icon_name(match folder_display_mode {
        FolderDisplayMode::Tree => "folder-symbolic",
        FolderDisplayMode::ImportedOnly => "view-list-symbolic",
    });
    folder_mode_toggle.add_css_class("flat");
    folder_mode_toggle.set_focusable(false);
    folder_mode_toggle.set_size_request(24, 24);
    set_folder_mode_toggle_presentation(&folder_mode_toggle, folder_display_mode);
    // Keep heading controls in the same left-to-right order everywhere:
    // section-specific action, add, collapse. For Folders that means
    // folder-mode, +, collapse. The first child is always the heading label.
    if let Some(label) = folder_heading.first_child() {
        folder_heading.insert_child_after(&folder_mode_toggle, Some(&label));
    } else {
        folder_heading.prepend(&folder_mode_toggle);
    }

    let folder_list = section_list();
    connect_filter_list(&folder_list, on_filter.clone(), filter_syncing.clone());

    let folder_scroll = gtk::ScrolledWindow::new();
    folder_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    folder_scroll.set_hexpand(true);
    folder_scroll.set_vexpand(true);
    folder_scroll.set_child(Some(&folder_list));

    let folder_revealer = gtk::Revealer::new();
    folder_revealer.set_transition_type(gtk::RevealerTransitionType::SlideDown);
    folder_revealer.set_reveal_child(true);
    folder_revealer.set_hexpand(true);
    folder_revealer.set_vexpand(true);
    folder_revealer.set_child(Some(&folder_scroll));

    // Keep the Folders heading in the paned end child. Even with the rows
    // collapsed, the heading remains the end child's minimum visible content,
    // so the native handle cannot drag over or hide it.
    let folder_section = gtk::Box::new(gtk::Orientation::Vertical, 0);
    folder_section.set_hexpand(true);
    folder_section.set_vexpand(true);
    folder_section.append(&folder_heading);
    // The second, independently resizable split is installed once the
    // Network Shares widgets have been constructed below.

    let section_paned = gtk::Paned::new(gtk::Orientation::Vertical);
    section_paned.set_hexpand(true);
    section_paned.set_vexpand(true);
    // Preserve the user's Album height when the window itself grows/shrinks;
    // the Folders pane absorbs ordinary window-size changes. The user can still
    // resize both sections directly with GtkPaned's native handle.
    section_paned.set_resize_start_child(false);
    section_paned.set_resize_end_child(true);
    section_paned.set_shrink_start_child(true);
    section_paned.set_shrink_end_child(false);
    section_paned.set_start_child(Some(&album_revealer));
    section_paned.set_end_child(Some(&folder_section));
    root.append(&section_paned);

    // Remember the user's last native divider position so collapsing Albums
    // does not throw away their preferred split. 260 is only a pre-layout
    // fallback; once GTK allocates the paned we remember its real position.
    let saved_album_pane_position = Rc::new(Cell::new(260));
    let album_pane_animating = Rc::new(Cell::new(false));
    let album_pane_animation_generation = Rc::new(Cell::new(0_u64));
    {
        let state = state.clone();
        let saved_position = saved_album_pane_position.clone();
        let animating = album_pane_animating.clone();
        section_paned.connect_position_notify(move |paned| {
            let position = paned.position();
            // Only treat native user dragging as a new saved position. During
            // our collapse/expand animation the pane position changes every
            // frame and must not overwrite the user's preferred split.
            if state.borrow().albums_expanded
                && !animating.get()
                && paned.start_child().is_some()
                && position > 0
            {
                saved_position.set(position);
            }
        });
    }

    // Keep GtkPaned as the only resize mechanism. For section collapse/expand,
    // animate the native pane position over the same duration as GtkRevealer,
    // then detach the Albums pane only after collapse completes. This keeps the
    // native divider future-proof while giving Albums the same smooth motion as
    // the other sidebar sections.
    album_revealer.set_transition_duration(ALBUM_PANE_ANIMATION_MS);

    let set_albums_expanded: Rc<dyn Fn(bool)> = {
        let state = state.clone();
        let paned = section_paned.clone();
        let revealer = album_revealer.clone();
        let indicator = album_indicator.clone();
        let saved_position = saved_album_pane_position.clone();
        let animating = album_pane_animating.clone();
        let animation_generation = album_pane_animation_generation.clone();
        Rc::new(move |expanded| {
            if state.borrow().albums_expanded == expanded {
                return;
            }

            if !expanded && !animating.get() {
                let position = paned.position();
                if position > 0 {
                    saved_position.set(position);
                }
            }

            state.borrow_mut().albums_expanded = expanded;
            indicator.set_icon_name(if expanded {
                "pan-down-symbolic"
            } else {
                "pan-end-symbolic"
            });
            indicator.set_tooltip_text(Some(if expanded { "Collapse" } else { "Expand" }));

            let generation = animation_generation.get().wrapping_add(1);
            animation_generation.set(generation);
            animating.set(true);

            if expanded {
                // Reattach hidden at zero height, then reveal and grow the
                // native pane. If a collapse was interrupted, continue from
                // the current position instead of jumping back to zero.
                let was_detached = paned.start_child().is_none();
                if was_detached {
                    revealer.set_reveal_child(false);
                    paned.set_start_child(Some(&revealer));
                    paned.set_position(0);
                }

                let from = paned.position().max(0);
                revealer.set_reveal_child(true);
                let target = saved_position.get().max(0);
                animate_sidebar_pane_position(
                    &paned,
                    from,
                    target,
                    animation_generation.clone(),
                    generation,
                    animating.clone(),
                    None,
                );
            } else {
                let from = paned.position().max(0);
                revealer.set_reveal_child(false);

                let paned_for_finish = paned.clone();
                let revealer_for_finish = revealer.clone();
                let state_for_finish = state.clone();
                let finish: Rc<dyn Fn()> = Rc::new(move || {
                    if !state_for_finish.borrow().albums_expanded {
                        paned_for_finish.set_start_child(None::<&gtk::Widget>);
                        revealer_for_finish.set_reveal_child(false);
                    }
                });

                animate_sidebar_pane_position(
                    &paned,
                    from,
                    0,
                    animation_generation.clone(),
                    generation,
                    animating.clone(),
                    Some(finish),
                );
            }
        })
    };

    {
        let state = state.clone();
        let set_expanded = set_albums_expanded.clone();
        album_indicator.connect_clicked(move |_| {
            let expanded = !state.borrow().albums_expanded;
            set_expanded(expanded);
        });
    }

    // Double-click anywhere on the Albums heading to collapse/expand.
    {
        let state = state.clone();
        let set_expanded = set_albums_expanded.clone();
        let double_click = gtk::GestureClick::new();
        double_click.set_button(1);
        double_click.set_propagation_phase(gtk::PropagationPhase::Capture);
        double_click.connect_pressed(move |gesture, n_press, _, _| {
            if n_press == 2 {
                let expanded = !state.borrow().albums_expanded;
                set_expanded(expanded);
                gesture.set_state(gtk::EventSequenceState::Claimed);
            }
        });
        album_heading.add_controller(double_click);
    }

    // Folder and Network Share collapse handlers are installed below, after
    // the shared native paned has been constructed.

    {
        let list = folder_list.clone();
        let state = state.clone();
        let toggle = folder_mode_toggle.clone();
        let on_folder_display_mode_changed = on_folder_display_mode_changed.clone();
        folder_mode_toggle.connect_clicked(move |_| {
            let new_mode = {
                let mut state = state.borrow_mut();
                state.folder_display_mode = match state.folder_display_mode {
                    FolderDisplayMode::Tree => FolderDisplayMode::ImportedOnly,
                    FolderDisplayMode::ImportedOnly => FolderDisplayMode::Tree,
                };
                state.folder_display_mode
            };
            set_folder_mode_toggle_presentation(&toggle, new_mode);
            rebuild_folder_list_from_rows(&list, &state);
            on_folder_display_mode_changed(new_mode);
        });
    }

    // Folder rows install their context menu while they are created, so these
    // callbacks must be available on the ListBox before populate_folders().
    unsafe {
        folder_list.set_data(FOLDER_REFRESH_KEY, on_refresh_folder.clone());
        folder_list.set_data(FOLDER_STATISTICS_KEY, on_folder_statistics);
        folder_list.set_data(FOLDER_REMOVE_KEY, on_remove_folder.clone());
        folder_list.set_data(FOLDER_FAVORITE_KEY, on_folder_favorite);
        folder_list.set_data(FOLDER_WATCH_KEY, on_folder_watch);
    }

    // NETWORK SHARES: registered remote sources (Phase 1: SMB). Kept strictly
    // separate from local Folders; the section lives below the Folders pane.
    let (share_heading, share_indicator) = collapsible_heading(
        "Network Shares",
        Some(on_add_share.clone()),
        "Add Network Share",
        true,
        None,
    );
    share_heading.add_css_class("sidebar-sticky-heading");
    let share_list = section_list();
    connect_filter_list(&share_list, on_filter.clone(), filter_syncing.clone());
    unsafe {
        outer.set_data(SHARE_LIST_KEY, share_list.clone());
        share_list.set_data(SHARE_OPEN_KEY, on_open_share);
        share_list.set_data(SHARE_RETRY_KEY, on_retry_share);
        share_list.set_data(FOLDER_REFRESH_KEY, on_refresh_folder);
        share_list.set_data(FOLDER_REMOVE_KEY, on_remove_folder);
    }
    // Native drag handle between Folders and Network Shares. Both sections
    // have independent scrollbars; dragging does not rebuild their lists.
    let share_scroll = gtk::ScrolledWindow::new();
    share_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    share_scroll.set_hexpand(true);
    share_scroll.set_vexpand(true);
    share_scroll.set_child(Some(&share_list));
    unsafe { outer.set_data(SHARE_SCROLL_KEY, share_scroll.clone()); }
    let share_section = gtk::Box::new(gtk::Orientation::Vertical, 0);
    share_section.set_hexpand(true);
    share_section.set_vexpand(true);
    share_section.append(&share_heading);
    let share_revealer = gtk::Revealer::new();
    share_revealer.set_transition_type(gtk::RevealerTransitionType::SlideDown);
    share_revealer.set_transition_duration(ALBUM_PANE_ANIMATION_MS);
    share_revealer.set_reveal_child(true);
    share_revealer.set_vexpand(true);
    share_revealer.set_child(Some(&share_scroll));
    share_section.append(&share_revealer);
    let folder_share_paned = gtk::Paned::new(gtk::Orientation::Vertical);
    // Use the SAME native separator and animation as Albums. Do not paint
    // an extra CSS border/gradient over it: that produced a double line.
    folder_share_paned.set_hexpand(true);
    folder_share_paned.set_vexpand(true);
    // Match the working Albums split: retain the upper pane's size as the
    // window changes, and let the lower pane absorb the change. Both remain
    // manually resizable with the native GtkPaned handle.
    folder_share_paned.set_resize_start_child(false);
    folder_share_paned.set_resize_end_child(true);
    folder_share_paned.set_shrink_start_child(true);
    folder_share_paned.set_shrink_end_child(false);
    folder_share_paned.set_start_child(Some(&folder_revealer));
    folder_share_paned.set_end_child(Some(&share_section));
    folder_section.append(&folder_share_paned);

    // Match Albums: native drag position is the single source of truth, while
    // collapse animations must not overwrite the user's chosen position.
    let saved_folder_position = Rc::new(Cell::new(200));
    let folder_animating = Rc::new(Cell::new(false));
    let folder_generation = Rc::new(Cell::new(0_u64));
    {
        let state = state.clone();
        let saved = saved_folder_position.clone();
        let animating = folder_animating.clone();
        folder_share_paned.connect_position_notify(move |paned| {
            let position = paned.position();
            if state.borrow().folders_expanded && !animating.get()
                && paned.start_child().is_some() && position > 0 {
                saved.set(position);
            }
        });
    }
    folder_revealer.set_transition_duration(ALBUM_PANE_ANIMATION_MS);
    let set_folders_expanded: Rc<dyn Fn(bool)> = {
        let state = state.clone();
        let paned = folder_share_paned.clone();
        let revealer = folder_revealer.clone();
        let indicator = folder_indicator.clone();
        let saved = saved_folder_position.clone();
        let animating = folder_animating.clone();
        let generation = folder_generation.clone();
        Rc::new(move |expanded| {
            if state.borrow().folders_expanded == expanded { return; }
            if !expanded && !animating.get() && paned.position() > 0 {
                saved.set(paned.position());
            }
            state.borrow_mut().folders_expanded = expanded;
            indicator.set_icon_name(if expanded { "pan-down-symbolic" } else { "pan-end-symbolic" });
            indicator.set_tooltip_text(Some(if expanded { "Collapse" } else { "Expand" }));
            let current = generation.get().wrapping_add(1);
            generation.set(current);
            animating.set(true);
            if expanded {
                if paned.start_child().is_none() {
                    revealer.set_reveal_child(false);
                    paned.set_start_child(Some(&revealer));
                    paned.set_position(0);
                }
                let from = paned.position().max(0);
                revealer.set_reveal_child(true);
                animate_sidebar_pane_position(&paned, from, saved.get().max(0),
                    generation.clone(), current, animating.clone(), None);
            } else {
                let from = paned.position().max(0);
                revealer.set_reveal_child(false);
                let paned_finish = paned.clone();
                let revealer_finish = revealer.clone();
                let state_finish = state.clone();
                let finish: Rc<dyn Fn()> = Rc::new(move || {
                    if !state_finish.borrow().folders_expanded {
                        paned_finish.set_start_child(None::<&gtk::Widget>);
                        revealer_finish.set_reveal_child(false);
                    }
                });
                animate_sidebar_pane_position(&paned, from, 0,
                    generation.clone(), current, animating.clone(), Some(finish));
            }
        })
    };
    {
        let state = state.clone();
        let toggle = set_folders_expanded.clone();
        folder_indicator.connect_clicked(move |_| {
            let expanded = !state.borrow().folders_expanded;
            toggle(expanded);
        });
    }
    {
        let state = state.clone();
        let toggle = set_folders_expanded.clone();
        let double_click = gtk::GestureClick::new();
        double_click.set_button(1);
        double_click.set_propagation_phase(gtk::PropagationPhase::Capture);
        double_click.connect_pressed(move |gesture, n_press, _, _| {
            if n_press == 2 {
                let expanded = !state.borrow().folders_expanded;
                toggle(expanded);
                gesture.set_state(gtk::EventSequenceState::Claimed);
            }
        });
        folder_heading.add_controller(double_click);
    }

    // Network Shares matches the Albums disclosure: keep the heading visible,
    // save the user's split, and animate the native pane to its minimum body.
    let share_animating = Rc::new(Cell::new(false));
    let share_generation = Rc::new(Cell::new(0_u64));
    let set_shares_expanded: Rc<dyn Fn(bool)> = {
        let state = state.clone();
        let revealer = share_revealer.clone();
        let indicator = share_indicator.clone();
        let section = share_section.clone();
        let paned = folder_share_paned.clone();
        let saved = saved_folder_position.clone();
        let folder_animating = folder_animating.clone();
        let animating = share_animating.clone();
        let generation = share_generation.clone();
        Rc::new(move |expanded| {
            if state.borrow().shares_expanded == expanded { return; }
            state.borrow_mut().shares_expanded = expanded;
            indicator.set_icon_name(if expanded { "pan-down-symbolic" } else { "pan-end-symbolic" });
            indicator.set_tooltip_text(Some(if expanded { "Collapse" } else { "Expand" }));
            let current = generation.get().wrapping_add(1);
            generation.set(current);
            animating.set(true);
            folder_animating.set(true);
            let from = paned.position().max(0);
            if expanded {
                section.set_vexpand(true);
                revealer.set_vexpand(true);
                revealer.set_reveal_child(true);
                // Restore the last manually selected split, not the collapsed
                // position (which belongs only to the temporary animation).
                let target = saved.get().max(0);
                let folder_animating = folder_animating.clone();
                let finish: Rc<dyn Fn()> = Rc::new(move || folder_animating.set(false));
                animate_sidebar_pane_position(&paned, from, target,
                    generation.clone(), current, animating.clone(), Some(finish));
            } else {
                if state.borrow().folders_expanded && from > 0 { saved.set(from); }
                revealer.set_reveal_child(false);
                revealer.set_vexpand(false);
                section.set_vexpand(false);
                let paned = paned.clone();
                let generation = generation.clone();
                let animating = animating.clone();
                let folder_animating = folder_animating.clone();
                glib::idle_add_local_once(move || {
                    if generation.get() != current { return; }
                    // GTK has now measured the remaining visible heading.
                    let target = paned.max_position().max(from);
                    let folder_animating_finish = folder_animating.clone();
                    let finish: Rc<dyn Fn()> = Rc::new(move || folder_animating_finish.set(false));
                    animate_sidebar_pane_position(&paned, from, target,
                        generation, current, animating, Some(finish));
                });
            }
        })
    };
    {
        let state = state.clone();
        let toggle = set_shares_expanded.clone();
        share_indicator.connect_clicked(move |_| {
            let expanded = !state.borrow().shares_expanded;
            toggle(expanded);
        });
    }
    {
        let state = state.clone();
        let toggle = set_shares_expanded.clone();
        let double_click = gtk::GestureClick::new();
        double_click.set_button(1);
        double_click.set_propagation_phase(gtk::PropagationPhase::Capture);
        double_click.connect_pressed(move |gesture, n_press, _, _| {
            if n_press == 2 {
                let expanded = !state.borrow().shares_expanded;
                toggle(expanded);
                gesture.set_state(gtk::EventSequenceState::Claimed);
            }
        });
        share_heading.add_controller(double_click);
    }

    // First map: reserve enough room for the Network Shares heading AND list.
    // One-shot setup only; never fight subsequent pointer drags.
    {
        let paned = folder_share_paned.clone();
        let saved = saved_folder_position.clone();
        let applied = Rc::new(Cell::new(false));
        folder_share_paned.connect_map(move |_| {
            if applied.replace(true) { return; }
            let paned = paned.clone();
            let saved = saved.clone();
            glib::idle_add_local_once(move || {
                let available = paned.height();
                if available > 160 {
                    let target = (available * 2 / 3).clamp(80, available - 80);
                    saved.set(target);
                    paned.set_position(target);
                }
            });
        });
    }

    populate_albums(&album_list, albums, &on_delete_album);
    populate_folders(&folder_list, folders, &state, &on_unavailable);
    refresh_network_shares(&outer, folders, &on_unavailable);

    outer.set_child(Some(&root));

    // On first presentation, give Albums enough room for five rows when five
    // are available. Measure the actual row widgets instead of assuming a
    // fixed pixel height, so font/theme/DPI changes keep the startup split
    // correct. A short album list still uses only its natural height.
    {
        let paned = section_paned.clone();
        let album_list = album_list.clone();
        let saved_position = saved_album_pane_position.clone();
        let applied = Rc::new(Cell::new(false));
        let applied_for_map = applied.clone();
        section_paned.connect_map(move |_| {
            if applied_for_map.replace(true) {
                return;
            }

            let paned = paned.clone();
            let album_list = album_list.clone();
            let saved_position = saved_position.clone();
            glib::idle_add_local_once(move || {
                let desired = list_natural_height_for_rows(&album_list, STARTUP_VISIBLE_ALBUM_ROWS);
                if desired <= 0 {
                    return;
                }

                let max_position = paned.max_position();
                let target = if max_position > 0 {
                    desired.min(max_position)
                } else {
                    desired
                };
                if target > 0 {
                    saved_position.set(target);
                    paned.set_position(target);
                }
            });
        });
    }

    // Store stable widget/state handles on the existing GtkScrolledWindow so
    // refresh() and append_folder() can update only the relevant sections
    // without changing any caller-facing API.
    unsafe {
        outer.set_data(STATE_KEY, state);
        outer.set_data(LIBRARY_LIST_KEY, library_list);
        outer.set_data(LIBRARY_REVEALER_KEY, library_revealer);
        outer.set_data(LIBRARY_INDICATOR_KEY, library_indicator);
        outer.set_data(ALBUM_LIST_KEY, album_list);
        outer.set_data(ALBUM_REVEALER_KEY, album_revealer);
        outer.set_data(ALBUM_INDICATOR_KEY, album_indicator);
        outer.set_data(FOLDER_LIST_KEY, folder_list);
        outer.set_data(FOLDER_SCROLL_KEY, folder_scroll);
        outer.set_data(FOLDER_REVEALER_KEY, folder_revealer);
        outer.set_data(FOLDER_INDICATOR_KEY, folder_indicator);
        outer.set_data(FOLDER_MODE_TOGGLE_KEY, folder_mode_toggle);
        outer.set_data(FOLDER_MODE_CHANGED_KEY, on_folder_display_mode_changed);
        outer.set_data(FILTER_SYNCING_KEY, filter_syncing);
    }

    let keyboard = gtk::EventControllerKey::new();
    keyboard.set_propagation_phase(gtk::PropagationPhase::Capture);
    let sidebar_for_keyboard = outer.clone();
    keyboard.connect_key_pressed(move |_, key, _, modifiers| {
        handle_keyboard_navigation(&sidebar_for_keyboard, key, modifiers)
    });
    outer.add_controller(keyboard);

    outer
}

/// Set the widget that receives focus when Tab leaves the sidebar.
pub fn set_keyboard_grid_target(scrolled: &gtk::ScrolledWindow, target: &gtk::Widget) {
    unsafe {
        scrolled.set_data(KEYBOARD_GRID_TARGET_KEY, target.clone());
    }

    let sidebar_for_grid = scrolled.clone();
    let grid_keyboard = gtk::EventControllerKey::new();
    grid_keyboard.set_propagation_phase(gtk::PropagationPhase::Capture);
    grid_keyboard.connect_key_pressed(move |_, key, _, _| {
        if key != gtk::gdk::Key::Tab && key != gtk::gdk::Key::ISO_Left_Tab {
            return glib::Propagation::Proceed;
        }
        let sections = navigation_sections(&sidebar_for_grid);
        if let Some(row) = selected_navigation_row(&sections)
            .or_else(|| sections.last().and_then(|(_, rows)| rows.first().cloned()))
        {
            select_navigation_row(&sections, &row);
        }
        glib::Propagation::Stop
    });
    target.add_controller(grid_keyboard);
}

fn handle_keyboard_navigation(
    scrolled: &gtk::ScrolledWindow,
    key: gtk::gdk::Key,
    modifiers: gtk::gdk::ModifierType,
) -> glib::Propagation {
    let shift_tab = key == gtk::gdk::Key::ISO_Left_Tab
        || (key == gtk::gdk::Key::Tab && modifiers.contains(gtk::gdk::ModifierType::SHIFT_MASK));
    if shift_tab {
        return focus_previous_sidebar_section(scrolled);
    }
    if key == gtk::gdk::Key::Tab {
        let target = unsafe {
            scrolled
                .data::<gtk::Widget>(KEYBOARD_GRID_TARGET_KEY)
                .map(|target| target.as_ref().clone())
        };
        if let Some(target) = target {
            target.grab_focus();
            return glib::Propagation::Stop;
        }
    }

    let sections = navigation_sections(scrolled);
    let all_rows = sections
        .iter()
        .flat_map(|(_, rows)| rows.iter().cloned())
        .collect::<Vec<_>>();
    if all_rows.is_empty() {
        return glib::Propagation::Proceed;
    }

    match key {
        gtk::gdk::Key::Up | gtk::gdk::Key::Down => {
            let selected = selected_navigation_row(&sections);
            let current = selected
                .as_ref()
                .and_then(|row| all_rows.iter().position(|candidate| candidate == row));
            let next = match (current, key == gtk::gdk::Key::Down) {
                (Some(index), true) => (index + 1).min(all_rows.len() - 1),
                (Some(index), false) => index.saturating_sub(1),
                (None, true) => 0,
                (None, false) => all_rows.len() - 1,
            };
            select_navigation_row(&sections, &all_rows[next]);
            glib::Propagation::Stop
        }
        gtk::gdk::Key::Left | gtk::gdk::Key::Right => {
            let Some((list, row)) = selected_folder_navigation_row(scrolled) else {
                return glib::Propagation::Proceed;
            };
            let rows = folder_rows(&list);
            let folder = unsafe {
                row.data::<Folder>("picasa-folder-record")
                    .map(|folder| folder.as_ref().clone())
            };
            let Some(folder) = folder else {
                return glib::Propagation::Proceed;
            };
            let has_children = unsafe {
                row.data::<bool>("picasa-folder-has-children")
                    .is_some_and(|value| *value.as_ref())
            };
            let state = sidebar_state(scrolled);
            let Some(state) = state else {
                return glib::Propagation::Proceed;
            };
            let expanded = state.borrow().expanded_folders.contains(&folder.id);

            if key == gtk::gdk::Key::Right {
                if has_children && !expanded {
                    state.borrow_mut().expanded_folders.insert(folder.id);
                    rebuild_folder_list_from_rows(&list, &state);
                    set_active_filter(scrolled, SidebarFilter::Folder(folder.id));
                } else if has_children {
                    let visible_rows = folder_rows(&list);
                    if let Some(child) = visible_rows
                        .iter()
                        .skip_while(|candidate| *candidate != &row)
                        .nth(1)
                    {
                        list.select_row(Some(child));
                    }
                }
            } else if has_children && expanded {
                state.borrow_mut().expanded_folders.remove(&folder.id);
                rebuild_folder_list_from_rows(&list, &state);
                set_active_filter(scrolled, SidebarFilter::Folder(folder.id));
            } else if let Some(parent_id) = folder.parent_id {
                if rows.iter().any(|candidate| unsafe {
                    candidate
                        .data::<Folder>("picasa-folder-record")
                        .is_some_and(|parent| parent.as_ref().id == parent_id)
                }) {
                    set_active_filter(scrolled, SidebarFilter::Folder(parent_id));
                }
            }
            glib::Propagation::Stop
        }
        _ => glib::Propagation::Proceed,
    }
}

fn folder_rows(list: &gtk::ListBox) -> Vec<gtk::ListBoxRow> {
    let mut rows = Vec::new();
    let mut child = list.first_child();
    while let Some(widget) = child {
        let next = widget.next_sibling();
        if let Ok(row) = widget.downcast::<gtk::ListBoxRow>() {
            if unsafe { row.data::<Folder>("picasa-folder-record") }.is_some() {
                rows.push(row);
            }
        }
        child = next;
    }
    rows
}

fn navigation_sections(
    scrolled: &gtk::ScrolledWindow,
) -> Vec<(gtk::ListBox, Vec<gtk::ListBoxRow>)> {
    [LIBRARY_LIST_KEY, ALBUM_LIST_KEY, FOLDER_LIST_KEY]
        .into_iter()
        .filter_map(|key| {
            let list = stored_widget::<gtk::ListBox>(scrolled, key)?;
            let mut rows = Vec::new();
            let mut child = list.first_child();
            while let Some(widget) = child {
                let next = widget.next_sibling();
                if let Ok(row) = widget.downcast::<gtk::ListBoxRow>() {
                    let selectable =
                        unsafe { row.data::<SidebarFilter>("picasa-filter") }.is_some();
                    if selectable {
                        rows.push(row);
                    }
                }
                child = next;
            }
            (!rows.is_empty()).then_some((list, rows))
        })
        .collect()
}

fn selected_navigation_row(
    sections: &[(gtk::ListBox, Vec<gtk::ListBoxRow>)],
) -> Option<gtk::ListBoxRow> {
    sections.iter().find_map(|(list, _)| list.selected_row())
}

fn selected_folder_navigation_row(
    scrolled: &gtk::ScrolledWindow,
) -> Option<(gtk::ListBox, gtk::ListBoxRow)> {
    let list = stored_widget::<gtk::ListBox>(scrolled, FOLDER_LIST_KEY)?;
    Some((list.clone(), list.selected_row()?))
}

fn select_navigation_row(sections: &[(gtk::ListBox, Vec<gtk::ListBoxRow>)], row: &gtk::ListBoxRow) {
    for (list, rows) in sections {
        if rows.iter().any(|candidate| candidate == row) {
            list.select_row(Some(row));
            row.grab_focus();
            break;
        }
    }
}

fn focus_previous_sidebar_section(scrolled: &gtk::ScrolledWindow) -> glib::Propagation {
    let sections = navigation_sections(scrolled);
    let Some(selected) = selected_navigation_row(&sections) else {
        return glib::Propagation::Proceed;
    };
    let Some(section_index) = sections
        .iter()
        .position(|(_, rows)| rows.iter().any(|row| row == &selected))
    else {
        return glib::Propagation::Proceed;
    };
    if section_index == 0 {
        return glib::Propagation::Proceed;
    }
    if let Some((list, rows)) = sections.get(section_index - 1) {
        if let Some(row) = rows.last() {
            list.select_row(Some(row));
            row.grab_focus();
        }
    }
    glib::Propagation::Stop
}

pub fn refresh(
    scrolled: &gtk::ScrolledWindow,
    folders: &[Folder],
    albums: &[Album],
    counts: SidebarCounts,
    on_create_album: Rc<dyn Fn()>,
    on_import_folder: Rc<dyn Fn()>,
    on_delete_album: Rc<dyn Fn(i64)>,
    on_unavailable: Rc<dyn Fn()>,
) {
    // The callback parameters stay in the signature for full compatibility
    // with the existing window submodules. Heading + buttons were wired once
    // in build(), so refresh only replaces dynamic rows.
    let _ = (&on_create_album, &on_import_folder);

    let Some(state) = sidebar_state(scrolled) else {
        return;
    };
    let Some(library_list) = stored_widget::<gtk::ListBox>(scrolled, LIBRARY_LIST_KEY) else {
        return;
    };
    let Some(album_list) = stored_widget::<gtk::ListBox>(scrolled, ALBUM_LIST_KEY) else {
        return;
    };
    let Some(folder_list) = stored_widget::<gtk::ListBox>(scrolled, FOLDER_LIST_KEY) else {
        return;
    };
    let folder_scroll_value = folder_scroll_value(scrolled);

    clear_list(&library_list);
    populate_library(&library_list, counts, &on_unavailable);

    clear_list(&album_list);
    populate_albums(&album_list, albums, &on_delete_album);

    clear_list(&folder_list);
    populate_folders(&folder_list, folders, &state, &on_unavailable);
    // The Network Shares section is a first-class partner of the Folders tree:
    // refresh() is the shared path for add/remove share flows, so it must
    // rebuild the share rows too or the section goes stale.
    refresh_network_shares(scrolled, folders, &on_unavailable);

    if let Some(revealer) = stored_widget::<gtk::Revealer>(scrolled, LIBRARY_REVEALER_KEY) {
        revealer.set_reveal_child(state.borrow().library_expanded);
    }
    if let Some(indicator) = stored_widget::<gtk::Button>(scrolled, LIBRARY_INDICATOR_KEY) {
        indicator.set_icon_name(if state.borrow().library_expanded {
            "pan-down-symbolic"
        } else {
            "pan-end-symbolic"
        });
    }
    if let Some(revealer) = stored_widget::<gtk::Revealer>(scrolled, ALBUM_REVEALER_KEY) {
        revealer.set_reveal_child(state.borrow().albums_expanded);
    }
    if let Some(indicator) = stored_widget::<gtk::Button>(scrolled, ALBUM_INDICATOR_KEY) {
        indicator.set_icon_name(if state.borrow().albums_expanded {
            "pan-down-symbolic"
        } else {
            "pan-end-symbolic"
        });
    }
    if let Some(revealer) = stored_widget::<gtk::Revealer>(scrolled, FOLDER_REVEALER_KEY) {
        let expanded = state.borrow().folders_expanded;
        revealer.set_reveal_child(expanded);
        revealer.set_vexpand(expanded);
    }
    if let Some(indicator) = stored_widget::<gtk::Button>(scrolled, FOLDER_INDICATOR_KEY) {
        indicator.set_icon_name(if state.borrow().folders_expanded {
            "pan-down-symbolic"
        } else {
            "pan-end-symbolic"
        });
    }

    if let Some(filter) = current_filter(scrolled) {
        set_active_filter(scrolled, filter);
    }
    restore_folder_scroll(scrolled, folder_scroll_value);
    let scrolled_for_location = scrolled.clone();
    glib::idle_add_local_once(move || apply_scroll_location(&scrolled_for_location));
}

pub fn refresh_library_counts(
    scrolled: &gtk::ScrolledWindow,
    counts: SidebarCounts,
    on_unavailable: &Rc<dyn Fn()>,
) {
    let Some(library_list) = stored_widget::<gtk::ListBox>(scrolled, LIBRARY_LIST_KEY) else {
        return;
    };

    clear_list(&library_list);
    populate_library(&library_list, counts, on_unavailable);

    if let Some(filter) = current_filter(scrolled) {
        set_active_filter(scrolled, filter);
    }
}

/// Rebuild only the folder rows from fresh folder data while preserving the
/// scroll position.
pub fn refresh_folder_rows(
    scrolled: &gtk::ScrolledWindow,
    folders: &[Folder],
    on_unavailable: &Rc<dyn Fn()>,
) {
    let Some(state) = sidebar_state(scrolled) else {
        return;
    };
    let Some(folder_list) = stored_widget::<gtk::ListBox>(scrolled, FOLDER_LIST_KEY) else {
        return;
    };
    let folder_scroll_value = folder_scroll_value(scrolled);
    clear_list(&folder_list);
    // Registered network shares are presented in their own section; the
    // Folders tree stays strictly local.
    let local_folders: Vec<Folder> = folders
        .iter()
        .filter(|folder| is_local_sidebar_folder(folder))
        .cloned()
        .collect();
    populate_folders(&folder_list, &local_folders, &state, on_unavailable);
    refresh_network_shares(scrolled, folders, on_unavailable);
    restore_folder_scroll(scrolled, folder_scroll_value);
}

/// Add the folder row immediately when an import starts. The normal refresh
/// remains responsible for counts and ordering once the import completes.
pub fn append_folder(
    scrolled: &gtk::ScrolledWindow,
    folder: &Folder,
    on_unavailable: Rc<dyn Fn()>,
) {
    // Network scan events must never append GVfs or remote URI folders into
    // the local pane, even temporarily before the next full refresh.
    if !is_local_sidebar_folder(folder) {
        return;
    }
    let Some(state) = sidebar_state(scrolled) else {
        return;
    };
    let Some(list) = stored_widget::<gtk::ListBox>(scrolled, FOLDER_LIST_KEY) else {
        return;
    };

    // Avoid duplicates while a scan emits FolderStarted events.
    let mut row = list.first_child();
    while let Some(current) = row {
        if let Ok(existing) = current.clone().downcast::<gtk::ListBoxRow>() {
            if let Some(value) = unsafe { existing.data::<SidebarFilter>("picasa-filter") } {
                if unsafe { *value.as_ref() } == SidebarFilter::Folder(folder.id) {
                    return;
                }
            }
        }
        row = current.next_sibling();
    }

    // During progressive import we may not yet have all parent rows. Add the
    // new row as a top-level entry; the next normal refresh rebuilds the exact
    // path hierarchy.
    append_folder_row(&list, folder, 0, false, &state, &on_unavailable);
}

/// Visually track the folder represented by the leading visible photo in the grid.
///
/// This is deliberately separate from ListBox selection: the selected sidebar
/// row continues to describe the active filter (All, Favourites, Album, etc.),
/// while this marker answers "where is the photo I am looking at?".  Because it
/// never selects the row, it cannot trigger the sidebar navigation callback.
pub fn set_scroll_location(scrolled: &gtk::ScrolledWindow, folder_id: Option<i64>) {
    let folder_id = folder_id.filter(|id| *id > 0).unwrap_or_default();
    let previous = unsafe {
        scrolled
            .data::<i64>(SCROLL_LOCATION_FOLDER_KEY)
            .map(|value| *value.as_ref())
            .unwrap_or_default()
    };
    if previous == folder_id {
        return;
    }

    unsafe {
        scrolled.set_data(SCROLL_LOCATION_FOLDER_KEY, folder_id);
    }
    // Coalesce the scroll-follow update into a single idle pass. Rebuilding the
    // sidebar tree when a folder's ancestors expand measured 42-77 ms per folder
    // change, which stalled the scroll frame.
    if SCROLL_LOCATION_SCHEDULED.with(|scheduled| scheduled.replace(true)) {
        return;
    }
    let scrolled = scrolled.clone();
    glib::idle_add_local_once(move || {
        SCROLL_LOCATION_SCHEDULED.with(|scheduled| scheduled.set(false));
        apply_scroll_location(&scrolled);
    });
}

fn apply_scroll_location(scrolled: &gtk::ScrolledWindow) {
    let Some(list) = stored_widget::<gtk::ListBox>(scrolled, FOLDER_LIST_KEY) else {
        return;
    };

    clear_scroll_location_rows(&list);

    let folder_id = unsafe {
        scrolled
            .data::<i64>(SCROLL_LOCATION_FOLDER_KEY)
            .map(|value| *value.as_ref())
            .unwrap_or_default()
    };
    if folder_id <= 0 {
        return;
    }

    let folders = unsafe {
        list.data::<Vec<Folder>>("picasa-folder-cache")
            .map(|data| data.as_ref().clone())
            .unwrap_or_default()
    };
    if folders.is_empty() {
        return;
    }

    let Some(state) = sidebar_state(scrolled) else {
        return;
    };
    let mode = state.borrow().folder_display_mode;
    let display_id = match mode {
        FolderDisplayMode::Tree => {
            // Passive scroll-follow must never expand/rebuild the sidebar. If
            // the exact child row is hidden under a collapsed ancestor, mark
            // the nearest ancestor that is already rendered instead.
            if row_for_filter(&list, SidebarFilter::Folder(folder_id)).is_some() {
                Some(folder_id)
            } else {
                folder_ancestor_ids(&folders, folder_id)
                    .into_iter()
                    .find(|ancestor_id| {
                        row_for_filter(&list, SidebarFilter::Folder(*ancestor_id)).is_some()
                    })
            }
        }
        FolderDisplayMode::ImportedOnly => {
            Some(imported_root_for_folder(&folders, folder_id).unwrap_or(folder_id))
        }
    };

    let Some(display_id) = display_id else {
        return;
    };
    let Some(row) = row_for_filter(&list, SidebarFilter::Folder(display_id)) else {
        return;
    };
    row.add_css_class("sidebar-scroll-location");
    scroll_folder_row_into_view(scrolled, &row);
}

fn clear_scroll_location_rows(list: &gtk::ListBox) {
    let mut child = list.first_child();
    while let Some(widget) = child {
        let next = widget.next_sibling();
        if let Ok(row) = widget.downcast::<gtk::ListBoxRow>() {
            row.remove_css_class("sidebar-scroll-location");
        }
        child = next;
    }
}

fn row_for_filter(list: &gtk::ListBox, filter: SidebarFilter) -> Option<gtk::ListBoxRow> {
    let mut child = list.first_child();
    while let Some(widget) = child {
        let next = widget.next_sibling();
        if let Ok(row) = widget.downcast::<gtk::ListBoxRow>() {
            let matches = unsafe {
                row.data::<SidebarFilter>("picasa-filter")
                    .is_some_and(|value| unsafe { *value.as_ref() == filter })
            };
            if matches {
                return Some(row);
            }
        }
        child = next;
    }
    None
}

fn imported_root_for_folder(folders: &[Folder], folder_id: i64) -> Option<i64> {
    let by_id: HashMap<i64, &Folder> = folders.iter().map(|folder| (folder.id, folder)).collect();
    let mut current = Some(folder_id);
    let mut visited = HashSet::new();
    while let Some(id) = current {
        if !visited.insert(id) {
            break;
        }
        let folder = by_id.get(&id)?;
        if folder.imported_root {
            return Some(folder.id);
        }
        current = folder.parent_id;
    }
    None
}

fn folder_ancestor_ids(folders: &[Folder], folder_id: i64) -> Vec<i64> {
    let by_id: HashMap<i64, &Folder> = folders.iter().map(|folder| (folder.id, folder)).collect();
    let mut result = Vec::new();
    let mut current = by_id.get(&folder_id).and_then(|folder| folder.parent_id);
    let mut visited = HashSet::new();
    while let Some(id) = current {
        if !visited.insert(id) {
            break;
        }
        result.push(id);
        current = by_id.get(&id).and_then(|folder| folder.parent_id);
    }
    result
}

fn scroll_folder_row_into_view(scrolled: &gtk::ScrolledWindow, row: &gtk::ListBoxRow) {
    let Some(folder_scroll) = stored_widget::<gtk::ScrolledWindow>(scrolled, FOLDER_SCROLL_KEY)
    else {
        return;
    };
    let row = row.clone();
    glib::idle_add_local_once(move || {
        // Bounds relative to the scroller (the folder list now sits below the
        // album section in the shared scroller, so allocation y is not enough).
        let Some(bounds) = row.compute_bounds(&folder_scroll) else {
            return;
        };
        let adjustment = folder_scroll.vadjustment();
        let page = adjustment.page_size();
        let row_top = bounds.y() as f64;
        let row_bottom = row_top + bounds.height() as f64;

        let target = if row_top < 0.0 {
            Some(adjustment.value() + row_top)
        } else if row_bottom > page {
            Some(adjustment.value() + row_bottom - page)
        } else {
            None
        };

        if let Some(target) = target {
            let upper = (adjustment.upper() - page).max(adjustment.lower());
            adjustment.set_value(target.clamp(adjustment.lower(), upper));
        }
    });
}

/// Folder rows currently presented to the user, in sidebar order.
///
/// Tree mode returns only the rows that are presently visible in the expanded
/// tree. Imported-only mode naturally returns just the explicitly imported
/// roots because those are the only rows built for that mode.
pub fn visible_folder_ids(scrolled: &gtk::ScrolledWindow) -> Vec<i64> {
    let Some(list) = stored_widget::<gtk::ListBox>(scrolled, FOLDER_LIST_KEY) else {
        return Vec::new();
    };

    let mut ids = Vec::new();
    let mut child = list.first_child();
    while let Some(widget) = child {
        let next = widget.next_sibling();
        if let Ok(row) = widget.downcast::<gtk::ListBoxRow>() {
            let filter = unsafe {
                row.data::<SidebarFilter>("picasa-filter")
                    .map(|filter| *filter.as_ref())
            };
            if let Some(SidebarFilter::Folder(folder_id)) = filter {
                ids.push(folder_id);
            }
        }
        child = next;
    }
    ids
}

pub fn set_folder_display_mode(scrolled: &gtk::ScrolledWindow, mode: FolderDisplayMode) -> bool {
    let Some(state) = sidebar_state(scrolled) else {
        return false;
    };
    if state.borrow().folder_display_mode == mode {
        return false;
    }

    state.borrow_mut().folder_display_mode = mode;
    if let Some(toggle) = stored_widget::<gtk::Button>(scrolled, FOLDER_MODE_TOGGLE_KEY) {
        set_folder_mode_toggle_presentation(&toggle, mode);
    }
    if let Some(list) = stored_widget::<gtk::ListBox>(scrolled, FOLDER_LIST_KEY) {
        rebuild_folder_list_from_rows(&list, &state);
    }
    let callback = unsafe {
        scrolled
            .data::<Rc<dyn Fn(FolderDisplayMode)>>(FOLDER_MODE_CHANGED_KEY)
            .map(|callback| callback.as_ref().clone())
    };
    if let Some(callback) = callback {
        callback(mode);
    }
    true
}

pub fn set_active_filter(scrolled: &gtk::ScrolledWindow, filter: SidebarFilter) {
    unsafe {
        scrolled.set_data(CURRENT_FILTER_KEY, filter);
    }

    // The old scroll-location marker is separate from ListBox selection. Once
    // a real destination becomes active it must be cleared even for Folder
    // destinations, otherwise GTK can show two highlighted folder rows: the
    // newly selected row plus the stale scroll-location marker.
    set_scroll_location(scrolled, None);

    let syncing = unsafe {
        scrolled
            .data::<Rc<Cell<bool>>>(FILTER_SYNCING_KEY)
            .map(|data| data.as_ref().clone())
    };
    let Some(syncing) = syncing else {
        return;
    };

    let folder_scroll_value = matches!(filter, SidebarFilter::Folder(_))
        .then(|| folder_scroll_value(scrolled))
        .flatten();

    syncing.set(true);
    if let Some(library_list) = stored_widget::<gtk::ListBox>(scrolled, LIBRARY_LIST_KEY) {
        library_list.unselect_all();
    }
    if let Some(album_list) = stored_widget::<gtk::ListBox>(scrolled, ALBUM_LIST_KEY) {
        album_list.unselect_all();
    }
    if let Some(folder_list) = stored_widget::<gtk::ListBox>(scrolled, FOLDER_LIST_KEY) {
        folder_list.unselect_all();
    }
    if let Some(share_list) = stored_widget::<gtk::ListBox>(scrolled, SHARE_LIST_KEY) {
        share_list.unselect_all();
    }

    match filter {
        SidebarFilter::All | SidebarFilter::Favorites | SidebarFilter::RecentlyAdded => {
            select_matching_row(scrolled, LIBRARY_LIST_KEY, filter);
        }
        SidebarFilter::Albums => {}
        SidebarFilter::Album(_) => {
            select_matching_row(scrolled, ALBUM_LIST_KEY, filter);
        }
        SidebarFilter::Folder(_) => {
            let shares = stored_widget::<gtk::ListBox>(scrolled, SHARE_LIST_KEY);
            let is_share = shares.as_ref().is_some_and(|list| {
                let mut child = list.first_child();
                while let Some(widget) = child {
                    if let Ok(row) = widget.clone().downcast::<gtk::ListBoxRow>() {
                        if unsafe { row.data::<SidebarFilter>("picasa-filter") }
                            .is_some_and(|value| unsafe { *value.as_ref() == filter })
                        {
                            return true;
                        }
                    }
                    child = widget.next_sibling();
                }
                false
            });
            select_matching_row(
                scrolled,
                if is_share {
                    SHARE_LIST_KEY
                } else {
                    FOLDER_LIST_KEY
                },
                filter,
            );
        }
    }
    syncing.set(false);
    restore_folder_scroll(scrolled, folder_scroll_value);
}

fn folder_scroll_value(scrolled: &gtk::ScrolledWindow) -> Option<f64> {
    stored_widget::<gtk::ScrolledWindow>(scrolled, FOLDER_SCROLL_KEY)
        .map(|folder_scroll| folder_scroll.vadjustment().value())
}

fn restore_folder_scroll(scrolled: &gtk::ScrolledWindow, value: Option<f64>) {
    let Some(value) = value else {
        return;
    };
    let scrolled = scrolled.clone();
    glib::idle_add_local_once(move || {
        let Some(folder_scroll) =
            stored_widget::<gtk::ScrolledWindow>(&scrolled, FOLDER_SCROLL_KEY)
        else {
            return;
        };
        let adjustment = folder_scroll.vadjustment();
        let max_value = (adjustment.upper() - adjustment.page_size()).max(adjustment.lower());
        adjustment.set_value(value.clamp(adjustment.lower(), max_value));
    });
}

/// Select the independently registered Network Shares row without entering
/// the local Folders tree or invoking a second gallery navigation.
pub fn scroll_to_network_share(scrolled: &gtk::ScrolledWindow, share_id: i64) {
    let Some(list) = stored_widget::<gtk::ListBox>(scrolled, SHARE_LIST_KEY) else {
        return;
    };
    let Some(row) = (|| {
        let mut child = list.first_child();
        while let Some(widget) = child {
            let next = widget.next_sibling();
            if let Ok(row) = widget.downcast::<gtk::ListBoxRow>() {
                if unsafe { row.data::<SidebarFilter>("picasa-filter") }
                    .is_some_and(|f| unsafe { *f.as_ref() == SidebarFilter::Folder(share_id) })
                {
                    return Some(row);
                }
            }
            child = next;
        }
        None
    })() else {
        return;
    };
    // set_active_filter selects this row with selection callbacks suppressed.
    set_active_filter(scrolled, SidebarFilter::Folder(share_id));
    // Explicitly scroll the Network Shares row into view. Focus alone is not
    // sufficient when the local Folders pane has its own scroll adjustment.
    let sidebar = scrolled.clone();
    glib::idle_add_local_once(move || {
        let Some(list) = stored_widget::<gtk::ListBox>(&sidebar, SHARE_LIST_KEY) else {
            return;
        };
        let Some(share_scroll) = stored_widget::<gtk::ScrolledWindow>(&sidebar, SHARE_SCROLL_KEY) else {
            return;
        };
        let mut child = list.first_child();
        while let Some(widget) = child {
            let next = widget.next_sibling();
            if let Ok(row) = widget.downcast::<gtk::ListBoxRow>() {
                let matches = unsafe { row.data::<SidebarFilter>("picasa-filter") }
                    .is_some_and(|filter| unsafe { *filter.as_ref() == SidebarFilter::Folder(share_id) });
                if matches {
                    let scroll_widget = share_scroll.clone().upcast::<gtk::Widget>();
                    if let Some(point) = row.compute_point(
                        &scroll_widget, &gtk::graphene::Point::new(0.0, 0.0),
                    ) {
                        let adjustment = share_scroll.vadjustment();
                        let target = adjustment.value() + f64::from(point.y())
                            - adjustment.page_size() / 3.0;
                        let maximum = (adjustment.upper() - adjustment.page_size())
                            .max(adjustment.lower());
                        adjustment.set_value(target.clamp(adjustment.lower(), maximum));
                    }
                    row.grab_focus();
                    crate::source::net_trace(format!(
                        "network_share_sidebar_focus folder={share_id} scrolled=true"
                    ));
                    break;
                }
            }
            child = next;
        }
    });
}

/// Reveal and scroll to an imported folder selected from search suggestions.
pub fn scroll_to_folder(scrolled: &gtk::ScrolledWindow, folder_id: i64) {
    let Some(state) = sidebar_state(scrolled) else {
        return;
    };
    let Some(list) = stored_widget::<gtk::ListBox>(scrolled, FOLDER_LIST_KEY) else {
        return;
    };

    // A navigation request should reveal the exact folder row, not only an
    // imported parent/root. Force Tree mode for Open in Folder/search reveals
    // so the full ancestor path exists in the sidebar, then continue scrolling
    // after the rebuilt rows have been allocated.
    if set_folder_display_mode(scrolled, FolderDisplayMode::Tree) {
        let scrolled = scrolled.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(100), move || {
            scroll_to_folder(&scrolled, folder_id);
        });
        return;
    }

    // A navigation request must make the target row visible even when the
    // user previously collapsed the entire Folders section.
    state.borrow_mut().folders_expanded = true;
    if let Some(revealer) = stored_widget::<gtk::Revealer>(scrolled, FOLDER_REVEALER_KEY) {
        revealer.set_reveal_child(true);
        revealer.set_vexpand(true);
    }
    if let Some(indicator) = stored_widget::<gtk::Button>(scrolled, FOLDER_INDICATOR_KEY) {
        indicator.set_icon_name("pan-down-symbolic");
    }

    // Search results can target a row below a collapsed ancestor. Expand the
    // path using the same cached folder data used for the normal tree.
    let folders = unsafe {
        list.data::<Vec<Folder>>("picasa-folder-cache")
            .map(|folders| folders.as_ref().clone())
            .unwrap_or_default()
    };
    let by_id: HashMap<i64, &Folder> = folders.iter().map(|folder| (folder.id, folder)).collect();
    let tree_mode = state.borrow().folder_display_mode == FolderDisplayMode::Tree;
    let mut parent = tree_mode
        .then(|| by_id.get(&folder_id).and_then(|folder| folder.parent_id))
        .flatten();
    let mut expanded = false;
    while let Some(parent_id) = parent {
        expanded |= state.borrow_mut().expanded_folders.insert(parent_id);
        parent = by_id.get(&parent_id).and_then(|folder| folder.parent_id);
    }
    let folder_scroll_value = folder_scroll_value(scrolled);
    if expanded {
        rebuild_folder_list_from_rows(&list, &state);

        // Rebuilding the tree schedules restoration of the previous scroll
        // position and GTK has not allocated the new rows yet. Retry after
        // both have had a main-loop turn so the target can be placed at the
        // top reliably.
        let scrolled = scrolled.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(100), move || {
            scroll_to_folder(&scrolled, folder_id);
        });
        return;
    }

    let mut child = list.first_child();
    while let Some(widget) = child {
        let next = widget.next_sibling();
        if let Ok(row) = widget.downcast::<gtk::ListBoxRow>() {
            let matches = unsafe {
                row.data::<SidebarFilter>("picasa-filter")
                    .is_some_and(|filter| *filter.as_ref() == SidebarFilter::Folder(folder_id))
            };
            if matches {
                // Select directly so this reveal does not schedule another
                // restoration of the old scroll position. The ListBox
                // selection signal normally invokes the navigation callback,
                // so suppress that callback for this programmatic selection.
                let syncing = unsafe {
                    scrolled
                        .data::<Rc<Cell<bool>>>(FILTER_SYNCING_KEY)
                        .map(|data| data.as_ref().clone())
                };
                if let Some(syncing) = &syncing {
                    syncing.set(true);
                }
                select_matching_row(scrolled, FOLDER_LIST_KEY, SidebarFilter::Folder(folder_id));
                row.grab_focus();
                if let Some(syncing) = syncing {
                    syncing.set(false);
                }
                if let Some(folder_scroll) =
                    stored_widget::<gtk::ScrolledWindow>(scrolled, FOLDER_SCROLL_KEY)
                {
                    let adjustment = folder_scroll.vadjustment();
                    // Row position in the scrolled content: the folder list now
                    // sits below the album section in the shared scroller, so
                    // its own allocation y is not the scroll offset.
                    let top = row
                        .compute_bounds(&folder_scroll)
                        .map(|bounds| adjustment.value() + f64::from(bounds.y()))
                        .unwrap_or_else(|| f64::from(row.allocation().y()));
                    // Keep the selected folder at the top of the folder pane
                    // so repeated navigation has a consistent destination.
                    adjustment.set_value(top.clamp(
                        adjustment.lower(),
                        (adjustment.upper() - adjustment.page_size()).max(adjustment.lower()),
                    ));
                }
                return;
            }
        }
        child = next;
    }
    restore_folder_scroll(scrolled, folder_scroll_value);
}

fn current_filter(scrolled: &gtk::ScrolledWindow) -> Option<SidebarFilter> {
    unsafe {
        scrolled
            .data::<SidebarFilter>(CURRENT_FILTER_KEY)
            .map(|filter| *filter.as_ref())
    }
}

fn sidebar_state(scrolled: &gtk::ScrolledWindow) -> Option<Rc<RefCell<SidebarState>>> {
    unsafe {
        scrolled
            .data::<Rc<RefCell<SidebarState>>>(STATE_KEY)
            .map(|state| state.as_ref().clone())
    }
}

/// Mark whether the sidebar is explicitly pinned open.
///
/// Pinned means mouse-leave must not auto-hide it.
pub fn set_pinned(scrolled: &gtk::ScrolledWindow, pinned: bool) {
    let Some(state) = sidebar_state(scrolled) else {
        return;
    };

    let state = state.borrow();
    state.pinned.set(pinned);

    if pinned {
        state.hover_open.set(false);
    }
}

/// Returns true when the sidebar was explicitly pinned open.
pub fn is_pinned(scrolled: &gtk::ScrolledWindow) -> bool {
    sidebar_state(scrolled)
        .map(|state| state.borrow().pinned.get())
        .unwrap_or(true)
}

/// Record that the sidebar was opened temporarily by the left-edge hover.
///
/// This never overrides a pinned sidebar.
pub fn set_hover_open(scrolled: &gtk::ScrolledWindow, hover_open: bool) {
    let Some(state) = sidebar_state(scrolled) else {
        return;
    };

    let state = state.borrow();
    if state.pinned.get() {
        state.hover_open.set(false);
    } else {
        state.hover_open.set(hover_open);
    }
}

/// Returns true only when the sidebar is currently open because of hover.
pub fn is_hover_open(scrolled: &gtk::ScrolledWindow) -> bool {
    sidebar_state(scrolled)
        .map(|state| {
            let state = state.borrow();
            state.hover_open.get() && !state.pinned.get()
        })
        .unwrap_or(false)
}

/// Clear temporary hover-open state after the sidebar auto-hides.
pub fn clear_hover_open(scrolled: &gtk::ScrolledWindow) {
    let Some(state) = sidebar_state(scrolled) else {
        return;
    };
    state.borrow().hover_open.set(false);
}

fn stored_widget<T>(scrolled: &gtk::ScrolledWindow, key: &str) -> Option<T>
where
    T: IsA<gtk::Widget> + Clone + 'static,
{
    unsafe {
        scrolled
            .data::<T>(key)
            .map(|widget| widget.as_ref().clone())
    }
}

fn select_matching_row(scrolled: &gtk::ScrolledWindow, list_key: &str, filter: SidebarFilter) {
    let Some(list) = stored_widget::<gtk::ListBox>(scrolled, list_key) else {
        return;
    };

    let mut child = list.first_child();
    while let Some(widget) = child {
        if let Ok(row) = widget.clone().downcast::<gtk::ListBoxRow>() {
            let matches = unsafe {
                row.data::<SidebarFilter>("picasa-filter")
                    .map(|value| *value.as_ref() == filter)
                    .unwrap_or(false)
            };
            if matches {
                list.select_row(Some(&row));
                return;
            }
        }
        child = widget.next_sibling();
    }
}

fn section_list() -> gtk::ListBox {
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::Single);
    list.set_hexpand(true);
    list.add_css_class("navigation-sidebar");
    list
}

fn connect_filter_list(
    list: &gtk::ListBox,
    on_filter: Rc<dyn Fn(SidebarFilter)>,
    syncing: Rc<Cell<bool>>,
) {
    list.connect_row_selected(move |_, row| {
        if syncing.get() {
            return;
        }
        let Some(row) = row else { return };
        if let Some(value) = unsafe { row.data::<SidebarFilter>("picasa-filter") } {
            let filter = unsafe { *value.as_ref() };
            // Share rows carry the op generated by their capture-phase press
            // gesture. Every other row (and keyboard activation) starts a fresh
            // op here so each click -> gallery interaction has a distinct id.
            let stored =
                unsafe { row.data::<u64>("picasa-trace-op") }.map(|op| unsafe { *op.as_ref() });
            if let Some(op) = stored {
                crate::source::set_trace_op(op);
            } else if std::env::var_os("PICASA_TRACE").is_some() {
                crate::source::set_trace_op(crate::source::new_trace_op());
            }
            let op = crate::source::current_trace_op();
            crate::source::net_trace(format!("filter_selected {filter:?} op={op}"));
            crate::source::net_trace("filter_selected_begin");
            on_filter(filter);
            crate::source::net_trace("filter_selected_end");
        }
    });
}

fn clear_list(list: &gtk::ListBox) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
}

fn populate_library(list: &gtk::ListBox, counts: SidebarCounts, on_unavailable: &Rc<dyn Fn()>) {
    append_filter(
        list,
        "image-x-generic-symbolic",
        "All Photos",
        None,
        Some(counts.photos),
        SidebarFilter::All,
        false,
        on_unavailable,
    );
    append_filter(
        list,
        "emote-love-symbolic",
        "Favourites",
        None,
        Some(counts.favorites),
        SidebarFilter::Favorites,
        false,
        on_unavailable,
    );
    append_filter(
        list,
        "appointment-soon-symbolic",
        "Recently Added",
        None,
        Some(counts.recently_added),
        SidebarFilter::RecentlyAdded,
        false,
        on_unavailable,
    );
}

fn list_natural_height_for_rows(list: &gtk::ListBox, max_rows: usize) -> i32 {
    if max_rows == 0 {
        return 0;
    }

    let mut height = 0;
    let mut rows = 0;
    let mut child = list.first_child();
    while let Some(widget) = child {
        let next = widget.next_sibling();
        if let Ok(row) = widget.downcast::<gtk::ListBoxRow>() {
            let (_, natural, _, _) = row.measure(gtk::Orientation::Vertical, -1);
            height += natural.max(0);
            rows += 1;
            if rows >= max_rows {
                break;
            }
        }
        child = next;
    }
    height
}

fn populate_albums(list: &gtk::ListBox, albums: &[Album], on_delete_album: &Rc<dyn Fn(i64)>) {
    for album in albums {
        append_album_filter(list, album, on_delete_album);
    }
}

fn album_icon_name() -> &'static str {
    if let Some(display) = gtk::gdk::Display::default() {
        let icon_theme = gtk::IconTheme::for_display(&display);
        if icon_theme.has_icon("folder-pictures-symbolic") {
            return "folder-pictures-symbolic";
        }
    }
    "image-x-generic-symbolic"
}

fn populate_folders(
    list: &gtk::ListBox,
    folders: &[Folder],
    state: &Rc<RefCell<SidebarState>>,
    on_unavailable: &Rc<dyn Fn()>,
) {
    // Filter for every rebuild path, not just refresh_folder_rows(): startup,
    // change-of-view and scan events all pass through this function.
    let local_folders: Vec<Folder> = folders
        .iter()
        .filter(|folder| is_local_sidebar_folder(folder))
        .cloned()
        .collect();
    let folders = local_folders.as_slice();
    unsafe {
        list.set_data("picasa-folder-cache", folders.to_vec());
        list.set_data("picasa-folder-unavailable-callback", on_unavailable.clone());
    }

    if folders.is_empty() {
        return;
    }

    if state.borrow().folder_display_mode == FolderDisplayMode::ImportedOnly {
        let mut imported = folders
            .iter()
            .filter(|folder| folder.imported_root)
            .collect::<Vec<_>>();
        imported.sort_by(|left, right| {
            left.name
                .to_lowercase()
                .cmp(&right.name.to_lowercase())
                .then_with(|| left.path.to_lowercase().cmp(&right.path.to_lowercase()))
        });
        for folder in imported {
            // Flat/Picasa-style mode deliberately shows only the folder the
            // user explicitly imported. The row title is the basename (for
            // example "Pictures"), while the full path remains a tooltip.
            append_folder_row(list, folder, 0, false, state, on_unavailable);
        }
        return;
    }

    let visible_folders = folders
        .iter()
        .filter(|folder| folder.photo_count > 0)
        .collect::<Vec<_>>();
    let by_id: HashMap<i64, &Folder> = visible_folders
        .iter()
        .map(|folder| (folder.id, *folder))
        .collect();
    let mut children: HashMap<Option<i64>, Vec<i64>> = HashMap::new();
    for folder in visible_folders {
        // imported_root controls scan scope only. It must not flatten an
        // explicitly imported folder out of its visual hierarchy; otherwise
        // synthetic parents such as /mnt/steam cannot collapse their children.
        children
            .entry(folder.parent_id)
            .or_default()
            .push(folder.id);
    }

    for ids in children.values_mut() {
        ids.sort_by(|left, right| {
            let left = by_id.get(left).expect("folder id exists");
            let right = by_id.get(right).expect("folder id exists");
            left.name
                .to_lowercase()
                .cmp(&right.name.to_lowercase())
                .then_with(|| left.path.to_lowercase().cmp(&right.path.to_lowercase()))
        });
    }

    let roots = children.get(&None).cloned().unwrap_or_default();
    for id in roots {
        append_folder_branch(list, id, 0, &by_id, &children, state, on_unavailable);
    }
}

fn append_folder_branch(
    list: &gtk::ListBox,
    folder_id: i64,
    depth: usize,
    by_id: &HashMap<i64, &Folder>,
    children: &HashMap<Option<i64>, Vec<i64>>,
    state: &Rc<RefCell<SidebarState>>,
    on_unavailable: &Rc<dyn Fn()>,
) {
    let Some(folder) = by_id.get(&folder_id).copied() else {
        return;
    };
    let has_children = children
        .get(&Some(folder_id))
        .is_some_and(|items| !items.is_empty());

    append_folder_row(list, folder, depth, has_children, state, on_unavailable);

    if has_children && state.borrow().expanded_folders.contains(&folder_id) {
        if let Some(child_ids) = children.get(&Some(folder_id)) {
            for child_id in child_ids {
                append_folder_branch(
                    list,
                    *child_id,
                    depth + 1,
                    by_id,
                    children,
                    state,
                    on_unavailable,
                );
            }
        }
    }
}

fn append_folder_row(
    list: &gtk::ListBox,
    folder: &Folder,
    depth: usize,
    has_children: bool,
    state: &Rc<RefCell<SidebarState>>,
    on_unavailable: &Rc<dyn Fn()>,
) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.set_margin_top(2);
    row.set_margin_bottom(2);

    let content = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    content.set_margin_top(4);
    content.set_margin_bottom(4);
    content.set_margin_start(4);
    content.set_margin_end(8);

    // Tree mode reserves a fixed chevron slot so folder icons and names align
    // at every depth. Imported-only mode is flat (no subfolders), so skip it
    // and left-align the folder icons with the Library rows.
    let reserve_disclosure =
        has_children || state.borrow().folder_display_mode == FolderDisplayMode::Tree;
    if reserve_disclosure {
        let disclosure_slot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        disclosure_slot.set_size_request(12, 18);
        disclosure_slot.set_width_request(12);
        if has_children {
            let expanded = state.borrow().expanded_folders.contains(&folder.id);
            let disclosure = gtk::Image::from_icon_name(if expanded {
                "pan-down-symbolic"
            } else {
                "pan-end-symbolic"
            });
            disclosure.add_css_class("folder-disclosure");
            disclosure.set_pixel_size(10);
            disclosure.set_halign(gtk::Align::Center);
            disclosure.set_valign(gtk::Align::Center);
            disclosure_slot.set_tooltip_text(Some(if expanded {
                "Collapse folder"
            } else {
                "Show subfolders"
            }));

            let folder_id = folder.id;
            let list_for_toggle = list.clone();
            let state_for_toggle = state.clone();
            let toggle = gtk::GestureClick::new();
            toggle.connect_pressed(move |gesture, _, _, _| {
                {
                    let mut state = state_for_toggle.borrow_mut();
                    if !state.expanded_folders.remove(&folder_id) {
                        state.expanded_folders.insert(folder_id);
                    }
                }
                // Rebuilding just the folder section requires the complete folder
                // data, which refresh() owns. Mark the row's requested state now;
                // GTK will show the updated tree on the next sidebar refresh.
                // To make expansion immediate, store a synthetic toggle marker and
                // let rebuild_folder_list_from_rows() reconstruct from row data.
                rebuild_folder_list_from_rows(&list_for_toggle, &state_for_toggle);
                gesture.set_state(gtk::EventSequenceState::Claimed);
            });
            disclosure_slot.add_controller(toggle);
            disclosure_slot.append(&disclosure);
        }
        content.append(&disclosure_slot);
    }

    let icon_overlay = gtk::Overlay::new();
    icon_overlay.set_size_request(18, 18);
    icon_overlay.set_halign(gtk::Align::Center);
    icon_overlay.set_valign(gtk::Align::Center);
    icon_overlay.set_hexpand(false);
    let icon = gtk::Image::from_icon_name("folder-symbolic");
    icon.set_pixel_size(18);
    icon_overlay.set_child(Some(&icon));

    if folder.watched {
        // A small eye emblem makes watched folders visible at a glance while
        // preserving the normal folder icon and row alignment.
        let watch_badge = gtk::Image::from_icon_name("view-reveal-symbolic");
        watch_badge.set_pixel_size(10);
        watch_badge.set_halign(gtk::Align::End);
        watch_badge.set_valign(gtk::Align::End);
        watch_badge.set_tooltip_text(Some("Watched folder"));
        icon_overlay.add_overlay(&watch_badge);
        icon_overlay.set_tooltip_text(Some("Watched folder"));
    }

    content.append(&icon_overlay);

    let labels = gtk::Box::new(gtk::Orientation::Vertical, 1);
    labels.set_hexpand(true);
    let title_label = gtk::Label::new(Some(&folder.name));
    title_label.set_xalign(0.0);
    title_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    labels.append(&title_label);
    content.append(&labels);

    let trailing = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let count_label = gtk::Label::new(Some(&format_count(folder.photo_count)));
    count_label.set_width_request(36);
    count_label.set_xalign(1.0);
    count_label.set_valign(gtk::Align::Center);
    count_label.add_css_class("dim-label");
    count_label.add_css_class("sidebar-count");
    count_label.add_css_class("folder-count");
    trailing.append(&count_label);

    if !folder.available {
        let warning = gtk::Button::with_label("!");
        warning.add_css_class("sidebar-offline-badge");
        warning.set_halign(gtk::Align::Center);
        warning.set_valign(gtk::Align::Center);
        warning.set_width_request(16);
        warning.set_height_request(16);
        warning.set_tooltip_text(Some("Source folder unavailable"));
        let on_unavailable = on_unavailable.clone();
        warning.connect_clicked(move |_| (on_unavailable)());
        trailing.append(&warning);
    }
    content.append(&trailing);

    row.set_child(Some(&content));
    if has_children {
        let folder_id = folder.id;
        let list_for_double_click = list.clone();
        let state_for_double_click = state.clone();
        let double_click = gtk::GestureClick::new();
        double_click.set_button(1);
        double_click.connect_pressed(move |gesture, n_press, _, _| {
            if n_press == 2 {
                let mut state = state_for_double_click.borrow_mut();
                if !state.expanded_folders.remove(&folder_id) {
                    state.expanded_folders.insert(folder_id);
                }
                drop(state);
                rebuild_folder_list_from_rows(&list_for_double_click, &state_for_double_click);
                gesture.set_state(gtk::EventSequenceState::Claimed);
            }
        });
        row.add_controller(double_click);
    }
    if folder.imported_root {
        row.set_tooltip_text(Some(&folder.path));
    }
    unsafe {
        row.set_data("picasa-filter", SidebarFilter::Folder(folder.id));
        row.set_data("picasa-folder-record", folder.clone());
        row.set_data("picasa-folder-depth", depth);
        row.set_data("picasa-folder-has-children", has_children);
        row.set_data("picasa-folder-unavailable-callback", on_unavailable.clone());
    }
    list.append(&row);
    add_folder_context_menu(&list, &row, folder);
    row
}

fn add_folder_context_menu(list: &gtk::ListBox, row: &gtk::ListBoxRow, folder: &Folder) {
    let refresh = unsafe {
        list.data::<Rc<dyn Fn(String)>>(FOLDER_REFRESH_KEY)
            .map(|callback| callback.as_ref().clone())
    };
    let statistics = unsafe {
        list.data::<Rc<dyn Fn(Folder)>>(FOLDER_STATISTICS_KEY)
            .map(|callback| callback.as_ref().clone())
    };
    let remove = unsafe {
        list.data::<Rc<dyn Fn(Folder)>>(FOLDER_REMOVE_KEY)
            .map(|callback| callback.as_ref().clone())
    };
    let favorite = unsafe {
        list.data::<Rc<dyn Fn(Folder, bool)>>(FOLDER_FAVORITE_KEY)
            .map(|callback| callback.as_ref().clone())
    };
    let watch = unsafe {
        list.data::<Rc<dyn Fn(Folder, bool)>>(FOLDER_WATCH_KEY)
            .map(|callback| callback.as_ref().clone())
    };
    let Some(refresh) = refresh else {
        return;
    };
    let Some(statistics) = statistics else {
        return;
    };
    let Some(remove) = remove else {
        return;
    };
    let Some(favorite) = favorite else {
        return;
    };
    let Some(watch) = watch else {
        return;
    };

    let folder_for_menu = folder.clone();
    let row_for_menu = row.clone();
    let list_for_refresh_gate = list.downgrade();
    let right_click = gtk::GestureClick::new();
    right_click.set_button(3);
    right_click.connect_pressed(move |gesture, _, _, _| {
        let popover = gtk::Popover::new();
        popover.set_has_arrow(true);
        popover.set_parent(&row_for_menu);
        let menu = gtk::Box::new(gtk::Orientation::Vertical, 2);
        menu.set_margin_top(6);
        menu.set_margin_bottom(6);
        menu.set_margin_start(6);
        menu.set_margin_end(6);

        let refresh_item = gtk::Button::with_label("Refresh folder");
        refresh_item.add_css_class("flat");
        refresh_item.set_sensitive(folder_for_menu.imported_root);
        if folder_for_menu.imported_root {
            let gate = list_for_refresh_gate.upgrade().and_then(|list| unsafe {
                list.data::<glib::WeakRef<gtk::Button>>(REFRESH_GATE_KEY)
                    .and_then(|gate| gate.as_ref().upgrade())
            });
            if let Some(gate) = gate {
                gate.bind_property("sensitive", &refresh_item, "sensitive")
                    .sync_create()
                    .build();
            }
        }
        if !folder_for_menu.imported_root {
            refresh_item
                .set_tooltip_text(Some("Only explicitly imported folders can be refreshed"));
        }
        let path = folder_for_menu.path.clone();
        let popover_for_refresh = popover.clone();
        let refresh = refresh.clone();
        refresh_item.connect_clicked(move |_| {
            popover_for_refresh.popdown();
            refresh(path.clone());
        });
        menu.append(&refresh_item);

        let watch_item = gtk::Button::with_label(if folder_for_menu.watched {
            "Stop Watching Folder"
        } else {
            "Watch Folder"
        });
        watch_item.add_css_class("flat");
        let folder = folder_for_menu.clone();
        let watch = watch.clone();
        let watched = !folder_for_menu.watched;
        let popover_for_watch = popover.clone();
        watch_item.connect_clicked(move |_| {
            popover_for_watch.popdown();
            watch(folder.clone(), watched);
        });
        menu.append(&watch_item);

        let statistics_item = gtk::Button::with_label("Folder statistics");
        statistics_item.add_css_class("flat");
        let folder = folder_for_menu.clone();
        let statistics = statistics.clone();
        let popover_for_statistics = popover.clone();
        statistics_item.connect_clicked(move |_| {
            popover_for_statistics.popdown();
            statistics(folder.clone());
        });
        menu.append(&statistics_item);

        let remove_item = gtk::Button::with_label("Remove from Library");
        remove_item.add_css_class("flat");
        remove_item.add_css_class("destructive-action");
        let folder = folder_for_menu.clone();
        let remove = remove.clone();
        let popover_for_remove = popover.clone();
        remove_item.connect_clicked(move |_| {
            popover_for_remove.popdown();
            remove(folder.clone());
        });
        menu.append(&remove_item);

        let add_favorites = gtk::Button::with_label("Add all photos to Favourites");
        add_favorites.add_css_class("flat");
        let folder = folder_for_menu.clone();
        let favorite_for_add = favorite.clone();
        let popover_for_add = popover.clone();
        add_favorites.connect_clicked(move |_| {
            popover_for_add.popdown();
            favorite_for_add(folder.clone(), true);
        });
        menu.append(&add_favorites);

        let remove_favorites = gtk::Button::with_label("Remove all photos from Favourites");
        remove_favorites.add_css_class("flat");
        let folder = folder_for_menu.clone();
        let favorite = favorite.clone();
        let popover_for_remove = popover.clone();
        remove_favorites.connect_clicked(move |_| {
            popover_for_remove.popdown();
            favorite(folder.clone(), false);
        });
        menu.append(&remove_favorites);

        popover.set_child(Some(&menu));
        popover.popup();
        gesture.set_state(gtk::EventSequenceState::Claimed);
    });
    row.add_controller(right_click);
}

// Folder expansion must be immediate, but the public toggle callback does not
// have the original &[Folder]. Every visible row therefore keeps its Folder
// record. We collect the full currently-known set from visible rows plus hidden
// descendants cached on the ListBox itself when populate_folders() runs.
fn rebuild_folder_list_from_rows(list: &gtk::ListBox, state: &Rc<RefCell<SidebarState>>) {
    let folder_scroll = list
        .ancestor(gtk::ScrolledWindow::static_type())
        .and_then(|widget| widget.downcast::<gtk::ScrolledWindow>().ok());
    let outer = folder_scroll.as_ref().and_then(|folder_scroll| {
        folder_scroll
            .ancestor(gtk::ScrolledWindow::static_type())
            .and_then(|widget| widget.downcast::<gtk::ScrolledWindow>().ok())
    });
    let folder_scroll_value = outer.as_ref().and_then(|outer| folder_scroll_value(outer));
    let folders = unsafe {
        list.data::<Vec<Folder>>("picasa-folder-cache")
            .map(|data| data.as_ref().clone())
            .unwrap_or_default()
    };
    let callback = unsafe {
        list.data::<Rc<dyn Fn()>>("picasa-folder-unavailable-callback")
            .map(|data| data.as_ref().clone())
            .unwrap_or_else(|| Rc::new(|| {}))
    };
    if folders.is_empty() {
        return;
    }
    clear_list(list);
    populate_folders(list, &folders, state, &callback);
    if let Some(outer) = outer {
        if let Some(filter) = current_filter(&outer) {
            set_active_filter(&outer, filter);
        }
        restore_folder_scroll(&outer, folder_scroll_value);
        let outer_for_location = outer.clone();
        glib::idle_add_local_once(move || apply_scroll_location(&outer_for_location));
    }
}

fn append_filter(
    list: &gtk::ListBox,
    icon_name: &str,
    title: &str,
    subtitle: Option<&str>,
    count: Option<i64>,
    filter: SidebarFilter,
    offline: bool,
    on_unavailable: &Rc<dyn Fn()>,
) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.set_margin_top(2);
    row.set_margin_bottom(2);
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    content.set_margin_top(6);
    content.set_margin_bottom(6);
    content.set_margin_start(8);
    content.set_margin_end(8);
    let icon = gtk::Image::from_icon_name(icon_name);
    icon.set_pixel_size(18);
    content.append(&icon);
    let labels = gtk::Box::new(gtk::Orientation::Vertical, 1);
    let title_label = gtk::Label::new(Some(title));
    title_label.set_xalign(0.0);
    title_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    labels.append(&title_label);
    if let Some(subtitle) = subtitle {
        let subtitle_label = gtk::Label::new(Some(subtitle));
        subtitle_label.set_xalign(0.0);
        subtitle_label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        subtitle_label.add_css_class("dim-label");
        labels.append(&subtitle_label);
    }
    labels.set_hexpand(true);
    content.append(&labels);

    if let Some(count) = count {
        let count_label = gtk::Label::new(Some(&format_count(count)));
        count_label.set_xalign(1.0);
        count_label.set_valign(gtk::Align::Center);
        count_label.add_css_class("dim-label");
        count_label.add_css_class("sidebar-count");
        content.append(&count_label);
    }

    if offline {
        let warning = gtk::Button::with_label("!");
        warning.add_css_class("sidebar-offline-badge");
        warning.set_halign(gtk::Align::Center);
        warning.set_valign(gtk::Align::Center);
        warning.set_width_request(16);
        warning.set_height_request(16);
        warning.set_tooltip_text(Some("Source folder unavailable"));
        let on_unavailable = on_unavailable.clone();
        warning.connect_clicked(move |_| (on_unavailable)());
        content.append(&warning);
    }
    row.set_child(Some(&content));
    unsafe {
        row.set_data("picasa-filter", filter);
    }
    list.append(&row);
    row
}

fn append_album_filter(list: &gtk::ListBox, album: &Album, on_delete: &Rc<dyn Fn(i64)>) {
    let no_unavailable_action: Rc<dyn Fn()> = Rc::new(|| {});
    let row = append_filter(
        list,
        album_icon_name(),
        &album.name,
        None,
        Some(album.photo_count),
        SidebarFilter::Album(album.id),
        false,
        &no_unavailable_action,
    );
    let album_id = album.id;
    let on_delete = on_delete.clone();
    let row_for_menu = row.clone();
    let right_click = gtk::GestureClick::new();
    right_click.set_button(3);
    right_click.connect_pressed(move |gesture, _, _, _| {
        let popover = gtk::Popover::new();
        popover.set_has_arrow(true);
        popover.set_parent(&row_for_menu);
        let delete = gtk::Button::with_label("Delete Album");
        delete.add_css_class("flat");
        delete.add_css_class("destructive-action");
        let on_delete = on_delete.clone();
        let popover_for_delete = popover.clone();
        delete.connect_clicked(move |_| {
            popover_for_delete.popdown();
            on_delete(album_id);
        });
        popover.set_child(Some(&delete));
        popover.popup();
        gesture.set_state(gtk::EventSequenceState::Claimed);
    });
    row.add_controller(right_click);
}

fn set_folder_mode_toggle_presentation(button: &gtk::Button, mode: FolderDisplayMode) {
    match mode {
        FolderDisplayMode::Tree => {
            button.set_icon_name("folder-symbolic");
            button.set_tooltip_text(Some("Show imported folders only"));
        }
        FolderDisplayMode::ImportedOnly => {
            button.set_icon_name("view-list-symbolic");
            button.set_tooltip_text(Some("Show folder tree"));
        }
    }
}

fn sidebar_pane_ease(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    // Smoothstep: zero velocity at both ends, so the pane does not visibly
    // snap when the Revealer starts or when the native handle disappears.
    t * t * (3.0 - 2.0 * t)
}

fn animate_sidebar_pane_position(
    paned: &gtk::Paned,
    from: i32,
    to: i32,
    generation: Rc<Cell<u64>>,
    expected_generation: u64,
    animating: Rc<Cell<bool>>,
    on_complete: Option<Rc<dyn Fn()>>,
) {
    if from == to {
        if generation.get() == expected_generation {
            paned.set_position(to);
            animating.set(false);
            if let Some(callback) = on_complete.as_ref() {
                callback();
            }
        }
        return;
    }

    let started = std::time::Instant::now();
    paned.add_tick_callback(move |paned, _| {
        if generation.get() != expected_generation {
            return glib::ControlFlow::Break;
        }

        let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
        let progress = (elapsed_ms / f64::from(ALBUM_PANE_ANIMATION_MS)).clamp(0.0, 1.0);
        let eased = sidebar_pane_ease(progress);
        let position = f64::from(from) + f64::from(to - from) * eased;
        paned.set_position(position.round() as i32);

        if progress >= 1.0 {
            paned.set_position(to);
            animating.set(false);
            if let Some(callback) = on_complete.as_ref() {
                callback();
            }
            glib::ControlFlow::Break
        } else {
            glib::ControlFlow::Continue
        }
    });
}

fn collapsible_heading(
    text: &str,
    action: Option<Rc<dyn Fn()>>,
    action_tooltip: &str,
    expanded: bool,
    title_action: Option<Rc<dyn Fn()>>,
) -> (gtk::Box, gtk::Button) {
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    content.add_css_class("sidebar-section-heading");
    content.set_hexpand(true);
    content.set_margin_start(8);
    content.set_margin_end(8);
    content.set_margin_top(8);
    content.set_margin_bottom(4);

    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.set_hexpand(true);
    label.add_css_class("sidebar-section-heading-title");
    if let Some(title_action) = title_action {
        let click = gtk::GestureClick::new();
        click.connect_pressed(move |gesture, _, _, _| {
            title_action();
            gesture.set_state(gtk::EventSequenceState::Claimed);
        });
        label.add_controller(click);
    }
    content.append(&label);

    // Keep the disclosure control rightmost. Any section-specific action sits
    // before it, so headings read consistently as: action / + / collapse.
    if let Some(action) = action {
        let add = gtk::Button::from_icon_name("list-add-symbolic");
        add.add_css_class("flat");
        add.set_size_request(24, 24);
        add.set_tooltip_text(Some(action_tooltip));
        add.connect_clicked(move |_| action());
        content.append(&add);
    }

    let indicator = gtk::Button::from_icon_name(if expanded {
        "pan-down-symbolic"
    } else {
        "pan-end-symbolic"
    });
    indicator.add_css_class("flat");
    indicator.set_focusable(false);
    indicator.set_size_request(24, 24);
    indicator.set_tooltip_text(Some(if expanded { "Collapse" } else { "Expand" }));
    content.append(&indicator);

    (content, indicator)
}

fn format_count(value: i64) -> String {
    let negative = value < 0;
    let digits = value.unsigned_abs().to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    if negative {
        format!("-{grouped}")
    } else {
        grouped
    }
}

/// Registered network shares (imported URI roots). Presented in their own
/// sidebar section below Folders, never mixed into the local folder tree.
fn refresh_network_shares(
    scrolled: &gtk::ScrolledWindow,
    folders: &[Folder],
    on_unavailable: &Rc<dyn Fn()>,
) {
    let generation = SHARE_COUNT_GENERATION.fetch_add(1, Ordering::Relaxed) + 1;
    crate::source::net_trace("shares_refresh_start");
    let Some(list) = stored_widget::<gtk::ListBox>(scrolled, SHARE_LIST_KEY) else {
        return;
    };
    clear_list(&list);
    let shares: Vec<Folder> = folders
        .iter()
        .filter(|folder| folder.imported_root && crate::db::is_remote_path(&folder.path))
        .cloned()
        .collect();
    if shares.is_empty() {
        crate::source::net_trace("shares_refresh_done count=0");
        return;
    }
    let mut labels = HashMap::new();
    for folder in &shares {
        let (_, count_label) = append_share_row(&list, folder, on_unavailable);
        labels.insert(folder.id, count_label);
    }
    list.set_visible(true);
    crate::source::net_trace(format!("shares_refresh_done count={}", shares.len()));

    // A share's registered smb:// URI may not be the SQLite ancestor of
    // photos indexed through /run/user/UID/gvfs/. Do not use Folder.photo_count
    // as the final Network Shares count: compute exactly the same path matches
    // as the share gallery, without blocking GTK or rebuilding any gallery.
    let share_paths: HashMap<i64, String> = shares.iter().map(|s| (s.id, s.path.clone())).collect();
    let folder_parents: HashMap<i64, Option<i64>> = folders
        .iter()
        .map(|folder| (folder.id, folder.parent_id))
        .collect();
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let counts = (|| -> anyhow::Result<HashMap<i64, i64>> {
            let connection = crate::db::open_default()?;
            let photos = crate::db::photos(&connection, None, false, None)?;
            let mut counts: HashMap<i64, i64> = shares.iter().map(|share| (share.id, 0)).collect();
            for photo in &photos {
                for share in &shares {
                    let mut ancestor = photo.folder_id;
                    let mut linked = false;
                    // Protect against a malformed/cyclic folder hierarchy.
                    for _ in 0..folder_parents.len() {
                        let Some(id) = ancestor else {
                            break;
                        };
                        if id == share.id {
                            linked = true;
                            break;
                        }
                        ancestor = folder_parents.get(&id).copied().flatten();
                    }
                    if linked
                        || crate::window::path_belongs_to_share(&share.path, &photo.path)
                        || photo.folder_path.as_deref().is_some_and(|path| {
                            crate::window::path_belongs_to_share(&share.path, path)
                        })
                    {
                        *counts.entry(share.id).or_default() += 1;
                    }
                }
            }
            Ok(counts)
        })();
        if let Err(error) = &counts {
            crate::source::net_trace(format!("shares_count_failed error={error}"));
        }
        let _ = sender.send(counts);
    });
    glib::timeout_add_local(Duration::from_millis(50), move || {
        if SHARE_COUNT_GENERATION.load(Ordering::Relaxed) != generation {
            return glib::ControlFlow::Break;
        }
        match receiver.try_recv() {
            Ok(Ok(counts)) => {
                for (id, count) in &counts {
                    if let Some(label) = labels.get(id) {
                        label.set_text(&format_count(*count));
                    }
                }
                SHARE_COUNT_CACHE.with(|cache| {
                    let mut cache = cache.borrow_mut();
                    for (id, count) in &counts {
                        if let Some(path) = share_paths.get(id) {
                            cache.insert(*id, (path.clone(), *count));
                        }
                    }
                });
                crate::source::net_trace(format!("shares_counts_ready count={}", counts.len()));
                glib::ControlFlow::Break
            }
            Ok(Err(_)) | Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                glib::ControlFlow::Break
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
        }
    });
}

fn append_share_row(
    list: &gtk::ListBox,
    folder: &Folder,
    on_unavailable: &Rc<dyn Fn()>,
) -> (gtk::ListBoxRow, gtk::Label) {
    let _ = on_unavailable;
    let row = gtk::ListBoxRow::new();
    row.set_margin_top(2);
    row.set_margin_bottom(2);
    unsafe {
        row.set_data("picasa-filter", SidebarFilter::Folder(folder.id));
    }
    row.set_tooltip_text(Some(folder.path.as_str()));

    let content = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    content.set_margin_top(4);
    content.set_margin_bottom(4);
    content.set_margin_start(4);
    content.set_margin_end(8);

    let icon_overlay = gtk::Overlay::new();
    icon_overlay.set_size_request(18, 18);
    icon_overlay.set_halign(gtk::Align::Center);
    icon_overlay.set_valign(gtk::Align::Center);
    let icon = gtk::Image::from_icon_name("folder-remote-symbolic");
    icon.set_pixel_size(18);
    icon_overlay.set_child(Some(&icon));
    if !folder.available {
        // Same offline indicator as local folders: the library rows survive,
        // only the original files are unreachable right now.
        let badge = gtk::Image::from_icon_name("dialog-warning-symbolic");
        badge.set_pixel_size(11);
        badge.set_halign(gtk::Align::End);
        badge.set_valign(gtk::Align::End);
        badge.add_css_class("unavailable-badge");
        badge.set_tooltip_text(Some(
            "Share unreachable - cached thumbnails remain available",
        ));
        icon_overlay.add_overlay(&badge);
    }
    content.append(&icon_overlay);

    let name = gtk::Label::new(Some(&folder.name));
    name.set_xalign(0.0);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    name.set_hexpand(true);
    content.append(&name);

    // Never make counts disappear while an asynchronous refresh is pending.
    // Show the previous path-matched value, or the stored folder count until
    // the new calculation arrives. Display zero explicitly when appropriate.
    let previous = SHARE_COUNT_CACHE.with(|cache| {
        cache
            .borrow()
            .get(&folder.id)
            .filter(|(path, _)| path == &folder.path)
            .map(|(_, count)| *count)
    });
    let count = gtk::Label::new(Some(&format_count(previous.unwrap_or(folder.photo_count))));
    count.add_css_class("dim-label");
    count.add_css_class("sidebar-count");
    count.set_valign(gtk::Align::Center);
    content.append(&count);

    row.set_child(Some(&content));
    add_share_context_menu(list, &row, folder);
    trace_share_row_press(&row, folder);
    list.append(&row);
    (row, count)
}

/// Earliest GTK input callback for a network-share row: a capture-phase press
/// gesture fires before the ListBox's row-selected handling, so it marks the
/// true start of the click -> gallery interaction being traced. The generated
/// operation id is stored on the row so the follow-up row-selected handler and
/// the whole downstream pipeline share the same op.
fn trace_share_row_press(row: &gtk::ListBoxRow, folder: &Folder) {
    let folder_id = folder.id;
    let row_for_press = row.clone();
    let press = gtk::GestureClick::new();
    press.set_button(1);
    press.connect_pressed(move |_, _, _, _| {
        let op = crate::source::new_trace_op();
        crate::source::set_trace_op(op);
        unsafe {
            row_for_press.set_data("picasa-trace-op", op);
        }
        crate::source::net_trace(format!("share_row_press folder={folder_id} op={op}"));
    });
    row.add_controller(press);
}

fn add_share_context_menu(list: &gtk::ListBox, row: &gtk::ListBoxRow, folder: &Folder) {
    let open = unsafe {
        list.data::<Rc<dyn Fn(Folder)>>(SHARE_OPEN_KEY)
            .map(|callback| callback.as_ref().clone())
    };
    let retry = unsafe {
        list.data::<Rc<dyn Fn(Folder)>>(SHARE_RETRY_KEY)
            .map(|callback| callback.as_ref().clone())
    };
    let refresh = unsafe {
        list.data::<Rc<dyn Fn(String)>>(FOLDER_REFRESH_KEY)
            .map(|callback| callback.as_ref().clone())
    };
    let remove = unsafe {
        list.data::<Rc<dyn Fn(Folder)>>(FOLDER_REMOVE_KEY)
            .map(|callback| callback.as_ref().clone())
    };
    let (Some(open), Some(retry), Some(refresh), Some(remove)) = (open, retry, refresh, remove)
    else {
        return;
    };

    let folder_for_menu = folder.clone();
    let row_for_menu = row.clone();
    let right_click = gtk::GestureClick::new();
    right_click.set_button(3);
    right_click.connect_pressed(move |gesture, _, _, _| {
        let popover = gtk::Popover::new();
        popover.set_has_arrow(true);
        popover.set_parent(&row_for_menu);
        let menu = gtk::Box::new(gtk::Orientation::Vertical, 2);
        menu.set_margin_top(6);
        menu.set_margin_bottom(6);
        menu.set_margin_start(6);
        menu.set_margin_end(6);

        let open_item = gtk::Button::with_label("Open");
        open_item.add_css_class("flat");
        {
            let folder = folder_for_menu.clone();
            let open = open.clone();
            let popover_for_open = popover.clone();
            open_item.connect_clicked(move |_| {
                popover_for_open.popdown();
                open(folder.clone());
            });
        }
        menu.append(&open_item);

        // This opens the registered share in the system file manager without
        // moving it into PIC's local Folders section.
        let open_network_folder = gtk::Button::with_label("Open Network Folder in Files");
        open_network_folder.add_css_class("flat");
        {
            let path = folder_for_menu.path.clone();
            let popover_for_folder = popover.clone();
            open_network_folder.connect_clicked(move |_| {
                popover_for_folder.popdown();
                let uri = crate::source::file(&path).uri();
                let _ = gio::AppInfo::launch_default_for_uri(&uri, None::<&gio::AppLaunchContext>);
            });
        }
        menu.append(&open_network_folder);

        let retry_item = gtk::Button::with_label("Retry Connection");
        retry_item.add_css_class("flat");
        {
            let folder = folder_for_menu.clone();
            let retry = retry.clone();
            let popover_for_retry = popover.clone();
            retry_item.connect_clicked(move |_| {
                popover_for_retry.popdown();
                retry(folder.clone());
            });
        }
        menu.append(&retry_item);

        let rescan_item = gtk::Button::with_label("Rescan");
        rescan_item.add_css_class("flat");
        rescan_item.set_sensitive(folder_for_menu.imported_root);
        {
            let path = folder_for_menu.path.clone();
            let refresh = refresh.clone();
            let popover_for_rescan = popover.clone();
            rescan_item.connect_clicked(move |_| {
                popover_for_rescan.popdown();
                refresh(path.clone());
            });
        }
        menu.append(&rescan_item);

        let remove_item = gtk::Button::with_label("Remove from Library");
        remove_item.add_css_class("flat");
        remove_item.add_css_class("destructive-action");
        {
            let folder = folder_for_menu.clone();
            let remove = remove.clone();
            let popover_for_remove = popover.clone();
            remove_item.set_tooltip_text(Some(
                "Removes the share and its index from PIC. Files on the server are never touched.",
            ));
            remove_item.connect_clicked(move |_| {
                popover_for_remove.popdown();
                remove(folder.clone());
            });
        }
        menu.append(&remove_item);

        popover.set_child(Some(&menu));
        popover.popup();
        gesture.set_state(gtk::EventSequenceState::Claimed);
    });
    row.add_controller(right_click);
}

#[cfg(test)]
mod local_network_section_tests {
    use super::*;

    fn folder(path: &str, imported_root: bool) -> Folder {
        Folder {
            id: 1,
            path: path.to_owned(),
            name: "test".to_owned(),
            parent_id: None,
            imported_root,
            watched: false,
            photo_count: 1,
            subfolder_count: 0,
            available: true,
        }
    }

    #[test]
    fn mounted_local_volumes_stay_under_folders() {
        assert!(is_local_sidebar_folder(&folder("/mnt/4TBP", true)));
        assert!(is_local_sidebar_folder(&folder("/mnt/4TBP/mypics", false)));
        assert!(is_local_sidebar_folder(&folder(
            "/run/media/peet/USB/Pics",
            false
        )));
    }

    #[test]
    fn photo_context_routing_preserves_local_mounts() {
        assert!(!is_network_photo_path("/mnt/4TBP/mypics/photo.jpg"));
        assert!(!is_network_photo_path("/run/media/peet/USB/Pics/photo.jpg"));
        assert!(is_network_photo_path(
            "smb://dietpi.local/4tbp/Work/Denver/photo.jpg"
        ));
        assert!(is_network_photo_path(
            "/run/user/1000/gvfs/smb-share:server=dietpi.local,share=4tbp/Work/Denver/photo.jpg"
        ));
    }

    #[test]
    fn gvfs_backed_network_photos_never_appear_under_folders() {
        assert!(!is_local_sidebar_folder(&folder(
            "smb://dietpi.local/4tbp/Work/Denver",
            true
        )));
        assert!(!is_local_sidebar_folder(&folder(
            "/run/user/1000/gvfs",
            false
        )));
        assert!(!is_local_sidebar_folder(&folder(
            "/run/user/1000/gvfs/smb-share:server=dietpi.local,share=4tbp/Work/Denver",
            false
        )));
        assert!(!is_local_sidebar_folder(&folder(
            "/run/user/1000/gvfs/sftp:host=somewhere/Photos",
            false
        )));
        assert!(!is_local_sidebar_folder(&folder("/run/user/1000", false)));
        assert!(is_local_sidebar_folder(&folder(
            "/run/user/1000/explicit-local-mount",
            true
        )));
    }
}
