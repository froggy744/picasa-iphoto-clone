use std::cell::RefCell;
use std::rc::Rc;

use gtk4 as gtk;
use libadwaita as adw;
use libadwaita::prelude::*;
use rusqlite::Connection;

#[derive(Clone, Default)]
pub struct SettingsWindow {
    window: Rc<RefCell<glib::WeakRef<adw::Window>>>,
}

impl SettingsWindow {
    pub fn present(
        &self,
        parent: &adw::ApplicationWindow,
        connection: Rc<RefCell<Connection>>,
        formats_changed: Rc<dyn Fn()>,
    ) {
        if let Some(window) = self.window.borrow().upgrade() {
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
            &formats_page(connection.clone(), formats_changed),
            Some("formats"),
            "File Formats",
        );
        stack.add_titled(
            &placeholder_page("Theme settings coming soon."),
            Some("themes"),
            "Themes",
        );
        stack.add_titled(
            &folders_page(&connection.borrow()),
            Some("folders"),
            "Folders",
        );
        stack.add_titled(&albums_page(&connection.borrow()), Some("albums"), "Albums");
        stack.add_titled(
            &library_page(&connection.borrow()),
            Some("library"),
            "Library",
        );

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

fn folders_page(connection: &Connection) -> gtk::ScrolledWindow {
    let content = page_content("Folders", "Folders currently registered in the library.");
    let folders = crate::db::folders(connection).unwrap_or_default();
    let list = settings_list();
    for folder in &folders {
        let status = if folder.available {
            "Available"
        } else {
            "Unavailable"
        };
        append_row(
            &list,
            &folder.name,
            Some(&format!(
                "{}\n{status} · {} photos",
                folder.path, folder.photo_count
            )),
            None,
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

fn library_page(connection: &Connection) -> gtk::ScrolledWindow {
    let content = page_content("Library", "Current library statistics.");
    let list = settings_list();
    let counts = crate::db::library_counts(connection).unwrap_or_default();
    let photos = crate::db::photos(connection, None, false, None).unwrap_or_default();
    let available = photos
        .iter()
        .filter(|photo| {
            photo
                .folder_path
                .as_deref()
                .map(crate::source::cached_source_available)
                .unwrap_or(true)
                && crate::source::cached_file_available(&photo.path)
        })
        .count() as i64;
    let unavailable = counts.photos.saturating_sub(available);
    let cache_size = crate::thumbnail::cache_size().unwrap_or_default();
    let database_size = crate::db::database_size(connection).unwrap_or_default();

    for (name, value) in [
        ("Total photos", counts.photos.to_string()),
        ("Total albums", counts.albums.to_string()),
        ("Total library folders", counts.folders.to_string()),
        ("Originals available", available.to_string()),
        ("Originals unavailable", unavailable.to_string()),
        ("Thumbnail cache size", format_bytes(cache_size)),
        ("Database size", format_bytes(database_size)),
    ] {
        append_row(&list, name, Some(&value), None);
    }
    content.append(&list);
    scroll_page(content)
}

fn placeholder_page(message: &str) -> gtk::ScrolledWindow {
    let content = page_content("Themes", message);
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
) {
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
    if let Some(subtitle) = subtitle {
        let subtitle = gtk::Label::new(Some(subtitle));
        subtitle.set_xalign(0.0);
        subtitle.set_wrap(true);
        subtitle.set_selectable(true);
        subtitle.add_css_class("dim-label");
        labels.append(&subtitle);
    }
    box_.append(&labels);
    if let Some(action) = action {
        box_.append(action);
    }
    row.set_child(Some(&box_));
    list.append(&row);
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
}
