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
    // Keep the debug sink inside the user's cache directory instead of a
    // world-writable fixed path (multi-user systems and sandboxed builds).
    let Some(cache_dir) = dirs::cache_dir().map(|dir| dir.join("pic-rs")) else {
        return;
    };
    let _ = std::fs::create_dir_all(&cache_dir);
    let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(cache_dir.join("ui-debug.log"))
    else {
        return;
    };
    let _ = writeln!(file, "{message}");
}

fn install_close_confirmation(
    window: &adw::ApplicationWindow,
    save_session: Rc<dyn Fn()>,
) {
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
        let save_session_for_response = save_session.clone();
        let close_confirmation_open_for_response = close_confirmation_open_for_request.clone();
        let close_confirmation_allowed_for_response = close_confirmation_allowed_for_request.clone();
        dialog.connect_response(None, move |dialog, response| {
            close_confirmation_open_for_response.set(false);
            if response == "close" {
                save_session_for_response();
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
