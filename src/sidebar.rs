use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

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

#[derive(Debug)]
struct SidebarState {
    albums_expanded: bool,
    folders_expanded: bool,
    expanded_folders: HashSet<i64>,
    pinned: Cell<bool>,
    hover_open: Cell<bool>,
}

impl Default for SidebarState {
    fn default() -> Self {
        Self {
            albums_expanded: true,
            folders_expanded: true,
            expanded_folders: HashSet::new(),
            pinned: Cell::new(true),
            hover_open: Cell::new(false),
        }
    }
}

const STATE_KEY: &str = "picasa-sidebar-state";
const LIBRARY_LIST_KEY: &str = "picasa-sidebar-library-list";
const ALBUM_LIST_KEY: &str = "picasa-sidebar-album-list";
const ALBUM_REVEALER_KEY: &str = "picasa-sidebar-album-revealer";
const ALBUM_INDICATOR_KEY: &str = "picasa-sidebar-album-indicator";
const FOLDER_LIST_KEY: &str = "picasa-sidebar-folder-list";
const FOLDER_REVEALER_KEY: &str = "picasa-sidebar-folder-revealer";
const FOLDER_INDICATOR_KEY: &str = "picasa-sidebar-folder-indicator";
const CURRENT_FILTER_KEY: &str = "picasa-sidebar-current-filter";
const FILTER_SYNCING_KEY: &str = "picasa-sidebar-filter-syncing";
const FOLDER_REFRESH_KEY: &str = "picasa-sidebar-folder-refresh";
const FOLDER_STATISTICS_KEY: &str = "picasa-sidebar-folder-statistics";
const FOLDER_REMOVE_KEY: &str = "picasa-sidebar-folder-remove";
const FOLDER_FAVORITE_KEY: &str = "picasa-sidebar-folder-favorite";

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
) -> gtk::ScrolledWindow {
    let on_filter: Rc<dyn Fn(SidebarFilter)> = Rc::new(on_filter);
    let state = Rc::new(RefCell::new(SidebarState::default()));
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

    // LIBRARY: fixed/static.
    let library_list = section_list();
    append_heading_static(&library_list, "Library");
    populate_library(&library_list, counts, &on_unavailable);
    connect_filter_list(&library_list, on_filter.clone(), filter_syncing.clone());
    root.append(&library_list);

    // ALBUMS: fixed/static in the sidebar, but its rows can be collapsed.
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
    root.append(&album_heading);

    let album_list = section_list();
    connect_filter_list(&album_list, on_filter.clone(), filter_syncing.clone());
    let album_revealer = gtk::Revealer::new();
    album_revealer.set_transition_type(gtk::RevealerTransitionType::SlideDown);
    album_revealer.set_reveal_child(true);
    album_revealer.set_child(Some(&album_list));
    root.append(&album_revealer);

    {
        let state = state.clone();
        let revealer = album_revealer.clone();
        let indicator = album_indicator.clone();
        album_indicator.connect_clicked(move |_| {
            let expanded = !state.borrow().albums_expanded;
            state.borrow_mut().albums_expanded = expanded;
            revealer.set_reveal_child(expanded);
            indicator.set_icon_name(if expanded {
                "pan-down-symbolic"
            } else {
                "pan-end-symbolic"
            });
        });
    }

    // FOLDERS: the heading is fixed. Only the folder-content scroller below
    // consumes remaining height and scrolls.
    let (folder_heading, folder_indicator) = collapsible_heading(
        "Folders",
        Some(on_import_folder.clone()),
        "Import Folder",
        true,
        None,
    );
    root.append(&folder_heading);

    let folder_list = section_list();
    connect_filter_list(&folder_list, on_filter, filter_syncing.clone());

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
    root.append(&folder_revealer);

    {
        let state = state.clone();
        let revealer = folder_revealer.clone();
        let indicator = folder_indicator.clone();
        folder_indicator.connect_clicked(move |_| {
            let expanded = !state.borrow().folders_expanded;
            state.borrow_mut().folders_expanded = expanded;
            revealer.set_reveal_child(expanded);
            indicator.set_icon_name(if expanded {
                "pan-down-symbolic"
            } else {
                "pan-end-symbolic"
            });
        });
    }

    populate_albums(&album_list, albums, &on_delete_album);
    populate_folders(&folder_list, folders, &state, &on_unavailable);

    outer.set_child(Some(&root));

    // Store stable widget/state handles on the existing GtkScrolledWindow so
    // refresh() and append_folder() can update only the relevant sections
    // without changing any caller-facing API.
    unsafe {
        folder_list.set_data(FOLDER_REFRESH_KEY, on_refresh_folder);
        folder_list.set_data(FOLDER_STATISTICS_KEY, on_folder_statistics);
        folder_list.set_data(FOLDER_REMOVE_KEY, on_remove_folder);
        folder_list.set_data(FOLDER_FAVORITE_KEY, on_folder_favorite);
        outer.set_data(STATE_KEY, state);
        outer.set_data(LIBRARY_LIST_KEY, library_list);
        outer.set_data(ALBUM_LIST_KEY, album_list);
        outer.set_data(ALBUM_REVEALER_KEY, album_revealer);
        outer.set_data(ALBUM_INDICATOR_KEY, album_indicator);
        outer.set_data(FOLDER_LIST_KEY, folder_list);
        outer.set_data(FOLDER_REVEALER_KEY, folder_revealer);
        outer.set_data(FOLDER_INDICATOR_KEY, folder_indicator);
        outer.set_data(FILTER_SYNCING_KEY, filter_syncing);
    }

    outer
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

    clear_list(&library_list);
    append_heading_static(&library_list, "Library");
    populate_library(&library_list, counts, &on_unavailable);

    clear_list(&album_list);
    populate_albums(&album_list, albums, &on_delete_album);

    clear_list(&folder_list);
    populate_folders(&folder_list, folders, &state, &on_unavailable);

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
        revealer.set_reveal_child(state.borrow().folders_expanded);
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
}

/// Add the folder row immediately when an import starts. The normal refresh
/// remains responsible for counts and ordering once the import completes.
pub fn append_folder(
    scrolled: &gtk::ScrolledWindow,
    folder: &Folder,
    on_unavailable: Rc<dyn Fn()>,
) {
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

pub fn set_active_filter(scrolled: &gtk::ScrolledWindow, filter: SidebarFilter) {
    unsafe {
        scrolled.set_data(CURRENT_FILTER_KEY, filter);
    }

    let syncing = unsafe {
        scrolled
            .data::<Rc<Cell<bool>>>(FILTER_SYNCING_KEY)
            .map(|data| data.as_ref().clone())
    };
    let Some(syncing) = syncing else {
        return;
    };

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

    match filter {
        SidebarFilter::All | SidebarFilter::Favorites | SidebarFilter::RecentlyAdded => {
            select_matching_row(scrolled, LIBRARY_LIST_KEY, filter);
        }
        SidebarFilter::Albums => {}
        SidebarFilter::Album(_) => {
            select_matching_row(scrolled, ALBUM_LIST_KEY, filter);
        }
        SidebarFilter::Folder(_) => {
            select_matching_row(scrolled, FOLDER_LIST_KEY, filter);
        }
    }
    syncing.set(false);
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
            on_filter(unsafe { *value.as_ref() });
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
        "Photos",
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
    unsafe {
        list.set_data("picasa-folder-cache", folders.to_vec());
        list.set_data("picasa-folder-unavailable-callback", on_unavailable.clone());
    }

    if folders.is_empty() {
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
        let parent = if folder.imported_root {
            None
        } else {
            folder.parent_id
        };
        children.entry(parent).or_default().push(folder.id);
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
    if std::env::var_os("PICASA_TRACE").is_some() {
        eprintln!(
            "FOLDER TRACE sidebar folders={} roots={:?} relationships={:?}",
            folders.len(),
            roots,
            folders
                .iter()
                .map(|folder| (
                    folder.id,
                    folder.path.clone(),
                    folder.parent_id,
                    folder.imported_root
                ))
                .collect::<Vec<_>>()
        );
    }
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

    // Every row gets the same fixed chevron slot. This keeps folder icons and
    // names aligned even when the hierarchy is deeply nested.
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

    let icon = gtk::Image::from_icon_name("folder-symbolic");
    icon.set_pixel_size(18);
    content.append(&icon);

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

        let refresh_item = gtk::Button::with_label("Refresh folder");
        refresh_item.add_css_class("flat");
        let path = folder_for_menu.path.clone();
        let popover_for_refresh = popover.clone();
        let refresh = refresh.clone();
        refresh_item.connect_clicked(move |_| {
            popover_for_refresh.popdown();
            refresh(path.clone());
        });
        menu.append(&refresh_item);

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

fn append_heading_static(list: &gtk::ListBox, text: &str) {
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.add_css_class("sidebar-section-heading-title");
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    content.add_css_class("sidebar-section-heading");
    content.set_hexpand(true);
    // ListBoxRow contributes its own horizontal padding; compensate here so
    // this static heading aligns with the direct heading boxes below it.
    content.set_margin_start(2);
    content.set_margin_end(8);
    content.set_margin_top(8);
    content.set_margin_bottom(4);
    content.append(&label);
    let row = gtk::ListBoxRow::new();
    row.set_selectable(false);
    row.set_activatable(false);
    row.set_child(Some(&content));
    list.append(&row);
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

    // Keep the disclosure indicator beside the + action on the right. This
    // matches the section-level controls visually and keeps the heading text
    // itself clear of toggle affordances.
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

    if let Some(action) = action {
        let add = gtk::Button::from_icon_name("list-add-symbolic");
        add.add_css_class("flat");
        add.set_size_request(24, 24);
        add.set_tooltip_text(Some(action_tooltip));
        add.connect_clicked(move |_| action());
        content.append(&add);
    }

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
