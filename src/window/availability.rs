fn run_ui_guarded(label: &str, action: impl FnOnce()) {
    if let Err(panic) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(action)) {
        eprintln!("UI CALLBACK PANIC RECOVERED: {label}: {panic:?}");
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

    // Recompute the loaded PhotoObjects before touching either view.
    if let Some(gallery) = gallery.borrow().upgrade() {
        gallery.refresh_availability();
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

