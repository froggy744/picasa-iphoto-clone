//! Reusable network-share browser extracted from the working "Big Pickle" PIC source.
//!
//! Dependencies: gtk4, gio, glib.
//!
//! Key behavior:
//! - Never use GtkFileDialog for remote URIs.
//! - Keep canonical smb:// / nfs:// URIs.
//! - Enumerate through GIO.
//! - For FileType::Mountable entries at an SMB server root, manually build the
//!   child URI instead of using enumerator.child(), because GVfs can produce
//!   bogus AppleDouble-like paths there.
//! - Capture mouse clicks on the row child box before GtkListBox consumes them.
//! - Mount/check the URI before loading it. Already-mounted subfolders return
//!   immediately through find_enclosing_mount + query_exists.

use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;

use gio::prelude::*;
use gtk::prelude::*;
use gtk4 as gtk;

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

fn query_exists(reference: &str, directory: bool) -> bool {
    if !reference.contains("://") {
        return if directory {
            Path::new(reference).is_dir()
        } else {
            Path::new(reference).is_file()
        };
    }
    uri_query_exists(&file(reference))
}

/// Longest a remote existence probe may block. gvfs SMB/NFS lookups can stall
/// for a very long time when a host is unreachable; without a bound, callers
/// on the GTK thread (sidebar population, startup) would freeze for minutes.
/// The probe result is cached per refresh generation, so this bounds the cost
/// of the rare uncached probe instead of recurring per row.
const URI_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(4);

fn uri_query_exists(uri_file: &gio::File) -> bool {
    let cancellable = gio::Cancellable::new();
    let timer = cancellable.clone();
    std::thread::spawn(move || {
        std::thread::sleep(URI_PROBE_TIMEOUT);
        timer.cancel();
    });
    let exists = uri_file.query_exists(Some(&cancellable));
    // Release the timer thread early on a fast answer.
    cancellable.cancel();
    exists
}


/// Convert a stored path/URI into a GIO file.
pub fn file(reference: &str) -> gio::File {
    if reference.contains("://") {
        gio::File::for_uri(reference)
    } else {
        gio::File::for_path(reference)
    }
}

pub fn net_trace(message: impl std::fmt::Display) {
    if std::env::var_os("PICASA_TRACE").is_some() {
        eprintln!("NETWORK {message}");
    }
}

pub fn mount_share_async(
    reference: &str,
    parent: Option<&gtk::Window>,
    on_result: impl FnOnce(Result<(), String>) + 'static,
) {
    let file = file(reference);
    net_trace(format!("mount_started uri={reference}"));
    let mount_operation = gtk::MountOperation::new(parent);
    let reference_for_callback = reference.to_string();
    // "Already mounted" counts as success: skip straight to the reachability
    // check when the location has an enclosing mount.
    if file.find_enclosing_mount(gio::Cancellable::NONE).is_ok()
        && query_exists(&reference_for_callback, true)
    {
        net_trace(format!(
            "mount_success uri={reference_for_callback} (already mounted)"
        ));
        on_result(Ok(()));
        return;
    }
    file.mount_enclosing_volume(
        gio::MountMountFlags::NONE,
        Some(&mount_operation),
        gio::Cancellable::NONE,
        move |result| {
            let outcome = match result {
                Ok(()) if query_exists(&reference_for_callback, true) => {
                    net_trace(format!("mount_success uri={}", reference_for_callback));
                    Ok(())
                }
                Ok(()) => Err(format!(
                    "mounted, but the location is not reachable: {}",
                    reference_for_callback
                )),
                Err(error) => {
                    if error.kind() == Some(gio::IOErrorEnum::AlreadyMounted) {
                        net_trace(format!(
                            "mount_success uri={} (already mounted)",
                            reference_for_callback
                        ));
                        on_result(Ok(()));
                        return;
                    }
                    net_trace(format!(
                        "mount_failed uri={} error={}",
                        reference_for_callback, error
                    ));
                    Err(describe_mount_error(&reference_for_callback, &error))
                }
            };
            on_result(outcome);
        },
    );
}

fn describe_mount_error(reference: &str, error: &glib::Error) -> String {
    use gio::IOErrorEnum;
    match error.kind() {
        Some(IOErrorEnum::NotFound) | Some(IOErrorEnum::NotMounted) => format!(
            "server or share not found: {reference}. Check the server address and the share name."
        ),
        Some(IOErrorEnum::PermissionDenied) => {
            format!("access denied for {reference}. Check the credentials for the share.")
        }
        Some(IOErrorEnum::TimedOut) => {
            format!("the server did not respond in time: {reference}. Is the NAS online?")
        }
        _ => format!("{reference}: {error}"),
    }
}

pub fn resolve_network_uri(reference: &str) -> Option<String> {
    if !reference.starts_with("network://") {
        return None;
    }
    let file = file(reference);
    let info = file.query_info(
        "standard::target-uri",
        gio::FileQueryInfoFlags::NONE,
        gio::Cancellable::NONE,
    );
    let target = info
        .ok()
        .and_then(|info| info.attribute_as_string("standard::target-uri"))
        .map(|value| value.to_string());
    net_trace(format!(
        "resolve {reference} -> {}",
        target.as_deref().unwrap_or("<unresolved>")
    ));
    target
}


pub fn network_browse_root(reference: &str) -> String {
    if reference.starts_with("network://") {
        if let Some(resolved) = resolve_network_uri(reference) {
            return network_browse_root(&resolved);
        }
    }
    let mut uri = reference.to_string();
    if !uri.contains("://") {
        // Local path: out of scope for the network browser - return unchanged
        // rather than ever guessing a home directory.
        return uri;
    }
    if !uri.ends_with('/') {
        uri.push('/');
    }
    uri
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

    net_trace(format!("browse_root uri={root_uri}"));
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
                            // Shares at a server root are mountable entries;
                            // give them the network icon so they are visually
                            // distinct from ordinary subfolders.
                            let icon_name = if mountable {
                                "folder-remote-symbolic"
                            } else {
                                "folder-symbolic"
                            };
                            let icon = gtk::Image::from_icon_name(icon_name);
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

                            // Mouse path. Capture phase + attached to the row's
                            // child box: in GTK4 the click inside a GtkListBoxRow
                            // targets the child widget, and the enclosing
                            // GtkListBox installs its own gesture on the row in
                            // the bubble phase. Capturing on the box guarantees
                            // this handler runs first and can claim the sequence.
                            let click = gtk::GestureClick::new();
                            click.set_propagation_phase(gtk::PropagationPhase::Capture);
                            click.connect_pressed(move |gesture, _, _, _| {
                                net_trace(format!(
                                    "browse_enter uri={uri_for_activate}"
                                ));
                                let parent_window =
                                    dialog_window_for_activate.clone().upcast::<gtk::Window>();
                                let error = error_for_activate.clone();
                                let load_for_mount = load_for_activate.clone();
                                let uri_for_mount = uri_for_activate.clone();
                                let uri_for_load = uri_for_activate.clone();
                                let uri_for_error = uri_for_activate.clone();
                                let load_slot_for_mount = load_for_mount.clone();
                                mount_share_async(
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
                                            net_trace(format!(
                                                "browse_failed uri={uri_for_error} error={message}"
                                            ));
                                            error.set_text(&message);
                                            error.set_visible(true);
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
                            row.connect_activate(move |_| {
                                net_trace(format!(
                                    "browse_enter_key uri={uri_for_key}"
                                ));
                                let parent_window =
                                    dialog_window_for_key.clone().upcast::<gtk::Window>();
                                let error = error_for_key.clone();
                                let load_for_mount = load_for_key.clone();
                                let uri_for_mount = uri_for_key.clone();
                                let uri_for_load = uri_for_key.clone();
                                let load_slot_for_mount = load_for_mount.clone();
                                mount_share_async(
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
                                            error.set_text(&message);
                                            error.set_visible(true);
                                        }
                                    },
                                );
                            });
                            list.append(&row);
                        }
                        Ok(NetworkBrowseMessage::Done(Ok(count))) => {
                            net_trace(format!(
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
                            net_trace(format!(
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
                net_trace(format!("browse_up uri={parent_uri}"));
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
            net_trace(format!("browse_selected uri={uri}"));
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

