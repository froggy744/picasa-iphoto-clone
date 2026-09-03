fn show_photo_context_menu(
    photo: crate::photo_object::PhotoObject,
    anchor: gtk::Widget,
    context: PhotoActionContext,
    x: f64,
    y: f64,
) {
    let popover = gtk::Popover::new();
    popover.set_has_arrow(true);
    popover.set_parent(&anchor);
    popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(
        x.round() as i32,
        y.round() as i32,
        1,
        1,
    )));

    let menu = gtk::Box::new(gtk::Orientation::Vertical, 2);
    menu.set_margin_top(6);
    menu.set_margin_bottom(6);
    menu.set_margin_start(6);
    menu.set_margin_end(6);

    let add_action = |label: &str| {
        let button = gtk::Button::with_label(label);
        button.set_halign(gtk::Align::Fill);
        button.add_css_class("flat");
        menu.append(&button);
        button
    };

    let open = add_action("Open");
    let open_with = add_action("Open With…");
    menu.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

    let album_selection = selected_photo_ids(&context, Some(photo.id()));
    let selection_provider: Rc<dyn Fn() -> Vec<i64>> = Rc::new(move || album_selection.clone());
    let add_to_album = gtk::MenuButton::new();
    add_to_album.set_label("Add to Album  ›");
    add_to_album.set_halign(gtk::Align::Fill);
    add_to_album.add_css_class("flat");
    add_to_album.set_popover(Some(&build_album_popover(
        context.clone(),
        selection_provider.clone(),
        {
            let popover = popover.clone();
            Rc::new(move || popover.popdown())
        },
    )));
    menu.append(&add_to_album);

    if let sidebar::SidebarFilter::Album(album_id) = context.filter.get() {
        let remove = add_action("Remove from Album");
        let remove_context = context.clone();
        let remove_selection = selection_provider.clone();
        let popover_for_remove = popover.clone();
        remove.connect_clicked(move |button| {
            popover_for_remove.popdown();
            if let Err(error) = db::remove_photos_from_album(
                &remove_context.connection.borrow(),
                album_id,
                &remove_selection(),
            ) {
                show_error(
                    button.upcast_ref(),
                    "Could not remove from album",
                    &error.to_string(),
                );
                return;
            }
            if let Some(lightbox) = remove_context.lightbox.upgrade() {
                lightbox.close();
            }
            refresh_photo_actions_grid(&remove_context);
            refresh_album_ui(&remove_context);
        });
    }
    menu.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    let copy = add_action("Copy");
    let copy_location = add_action("Copy File Location");
    menu.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    let move_file = add_action("Move…");
    let rename = add_action("Rename…");
    let file_manager = add_action("Open in File Manager");
    let print = add_action("Print");
    let properties = add_action("Properties");
    menu.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    let delete = add_action("Delete");
    delete.add_css_class("destructive-action");

    let photo_for_open = photo.clone();
    let popover_for_open = popover.clone();
    let lightbox_for_open = context.lightbox.clone();
    open.connect_clicked(move |_| {
        if let Some(lightbox) = lightbox_for_open.upgrade() {
            lightbox.open(vec![photo_for_open.clone()], 0);
        }
        popover_for_open.popdown();
    });

    let file = crate::source::file(&photo.path());
    let uri = file.uri();
    let popover_for_uri = popover.clone();
    open_with.connect_clicked(move |_| {
        let _ = gio::AppInfo::launch_default_for_uri(&uri, None::<&gio::AppLaunchContext>);
        popover_for_uri.popdown();
    });

    let path_for_copy = photo.path();
    let display = anchor.display();
    let popover_for_copy = popover.clone();
    copy_location.connect_clicked(move |_| {
        display.clipboard().set_text(&path_for_copy);
        popover_for_copy.popdown();
    });

    let file_for_manager = file.clone();
    let popover_for_manager = popover.clone();
    file_manager.connect_clicked(move |_| {
        if let Some(parent) = file_for_manager.parent() {
            let _ =
                gio::AppInfo::launch_default_for_uri(&parent.uri(), None::<&gio::AppLaunchContext>);
        }
        popover_for_manager.popdown();
    });

    // These actions are not implemented yet; do not present them as working
    // commands that merely close the menu.
    copy.set_sensitive(false);
    move_file.set_sensitive(false);
    print.set_sensitive(false);

    let photo_for_rename = photo.clone();
    let anchor_for_rename = anchor.clone();
    let context_for_rename = context.clone();
    let popover_for_rename = popover.clone();
    rename.connect_clicked(move |_| {
        popover_for_rename.popdown();
        show_rename_dialog(
            &anchor_for_rename,
            photo_for_rename.clone(),
            context_for_rename.clone(),
        );
    });

    let photo_for_properties = photo.clone();
    let anchor_for_properties = anchor.clone();
    let context_for_properties = context.clone();
    let popover_for_properties = popover.clone();
    properties.connect_clicked(move |_| {
        popover_for_properties.popdown();
        show_properties_dialog(
            &anchor_for_properties,
            &photo_for_properties,
            &context_for_properties,
        );
    });

    let photo_for_delete = photo;
    let anchor_for_delete = anchor.clone();
    let context_for_delete = context;
    let popover_for_delete = popover.clone();
    delete.connect_clicked(move |_| {
        popover_for_delete.popdown();
        show_delete_confirmation(
            &anchor_for_delete,
            photo_for_delete.clone(),
            context_for_delete.clone(),
        );
    });

    popover.set_child(Some(&menu));
    popover.popup();
    if std::env::var_os("PICASA_TRACE").is_some() {
        let popover = popover.clone();
        glib::idle_add_local_once(move || {
            eprintln!(
                "UI TRACE photo_context_menu visible={} mapped={} size={}x{}",
                popover.is_visible(),
                popover.is_mapped(),
                popover.width(),
                popover.height()
            );
        });
    }
}

fn show_rename_dialog(
    parent: &gtk::Widget,
    photo: crate::photo_object::PhotoObject,
    context: PhotoActionContext,
) {
    let entry = gtk::Entry::new();
    entry.set_text(&photo.filename());
    entry.set_activates_default(true);
    entry.select_region(0, -1);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let label = gtk::Label::new(Some("Enter a new file name:"));
    label.set_xalign(0.0);
    content.append(&label);
    content.append(&entry);

    let dialog = adw::AlertDialog::builder()
        .heading("Rename photo")
        .extra_child(&content)
        .close_response("cancel")
        .default_response("rename")
        .build();
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("rename", "Rename");
    dialog.set_response_appearance("rename", adw::ResponseAppearance::Suggested);

    let parent_for_response = parent.clone();
    dialog.connect_response(Some("rename"), move |_, _| {
        let new_name = entry.text().trim().to_string();
        if !valid_file_name(&new_name) {
            show_error(
                &parent_for_response,
                "Invalid file name",
                "Enter a file name without folder separators.",
            );
            return;
        }
        if new_name == photo.filename() {
            return;
        }

        let old_name = photo.filename();
        let old_cache = photo.cached_thumbnail_path();
        let fingerprint = db::photo(&context.connection.borrow(), photo.id())
            .ok()
            .flatten()
            .map(|record| (record.mtime, record.size_bytes));
        let source = crate::source::file(&photo.path());
        let renamed = match source.set_display_name(&new_name, gio::Cancellable::NONE) {
            Ok(file) => file,
            Err(error) => {
                show_error(
                    &parent_for_response,
                    "Could not rename photo",
                    &error.to_string(),
                );
                return;
            }
        };
        let new_reference = crate::source::reference(&renamed);
        if let Err(error) =
            db::set_photo_path(&context.connection.borrow(), photo.id(), &new_reference)
        {
            // Keep the filesystem and database referring to the same path if
            // the unique-path update unexpectedly fails.
            let _ = renamed.set_display_name(&old_name, gio::Cancellable::NONE);
            show_error(
                &parent_for_response,
                "Could not rename photo",
                &error.to_string(),
            );
            return;
        }

        photo.set_path(new_reference.as_str());
        photo.set_filename(crate::source::filename(&new_reference));
        if let (Some(old_cache), Some((mtime, size_bytes))) = (old_cache, fingerprint) {
            if let Ok(new_cache) = crate::thumbnail::cache_path(&new_reference, mtime, size_bytes) {
                if let Some(parent) = new_cache.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if std::fs::rename(&old_cache, &new_cache).is_ok() || new_cache.is_file() {
                    let new_cache = new_cache.to_string_lossy().into_owned();
                    photo.set_cached_thumbnail_path(new_cache.as_str());
                }
            }
        }
        context.info.set_photo(Some(&photo));
        context.selected_photo.replace(Some(photo.clone()));
    });
    dialog.present(Some(parent));
}

fn valid_file_name(name: &str) -> bool {
    if name.is_empty() || matches!(name, "." | "..") {
        return false;
    }
    std::path::Path::new(name)
        .file_name()
        .is_some_and(|file_name| file_name == name)
}

fn show_delete_confirmation(
    parent: &gtk::Widget,
    photo: crate::photo_object::PhotoObject,
    context: PhotoActionContext,
) {
    let dialog = adw::AlertDialog::builder()
        .heading(format!("Move “{}” to Trash?", photo.filename()))
        .body("The photo will be removed from the library and moved to the system Trash.")
        .close_response("cancel")
        .default_response("cancel")
        .build();
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("delete", "Move to Trash");
    dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);

    let parent_for_response = parent.clone();
    dialog.connect_response(Some("delete"), move |_, _| {
        if let Err(error) = db::set_trashed(&context.connection.borrow(), photo.id(), true) {
            show_error(
                &parent_for_response,
                "Could not delete photo",
                &error.to_string(),
            );
            return;
        }

        if let Err(error) = crate::source::file(&photo.path()).trash(gio::Cancellable::NONE) {
            let _ = db::set_trashed(&context.connection.borrow(), photo.id(), false);
            show_error(
                &parent_for_response,
                "Could not move photo to Trash",
                &error.to_string(),
            );
            return;
        }

        if let Some(lightbox) = context.lightbox.upgrade() {
            lightbox.close();
        }
        context.selected_photo.replace(None);
        context.info.set_photo(None);
        refresh_photo_actions_grid(&context);
    });
    dialog.present(Some(parent));
}

fn refresh_photo_actions_grid(context: &PhotoActionContext) {
    let Some(gallery) = context.gallery.borrow().upgrade() else {
        return;
    };
    let search = context.search.borrow().clone();
    refresh_grid(
        &context.connection,
        context.filter.get(),
        &search,
        context.sort.get(),
        &gallery,
    );
}

fn show_properties_dialog(
    parent: &gtk::Widget,
    photo: &crate::photo_object::PhotoObject,
    context: &PhotoActionContext,
) {
    let record = db::photo(&context.connection.borrow(), photo.id())
        .ok()
        .flatten();
    let path = record
        .as_ref()
        .map(|photo| photo.path.clone())
        .unwrap_or_else(|| photo.path());
    let width = record
        .as_ref()
        .and_then(|photo| photo.width)
        .unwrap_or_else(|| photo.width());
    let height = record
        .as_ref()
        .and_then(|photo| photo.height)
        .unwrap_or_else(|| photo.height());
    let dimensions = if width > 0 && height > 0 {
        format!("{width} × {height}")
    } else {
        "Unknown".to_string()
    };
    let size = record
        .as_ref()
        .and_then(|photo| photo.size_bytes)
        .unwrap_or_else(|| photo.size_bytes());
    let camera = record
        .as_ref()
        .and_then(|photo| photo.camera.as_deref())
        .unwrap_or("Unknown camera");
    let date = record
        .as_ref()
        .and_then(|photo| photo.taken_at.as_deref())
        .map(crate::infobar::format_date)
        .unwrap_or_else(|| "Unknown date".to_string());
    let rotation = record
        .as_ref()
        .map(|photo| photo.rotation)
        .unwrap_or_else(|| photo.rotation());
    let favorite = record
        .as_ref()
        .map(|photo| photo.favorite)
        .unwrap_or_else(|| photo.favorite());

    let body = format!(
        "Location: {path}\nDimensions: {dimensions}\nSize: {}\nCamera: {camera}\nDate: {date}\nRotation: {rotation}°\nFavourite: {}",
        crate::infobar::format_size(size),
        if favorite { "Yes" } else { "No" }
    );
    let dialog = adw::AlertDialog::builder()
        .heading(photo.filename())
        .body(body)
        .close_response("close")
        .build();
    dialog.add_response("close", "Close");
    dialog.present(Some(parent));
}

fn show_error(parent: &gtk::Widget, heading: &str, message: &str) {
    let dialog = adw::AlertDialog::builder()
        .heading(heading)
        .body(message)
        .close_response("close")
        .build();
    dialog.add_response("close", "Close");
    dialog.present(Some(parent));
}
