use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk4 as gtk;
use libadwaita as adw;
use libadwaita::prelude::*;
use rusqlite::Connection;

#[derive(Clone, Default)]
pub struct SettingsWindow {
    window: Rc<RefCell<glib::WeakRef<adw::Window>>>,
    stack: Rc<RefCell<glib::WeakRef<gtk::Stack>>>,
}

impl SettingsWindow {
    pub fn present(
        &self,
        parent: &adw::ApplicationWindow,
        connection: Rc<RefCell<Connection>>,
        formats_changed: Rc<dyn Fn()>,
        folder_watch_changed: Rc<dyn Fn(i64, bool)>,
        theme_changed: Rc<dyn Fn()>,
        initial_page: Option<&str>,
    ) {
        if let Some(window) = self.window.borrow().upgrade() {
            if let (Some(page), Some(stack)) = (initial_page, self.stack.borrow().upgrade()) {
                stack.set_visible_child_name(page);
            }
            window.present();
            return;
        }

        let window = adw::Window::new();
        window.set_title(Some("Settings"));
        window.set_default_size(860, 620);
        window.set_transient_for(Some(parent));
        window.set_destroy_with_parent(true);
        window.set_modal(false);

        let layout = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let header = adw::HeaderBar::new();
        header.set_title_widget(Some(&gtk::Label::new(Some("Settings"))));
        layout.append(&header);

        let stack = gtk::Stack::new();
        stack.set_hexpand(true);
        stack.set_vexpand(true);
        stack.set_transition_type(gtk::StackTransitionType::Crossfade);

        stack.add_titled(
            &formats_page(connection.clone(), formats_changed.clone()),
            Some("formats"),
            "File Formats",
        );
        stack.add_titled(
            &themes_page(connection.clone(), theme_changed),
            Some("themes"),
            "Themes",
        );
        stack.add_titled(
            &folders_page(connection.clone(), folder_watch_changed),
            Some("folders"),
            "Folders",
        );
        stack.add_titled(&albums_page(&connection.borrow()), Some("albums"), "Albums");
        stack.add_titled(
            &library_page(connection.clone(), formats_changed),
            Some("library"),
            "Library",
        );
        if let Some(page) = initial_page {
            stack.set_visible_child_name(page);
        }

        let categories = gtk::StackSidebar::new();
        categories.set_stack(&stack);
        categories.set_width_request(190);
        categories.set_vexpand(true);

        let split = gtk::Paned::new(gtk::Orientation::Horizontal);
        split.set_start_child(Some(&categories));
        split.set_end_child(Some(&stack));
        split.set_position(190);
        split.set_resize_start_child(false);
        split.set_shrink_start_child(false);
        split.set_wide_handle(true);
        split.set_vexpand(true);
        layout.append(&split);
        window.set_content(Some(&layout));

        self.window.borrow_mut().set(Some(&window));
        self.stack.borrow_mut().set(Some(&stack));
        window.present();
    }
}

fn formats_page(
    connection: Rc<RefCell<Connection>>,
    formats_changed: Rc<dyn Fn()>,
) -> gtk::ScrolledWindow {
    let content = page_content(
        "File Formats",
        "Choose which indexed image formats are visible in the library.",
    );
    let list = settings_list();
    for format in crate::image_format::all() {
        let extensions = format
            .extensions
            .iter()
            .map(|extension| format!(".{extension}"))
            .collect::<Vec<_>>()
            .join(", ");
        let toggle = gtk::Switch::new();
        toggle.set_valign(gtk::Align::Center);
        toggle.set_active(
            crate::image_format::is_enabled(&connection.borrow(), format).unwrap_or(true),
        );
        let connection = connection.clone();
        let formats_changed = formats_changed.clone();
        toggle.connect_active_notify(move |toggle| {
            if let Err(error) =
                crate::image_format::set_enabled(&connection.borrow(), format, toggle.is_active())
            {
                eprintln!("Could not save {} visibility: {error}", format.name);
                return;
            }
            formats_changed();
        });
        append_row(
            &list,
            format.name,
            Some(&extensions),
            Some(toggle.upcast_ref()),
        );
    }
    content.append(&list);
    scroll_page(content)
}

fn folders_page(
    connection: Rc<RefCell<Connection>>,
    folder_watch_changed: Rc<dyn Fn(i64, bool)>,
) -> gtk::ScrolledWindow {
    let content = page_content("Folders", "Folders currently registered in the library.");
    let watching_title = gtk::Label::new(Some("Automatic folder watching"));
    watching_title.set_halign(gtk::Align::Start);
    watching_title.add_css_class("heading");
    content.append(&watching_title);
    let watching_description = gtk::Label::new(Some(
        "When enabled, new and modified photos are detected automatically and imported or refreshed in the library. Other filesystem changes trigger a folder refresh.",
    ));
    watching_description.set_halign(gtk::Align::Start);
    watching_description.set_wrap(true);
    watching_description.add_css_class("dim-label");
    content.append(&watching_description);
    let folders = crate::db::folders(&connection.borrow()).unwrap_or_default();
    let list = settings_list();
    for folder in &folders {
        let status = if folder.available {
            "Available"
        } else {
            "Unavailable"
        };
        let watch = gtk::Switch::new();
        watch.set_valign(gtk::Align::Center);
        watch.set_active(folder.watched);
        watch.set_tooltip_text(Some("Watch this folder for changes"));
        let connection = connection.clone();
        let folder_watch_changed = folder_watch_changed.clone();
        let folder_id = folder.id;
        watch.connect_active_notify(move |switch| {
            let watched = switch.is_active();
            if let Err(error) =
                crate::db::set_folder_watched(&connection.borrow(), folder_id, watched)
            {
                eprintln!("Could not save folder watch setting: {error}");
                switch.set_active(!watched);
                return;
            }
            folder_watch_changed(folder_id, watched);
        });
        append_row(
            &list,
            &folder.name,
            Some(&format!(
                "{}\n{status} · {} photos{}",
                folder.path,
                folder.photo_count,
                if folder.watched { " · Watching" } else { "" }
            )),
            Some(watch.upcast_ref()),
        );
    }
    append_empty_state(&list, "No library folders", folders.is_empty());
    content.append(&list);
    scroll_page(content)
}

fn albums_page(connection: &Connection) -> gtk::ScrolledWindow {
    let content = page_content("Albums", "Virtual albums in this library.");
    let albums = crate::db::albums(connection).unwrap_or_default();
    let list = settings_list();
    for album in &albums {
        append_row(
            &list,
            &album.name,
            Some(&format!("{} photos", album.photo_count)),
            None,
        );
    }
    append_empty_state(&list, "No albums", albums.is_empty());
    content.append(&list);
    scroll_page(content)
}

fn library_page(
    connection: Rc<RefCell<Connection>>,
    recently_added_changed: Rc<dyn Fn()>,
) -> gtk::ScrolledWindow {
    let content = page_content("Library", "Current library statistics.");
    let list = settings_list();
    let recent_limit = gtk::SpinButton::with_range(1.0, 1_000_000.0, 1.0);
    recent_limit.set_value(crate::db::recently_added_limit(&connection.borrow()) as f64);
    recent_limit.set_numeric(true);
    recent_limit.set_digits(0);
    let connection_for_limit = connection.clone();
    recent_limit.connect_value_changed(move |spin| {
        let value = spin.value_as_int().max(1);
        if let Err(error) = crate::db::set_setting(
            &connection_for_limit.borrow(),
            crate::db::RECENTLY_ADDED_LIMIT_SETTING_KEY,
            &value.to_string(),
        ) {
            eprintln!("Could not save Recently Added limit: {error}");
            return;
        }
        recently_added_changed();
    });
    append_row(
        &list,
        "Recently Added limit",
        Some("Maximum number of photos shown in the Recently Added view."),
        Some(recent_limit.upcast_ref()),
    );
    let counts = crate::db::library_counts(&connection.borrow()).unwrap_or_default();
    let thumbnail_count = crate::thumbnail::cache_count().unwrap_or_default();
    let cache_size = crate::thumbnail::cache_size().unwrap_or_default();
    let database_size = crate::db::database_size(&connection.borrow()).unwrap_or_default();
    let available = crate::db::setting(
        &connection.borrow(),
        crate::db::LIBRARY_AVAILABLE_SETTING_KEY,
    )
    .ok()
    .flatten()
    .unwrap_or_else(|| "Not calculated yet".to_string());
    let unavailable = crate::db::setting(
        &connection.borrow(),
        crate::db::LIBRARY_UNAVAILABLE_SETTING_KEY,
    )
    .ok()
    .flatten()
    .unwrap_or_else(|| "Not calculated yet".to_string());
    let updated = gtk::Label::new(
        crate::db::setting(
            &connection.borrow(),
            crate::db::LIBRARY_STATS_UPDATED_SETTING_KEY,
        )
        .ok()
        .flatten()
        .as_deref(),
    );
    updated.set_xalign(1.0);

    for (name, value) in [
        ("Total photos", counts.photos.to_string()),
        ("Total thumbnails", thumbnail_count.to_string()),
        ("Total albums", counts.albums.to_string()),
        ("Total library folders", counts.folders.to_string()),
        ("Thumbnail cache size", format_bytes(cache_size)),
        ("Database size", format_bytes(database_size)),
    ] {
        append_row(&list, name, Some(&value), None);
    }
    let available_label = append_row(&list, "Originals available", Some(&available), None)
        .expect("availability value row has a value label");
    let unavailable_label = append_row(&list, "Originals unavailable", Some(&unavailable), None)
        .expect("availability value row has a value label");
    let update_button = gtk::Button::with_label("Update now");
    update_button.set_valign(gtk::Align::Center);
    let updated_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    updated_row.append(&update_button);
    updated_row.append(&updated);
    append_row(
        &list,
        "Availability stats last updated",
        Some("Values are read from the database. Update only when needed."),
        Some(updated_row.upcast_ref()),
    );
    content.append(&list);

    let stats_running = Rc::new(Cell::new(false));
    let connection_for_update = connection.clone();
    let available_for_update = available_label.clone();
    let unavailable_for_update = unavailable_label.clone();
    let updated_for_update = updated.clone();
    let button_for_update = update_button.clone();
    let stats_running_for_update = stats_running.clone();
    update_button.connect_clicked(move |_| {
        if stats_running_for_update.replace(true) {
            return;
        }
        button_for_update.set_sensitive(false);
        updated_for_update.set_text("Updating…");
        schedule_availability_stats(
            connection_for_update.clone(),
            available_for_update.clone(),
            unavailable_for_update.clone(),
            updated_for_update.clone(),
            button_for_update.clone(),
            stats_running_for_update.clone(),
        );
    });
    scroll_page(content)
}

fn schedule_availability_stats(
    connection: Rc<RefCell<Connection>>,
    available_label: gtk::Label,
    unavailable_label: gtk::Label,
    updated: gtk::Label,
    update_button: gtk::Button,
    stats_running: Rc<Cell<bool>>,
) {
    const PAGE_SIZE: usize = 256;
    let mut offset = 0usize;
    let mut available = 0i64;
    let mut unavailable = 0i64;
    glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
        let page = match crate::db::photo_availability_page(&connection.borrow(), PAGE_SIZE, offset)
        {
            Ok(page) => page,
            Err(error) => {
                eprintln!("Could not update library availability stats: {error}");
                stats_running.set(false);
                update_button.set_sensitive(true);
                return glib::ControlFlow::Break;
            }
        };
        if page.is_empty() {
            let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M").to_string();
            if let Err(error) = crate::db::set_setting(
                &connection.borrow(),
                crate::db::LIBRARY_AVAILABLE_SETTING_KEY,
                &available.to_string(),
            ) {
                eprintln!("Could not save available library stat: {error}");
            }
            if let Err(error) = crate::db::set_setting(
                &connection.borrow(),
                crate::db::LIBRARY_UNAVAILABLE_SETTING_KEY,
                &unavailable.to_string(),
            ) {
                eprintln!("Could not save unavailable library stat: {error}");
            }
            if let Err(error) = crate::db::set_setting(
                &connection.borrow(),
                crate::db::LIBRARY_STATS_UPDATED_SETTING_KEY,
                &timestamp,
            ) {
                eprintln!("Could not save library stats timestamp: {error}");
            }
            updated.set_text(&timestamp);
            stats_running.set(false);
            update_button.set_sensitive(true);
            return glib::ControlFlow::Break;
        }

        for (path, _folder_path) in &page {
            let is_available = crate::source::cached_file_available(path);
            if is_available {
                available += 1;
            } else {
                unavailable += 1;
            }
        }
        offset += page.len();
        available_label.set_text(&available.to_string());
        unavailable_label.set_text(&unavailable.to_string());
        updated.set_text("Updating…");
        glib::ControlFlow::Continue
    });
}

/// Refresh cached availability values after an import or refresh without
/// requiring the Settings window to be open. Work is paginated so the GTK
/// loop remains responsive for large libraries and network shares.
pub fn refresh_library_availability_stats(connection: Rc<RefCell<Connection>>) {
    const PAGE_SIZE: usize = 256;
    let mut offset = 0usize;
    let mut available = 0i64;
    let mut unavailable = 0i64;
    glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
        let page = match crate::db::photo_availability_page(&connection.borrow(), PAGE_SIZE, offset)
        {
            Ok(page) => page,
            Err(error) => {
                eprintln!("Could not refresh library availability stats: {error}");
                return glib::ControlFlow::Break;
            }
        };
        if page.is_empty() {
            let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M").to_string();
            if let Err(error) = crate::db::set_setting(
                &connection.borrow(),
                crate::db::LIBRARY_AVAILABLE_SETTING_KEY,
                &available.to_string(),
            ) {
                eprintln!("Could not save available library stat: {error}");
            }
            if let Err(error) = crate::db::set_setting(
                &connection.borrow(),
                crate::db::LIBRARY_UNAVAILABLE_SETTING_KEY,
                &unavailable.to_string(),
            ) {
                eprintln!("Could not save unavailable library stat: {error}");
            }
            if let Err(error) = crate::db::set_setting(
                &connection.borrow(),
                crate::db::LIBRARY_STATS_UPDATED_SETTING_KEY,
                &timestamp,
            ) {
                eprintln!("Could not save library stats timestamp: {error}");
            }
            return glib::ControlFlow::Break;
        }
        for (path, _folder_path) in &page {
            let is_available = crate::source::cached_file_available(path);
            if is_available {
                available += 1;
            } else {
                unavailable += 1;
            }
        }
        offset += page.len();
        glib::ControlFlow::Continue
    });
}

// Keep the original key so existing bookshelf preferences survive the theme expansion.
pub(crate) const BOOKSHELF_SETTING_KEY: &str = "iphone-bookshelf-albums";
pub(crate) const ALBUM_VIEW_STYLE_SETTING_KEY: &str = "albums-home-style";
pub(crate) const ALBUM_BOOKSHELF_ENABLED_SETTING_KEY: &str = "albums-bookshelf-enabled";
pub(crate) const ALBUM_COVERS_ENABLED_SETTING_KEY: &str = "albums-covers-enabled";
pub(crate) const ALBUM_BOOKSHELF_BACKGROUND_SETTING_KEY: &str = "albums-bookshelf-background";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct AlbumAppearance {
    pub bookshelf_enabled: bool,
    pub covers_enabled: bool,
    pub background_index: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum AlbumViewStyle {
    #[default]
    Default,
    Bookshelf,
    AlbumCovers,
}

#[cfg(test)]
impl AlbumViewStyle {
    fn setting_value(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Bookshelf => "bookshelf",
            Self::AlbumCovers => "album-covers",
        }
    }
}

pub(crate) fn album_view_style(connection: &Connection) -> AlbumViewStyle {
    match crate::db::setting(connection, ALBUM_VIEW_STYLE_SETTING_KEY)
        .ok()
        .flatten()
        .as_deref()
    {
        Some("bookshelf") => AlbumViewStyle::Bookshelf,
        Some("album-covers") => AlbumViewStyle::AlbumCovers,
        Some("default") => AlbumViewStyle::Default,
        _ if crate::db::setting(connection, BOOKSHELF_SETTING_KEY)
            .ok()
            .flatten()
            .as_deref()
            == Some("true") =>
        {
            AlbumViewStyle::Bookshelf
        }
        _ => AlbumViewStyle::Default,
    }
}

fn saved_bool(connection: &Connection, key: &str) -> Option<bool> {
    crate::db::setting(connection, key)
        .ok()
        .flatten()
        .and_then(|value| match value.as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        })
}

pub(crate) fn album_appearance(connection: &Connection) -> AlbumAppearance {
    let legacy_style = album_view_style(connection);
    AlbumAppearance {
        bookshelf_enabled: saved_bool(connection, ALBUM_BOOKSHELF_ENABLED_SETTING_KEY)
            .unwrap_or(legacy_style == AlbumViewStyle::Bookshelf),
        covers_enabled: saved_bool(connection, ALBUM_COVERS_ENABLED_SETTING_KEY)
            .unwrap_or(legacy_style != AlbumViewStyle::Default),
        background_index: crate::db::setting(connection, ALBUM_BOOKSHELF_BACKGROUND_SETTING_KEY)
            .ok()
            .flatten()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(0),
    }
}

pub(crate) fn set_bookshelf_enabled(connection: &Connection, enabled: bool) -> anyhow::Result<()> {
    crate::db::set_setting(
        connection,
        ALBUM_BOOKSHELF_ENABLED_SETTING_KEY,
        if enabled { "true" } else { "false" },
    )
}

pub(crate) fn set_covers_enabled(connection: &Connection, enabled: bool) -> anyhow::Result<()> {
    crate::db::set_setting(
        connection,
        ALBUM_COVERS_ENABLED_SETTING_KEY,
        if enabled { "true" } else { "false" },
    )
}

pub(crate) fn next_bookshelf_background(
    connection: &Connection,
    background_count: usize,
) -> anyhow::Result<usize> {
    if background_count == 0 {
        anyhow::bail!("no bookshelf backgrounds are available");
    }
    let next = (album_appearance(connection).background_index + 1) % background_count;
    crate::db::set_setting(
        connection,
        ALBUM_BOOKSHELF_BACKGROUND_SETTING_KEY,
        &next.to_string(),
    )?;
    set_bookshelf_enabled(connection, true)?;
    Ok(next)
}

pub(crate) fn disable_all_album_themes(connection: &Connection) -> anyhow::Result<()> {
    set_bookshelf_enabled(connection, false)?;
    set_covers_enabled(connection, false)
}

#[cfg(test)]
fn set_album_view_style(connection: &Connection, style: AlbumViewStyle) -> anyhow::Result<()> {
    crate::db::set_setting(
        connection,
        ALBUM_VIEW_STYLE_SETTING_KEY,
        style.setting_value(),
    )
}

fn themes_page(
    connection: Rc<RefCell<Connection>>,
    theme_changed: Rc<dyn Fn()>,
) -> gtk::ScrolledWindow {
    let content = page_content("Themes", "Customize theme options.");
    let heading = gtk::Label::new(Some("Albums"));
    heading.set_halign(gtk::Align::Start);
    heading.add_css_class("heading");
    content.append(&heading);

    let list = settings_list();
    let appearance = album_appearance(&connection.borrow());
    let updating = Rc::new(Cell::new(false));

    let bookshelf = gtk::Switch::new();
    bookshelf.set_active(appearance.bookshelf_enabled);
    bookshelf.set_valign(gtk::Align::Center);
    append_row(
        &list,
        "Bookshelf",
        Some("Show the selected wooden background behind albums."),
        Some(bookshelf.upcast_ref()),
    );

    let next_background = gtk::Button::with_label("Next Background");
    next_background.set_valign(gtk::Align::Center);
    append_row(
        &list,
        "Bookshelf background",
        Some("Cycle through the available bookshelf images."),
        Some(next_background.upcast_ref()),
    );

    let album_covers = gtk::Switch::new();
    album_covers.set_active(appearance.covers_enabled);
    album_covers.set_valign(gtk::Align::Center);
    append_row(
        &list,
        "Album Covers",
        Some("Show decorative covers around album thumbnails."),
        Some(album_covers.upcast_ref()),
    );

    let disable_all = gtk::Button::with_label("Disable All Themes");
    disable_all.set_valign(gtk::Align::Center);
    disable_all.set_sensitive(appearance.bookshelf_enabled || appearance.covers_enabled);
    append_row(
        &list,
        "Default appearance",
        Some("Turn off the bookshelf and album covers."),
        Some(disable_all.upcast_ref()),
    );

    {
        let connection = connection.clone();
        let theme_changed = theme_changed.clone();
        let updating = updating.clone();
        let disable_all = disable_all.clone();
        let album_covers = album_covers.clone();
        bookshelf.connect_active_notify(move |bookshelf| {
            if updating.get() {
                return;
            }
            if let Err(error) = set_bookshelf_enabled(&connection.borrow(), bookshelf.is_active()) {
                eprintln!("Could not save bookshelf setting: {error}");
                return;
            }
            disable_all.set_sensitive(bookshelf.is_active() || album_covers.is_active());
            theme_changed();
        });
    }
    {
        let connection = connection.clone();
        let theme_changed = theme_changed.clone();
        let updating = updating.clone();
        let disable_all = disable_all.clone();
        let bookshelf = bookshelf.clone();
        album_covers.connect_active_notify(move |covers| {
            if updating.get() {
                return;
            }
            if let Err(error) = set_covers_enabled(&connection.borrow(), covers.is_active()) {
                eprintln!("Could not save album cover setting: {error}");
                return;
            }
            disable_all.set_sensitive(bookshelf.is_active() || covers.is_active());
            theme_changed();
        });
    }
    {
        let connection = connection.clone();
        let theme_changed = theme_changed.clone();
        let updating = updating.clone();
        let bookshelf = bookshelf.clone();
        let disable_all = disable_all.clone();
        next_background.connect_clicked(move |_| {
            if let Err(error) = next_bookshelf_background(
                &connection.borrow(),
                crate::albums_view::bookshelf_background_count(),
            ) {
                eprintln!("Could not save bookshelf background: {error}");
                return;
            }
            updating.set(true);
            bookshelf.set_active(true);
            updating.set(false);
            disable_all.set_sensitive(true);
            theme_changed();
        });
    }
    {
        let connection = connection.clone();
        let theme_changed = theme_changed.clone();
        let updating = updating.clone();
        let bookshelf = bookshelf.clone();
        let album_covers = album_covers.clone();
        disable_all.connect_clicked(move |button| {
            if let Err(error) = disable_all_album_themes(&connection.borrow()) {
                eprintln!("Could not disable album themes: {error}");
                return;
            }
            updating.set(true);
            bookshelf.set_active(false);
            album_covers.set_active(false);
            updating.set(false);
            button.set_sensitive(false);
            theme_changed();
        });
    }
    content.append(&list);
    scroll_page(content)
}

fn page_content(title: &str, subtitle: &str) -> gtk::Box {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 16);
    content.set_margin_top(28);
    content.set_margin_bottom(28);
    content.set_margin_start(28);
    content.set_margin_end(28);
    let title = gtk::Label::new(Some(title));
    title.set_xalign(0.0);
    title.add_css_class("title-1");
    let subtitle = gtk::Label::new(Some(subtitle));
    subtitle.set_xalign(0.0);
    subtitle.set_wrap(true);
    subtitle.add_css_class("dim-label");
    content.append(&title);
    content.append(&subtitle);
    content
}

fn settings_list() -> gtk::ListBox {
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::None);
    list.add_css_class("boxed-list");
    list
}

fn append_row(
    list: &gtk::ListBox,
    title: &str,
    subtitle: Option<&str>,
    action: Option<&gtk::Widget>,
) -> Option<gtk::Label> {
    let row = gtk::ListBoxRow::new();
    row.set_activatable(false);
    let box_ = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    box_.set_margin_top(10);
    box_.set_margin_bottom(10);
    box_.set_margin_start(12);
    box_.set_margin_end(12);
    let labels = gtk::Box::new(gtk::Orientation::Vertical, 2);
    labels.set_hexpand(true);
    let title = gtk::Label::new(Some(title));
    title.set_xalign(0.0);
    labels.append(&title);
    let subtitle_label = if let Some(subtitle) = subtitle {
        let subtitle = gtk::Label::new(Some(subtitle));
        subtitle.set_xalign(0.0);
        subtitle.set_wrap(true);
        subtitle.set_selectable(true);
        subtitle.add_css_class("dim-label");
        labels.append(&subtitle);
        Some(subtitle)
    } else {
        None
    };
    box_.append(&labels);
    if let Some(action) = action {
        box_.append(action);
    }
    row.set_child(Some(&box_));
    list.append(&row);
    subtitle_label
}

fn append_empty_state(list: &gtk::ListBox, message: &str, empty: bool) {
    if empty {
        append_row(list, message, None, None);
    }
}

fn scroll_page(content: gtk::Box) -> gtk::ScrolledWindow {
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroll.set_child(Some(&content));
    scroll
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::format_bytes;

    #[test]
    fn byte_sizes_are_human_readable() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1536), "1.5 KB");
    }

    #[test]
    fn album_view_style_defaults_and_migrates_the_bookshelf_toggle() {
        use super::*;

        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch("CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL)")
            .unwrap();
        assert_eq!(album_view_style(&connection), AlbumViewStyle::Default);

        crate::db::set_setting(&connection, BOOKSHELF_SETTING_KEY, "true").unwrap();
        assert_eq!(album_view_style(&connection), AlbumViewStyle::Bookshelf);

        set_album_view_style(&connection, AlbumViewStyle::AlbumCovers).unwrap();
        assert_eq!(album_view_style(&connection), AlbumViewStyle::AlbumCovers);
        assert_eq!(
            crate::db::setting(&connection, ALBUM_VIEW_STYLE_SETTING_KEY)
                .unwrap()
                .as_deref(),
            Some("album-covers"),
        );

        set_album_view_style(&connection, AlbumViewStyle::Default).unwrap();
        assert_eq!(album_view_style(&connection), AlbumViewStyle::Default);
    }

    #[test]
    fn album_appearance_migrates_existing_styles_to_independent_options() {
        use super::*;

        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch("CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL)")
            .unwrap();

        crate::db::set_setting(&connection, ALBUM_VIEW_STYLE_SETTING_KEY, "bookshelf").unwrap();
        assert_eq!(
            album_appearance(&connection),
            AlbumAppearance {
                bookshelf_enabled: true,
                covers_enabled: true,
                background_index: 0,
            }
        );

        crate::db::set_setting(&connection, ALBUM_VIEW_STYLE_SETTING_KEY, "album-covers").unwrap();
        assert_eq!(
            album_appearance(&connection),
            AlbumAppearance {
                bookshelf_enabled: false,
                covers_enabled: true,
                background_index: 0,
            }
        );
    }

    #[test]
    fn bookshelf_and_covers_can_be_changed_independently() {
        use super::*;

        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch("CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL)")
            .unwrap();

        set_bookshelf_enabled(&connection, true).unwrap();
        set_covers_enabled(&connection, false).unwrap();
        assert_eq!(
            album_appearance(&connection),
            AlbumAppearance {
                bookshelf_enabled: true,
                covers_enabled: false,
                background_index: 0,
            }
        );

        set_bookshelf_enabled(&connection, false).unwrap();
        set_covers_enabled(&connection, true).unwrap();
        assert_eq!(
            album_appearance(&connection),
            AlbumAppearance {
                bookshelf_enabled: false,
                covers_enabled: true,
                background_index: 0,
            }
        );
    }

    #[test]
    fn next_bookshelf_background_enables_bookshelf_and_wraps() {
        use super::*;

        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch("CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL)")
            .unwrap();

        assert_eq!(next_bookshelf_background(&connection, 4).unwrap(), 1);
        assert!(album_appearance(&connection).bookshelf_enabled);
        assert_eq!(next_bookshelf_background(&connection, 4).unwrap(), 2);
        assert_eq!(next_bookshelf_background(&connection, 4).unwrap(), 3);
        assert_eq!(next_bookshelf_background(&connection, 4).unwrap(), 0);
    }

    #[test]
    fn disabling_all_themes_preserves_the_selected_background() {
        use super::*;

        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch("CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL)")
            .unwrap();

        set_bookshelf_enabled(&connection, true).unwrap();
        set_covers_enabled(&connection, true).unwrap();
        next_bookshelf_background(&connection, 4).unwrap();
        disable_all_album_themes(&connection).unwrap();

        assert_eq!(
            album_appearance(&connection),
            AlbumAppearance {
                bookshelf_enabled: false,
                covers_enabled: false,
                background_index: 1,
            }
        );
    }

    #[test]
    #[ignore = "requires a GTK display; run with --ignored --test-threads=1"]
    fn album_appearance_controls_persist_and_notify() {
        use super::*;

        fn find_controls(
            widget: &gtk::Widget,
            switches: &mut Vec<gtk::Switch>,
            buttons: &mut Vec<gtk::Button>,
        ) {
            if let Some(switch) = widget.downcast_ref::<gtk::Switch>() {
                switches.push(switch.clone());
            }
            if let Some(button) = widget.downcast_ref::<gtk::Button>() {
                buttons.push(button.clone());
            }
            let mut child = widget.first_child();
            while let Some(widget) = child {
                find_controls(&widget, switches, buttons);
                child = widget.next_sibling();
            }
        }

        gtk::init().unwrap();
        let path = std::env::temp_dir().join(format!(
            "pic-bookshelf-setting-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let connection = Rc::new(RefCell::new(Connection::open(&path).unwrap()));
        connection
            .borrow()
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
            )
            .unwrap();
        let notified = Rc::new(Cell::new(0));
        let notified_for_callback = notified.clone();
        let page = themes_page(
            connection.clone(),
            Rc::new(move || notified_for_callback.set(notified_for_callback.get() + 1)),
        );
        let mut switches = Vec::new();
        let mut buttons = Vec::new();
        find_controls(page.upcast_ref(), &mut switches, &mut buttons);
        assert_eq!(
            buttons
                .iter()
                .filter_map(|button| button.label())
                .collect::<Vec<_>>(),
            ["Next Background", "Disable All Themes"],
        );
        assert_eq!(switches.len(), 2);
        assert!(!switches[0].is_active());
        assert!(!switches[1].is_active());

        switches[1].set_active(true);
        assert_eq!(notified.get(), 1);
        buttons[0].emit_clicked();
        assert_eq!(notified.get(), 2);
        assert!(switches[0].is_active());
        assert_eq!(
            album_appearance(&connection.borrow()),
            AlbumAppearance {
                bookshelf_enabled: true,
                covers_enabled: true,
                background_index: 1,
            }
        );
        buttons[1].emit_clicked();
        assert_eq!(notified.get(), 3);
        assert!(!buttons[1].is_sensitive());
        drop(page);
        drop(connection);

        let reopened = Connection::open(&path).unwrap();
        assert_eq!(
            album_appearance(&reopened),
            AlbumAppearance {
                bookshelf_enabled: false,
                covers_enabled: false,
                background_index: 1,
            }
        );
        drop(reopened);
        std::fs::remove_file(path).unwrap();
    }
}
