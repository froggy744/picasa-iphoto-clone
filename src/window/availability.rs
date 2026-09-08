use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

static AVAILABILITY_GENERATION: AtomicU64 = AtomicU64::new(0);

#[derive(Default)]
struct ReconnectedSources {
    mounted: std::collections::HashSet<String>,
    pending: std::collections::HashSet<String>,
}

impl ReconnectedSources {
    fn update(&mut self, mounted: std::collections::HashSet<String>) {
        self.pending.extend(mounted.difference(&self.mounted).cloned());
        // A drive may disappear again while recovery waits for another scan.
        self.pending.retain(|root| mounted.contains(root));
        self.mounted = mounted;
    }

    fn includes(&self, path: &str) -> bool {
        let file = crate::source::file(path);
        self.pending.iter().any(|root| {
            let root = crate::source::file(root);
            file.equal(&root) || file.has_prefix(&root)
        })
    }
}

fn mounted_source_roots() -> std::collections::HashSet<String> {
    let mut roots = std::collections::HashSet::new();
    for mount in gio::VolumeMonitor::get().mounts() {
        let root = mount.root();
        roots.insert(root.uri().to_string());
        if let Some(path) = root.path() {
            roots.insert(gio::File::for_path(path).uri().to_string());
        }
    }
    #[cfg(unix)]
    for mount in gio::UnixMountEntry::mounts().0 {
        roots.insert(gio::File::for_path(mount.mount_path()).uri().to_string());
    }
    roots
}

#[cfg(test)]
mod reconnect_tests {
    use super::*;

    fn roots(paths: &[&str]) -> std::collections::HashSet<String> {
        paths.iter().map(|path| path.to_string()).collect()
    }

    #[test]
    fn unchanged_mounts_and_unmounts_do_not_request_recovery() {
        let initial = roots(&["file:///", "file:///media/usb"]);
        let mut sources = ReconnectedSources {
            mounted: initial.clone(),
            ..Default::default()
        };
        sources.update(initial.clone());
        assert!(sources.pending.is_empty());
        sources.update(roots(&["file:///"]));
        assert!(sources.pending.is_empty());
        sources.update(initial.clone());
        assert_eq!(sources.pending, roots(&["file:///media/usb"]));
        sources.pending.clear();
        sources.update(initial);
        assert!(sources.pending.is_empty());
    }

    #[test]
    fn reconnect_scope_matches_path_components_and_uri_references() {
        let mut sources = ReconnectedSources::default();
        sources.update(roots(&["file:///media/usb", "smb://server/photos"]));
        assert!(sources.includes("/media/usb/album/photo.jpg"));
        assert!(sources.includes("file:///media/usb/album/photo.jpg"));
        assert!(sources.includes("/media/usb"));
        assert!(sources.includes("smb://server/photos/album/photo.jpg"));
        assert!(!sources.includes("/media/usb-backup/photo.jpg"));
        assert!(!sources.includes("/media/other/photo.jpg"));
        assert!(!sources.includes("smb://server/photos-backup/photo.jpg"));
        assert!(!sources.includes("smb://other/photos/photo.jpg"));
    }

    #[test]
    fn queued_reconnects_accumulate_and_disconnected_drives_are_removed() {
        let mut sources = ReconnectedSources::default();
        sources.update(roots(&["file:///media/one"]));
        sources.update(roots(&["file:///media/one", "file:///media/two"]));
        assert_eq!(sources.pending.len(), 2);
        sources.update(roots(&["file:///media/two"]));
        assert_eq!(sources.pending, roots(&["file:///media/two"]));
        sources.update(roots(&[]));
        assert!(sources.pending.is_empty());
    }
}

fn spawn_thumbnail_recovery(
    photos: Vec<db::Photo>,
    startup_paths: std::sync::Arc<std::collections::HashSet<String>>,
    generation: u64,
    sender: std::sync::mpsc::Sender<ScanUiEvent>,
    control: scanner::ScanControl,
) {
    std::thread::spawn(move || {
        let send = |event| {
            let _ = sender.send(ScanUiEvent { generation, event });
        };
        let items = photos
            .into_iter()
            .map(|photo| (photo.path, photo.mtime, photo.size_bytes))
            .collect();
        let (mut ready, mut offline) = crate::thumbnail::recovery_items(items);
        ready.sort_by_key(|(path, _, _)| !startup_paths.contains(path));
        if std::env::var_os("PICASA_TRACE").is_some() {
            eprintln!("THUMB RECOVERY ready={} offline={offline}", ready.len());
        }
        if !ready.is_empty() && !control.is_cancelled() {
            send(scanner::ScanEvent::ThumbnailsStarted { total: ready.len() });
        }
        let mut failed = 0;
        for chunk in ready.chunks(64) {
            if control.is_cancelled() {
                break;
            }
            let results = crate::thumbnail::create_many_cancellable(
                chunk,
                || control.is_cancelled(),
                |path| {
                    send(scanner::ScanEvent::ThumbnailCreated { path: path.into() });
                },
            );
            for ((path, _, _), result) in chunk.iter().zip(results) {
                if let Some(Err(error)) = result {
                    // The drive can disappear during an active thumbnail pass.
                    if !crate::source::file_available(path) {
                        offline += 1;
                        continue;
                    }
                    failed += 1;
                    send(scanner::ScanEvent::Failed {
                        path: path.into(),
                        error: format!("thumbnail: {error}"),
                    });
                }
            }
        }
        send(scanner::ScanEvent::ThumbnailsDeferred { total: offline });
        send(if control.is_cancelled() {
            scanner::ScanEvent::Cancelled { imported: 0 }
        } else {
            scanner::ScanEvent::Finished {
                imported: 0,
                failed,
            }
        });
    });
}

fn run_ui_guarded(label: &str, action: impl FnOnce()) {
    let started = std::time::Instant::now();
    if let Err(panic) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(action)) {
        eprintln!("UI CALLBACK PANIC RECOVERED: {label}: {panic:?}");
    }
    if std::env::var_os("PICASA_TRACE").is_some() {
        eprintln!(
            "UI PERF callback={} elapsed_ms={}",
            label,
            started.elapsed().as_millis()
        );
    }
}

fn show_unavailable_dialog(
    parent: &gtk::Widget,
    photo: crate::photo_object::PhotoObject,
    connection: Rc<RefCell<Connection>>,
    availability_refresh: Rc<dyn Fn()>,
) {
    let dialog = adw::AlertDialog::builder()
        .heading("Original photo unavailable")
        .body(format!(
            "{}\n\nThe drive may be unmounted, or the file may have been moved or deleted.",
            photo.path()
        ))
        .close_response("cancel")
        .build();
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("retry", "Retry");
    dialog.add_response("remove", "Remove from Library");
    dialog.set_response_appearance("remove", adw::ResponseAppearance::Destructive);

    let photo_for_response = photo.clone();
    let connection_for_response = connection.clone();
    let availability_refresh_for_response = availability_refresh.clone();
    dialog.connect_response(None, move |_, response| {
        if response == "retry" {
            availability_refresh_for_response();
        } else if response == "remove" {
            if let Err(error) = db::set_trashed(
                &connection_for_response.borrow(),
                photo_for_response.id(),
                true,
            ) {
                eprintln!("Could not remove unavailable photo from library: {error}");
            }
            availability_refresh_for_response();
        } else {
            return;
        }
    });
    dialog.present(Some(parent));
}

fn refresh_availability_ui(
    connection: &Rc<RefCell<Connection>>,
    gallery: &Rc<RefCell<Weak<grid::Gallery>>>,
    sidebar: &Rc<RefCell<Option<gtk::ScrolledWindow>>>,
    availability_refresh_slot: &Rc<RefCell<Option<Rc<dyn Fn()>>>>,
    create_album: Rc<dyn Fn()>,
    import_folder: Rc<dyn Fn()>,
    delete_album: Rc<dyn Fn(i64)>,
) {
    crate::source::refresh_availability();

    let generation = AVAILABILITY_GENERATION.fetch_add(1, AtomicOrdering::Relaxed) + 1;
    if let Some(gallery) = gallery.borrow().upgrade() {
        let snapshot = gallery.availability_snapshot();
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let updates = snapshot
                .into_iter()
                .map(|(id, path)| (id, crate::source::cached_file_available(&path)))
                .collect::<Vec<_>>();
            let _ = sender.send((generation, updates));
        });

        let gallery_for_result = gallery.clone();
        glib::timeout_add_local(Duration::from_millis(50), move || {
            match receiver.try_recv() {
                Ok((result_generation, updates)) => {
                    if result_generation == AVAILABILITY_GENERATION.load(AtomicOrdering::Relaxed) {
                        gallery_for_result.apply_availability(&updates);
                    }
                    glib::ControlFlow::Break
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
            }
        });
    }

    if let (Ok(folders), Ok(albums), Ok(counts)) = (
        db::folders(&connection.borrow()),
        db::albums(&connection.borrow()),
        db::sidebar_counts(&connection.borrow()),
    ) {
        if let Some(sidebar) = sidebar.borrow().as_ref() {
            if let Some(on_unavailable) = availability_refresh_slot.borrow().as_ref().cloned() {
                sidebar::refresh(
                    sidebar,
                    &folders,
                    &albums,
                    counts,
                    create_album,
                    import_folder,
                    delete_album,
                    on_unavailable,
                );
            }
        }
    }
}
