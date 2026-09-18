fn show_folder_statistics(
    parent: &gtk::Widget,
    connection: Rc<RefCell<Connection>>,
    folder: db::Folder,
) {
    let photos = db::photos(&connection.borrow(), Some(folder.id), false, None).unwrap_or_default();
    let direct_photos = photos
        .iter()
        .filter(|photo| photo.folder_id == Some(folder.id))
        .count();
    let available = photos
        .iter()
        .filter(|photo| crate::source::cached_file_available(&photo.path))
        .count();
    let total_bytes: u64 = photos
        .iter()
        .filter_map(|photo| photo.size_bytes)
        .filter_map(|size| u64::try_from(size).ok())
        .sum();
    let body = format!(
        "{}\n\nTotal photos: {}\nPhotos directly in this folder: {}\nSubfolders: {}\nOriginals available: {}\nOriginals unavailable: {}\nTotal file size: {}",
        folder.path,
        photos.len(),
        direct_photos,
        folder.subfolder_count,
        available,
        photos.len().saturating_sub(available),
        format_folder_bytes(total_bytes),
    );
    let dialog = adw::AlertDialog::builder()
        .heading(format!("{} statistics", folder.name))
        .body(body)
        .close_response("close")
        .build();
    dialog.add_response("close", "Close");
    dialog.present(Some(parent));
}

fn format_folder_bytes(bytes: u64) -> String {
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

pub(crate) fn debug_log(message: &str) {
    use std::io::Write;
    let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/tmp/pic-ui-debug.log")
    else {
        return;
    };
    let _ = writeln!(file, "{message}");
}

fn install_close_confirmation(window: &adw::ApplicationWindow) {
    let close_confirmation_open = Rc::new(Cell::new(false));
    let close_confirmation_allowed = Rc::new(Cell::new(false));
    let close_confirmation_open_for_request = close_confirmation_open.clone();
    let close_confirmation_allowed_for_request = close_confirmation_allowed.clone();
    window.connect_close_request(move |window| {
        if close_confirmation_allowed_for_request.get() {
            return glib::Propagation::Proceed;
        }
        if close_confirmation_open_for_request.replace(true) {
            return glib::Propagation::Stop;
        }

        let dialog = adw::AlertDialog::builder()
            .heading("Close Picasa?")
            .body("Are you sure you want to close the application?")
            .default_response("cancel")
            .close_response("cancel")
            .build();
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("close", "Close Picasa");
        dialog.set_response_appearance("close", adw::ResponseAppearance::Destructive);

        let window_for_response = window.clone();
        let close_confirmation_open_for_response = close_confirmation_open_for_request.clone();
        let close_confirmation_allowed_for_response = close_confirmation_allowed_for_request.clone();
        dialog.connect_response(None, move |dialog, response| {
            close_confirmation_open_for_response.set(false);
            if response == "close" {
                close_confirmation_allowed_for_response.set(true);
                window_for_response.close();
            } else {
                dialog.close();
            }
        });
        dialog.present(Some(window));
        glib::Propagation::Stop
    });

}

/// Phase 1 "Add Network Share" dialog (SMB only): a display name, a server
/// (host or host:port) and a share name are combined into `smb://server/share`
/// and handed to `on_added`. Nothing here mounts, sudo's or touches fstab:
/// enumeration and reads go through PIC's GIO source layer.
pub fn show_add_network_share_dialog(
    parent: gtk::Widget,
    on_connect: Rc<dyn Fn(String, String)>,
) {
    use gtk::prelude::*;

    let window = parent
        .ancestor(gtk::Window::static_type())
        .and_downcast::<gtk::Window>()
        .or_else(|| parent.root().and_downcast::<gtk::Window>());
    let dialog = gtk::Dialog::new();
    dialog.set_title(Some("Add Network Share"));
    if let Some(window) = &window {
        dialog.set_transient_for(Some(window));
    }
    dialog.set_modal(true);
    dialog.set_default_width(440);
    dialog.add_button("Cancel", gtk::ResponseType::Cancel);
    let connect_button = dialog.add_button("Connect", gtk::ResponseType::Accept);
    dialog.set_default_response(gtk::ResponseType::Accept);

    let content = dialog.content_area();
    content.set_spacing(10);
    content.set_margin_top(14);
    content.set_margin_bottom(6);
    content.set_margin_start(14);
    content.set_margin_end(14);

    let grid = gtk::Grid::new();
    grid.set_row_spacing(8);
    grid.set_column_spacing(10);

    // Protocol: SMB/CIFS and NFS share the same gvfs pipeline; CIFS is handled
    // by the SMB backend, so there are exactly two choices here.
    let protocol = gtk::DropDown::from_strings(&["SMB / CIFS", "NFS"]);
    protocol.set_selected(0);

    let make_entry = |placeholder: &str| {
        let entry = gtk::Entry::new();
        entry.set_hexpand(true);
        entry.set_placeholder_text(Some(placeholder));
        entry
    };
    let name_entry = make_entry("Synology NAS (optional)");
    let server_entry = make_entry("192.168.1.20 or nas.local");
    let share_entry = make_entry("Photos (leave empty to browse all shares)");

    let key = gtk::Label::new(Some("Protocol"));
    key.set_xalign(0.0);
    key.add_css_class("dim-label");
    grid.attach(&key, 0, 0, 1, 1);
    grid.attach(&protocol, 1, 0, 1, 1);
    for (row, label, entry) in
        [(1, "Name", &name_entry), (2, "Server", &server_entry), (3, "Share / Export", &share_entry)]
    {
        let key = gtk::Label::new(Some(label));
        key.set_xalign(0.0);
        key.add_css_class("dim-label");
        grid.attach(&key, 0, row, 1, 1);
        grid.attach(entry, 1, row, 1, 1);
    }
    content.append(&grid);

    // Discovered locations (network:// via gvfs). Best-effort: runs in the
    // background with a ~3s timeout, fills the list as results arrive, and an
    // empty result never blocks manual entry.
    // When the user picks a discovered location, its gvfs-supplied URI is
    // used as-is for Connect (gvfs resolves dnssd/wsdd names to the real
    // host). Manual typing clears it and falls back to building the URI from
    // the Server/Share fields.
    let discovered_uri: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let updating_fields: Rc<Cell<bool>> = Rc::new(Cell::new(false));
    // First discovered SMB target: the fallback when Connect is pressed with
    // no server typed (discovery is convenience for exactly this case).
    let first_smb_target: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let discovered_list = gtk::ListBox::new();
    discovered_list.set_selection_mode(gtk::SelectionMode::None);
    discovered_list.add_css_class("navigation-sidebar");
    let discovered_scroll = gtk::ScrolledWindow::new();
    discovered_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    discovered_scroll.set_max_content_height(110);
    discovered_scroll.set_propagate_natural_height(true);
    discovered_scroll.set_child(Some(&discovered_list));
    discovered_scroll.set_visible(false);
    let discovered_header = gtk::Label::new(Some("Discovered locations"));
    discovered_header.set_xalign(0.0);
    discovered_header.add_css_class("dim-label");
    discovered_header.set_visible(false);
    content.append(&discovered_header);
    content.append(&discovered_scroll);

    {
        let receiver = crate::source::discover_network_locations();
        let list = discovered_list.clone();
        let header = discovered_header.clone();
        let scroll = discovered_scroll.clone();
        let server_for_rows = server_entry.clone();
        let share_for_rows = share_entry.clone();
        let protocol_for_rows = protocol.clone();
        let discovered_for_rows = discovered_uri.clone();
        let updating_for_rows = updating_fields.clone();
        let list_focus_set = Rc::new(Cell::new(false));
        let first_smb_target = first_smb_target.clone();
        let name_for_rows = name_entry.clone();
        let on_connect_for_rows = on_connect.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(50), move || {
            match receiver.try_recv() {
                Ok(crate::source::NetworkDiscoveryEvent::Item(name, uri)) => {
                    // Remember the first SMB target: pressing Connect without
                    // typing connects to it (discovery convenience).
                    if first_smb_target.borrow().is_none() && uri.starts_with("smb://") {
                        *first_smb_target.borrow_mut() = Some(uri.clone());
                    }
                    if !header.is_visible() {
                        header.set_visible(true);
                        scroll.set_visible(true);
                    }
                    let row = gtk::ListBoxRow::new();
                    row.set_focusable(true);
                    let box_ = gtk::Box::new(gtk::Orientation::Horizontal, 6);
                    box_.set_margin_top(2);
                    box_.set_margin_bottom(2);
                    let icon = gtk::Image::from_icon_name("folder-remote-symbolic");
                    icon.set_pixel_size(14);
                    box_.append(&icon);
                    let label = gtk::Label::new(Some(&name));
                    label.set_xalign(0.0);
                    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
                    label.set_hexpand(true);
                    box_.append(&label);
                    row.set_child(Some(&box_));
                    row.set_tooltip_text(Some(&uri));
                    // Use the URI gvfs supplied. Discovered entries look like
                    // network:///dnssd-server-DIETPI._smb._tcp - extract the
                    // server name and service type so the entries get clean
                    // values (server = name, protocol = service type).
                    let server_for_row = server_for_rows.clone();
                    let protocol_for_row = protocol_for_rows.clone();
                    let share_for_row = share_for_rows.clone();
                    let discovered_for_row = discovered_for_rows.clone();
                    let updating = updating_for_rows.clone();
                    let decoded = glib::uri_unescape_string(uri.as_str(), None::<&str>)
                        .unwrap_or_else(|| uri.clone().into());
                    let click = gtk::GestureClick::new();
                    crate::source::net_trace(format!(
                        "discovery_row_clicked name={name} uri={uri}"
                    ));
                    click.connect_pressed(move |gesture, _, _, _| {
                        // Remember the gvfs URI; Connect will use it verbatim.
                        *discovered_for_row.borrow_mut() = Some(decoded.to_string());
                        updating.set(true);
                        // Protocol from the service type: NFS announcements get
                        // the nfs scheme; everything else rides SMB.
                        if decoded.contains("._nfs.") {
                            protocol_for_row.set_selected(1);
                        } else {
                            protocol_for_row.set_selected(0);
                        }
                        // Fill the fields with the PARSED target (host + first
                        // path segment) so they stay clean and resolvable even
                        // if the stored URI is later cleared.
                        let host = crate::source::network_uri_host(&uri);
                        server_for_row.set_text(&host);
                        let path = uri.split("://").nth(1).and_then(|rest| rest.split_once('/').map(|(_, p)| p));
                        if let Some(share) = path {
                            let first = share.split('/').next().unwrap_or("");
                            if !first.is_empty() {
                                share_for_row.set_text(first);
                            }
                        }
                        updating.set(false);
                        gesture.set_state(gtk::EventSequenceState::Claimed);
                    });
                    row.add_controller(click);
                    row.set_focusable(true);
                    // Keyboard users: Enter on a focused row connects directly.
                    let name_for_activate = name_for_rows.clone();
                    let on_connect_for_row = on_connect_for_rows.clone();
                    row.connect_activate(move |row| {
                        let uri = row
                            .tooltip_text()
                            .map(|tooltip| tooltip.to_string())
                            .unwrap_or_default();
                        crate::source::net_trace(format!(
                            "discovery_row_activated uri={uri}"
                        ));
                        let name = name_for_activate.text().trim().to_string();
                        on_connect_for_row(name, uri);
                    });
                    list.append(&row);
                    // Give the list keyboard focus as soon as the first result
                    // arrives, so arrows + Enter drive discovery.
                    if !list_focus_set.get() {
                        list_focus_set.set(true);
                        list.select_row(Some(&row));
                        list.grab_focus();
                    }
                    glib::ControlFlow::Continue
                }
                Ok(crate::source::NetworkDiscoveryEvent::Done(_)) => glib::ControlFlow::Break,
                Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
            }
        });
    }

    let hint = gtk::Label::new(Some(
        "Connects through the system's network services (guest or saved credentials). Enter a Share to jump straight into it, or leave it empty to browse all shares - then pick the folder with your photos.",
    ));
    hint.set_xalign(0.0);
    hint.set_wrap(true);
    hint.add_css_class("dim-label");
    content.append(&hint);

    let discovered_for_changed = discovered_uri.clone();
    let updating_for_changed = updating_fields.clone();
    for entry in [&server_entry, &share_entry] {
        let discovered = discovered_for_changed.clone();
        let updating = updating_for_changed.clone();
        entry.connect_changed(move |_| {
            if !updating.get() {
                *discovered.borrow_mut() = None;
            }
        });
    }

    let connect_for_activate = connect_button.clone();
    for entry in [&name_entry, &server_entry, &share_entry] {
        let button = connect_for_activate.clone();
        entry.connect_activate(move |_| {
            button.activate();
        });
    }

    let entries_for_response = (name_entry.clone(), server_entry.clone(), share_entry.clone(), protocol);
    let parent_for_response = parent.clone();
    dialog.connect_response(move |dialog, response| {
        if response != gtk::ResponseType::Accept {
            crate::source::net_trace("dialog_cancelled");
            dialog.close();
            return;
        }
        let (name_entry, server_entry, _share_entry, protocol) = &entries_for_response;
        // A discovered location is used exactly as gvfs supplied it (gvfs
        // resolves dnssd/wsdd names to the real host); otherwise build the
        // URI from the typed server/share.
        // The user pressed Connect without typing a server: fall back to the
        // first discovered SMB location (discovery is convenience; manual
        // entry wins when provided).
        let typed_server = server_entry.text().trim().to_string();
        let browse_root = if typed_server.is_empty() {
            match first_smb_target.borrow().clone() {
                Some(target) => target,
                None => {
                    show_error(
                        &parent_for_response,
                        "Could not connect to network share",
                        "A server address is required (for example 192.168.1.20 or nas.local).",
                    );
                    dialog.close();
                    return;
                }
            }
        } else {
            let scheme = if protocol.selected() == 1 { "nfs" } else { "smb" };
            let root = crate::source::build_network_uri(
                scheme,
                &typed_server,
                &_share_entry.text(),
            );
            if root.ends_with(format!("://{}/", scheme).as_str()) {
                show_error(
                    &parent_for_response,
                    "Could not connect to network share",
                    "A server address is required (for example 192.168.1.20 or nas.local).",
                );
                return;
            }
            root
        };
        if browse_root.is_empty() {
            dialog.close();
            return;
        }
        let name = name_entry.text().trim().to_string();
        crate::source::net_trace(format!("dialog_accepted root={browse_root} name={name}"));
        on_connect(name, browse_root);
        dialog.close();
    });

    if std::env::var_os("PICASA_TRACE").is_some() {
        let dialog_for_keys = dialog.clone();
        let key_logger = gtk::EventControllerKey::new();
        key_controller_log(&dialog_for_keys, key_logger);
    }
    crate::source::net_trace("dialog_opened");
    dialog.present();
}

/// Trace keys + focused widget inside the Add Network Share dialog.
fn key_controller_log(dialog: &gtk::Dialog, controller: gtk::EventControllerKey) {
    use gtk::prelude::*;
    let _dialog = dialog.clone();
    controller.connect_key_pressed(move |_, keyval, _, _| {
        crate::source::net_trace(format!("dialog key={keyval}"));
        glib::Propagation::Proceed
    });
    dialog.add_controller(controller);
}

/// Message stream for the in-app remote folder browser.
pub enum NetworkBrowseMessage {
    /// A subdirectory of the currently listed location.
    Item(String, String, bool),
    /// Enumeration finished: Ok(count) or the actual GIO error text.
    Done(Result<usize, String>),
}

/// In-app GIO folder browser for remote locations. The GTK file chooser cannot
/// reliably display remote `smb://`/`nfs://` initial folders (it falls back to
/// $HOME), so browsing happens here through `enumerate_children` - the stored
/// result stays a canonical `smb://`/`nfs://` URI, never a gvfs mount path.
pub fn show_network_folder_browser(
    parent: gtk::Widget,
    root_uri: String,
    on_selected: Rc<dyn Fn(String)>,
) {
    use gtk::prelude::*;

    crate::source::net_trace(format!("browse_root uri={root_uri}"));
    let window = parent
        .ancestor(gtk::Window::static_type())
        .and_downcast::<gtk::Window>()
        .or_else(|| parent.root().and_downcast::<gtk::Window>());

    let dialog = gtk::Dialog::new();
    dialog.set_title(Some("Choose the photo folder"));
    if let Some(window) = &window {
        dialog.set_transient_for(Some(window));
    }
    dialog.set_modal(true);
    dialog.set_default_size(640, 500);
    dialog.add_button("Cancel", gtk::ResponseType::Cancel);
    let select_button = dialog
        .add_button("Select This Folder", gtk::ResponseType::Accept)
        .downcast::<gtk::Button>()
        .expect("Select This Folder is a button");
    dialog.set_default_response(gtk::ResponseType::Accept);

    let content = dialog.content_area();
    content.set_spacing(8);
    content.set_margin_top(10);
    content.set_margin_bottom(6);
    content.set_margin_start(10);
    content.set_margin_end(10);

    // Location bar: Up button + current network URI.
    let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let up_button = gtk::Button::from_icon_name("go-up-symbolic");
    up_button.set_tooltip_text(Some("Up one folder"));
    let path_label = gtk::Label::new(Some(&root_uri));
    path_label.set_xalign(0.0);
    path_label.set_ellipsize(gtk::pango::EllipsizeMode::Start);
    path_label.set_hexpand(true);
    path_label.add_css_class("dim-label");
    top.append(&up_button);
    top.append(&path_label);
    content.append(&top);

    let status_label = gtk::Label::new(Some("Listing…"));
    status_label.set_xalign(0.0);
    status_label.set_visible(false);
    content.append(&status_label);

    let error_label = gtk::Label::new(None);
    error_label.set_xalign(0.0);
    error_label.set_wrap(true);
    error_label.set_visible(false);
    content.append(&error_label);

    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::Single);
    list.add_css_class("navigation-sidebar");
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroll.set_hexpand(true);
    scroll.set_vexpand(true);
    scroll.set_min_content_height(320);
    scroll.set_child(Some(&list));
    content.append(&scroll);

    // Shared state.
    let current_uri: Rc<RefCell<String>> = Rc::new(RefCell::new(root_uri.clone()));
    let generation: Rc<Cell<u64>> = Rc::new(Cell::new(0));
    let cancellable: Rc<RefCell<Option<gio::Cancellable>>> = Rc::new(RefCell::new(None));
    let load_slot: Rc<RefCell<Option<Rc<dyn Fn(String)>>>> = Rc::new(RefCell::new(None));
    // True while the current listing still contains mountable entries (shares
    // on a server root): selecting such a location would import nothing, so
    // the user must descend into a share first.
    let contains_mountables = Rc::new(Cell::new(false));

    // Load (list) one remote directory: enumerate on a worker thread, deliver
    // subdirectories to the main loop, show real GIO errors on failure.
    let load_uri: Rc<dyn Fn(String)> = {
        let list = list.clone();
        let status_for_load = status_label.clone();
        let error_for_load = error_label.clone();
        let path_for_load = path_label.clone();
        let current_for_load = current_uri.clone();
        let generation_for_load = generation.clone();
        let cancellable_for_load = cancellable.clone();
        let load_slot = load_slot.clone();
        let contains_mountables_for_load = contains_mountables.clone();
        let dialog_window_for_load = dialog.clone();
        Rc::new(move |uri: String| {
            generation_for_load.set(generation_for_load.get() + 1);
            let my_generation = generation_for_load.get();
            *current_for_load.borrow_mut() = uri.clone();
            path_for_load.set_text(&uri);
            error_for_load.set_visible(false);
            status_for_load.set_text("Listing…");
            status_for_load.set_visible(true);
            while let Some(child) = list.first_child() {
                list.remove(&child);
            }
            if let Some(old) = cancellable_for_load.borrow_mut().as_ref() {
                old.cancel();
            }
            let fresh_cancellable = gio::Cancellable::new();
            *cancellable_for_load.borrow_mut() = Some(fresh_cancellable.clone());

            let (sender, receiver) = std::sync::mpsc::channel::<NetworkBrowseMessage>();
            let uri_for_worker = uri.clone();
            std::thread::spawn(move || {
                let file = gio::File::for_uri(&uri_for_worker);
                let mut directories: Vec<(String, String, bool)> = Vec::new();
                let mut failure: Option<String> = None;
                if let Ok(enumerator) = file.enumerate_children(
                    "standard::name,standard::type",
                    gio::FileQueryInfoFlags::NONE,
                    Some(&fresh_cancellable),
                ) {
                    loop {
                        match enumerator.next_file(Some(&fresh_cancellable)) {
                            Ok(Some(info)) => {
                                // Shares on a server root are MOUNTABLE
                                // entries; subfolders are DIRECTORIES.
                                let kind = info.file_type();
                                let browsable = kind == gio::FileType::Directory
                                    || kind == gio::FileType::Mountable;
                                let mountable = kind == gio::FileType::Mountable;
                                if browsable {
                                    let name = info.name().to_string_lossy().into_owned();
                                    if name.starts_with('.') {
                                        continue;
                                    }
                                    let child = enumerator.child(&info);
                                    let child_uri = child.uri().to_string();
                                    directories.push((name, child_uri, mountable));
                                }
                            }
                            Ok(None) => break,
                            Err(error) => {
                                if error.kind() != Some(gio::IOErrorEnum::Cancelled) {
                                    failure = Some(format!("{error}"));
                                }
                                break;
                            }
                        }
                    }
                }
                match failure {
                    Some(message) => {
                        let _ = sender.send(NetworkBrowseMessage::Done(Err(message)));
                    }
                    None => {
                        directories.sort_by(|left, right| {
                            left.0.to_lowercase().cmp(&right.0.to_lowercase())
                        });
                        for (name, child_uri, mountable) in &directories {
                            if sender
                                .send(NetworkBrowseMessage::Item(
                                    name.clone(),
                                    child_uri.clone(),
                                    *mountable,
                                ))
                                .is_err()
                            {
                                break;
                            }
                        }
                        let _ = sender.send(NetworkBrowseMessage::Done(Ok(directories.len())));
                    }
                }
            });

            // Deliver results on the main loop; a stale poll (superseded by a
            // newer load) stops itself via the generation counter.
            let list = list.clone();
            let status = status_for_load.clone();
            let error = error_for_load.clone();
            let error_for_poll = error_for_load.clone();
            let load_slot = load_slot.clone();
            let dialog_window_for_rows = dialog_window_for_load.clone();
            let contains_mountables_for_poll = contains_mountables_for_load.clone();
            let uri_for_poll = uri.clone();
            let generation_for_poll = generation_for_load.clone();
            glib::timeout_add_local(std::time::Duration::from_millis(50), move || {
                if generation_for_poll.get() != my_generation {
                    return glib::ControlFlow::Break;
                }
                for _ in 0..25 {
                    match receiver.try_recv() {
                        Ok(NetworkBrowseMessage::Item(name, child_uri, mountable)) => {
                            // Own the values: the row's handlers outlive this
                            // FnMut poll iteration.
                            let child_uri = child_uri.clone();
                            if mountable {
                                contains_mountables_for_poll.set(true);
                            }
                            let row = gtk::ListBoxRow::new();
                            row.set_focusable(true);
                            let box_ = gtk::Box::new(gtk::Orientation::Horizontal, 6);
                            box_.set_margin_top(2);
                            box_.set_margin_bottom(2);
                            let icon = gtk::Image::from_icon_name("folder-symbolic");
                            icon.set_pixel_size(16);
                            box_.append(&icon);
                            let label = gtk::Label::new(Some(&name));
                            label.set_xalign(0.0);
                            label.set_ellipsize(gtk::pango::EllipsizeMode::End);
                            label.set_hexpand(true);
                            box_.append(&label);
                            row.set_child(Some(&box_));
                            // Per-row clones: the row outlives this poll
                            // iteration, so every capture is a fresh local.
                            let load_for_activate = load_slot.clone();
                            let dialog_window_for_activate = dialog_window_for_rows.clone();
                            let error_for_activate = error_for_poll.clone();
                            let uri_for_activate = child_uri.clone();
                            row.connect_activate(move |_| {
                                crate::source::net_trace(format!(
                                    "browse_enter uri={uri_for_activate}"
                                ));
                                // Shares are mountable entries: mount through
                                // gvfs first (auth dialog if needed), then list
                                // the folder contents.
                                let parent_window =
                                    dialog_window_for_activate.clone().upcast::<gtk::Window>();
                                let error = error_for_activate.clone();
                                let load_for_mount = load_for_activate.clone();
                                let uri_for_mount = uri_for_activate.clone();
                                let uri_for_load = uri_for_activate.clone();
                                let uri_for_error = uri_for_activate.clone();
                                let load_slot_for_mount = load_for_mount.clone();
                                crate::source::mount_share_async(
                                    &uri_for_mount,
                                    Some(&parent_window),
                                    move |result| match result {
                                        Ok(()) => {
                                            if let Some(load) =
                                                load_slot_for_mount.borrow().as_ref()
                                            {
                                                load(uri_for_load.clone());
                                            }
                                        }
                                        Err(message) => {
                                            crate::source::net_trace(format!(
                                                "browse_failed uri={uri_for_error} error={message}"
                                            ));
                                            error.set_text(&message);
                                            error.set_visible(true);
                                        }
                                    },
                                );
                            });
                            list.append(&row);
                        }
                        Ok(NetworkBrowseMessage::Done(Ok(count))) => {
                            crate::source::net_trace(format!(
                                "browse_list uri={uri_for_poll} count={count}"
                            ));
                            if count == 0 {
                                status.set_text("No subfolders");
                                status.set_visible(true);
                            } else {
                                status.set_visible(false);
                                // Keyboard: focus the first row so arrows +
                                // Enter navigate immediately after loading.
                                if let Some(first) = list.first_child() {
                                    list.select_row(first.downcast_ref::<gtk::ListBoxRow>());
                                    first.grab_focus();
                                }
                            }
                            return glib::ControlFlow::Break;
                        }
                        Ok(NetworkBrowseMessage::Done(Err(message))) => {
                            crate::source::net_trace(format!(
                                "browse_failed uri={uri_for_poll} error={message}"
                            ));
                            error.set_text(&message);
                            error.set_visible(true);
                            status.set_visible(false);
                            return glib::ControlFlow::Break;
                        }
                        Err(_) => return glib::ControlFlow::Break,
                    }
                }
                glib::ControlFlow::Continue
            });
        })
    };
    *load_slot.borrow_mut() = Some(load_uri.clone());

    // Up navigation.
    {
        let load = load_uri.clone();
        let current = current_uri.clone();
        up_button.connect_clicked(move |_| {
            let uri = current.borrow().clone();
            if let Some(parent_file) = gio::File::for_uri(&uri).parent() {
                let parent_uri = parent_file.uri().to_string();
                crate::source::net_trace(format!("browse_up uri={parent_uri}"));
                load(parent_uri);
            }
        });
    }

    // Select This Folder: registers the folder being BROWSED.
    {
        let current = current_uri.clone();
        let on_selected = on_selected.clone();
        let dialog_for_select = dialog.clone();
        let contains_mountables_for_select = contains_mountables.clone();
        let status_for_select = status_label.clone();
        select_button.connect_clicked(move |_| {
            let uri = current.borrow().clone();
            if contains_mountables_for_select.get() {
                // A server root only lists shares (mountable entries) - there
                // is nothing to import here: descend into a share and choose
                // the folder that contains your photos.
                status_for_select.set_text(
                    "Enter a share above and choose the folder that contains your photos.",
                );
                status_for_select.set_visible(true);
                return;
            }
            crate::source::net_trace(format!("browse_selected uri={uri}"));
            on_selected(uri.clone());
            dialog_for_select.close();
        });
    }

    // Cancel pending enumeration when the dialog goes away.
    {
        let cancellable = cancellable.clone();
        dialog.connect_close_request(move |_| {
            if let Some(cancellable) = cancellable.borrow().as_ref() {
                cancellable.cancel();
            }
            glib::Propagation::Proceed
        });
    }

    load_uri(root_uri);
    dialog.present();
}

fn status_label_set(label: &gtk::Label, text: &str) {
    label.set_text(text);
    label.set_visible(true);
}
