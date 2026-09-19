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

fn percent_encode(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    for byte in segment.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(*byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
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

/// Phase 1 "Add Network Share" dialog: a display name, a protocol, a server
/// (host or host:port) and a share/export name are combined into a canonical
/// `smb://server/share` or `nfs://server/export` URI and handed to `on_connect`.
/// Nothing here mounts, sudo's or touches fstab: enumeration and reads go
/// through PIC's GIO source layer.
///
/// Discovery runs asynchronously and must populate the list on the FIRST open.
/// A visible spinner covers the scan, empty/error states explain a bare
/// result, and a `closed` flag stops stale poll callbacks from touching a
/// dialog that was dismissed.
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
    // Generous, resizable initial size: the discovered-locations list owns the
    // free vertical space (below) and expands as the window grows.
    dialog.set_default_size(560, 600);
    dialog.set_resizable(true);
    dialog.add_button("Cancel", gtk::ResponseType::Cancel);
    let connect_button = dialog.add_button("Connect", gtk::ResponseType::Accept);
    dialog.set_default_response(gtk::ResponseType::Accept);

    // Stops the discovery poll from mutating a closed/destroyed dialog: set on
    // every close path (Cancel, window X, successful Connect).
    let dialog_closed: Rc<Cell<bool>> = Rc::new(Cell::new(false));
    {
        let closed = dialog_closed.clone();
        dialog.connect_close_request(move |_| {
            crate::source::net_trace("discovery_ui_closed_flag");
            closed.set(true);
            glib::Propagation::Proceed
        });
    }

    let content = dialog.content_area();
    content.set_spacing(10);
    content.set_margin_top(14);
    content.set_margin_bottom(6);
    content.set_margin_start(14);
    content.set_margin_end(14);

    // Protocol: SMB/CIFS and NFS share the same gvfs pipeline; CIFS is handled
    // by the SMB backend, so there are exactly two choices here. NFS differs
    // only in how the path is read (an export path, not a share name).
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

    let grid = gtk::Grid::new();
    grid.set_row_spacing(8);
    grid.set_column_spacing(10);
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
    // background, fills the list as results arrive, and an empty result never
    // blocks manual entry.
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
    // The list owns the free vertical space: it expands with the window and
    // scrolls on its own instead of squeezing into a fixed 110px strip.
    discovered_scroll.set_vexpand(true);
    discovered_scroll.set_min_content_height(180);
    discovered_scroll.set_child(Some(&discovered_list));
    discovered_scroll.set_visible(false);
    let discovered_header = gtk::Label::new(Some("Discovered locations"));
    discovered_header.set_xalign(0.0);
    discovered_header.add_css_class("dim-label");
    discovered_header.set_visible(false);
    content.append(&discovered_header);
    content.append(&discovered_scroll);

    // Loading indicator while discovery runs, then empty/error notes.
    let discovery_status_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    discovery_status_box.set_hexpand(true);
    let discovery_spinner = gtk::Spinner::new();
    discovery_spinner.set_hexpand(false);
    discovery_spinner.start();
    let discovery_status = gtk::Label::new(Some("Discovering network locations…"));
    discovery_status.set_xalign(0.0);
    discovery_status.set_hexpand(true);
    discovery_status.add_css_class("dim-label");
    discovery_status_box.append(&discovery_spinner);
    discovery_status_box.append(&discovery_status);
    content.append(&discovery_status_box);

    let discovery_note = gtk::Label::new(None);
    discovery_note.set_xalign(0.0);
    discovery_note.set_wrap(true);
    discovery_note.add_css_class("dim-label");
    discovery_note.set_visible(false);
    content.append(&discovery_note);

    let hint = gtk::Label::new(Some(
        "Connects through the system's network services (guest or saved credentials). Enter a Share to jump straight into it, or leave it empty to browse all shares - then pick the folder with your photos. For NFS, enter the export path (for example exports/Work or mnt/4TBP) - the server's real exports appear in the list once a server is set.",
    ));
    hint.set_xalign(0.0);
    hint.set_wrap(true);
    hint.add_css_class("dim-label");
    content.append(&hint);

    // Discovery poll: drains the worker channel on the GTK main thread, then
    // stops. A closed dialog halts the poll on its next tick, the spinner is
    // replaced by the list or an explicit empty/error state.
    crate::source::net_trace("discovery_ui_started");
    // Seed the list from the warm discovery cache (startup prefetch or an
    // earlier dialog run) so the FIRST open already shows the shares, then
    // merge the live discovery events into the same stream. The poll below
    // drops duplicate URIs, so cache hits found again live are ignored.
    let (sender, receiver) =
        std::sync::mpsc::channel::<crate::source::NetworkDiscoveryEvent>();
    // Clone for the NFS export lookup: gvfs cannot list NFS exports, so
    // `showmount -e` results are fed into this same channel and rendered
    // by the poll below, even after the gvfs discovery finished.
    let nfs_sender_slot = Rc::new(RefCell::new(Some(sender.clone())));
    let cached = crate::source::cached_network_locations();
    crate::source::net_trace(format!(
        "discovery_cache_seeded count={}",
        cached.len()
    ));
    for (name, uri) in cached {
        let _ = sender.send(crate::source::NetworkDiscoveryEvent::Item(name, uri));
    }
    let sender_for_live = sender;
    std::thread::spawn(move || {
        let live = crate::source::discover_network_locations();
        while let Ok(event) = live.recv() {
            if sender_for_live.send(event).is_err() {
                break;
            }
        }
    });
    {
        let list = discovered_list.clone();
        let header = discovered_header.clone();
        let scroll = discovered_scroll.clone();
        let status_box = discovery_status_box.clone();
        let spinner = discovery_spinner.clone();
        let status = discovery_status.clone();
        let note = discovery_note.clone();
        let closed = dialog_closed.clone();
        let server_for_rows = server_entry.clone();
        let share_for_rows = share_entry.clone();
        let protocol_for_rows = protocol.clone();
        let discovered_for_rows = discovered_uri.clone();
        let updating_for_rows = updating_fields.clone();
        let list_focus_set = Rc::new(Cell::new(false));
        let first_smb_target = first_smb_target.clone();
        let name_for_rows = name_entry.clone();
        let on_connect_for_rows = on_connect.clone();
        let shown_uris = Rc::new(RefCell::new(std::collections::HashSet::new()));
        let mut items_added = 0u32;
        glib::timeout_add_local(std::time::Duration::from_millis(50), move || {
            if closed.get() {
                crate::source::net_trace("discovery_ui_stopped_closed");
                return glib::ControlFlow::Break;
            }
            for _ in 0..25 {
                match receiver.try_recv() {
                    Ok(crate::source::NetworkDiscoveryEvent::Item(name, uri)) => {
                        // Cache-seeded rows and live results can overlap; the
                        // second occurrence of a URI is skipped.
                        if !shown_uris.borrow_mut().insert(uri.clone()) {
                            continue;
                        }
                        items_added += 1;
                        // A late result (e.g. an NFS export found after the
                        // zero-state was shown) replaces the empty note.
                        note.set_visible(false);
                        crate::source::net_trace(format!(
                            "discovery_ui_item n={items_added} uri={uri}"
                        ));
                        // Persist for the next dialog open.
                        crate::source::remember_discovered_location(
                            name.clone(),
                            uri.clone(),
                        );
                        // Remember the first SMB target: pressing Connect without
                        // typing connects to it (discovery convenience).
                        if first_smb_target.borrow().is_none() && uri.starts_with("smb://") {
                            *first_smb_target.borrow_mut() = Some(uri.clone());
                        }
                        // First result: switch from the spinner to the list.
                        if !header.is_visible() {
                            header.set_visible(true);
                            scroll.set_visible(true);
                            status_box.set_visible(false);
                            spinner.stop();
                        }
                        let row = gtk::ListBoxRow::new();
                        row.set_focusable(true);
                        let box_ = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                        box_.set_margin_top(6);
                        box_.set_margin_bottom(6);
                        let icon = gtk::Image::from_icon_name("folder-remote-symbolic");
                        icon.set_pixel_size(18);
                        box_.append(&icon);
                        // Name on one line, the gvfs URI underneath so the user
                        // can tell SMB from NFS and see the exact host/path at
                        // a glance instead of hovering for a tooltip.
                        let text_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
                        text_box.set_hexpand(true);
                        let label = gtk::Label::new(Some(&name));
                        label.set_xalign(0.0);
                        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
                        text_box.append(&label);
                        let uri_label = gtk::Label::new(Some(&uri));
                        uri_label.set_xalign(0.0);
                        uri_label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
                        uri_label.add_css_class("dim-label");
                        text_box.append(&uri_label);
                        box_.append(&text_box);
                        row.set_child(Some(&box_));
                        row.set_tooltip_text(Some(&uri));
                        // Use the URI gvfs supplied (NFS already normalized by
                        // the discovery worker, so no :2049 service port gets
                        // stored). Discovered entries look like
                        // network:///dnssd-server-DIETPI._smb._tcp - extract
                        // the server name and service type so the entries get
                        // clean values (server = name, protocol = service type).
                        let server_for_row = server_for_rows.clone();
                        let protocol_for_row = protocol_for_rows.clone();
                        let share_for_row = share_for_rows.clone();
                        let discovered_for_row = discovered_for_rows.clone();
                        let updating = updating_for_rows.clone();
                        let decoded = glib::uri_unescape_string(uri.as_str(), None::<&str>)
                            .unwrap_or_else(|| uri.clone().into());
                        let click = gtk::GestureClick::new();
                        let click_name = name.clone();
                        let click_uri = uri.clone();
                        click.connect_pressed(move |gesture, _, _, _| {
                            // Trace INSIDE the handler: the previous placement
                            // logged at row-build time, so the trace could never
                            // show whether a row was actually clicked.
                            crate::source::net_trace(format!(
                                "discovery_row_clicked name={click_name} uri={click_uri}"
                            ));
                            // NFS topics are service endpoints
                            // (nfs://host:2049/export); Connect must mount the
                            // export root nfs://host/export, never the RPC port.
                            let is_nfs = decoded.starts_with("nfs://")
                                || decoded.contains("._nfs.");
                            let effective = if is_nfs {
                                crate::source::normalize_nfs_uri(&decoded)
                            } else {
                                decoded.to_string()
                            };
                            // Remember the gvfs URI; Connect will use it verbatim.
                            *discovered_for_row.borrow_mut() = Some(effective.clone());
                            updating.set(true);
                            // Protocol from the service type: NFS announcements
                            // get the nfs scheme; everything else rides SMB.
                            if is_nfs {
                                protocol_for_row.set_selected(1);
                            } else {
                                protocol_for_row.set_selected(0);
                            }
                            // Fill the fields with the PARSED target (host + path)
                            // so they stay clean and resolvable even if the
                            // stored URI is later cleared. NFS keeps the WHOLE
                            // URI path as the export path; SMB keeps only the
                            // first share segment.
                            let host = crate::source::network_uri_host(&effective);
                            server_for_row.set_text(&host);
                            let path = effective
                                .split("://")
                                .nth(1)
                                .and_then(|rest| rest.split_once('/').map(|(_, p)| p));
                            if let Some(path) = path {
                                let path = path.trim_end_matches('/');
                                if is_nfs {
                                    if !path.is_empty() {
                                        share_for_row.set_text(path);
                                    }
                                } else {
                                    let first = path.split('/').next().unwrap_or("");
                                    if !first.is_empty() {
                                        share_for_row.set_text(first);
                                    }
                                }
                            }
                            updating.set(false);
                            gesture.set_state(gtk::EventSequenceState::Claimed);
                        });
                        // Capture phase + attached to the row's child box: the
                        // enclosing GtkListBox installs its own press gesture on
                        // the row in the bubble phase and wins otherwise, so the
                        // fields were never filled and Connect fell back to the
                        // first discovered SMB target.
                        click.set_propagation_phase(gtk::PropagationPhase::Capture);
                        box_.add_controller(click);
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
                    }
                    Ok(crate::source::NetworkDiscoveryEvent::Done(count)) => {
                        crate::source::net_trace(format!(
                            "discovery_ui_done count={count} shown={items_added}"
                        ));
                        if items_added == 0 {
                            // Stop the spinner and leave an explicit,
                            // actionable empty state instead of a blank dialog.
                            spinner.stop();
                            status.set_text("No network locations discovered.");
                            note.set_text(
                                "Enter the server address manually below, or check that the NAS \
                                 is online and reachable on this network.",
                            );
                            note.set_visible(true);
                        } else {
                            status_box.set_visible(false);
                        }
                        // Keep polling: NFS export lookups (showmount) feed
                        // rows into this channel after the gvfs discovery has
                        // finished. The dialog-close check above ends the poll.
                        return glib::ControlFlow::Continue;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => continue,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        crate::source::net_trace("discovery_ui_disconnected");
                        spinner.stop();
                        status.set_text("Discovery could not start.");
                        note.set_text("Enter the server address manually below.");
                        note.set_visible(true);
                        return glib::ControlFlow::Break;
                    }
                }
            }
            glib::ControlFlow::Continue
        });
    }

    // NFS export lookup: gvfs cannot enumerate NFS exports (NFS has no
    // share-listing protocol), so query the server's mountd with
    // `showmount -e` on a worker thread and feed the real exports into the
    // discovered list. Debounced 400 ms behind protocol/server changes; the
    // results are nfs://host/export rows that Connect can use verbatim.
    let nfs_lookup_generation: Rc<Cell<u64>> = Rc::new(Cell::new(0));
    let schedule_nfs_lookup: Rc<dyn Fn()> = {
        let protocol = protocol.clone();
        let server_entry = server_entry.clone();
        let sender_slot = nfs_sender_slot.clone();
        let generation = nfs_lookup_generation.clone();
        Rc::new(move || {
            if protocol.selected() != 1 {
                return;
            }
            let host = server_entry.text().trim().to_string();
            if host.is_empty() {
                return;
            }
            generation.set(generation.get() + 1);
            let my_generation = generation.get();
            let sender_slot = sender_slot.clone();
            let host_for_tick = host.clone();
            let generation_for_tick = generation.clone();
            glib::timeout_add_local(std::time::Duration::from_millis(400), move || {
                if generation_for_tick.get() != my_generation {
                    return glib::ControlFlow::Break;
                }
                crate::source::net_trace(format!("nfs_lookup_started host={host_for_tick}"));
                // Clone the channel sender on the main thread: the Sender is
                // Send, the Rc/RefCell slot itself is not.
                let sender_for_exports = sender_slot.borrow().clone();
                let host_for_worker = host_for_tick.clone();
                std::thread::spawn(move || {
                    let exports = crate::source::query_nfs_exports(&host_for_worker);
                    let Some(sender) = sender_for_exports else {
                        return;
                    };
                    for export in exports {
                        let _ = sender.send(crate::source::NetworkDiscoveryEvent::Item(
                            export.clone(),
                            format!("nfs://{host_for_worker}/{export}"),
                        ));
                    }
                });
                glib::ControlFlow::Break
            });
        })
    };
    {
        let schedule = schedule_nfs_lookup.clone();
        protocol.connect_selected_item_notify(move |_| {
            schedule();
        });
    }
    {
        let schedule = schedule_nfs_lookup.clone();
        server_entry.connect_changed(move |_| {
            schedule();
        });
    }

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
        // Response fires for Cancel, the window X (DeleteEvent) and - after
        // `dialog.close()` - again. Only a real Cancel is a user cancellation;
        // DeleteEvent is the normal teardown following Accept/close.
        match response {
            gtk::ResponseType::Accept => {}
            gtk::ResponseType::Cancel => {
                crate::source::net_trace("dialog_cancelled");
                dialog.close();
                return;
            }
            _ => {
                crate::source::net_trace("dialog_closed");
                return;
            }
        }
        let (name_entry, server_entry, _share_entry, protocol) = &entries_for_response;
        // A discovered location is used exactly as gvfs supplied it (gvfs
        // resolves dnssd/wsdd names to the real host); otherwise build the
        // URI from the typed server/share.
        // The user pressed Connect without typing a server: fall back to the
        // first discovered SMB location (discovery is convenience; manual
        // entry wins when provided).
        let typed_server = server_entry.text().trim().to_string();
        crate::source::net_trace(format!(
            "dialog_connect_state server='{typed_server}' discovered={} protocol={}",
            discovered_uri.borrow().is_some(),
            protocol.selected()
        ));
        let browse_root = if typed_server.is_empty() {
            match first_smb_target.borrow().clone() {
                Some(target) => {
                    crate::source::net_trace(format!(
                        "dialog_connect_source=fallback_first_smb uri={target}"
                    ));
                    target
                }
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
            // Preserve the scheme the discovery dropdown selected. If the
            // user picked an NFS-discovered location, its URI already has
            // nfs://; do not silently rewrite it to smb://.
            let discovered = discovered_uri.borrow().clone();
            if let Some(uri) = discovered {
                // Discovered URIs are used verbatim: they already carry the
                // correct scheme, host and path that gvfs resolved, and NFS
                // entries were normalized (no service port) at row-click time.
                crate::source::net_trace(format!(
                    "dialog_connect_source=discovered uri={uri}"
                ));
                uri
            } else {
                let scheme = if protocol.selected() == 1 { "nfs" } else { "smb" };
                let root = crate::source::build_network_uri(
                    scheme,
                    &typed_server,
                    &_share_entry.text(),
                );
                crate::source::net_trace(format!(
                    "dialog_connect_source=typed uri={root}"
                ));
                if root.ends_with(format!("://{}/", scheme).as_str()) {
                    show_error(
                        &parent_for_response,
                        "Could not connect to network share",
                        "A server address is required (for example 192.168.1.20 or nas.local).",
                    );
                    return;
                }
                root
            }
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
    // Large, resizable: the folder list below expands with the window.
    dialog.set_default_size(700, 580);
    dialog.set_resizable(true);
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

    // Location bar: Back + Up buttons + current network URI.
    let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let back_button = gtk::Button::from_icon_name("go-previous-symbolic");
    back_button.set_tooltip_text(Some("Back"));
    back_button.set_sensitive(false);
    let up_button = gtk::Button::from_icon_name("go-up-symbolic");
    up_button.set_tooltip_text(Some("Up one folder"));
    let path_label = gtk::Label::new(Some(&root_uri));
    path_label.set_xalign(0.0);
    path_label.set_ellipsize(gtk::pango::EllipsizeMode::Start);
    path_label.set_hexpand(true);
    path_label.add_css_class("dim-label");
    top.append(&back_button);
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
    scroll.set_min_content_height(380);
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
    // The visited-URI stack for the Back button. The most recent entry is the
    // location shown before the current one; Back pops it and loads it.
    let history: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    // True while a Back-triggered load is running so load_uri does not re-push
    // the URI it is navigating back to.
    let back_active: Rc<Cell<bool>> = Rc::new(Cell::new(false));
    // Set when the dialog is being dismissed (Cancel, X or DeleteEvent): stops
    // in-flight mounts/polls from touching a closed dialog.
    let browse_closed: Rc<Cell<bool>> = Rc::new(Cell::new(false));

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
        let history_for_load = history.clone();
        let back_active_for_load = back_active.clone();
        let back_button_for_load = back_button.clone();
        let browse_closed_for_load = browse_closed.clone();
        Rc::new(move |uri: String| {
            generation_for_load.set(generation_for_load.get() + 1);
            let my_generation = generation_for_load.get();
            // Record the location being left for the Back stack. A Back load
            // already consumes the top entry, so it must not push again.
            let from_back = back_active_for_load.replace(false);
            if !from_back {
                let previous = current_for_load.borrow().clone();
                if previous != uri {
                    history_for_load.borrow_mut().push(previous);
                }
            }
            back_button_for_load.set_sensitive(!history_for_load.borrow().is_empty());
            *current_for_load.borrow_mut() = uri.clone();
            path_for_load.set_text(&uri);
            error_for_load.set_visible(false);
            status_for_load.set_text("Listing…");
            status_for_load.set_visible(true);
            contains_mountables_for_load.set(false);
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
                let enumerate_started = std::time::Instant::now();
                let file = gio::File::for_uri(&uri_for_worker);
                let mut directories: Vec<(String, String, bool)> = Vec::new();
                let mut failure: Option<String> = None;
                if let Ok(enumerator) = file.enumerate_children(
                    "standard::name,standard::type,standard::is-hidden",
                    gio::FileQueryInfoFlags::NONE,
                    Some(&fresh_cancellable),
                ) {
                    loop {
                        match enumerator.next_file(Some(&fresh_cancellable)) {
                            Ok(Some(info)) => {
                                // Only directories and mountable shares are
                                // navigable here. Files are ignored.
                                let kind = info.file_type();
                                let browsable = kind == gio::FileType::Directory
                                    || kind == gio::FileType::Mountable;
                                if !browsable {
                                    continue;
                                }
                                // gvfs marks macOS AppleDouble artifacts
                                // (._Foo), .DS_Store and similar metadata as
                                // hidden on SMB/NFS. They look like folders
                                // in the raw listing but are not real
                                // directories - enumerating them returns 0.
                                // gvfs does not always populate
                                // standard::is-hidden (e.g. on SMB), so guard
                                // the call to avoid a GLib-GIO critical.
                                if info.has_attribute("standard::is-hidden")
                                    && info.is_hidden()
                                {
                                    continue;
                                }
                                let name = info.name().to_string_lossy().into_owned();
                                let trimmed = name.trim();
                                // Belt-and-braces: even when gvfs does not
                                // mark them hidden, skip dotfiles and the
                                // AppleDouble prefix.
                                if trimmed.is_empty()
                                    || trimmed.starts_with('.')
                                    || trimmed == ".DS_Store"
                                {
                                    continue;
                                }
                                let mountable = kind == gio::FileType::Mountable;
                                let child_uri = if mountable {
                                    // gvfs mangles child construction at a
                                    // server root into AppleDouble paths
                                    // (smb://host/._share), so build the
                                    // share segment manually instead.
                                    format!(
                                        "{}/{}",
                                        uri_for_worker.trim_end_matches('/'),
                                        percent_encode(&trimmed)
                                    )
                                } else {
                                    enumerator.child(&info).uri().to_string()
                                };
                                directories.push((name, child_uri, mountable));
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
                        crate::source::net_trace(format!(
                            "browse_enumerate uri={uri_for_worker} ms={:.1} failed",
                            enumerate_started.elapsed().as_secs_f64() * 1000.0
                        ));
                        let _ = sender.send(NetworkBrowseMessage::Done(Err(message)));
                    }
                    None => {
                        crate::source::net_trace(format!(
                            "browse_enumerate uri={uri_for_worker} count={} ms={:.1}",
                            directories.len(),
                            enumerate_started.elapsed().as_secs_f64() * 1000.0
                        ));
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
            let browse_closed_for_rows = browse_closed_for_load.clone();
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
                            box_.set_margin_top(6);
                            box_.set_margin_bottom(6);
                            // Shares at a server root are mountable entries;
                            // give them the network icon so they are visually
                            // distinct from ordinary subfolders.
                            let icon_name = if mountable {
                                "folder-remote-symbolic"
                            } else {
                                "folder-symbolic"
                            };
                            let icon = gtk::Image::from_icon_name(icon_name);
                            icon.set_pixel_size(18);
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
                            let row_mountable = mountable;
                            let row_closed = browse_closed_for_rows.clone();

                            // Mouse path. Capture phase + attached to the row's
                            // child box: in GTK4 the click inside a GtkListBoxRow
                            // targets the child widget, and the enclosing
                            // GtkListBox installs its own gesture on the row in
                            // the bubble phase. Capturing on the box guarantees
                            // this handler runs first and can claim the sequence.
                            let click = gtk::GestureClick::new();
                            click.set_propagation_phase(gtk::PropagationPhase::Capture);
                            click.connect_pressed(move |gesture, _, _, _| {
                                crate::source::net_trace(format!(
                                    "browse_enter uri={uri_for_activate}"
                                ));
                                if row_closed.get() {
                                    gesture.set_state(gtk::EventSequenceState::Claimed);
                                    return;
                                }
                                let parent_window =
                                    dialog_window_for_activate.clone().upcast::<gtk::Window>();
                                let error = error_for_activate.clone();
                                let load_for_mount = load_for_activate.clone();
                                let uri_for_mount = uri_for_activate.clone();
                                let uri_for_load = uri_for_activate.clone();
                                let uri_for_error = uri_for_activate.clone();
                                let load_slot_for_mount = load_for_mount.clone();
                                let closed_for_mount = row_closed.clone();
                                // An NFS export root is mountable (it is a
                                // GVolume), but a subdirectory of an export is
                                // not: mounting it fails with "Location is not
                                // mountable". Such a folder is listed through
                                // the already-mounted export, so enumerate it
                                // directly instead of mounting first. SMB keeps
                                // the existing mount-then-list path untouched.
                                let nfs_subdirectory =
                                    uri_for_mount.starts_with("nfs://") && !row_mountable;
                                if nfs_subdirectory {
                                    if let Some(load) =
                                        load_slot_for_mount.borrow().as_ref()
                                    {
                                        load(uri_for_load.clone());
                                    }
                                    gesture.set_state(gtk::EventSequenceState::Claimed);
                                    return;
                                }
                                crate::source::mount_share_async(
                                    &uri_for_mount,
                                    Some(&parent_window),
                                    move |result| {
                                        if closed_for_mount.get() {
                                            return;
                                        }
                                        match result {
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
                                        }
                                    },
                                );
                                gesture.set_state(gtk::EventSequenceState::Claimed);
                            });
                            box_.add_controller(click);
                            // Keep the row itself focusable so keyboard Enter /
                            // Space still activates it.
                            row.set_focusable(true);

                            // Keyboard path: same mount-then-list as the mouse.
                            let load_for_key = load_slot.clone();
                            let dialog_window_for_key = dialog_window_for_rows.clone();
                            let error_for_key = error_for_poll.clone();
                            let uri_for_key = child_uri.clone();
                            let key_mountable = mountable;
                            let key_closed = browse_closed_for_rows.clone();
                            row.connect_activate(move |_| {
                                crate::source::net_trace(format!(
                                    "browse_enter_key uri={uri_for_key}"
                                ));
                                if key_closed.get() {
                                    return;
                                }
                                let parent_window =
                                    dialog_window_for_key.clone().upcast::<gtk::Window>();
                                let error = error_for_key.clone();
                                let load_for_mount = load_for_key.clone();
                                let uri_for_mount = uri_for_key.clone();
                                let uri_for_load = uri_for_key.clone();
                                let load_slot_for_mount = load_for_mount.clone();
                                let closed_for_mount = key_closed.clone();
                                // Same NFS rule as the mouse path: only export
                                // roots are mountable; subdirectories enumerate
                                // directly through the mounted export.
                                let nfs_subdirectory =
                                    uri_for_mount.starts_with("nfs://") && !key_mountable;
                                if nfs_subdirectory {
                                    if let Some(load) =
                                        load_slot_for_mount.borrow().as_ref()
                                    {
                                        load(uri_for_load.clone());
                                    }
                                    return;
                                }
                                crate::source::mount_share_async(
                                    &uri_for_mount,
                                    Some(&parent_window),
                                    move |result| {
                                        if closed_for_mount.get() {
                                            return;
                                        }
                                        match result {
                                            Ok(()) => {
                                                if let Some(load) =
                                                    load_slot_for_mount.borrow().as_ref()
                                                {
                                                    load(uri_for_load.clone());
                                                }
                                            }
                                            Err(message) => {
                                                error.set_text(&message);
                                                error.set_visible(true);
                                            }
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
                        // A momentarily empty channel is normal: the worker
                        // enumerates on a thread and a cold gvfs session can
                        // take hundreds of ms, far past the first 50 ms tick.
                        // Only a disconnected sender (worker gone without a
                        // Done) ends the poll. Breaking on Empty killed the
                        // poll on its first tick whenever enumeration was
                        // slower than one tick, leaving the list empty until
                        // the dialog was closed and re-opened.
                        Err(std::sync::mpsc::TryRecvError::Empty) => continue,
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                            return glib::ControlFlow::Break;
                        }
                    }
                }
                glib::ControlFlow::Continue
            });
        })
    };
    *load_slot.borrow_mut() = Some(load_uri.clone());

    // Back navigation: pop the most recent visited location.
    {
        let load = load_uri.clone();
        let history = history.clone();
        let back_active = back_active.clone();
        let back_button = back_button.clone();
        back_button.connect_clicked(move |_| {
            let target = history.borrow_mut().pop();
            if let Some(target) = target {
                back_active.set(true);
                crate::source::net_trace(format!("browse_back uri={target}"));
                load(target);
            }
        });
    }

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

    // Cancel pending enumeration and invalidate in-flight polls when the dialog
    // goes away. The X button, Cancel and a completed Select all end here; only
    // a Cancel response is traced as a user cancellation. The generation bump
    // makes any still-polling load_uri callback stop on its next tick.
    {
        let close_closed = browse_closed.clone();
        let close_cancellable = cancellable.clone();
        let close_generation = generation.clone();
        dialog.connect_close_request(move |_| {
            close_closed.set(true);
            close_generation.set(close_generation.get() + 1);
            if let Some(cancellable) = close_cancellable.borrow().as_ref() {
                cancellable.cancel();
            }
            glib::Propagation::Proceed
        });
        let response_closed = browse_closed.clone();
        let response_cancellable = cancellable.clone();
        let response_generation = generation.clone();
        dialog.connect_response(move |dialog, response| {
            match response {
                gtk::ResponseType::Cancel => {
                    crate::source::net_trace("browse_cancelled");
                    response_closed.set(true);
                    response_generation.set(response_generation.get() + 1);
                    if let Some(cancellable) = response_cancellable.borrow().as_ref() {
                        cancellable.cancel();
                    }
                    dialog.close();
                }
                gtk::ResponseType::DeleteEvent => {
                    crate::source::net_trace("browse_closed");
                    response_closed.set(true);
                    response_generation.set(response_generation.get() + 1);
                    if let Some(cancellable) = response_cancellable.borrow().as_ref() {
                        cancellable.cancel();
                    }
                }
                _ => {}
            }
        });
    }

    load_uri(root_uri);
    dialog.present();
}