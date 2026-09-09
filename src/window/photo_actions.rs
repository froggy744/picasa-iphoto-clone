use gio::prelude::AppInfoExt;

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
    // Grid context menus use pointer-local coordinates. Lightbox passes a
    // negative sentinel because its capture controller lives on a different
    // widget than the stable popover anchor; in that case let GTK position the
    // popover relative to the viewport instead of giving it invalid coords.
    if x >= 0.0 && y >= 0.0 {
        popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(
            x.round() as i32,
            y.round() as i32,
            1,
            1,
        )));
    }

    let menu = gtk::Box::new(gtk::Orientation::Vertical, 2);
    // This popover is anchored to a virtualized GridView tile. In that
    // context GTK can otherwise map the popover before it has calculated a
    // natural size, resulting in a visible but unusable 0x0 menu.
    menu.set_width_request(340);
    menu.set_height_request(1);
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
    let edit = add_action("Edit Photo…");
    let open_with = add_action("Open With…");
    let open_in_folder = (!matches!(
        context.filter.get(),
        sidebar::SidebarFilter::Albums
    ))
    .then(|| add_action("Open in Folder"));
    menu.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

    let selection_ids = selected_photo_ids(&context, Some(photo.id()));
    if std::env::var_os("PICASA_TRACE").is_some() {
        eprintln!(
            "COLLAGE TRACE context clicked_id={} selected_ids={:?}",
            photo.id(),
            selection_ids
        );
    }
    let selection_for_provider = selection_ids.clone();
    let selection_provider: Rc<dyn Fn() -> Vec<i64>> =
        Rc::new(move || selection_for_provider.clone());
    let add_to_album = gtk::MenuButton::new();
    add_to_album.set_label("Add to Album");
    add_to_album.set_direction(gtk::ArrowType::None);
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

    let collage_ids = selection_ids.clone();
    let collage = add_action("Create Collage…");
    collage.set_sensitive(collage_ids.len() >= 2);
    let collage_context = context.clone();
    let collage_popover = popover.clone();
    collage.connect_clicked(move |_| {
        collage_popover.popdown();
        if std::env::var_os("PICASA_TRACE").is_some() {
            eprintln!("COLLAGE TRACE open ids={:?}", collage_ids);
        }
        (collage_context.open_collage)(collage_ids.clone());
    });

    menu.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    let copy_edits = add_action("Copy Edits");
    let paste_edits = add_action("Paste Edits");
    let reset_edits = add_action("Reset Edits");
    let clicked_recipe = photo.edit_recipe();
    let clicked_is_edited = !crate::edit::EditRecipe::decode(&clicked_recipe).is_default();
    copy_edits.set_sensitive(clicked_is_edited);
    paste_edits.set_sensitive(context.edit_clipboard.borrow().is_some());
    reset_edits.set_sensitive(clicked_is_edited || selection_ids.iter().any(|id| {
        db::photo(&context.connection.borrow(), *id)
            .ok()
            .flatten()
            .is_some_and(|item| !crate::edit::EditRecipe::decode(&item.edit_recipe).is_default())
    }));

    {
        let clipboard = context.edit_clipboard.clone();
        let recipe = clicked_recipe.clone();
        let popover = popover.clone();
        copy_edits.connect_clicked(move |_| {
            clipboard.replace(Some(recipe.clone()));
            popover.popdown();
        });
    }
    {
        let paste_context = context.clone();
        let paste_selection = selection_ids.clone();
        let popover = popover.clone();
        paste_edits.connect_clicked(move |button| {
            let Some(recipe) = paste_context.edit_clipboard.borrow().clone() else {
                return;
            };
            for id in &paste_selection {
                if let Err(error) = db::set_edit_recipe(&paste_context.connection.borrow(), *id, &recipe) {
                    show_error(button.upcast_ref(), "Could not paste edits", &error.to_string());
                    return;
                }
                if let Some(gallery) = paste_context.gallery.borrow().upgrade() {
                    gallery.update_edit_recipe(*id, &recipe);
                }
                let selected = {
                    paste_context.selected_photo.borrow().as_ref().cloned()
                };
                if let Some(selected) = selected {
                    if selected.id() == *id {
                        selected.set_edit_recipe(recipe.clone());
                        paste_context.selected_photo.replace(Some(selected.clone()));
                        paste_context.info.set_photo(Some(&selected));
                    }
                }
            }
            popover.popdown();
            refresh_photo_actions_grid(&paste_context);
        });
    }
    {
        let reset_context = context.clone();
        let reset_selection = selection_ids.clone();
        let popover = popover.clone();
        reset_edits.connect_clicked(move |button| {
            for id in &reset_selection {
                if let Err(error) = db::set_edit_recipe(&reset_context.connection.borrow(), *id, "") {
                    show_error(button.upcast_ref(), "Could not reset edits", &error.to_string());
                    return;
                }
                if let Some(gallery) = reset_context.gallery.borrow().upgrade() {
                    gallery.update_edit_recipe(*id, "");
                }
                let selected = {
                    reset_context.selected_photo.borrow().as_ref().cloned()
                };
                if let Some(selected) = selected {
                    if selected.id() == *id {
                        selected.set_edit_recipe(String::new());
                        reset_context.selected_photo.replace(Some(selected.clone()));
                        reset_context.info.set_photo(Some(&selected));
                    }
                }
            }
            popover.popdown();
            refresh_photo_actions_grid(&reset_context);
        });
    }

    let favorite_label = if photo.favorite() {
        "Remove from Favourites"
    } else {
        "Add to Favourites"
    };
    let favorite = add_action(favorite_label);
    let favorite_context = context.clone();
    let favorite_selection = selection_provider.clone();
    let favorite_photo = photo.clone();
    let popover_for_favorite = popover.clone();
    favorite.connect_clicked(move |button| {
        let target = !favorite_photo.favorite();
        let ids = favorite_selection();
        let count = ids.len();
        for id in ids {
            if let Err(error) = db::set_favorite(
                &favorite_context.connection.borrow(),
                id,
                target,
            ) {
                show_error(
                    button.upcast_ref(),
                    "Could not update favourite",
                    &error.to_string(),
                );
                return;
            }
        }

        let selected_id = favorite_context
            .selected_photo
            .borrow()
            .as_ref()
            .map(|selected| selected.id());
        if selected_id == Some(favorite_photo.id()) {
            favorite_photo.set_favorite(target);
            favorite_context
                .selected_photo
                .replace(Some(favorite_photo.clone()));
            favorite_context.info.set_photo(Some(&favorite_photo));
        }
        popover_for_favorite.popdown();
        refresh_photo_actions_grid(&favorite_context);
        refresh_favorite_sidebar(&favorite_context);
    });

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

    let edit_context = context.clone();
    let edit_id = photo.id();
    let popover_for_edit = popover.clone();
    edit.connect_clicked(move |_| {
        popover_for_edit.popdown();
        (edit_context.open_edit)(edit_id);
    });

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
    let open_with_file = file.clone();
    let open_with_anchor = open_with.clone();
    let popover_for_open_with = popover.clone();
    open_with.connect_clicked(move |_| {
        popover_for_open_with.popdown();
        show_open_with_dialog(open_with_anchor.upcast_ref(), &open_with_file);
    });

    if let Some(open_in_folder) = open_in_folder {
        let folder_id = photo.folder_id();
        let navigate_to_folder = context.navigate_to_folder.clone();
        let popover_for_folder = popover.clone();
        let photo_id = photo.id();
        open_in_folder.connect_clicked(move |_| {
            if folder_id != 0 {
                navigate_to_folder(folder_id, photo_id);
            }
            popover_for_folder.popdown();
        });
    }

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
        open_file_in_manager(&file_for_manager);
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
    // GridView tiles are virtualized. Defer opening until the selection and
    // allocation pass triggered by the secondary-button event has completed;
    // otherwise the popover can remain unmapped at 0x0.
    let popover_for_popup = popover.clone();
    glib::idle_add_local_once(move || {
        popover_for_popup.popup();
    });
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

fn open_file_in_manager(file: &gio::File) {
    if let Some(path) = file.path() {
        // Nautilus is the only file manager whose selection option is verified
        // in the supported Linux environment. Spawn it so the GTK main thread
        // remains responsive while it opens and selects the file.
        if std::process::Command::new("nautilus")
            .arg("--select")
            .arg(path)
            .spawn()
            .is_ok()
        {
            return;
        }
    }

    if let Some(parent) = file.parent() {
        let _ = gio::AppInfo::launch_default_for_uri(
            &parent.uri(),
            None::<&gio::AppLaunchContext>,
        );
    }
}

fn show_open_with_dialog(parent: &gtk::Widget, file: &gio::File) {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 2);
    content.set_margin_top(6);
    content.set_margin_bottom(6);
    content.set_margin_start(6);
    content.set_margin_end(6);

    let path = file
        .path()
        .unwrap_or_else(|| std::path::PathBuf::from(file.uri().as_str()));
    let (content_type, _) = gio::content_type_guess(Some(&path), None::<&[u8]>);
    let apps = gio::AppInfo::all_for_type(content_type.as_str());
    let mut app_buttons = Vec::new();
    let dialog = if apps.is_empty() {
        adw::AlertDialog::builder()
            .heading("Open With")
            .body("No applications are registered for this file type.")
            .close_response("close")
            .build()
    } else {
        for app in apps {
            let button = gtk::Button::with_label(app.display_name().as_str());
            button.set_halign(gtk::Align::Fill);
            button.add_css_class("flat");
            app_buttons.push((button.clone(), app));
            content.append(&button);
        }
        let dialog = adw::AlertDialog::builder()
            .heading("Open With")
            .body("Choose an application for this file:")
            .extra_child(&content)
            .close_response("close")
            .build();
        for (button, app) in app_buttons {
            let app_for_launch = app.clone();
            let file_for_launch = file.clone();
            let dialog_for_launch = dialog.clone();
            button.connect_clicked(move |_| {
                let _ = app_for_launch.launch(
                    std::slice::from_ref(&file_for_launch),
                    None::<&gio::AppLaunchContext>,
                );
                dialog_for_launch.close();
            });
        }
        dialog
    };
    dialog.add_response("close", "Cancel");
    dialog.present(Some(parent));
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
    gallery.refresh_favorite_indicators();
    let search = context.search.borrow().clone();
    refresh_grid(
        &context.connection,
        context.filter.get(),
        &search,
        context.sort.get(),
        &gallery,
    );
}

fn refresh_favorite_sidebar(context: &PhotoActionContext) {
    let Some(sidebar) = context.sidebar.borrow().as_ref().cloned() else {
        return;
    };
    let Ok(counts) = db::sidebar_counts(&context.connection.borrow()) else {
        return;
    };
    sidebar::refresh_library_counts(&sidebar, counts, &context.on_unavailable);
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
