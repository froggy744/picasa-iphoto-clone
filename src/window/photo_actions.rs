use gio::prelude::{AppInfoExt, SettingsExt};

thread_local! {
    static ACTIVE_PHOTO_MENU: RefCell<Option<gtk::Widget>> = RefCell::new(None);
}

fn dismiss_active_photo_context_menu() -> bool {
    ACTIVE_PHOTO_MENU.with(|active| {
        let Some(menu) = active.borrow_mut().take() else {
            return false;
        };
        menu.set_visible(false);
        if menu.parent().is_some() {
            menu.unparent();
        }
        if std::env::var_os("PICASA_TRACE").is_some() {
            eprintln!("UI TRACE photo_context_menu_dismiss");
        }
        true
    })
}

fn photo_context_menu_contains(widget: &gtk::Widget) -> bool {
    ACTIVE_PHOTO_MENU.with(|active| {
        let active = active.borrow();
        let Some(menu) = active.as_ref() else {
            return false;
        };
        let mut current = Some(widget.clone());
        while let Some(item) = current {
            if item == *menu {
                return true;
            }
            current = item.parent();
        }
        false
    })
}

/// A submenu trigger inside the photo context menu: flat row, label and arrow,
/// matching the other submenu entries.
fn context_submenu_button(label: &str, css_class: &str) -> gtk::MenuButton {
    let button = gtk::MenuButton::new();
    button.set_focus_on_click(false);
    button.set_focusable(false);
    button.set_direction(gtk::ArrowType::None);
    button.set_halign(gtk::Align::Fill);
    button.add_css_class("flat");
    button.add_css_class("photo-context-submenu");
    button.add_css_class(css_class);
    button.set_height_request(28);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let text = gtk::Label::new(Some(label));
    text.set_xalign(0.0);
    text.set_hexpand(true);
    row.append(&text);
    row.append(&gtk::Image::from_icon_name("pan-end-symbolic"));
    button.set_child(Some(&row));
    button
}

/// This submenu lives inside a mouse context menu. Keep its transient buttons
/// from becoming the window focus: removing a focused submenu and parent menu
/// makes GtkGridView focus its first item and scroll to top. The separately
/// built infobar album menu remains keyboard-focusable.
fn unfocus_submenu(popover: &gtk::Popover) {
    popover.set_focusable(false);
    if let Some(menu) = popover.child() {
        let mut child = menu.first_child();
        while let Some(widget) = child {
            let next = widget.next_sibling();
            if let Ok(button) = widget.downcast::<gtk::Button>() {
                button.set_focus_on_click(false);
                button.set_focusable(false);
            }
            child = next;
        }
    }
}

fn show_photo_context_menu(
    photo: crate::photo_object::PhotoObject,
    anchor: gtk::Widget,
    context: PhotoActionContext,
    x: f64,
    y: f64,
) {
    // One stable context-menu host is shared by the grid and lightbox.
    // The menu itself is a normal GtkOverlay child, not a GtkPopover.
    // This avoids popup/grab/allocation races and recycled-tile parenting.
    dismiss_active_photo_context_menu();

    let Some(host) = context
        .context_menu_host
        .borrow()
        .as_ref()
        .and_then(glib::WeakRef::upgrade)
    else {
        return;
    };
    let host_widget = host.clone().upcast::<gtk::Widget>();
    let click_point = if anchor == host_widget {
        gtk::graphene::Point::new(x as f32, y as f32)
    } else {
        let Some(point) = anchor.compute_point(
            &host_widget,
            &gtk::graphene::Point::new(x as f32, y as f32),
        ) else {
            return;
        };
        point
    };

    // This is a normal overlay widget rather than GtkPopoverMenu, so the
    // theme would otherwise give every GtkButton regular toolbar/dialog
    // padding.  Apply a small context-menu-specific CSS class so rows look
    // and measure like menu items instead of large push buttons.
    let css = gtk::CssProvider::new();
    css.load_from_data(
        ".photo-context-menu {
             padding: 6px;
             border-radius: 12px;
             border: 1px solid alpha(currentColor, 0.10);
             background-color: @popover_bg_color;
             box-shadow: 0 4px 14px alpha(black, 0.16);
         }
         .photo-context-item {
             min-height: 28px;
             padding: 0 12px;
             margin: 0;
             border: 0;
             border-radius: 6px;
             background: transparent;
             box-shadow: none;
             font-size: 14px;
             font-weight: 400;
         }
         .photo-context-item:hover {
             background-color: alpha(currentColor, 0.07);
         }
         .photo-context-item:disabled {
             background: transparent;
             box-shadow: none;
             opacity: 0.45;
         }
         .photo-context-item > label {
             padding: 0;
             margin: 0;
             font-size: 14px;
             font-weight: 400;
         }
         .photo-context-submenu {
             min-height: 28px;
             padding: 0;
             margin: 0;
             background: transparent;
             box-shadow: none;
         }
         .photo-context-submenu > button {
             min-height: 28px;
             padding: 0 12px;
             margin: 0;
             border: 0;
             border-radius: 6px;
             background: transparent;
             box-shadow: none;
             font-size: 14px;
             font-weight: 400;
         }
         .photo-context-submenu > button:hover {
             background-color: alpha(currentColor, 0.07);
         }
         .photo-context-submenu label {
             font-size: 14px;
             font-weight: 400;
         }
         .photo-context-submenu > button > box {
             padding: 0;
             margin: 0;
         }
         .photo-context-separator {
             min-height: 1px;
             padding: 0;
             margin: 4px 8px;
         }"
    );
    gtk::style_context_add_provider_for_display(
        &host.display(),
        &css,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    let menu = gtk::Box::new(gtk::Orientation::Vertical, 0);
    menu.set_width_request(236);
    menu.set_margin_top(0);
    menu.set_margin_bottom(0);
    menu.set_margin_start(0);
    menu.set_margin_end(0);

    let add_action = |text: &str| {
        let button = gtk::Button::new();
        button.set_focus_on_click(false);
        button.set_halign(gtk::Align::Fill);
        button.set_height_request(28);
        button.set_margin_top(0);
        button.set_margin_bottom(0);
        button.add_css_class("flat");
        button.add_css_class("photo-context-item");

        let label = gtk::Label::new(Some(text));
        label.set_xalign(0.0);
        label.set_hexpand(true);
        button.set_child(Some(&label));

        menu.append(&button);
        button
    };

    let menu_to_hide: Rc<RefCell<Option<gtk::Widget>>> = Rc::new(RefCell::new(None));
    let dismiss_menu: Rc<dyn Fn()> = {
        let menu_to_hide = menu_to_hide.clone();
        Rc::new(move || {
            if let Some(menu) = menu_to_hide.borrow_mut().take() {
                menu.set_visible(false);
                if menu.parent().is_some() {
                    menu.unparent();
                }
            }
            ACTIVE_PHOTO_MENU.with(|active| {
                active.borrow_mut().take();
            });
        })
    };

    let open = add_action("Open");
    let edit = add_action("Edit Photo…");
    let open_with = add_action("Open With…");
    let open_in_folder = (!matches!(
        context.filter.get(),
        sidebar::SidebarFilter::Albums
    ))
    .then(|| add_action("Open in Folder"));
    {
        let separator = gtk::Separator::new(gtk::Orientation::Horizontal);
        separator.add_css_class("photo-context-separator");
        menu.append(&separator);
    }

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
    let add_to_album = context_submenu_button("Add to Album", "photo-context-album");
    let album_scroll_y = context
        .gallery
        .borrow()
        .upgrade()
        .map(|gallery| gallery.scroll_position())
        .unwrap_or_default();
    let album_photo_id = photo.id();
    let album_gallery = context.gallery.clone();
    let restore_album_view: Rc<dyn Fn()> = Rc::new(move || {
        if let Some(gallery) = album_gallery.borrow().upgrade() {
            gallery.restore_context_view(album_photo_id, album_scroll_y);
        }
    });
    let album_popover = build_album_popover(
        context.clone(),
        selection_provider.clone(),
        {
            dismiss_menu.clone()
        },
        restore_album_view.clone(),
    );
    unfocus_submenu(&album_popover);
    add_to_album.set_popover(Some(&album_popover));
    menu.append(&add_to_album);

    let collage_ids = selection_ids.clone();
    let collage = add_action("Create Collage…");
    collage.set_sensitive(collage_ids.len() >= 2);
    let collage_context = context.clone();
    let dismiss_menu_for_collage = dismiss_menu.clone();
    collage.connect_clicked(move |_| {
        dismiss_menu_for_collage();
        if std::env::var_os("PICASA_TRACE").is_some() {
            eprintln!("COLLAGE TRACE open ids={:?}", collage_ids);
        }
        (collage_context.open_collage)(collage_ids.clone());
    });

    {
        let separator = gtk::Separator::new(gtk::Orientation::Horizontal);
        separator.add_css_class("photo-context-separator");
        menu.append(&separator);
    }
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
        let dismiss_menu = dismiss_menu.clone();
        copy_edits.connect_clicked(move |_| {
            clipboard.replace(Some(recipe.clone()));
            dismiss_menu();
        });
    }
    {
        let paste_context = context.clone();
        let paste_selection = selection_ids.clone();
        let dismiss_menu = dismiss_menu.clone();
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
            if let Some(lightbox) = paste_context.lightbox.upgrade() {
                lightbox.refresh_current();
            }
            dismiss_menu();
        });
    }
    {
        let reset_context = context.clone();
        let reset_selection = selection_ids.clone();
        let dismiss_menu = dismiss_menu.clone();
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
            if let Some(lightbox) = reset_context.lightbox.upgrade() {
                lightbox.refresh_current();
            }
            dismiss_menu();
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
    let dismiss_menu_for_favorite = dismiss_menu.clone();
    favorite.connect_clicked(move |button| {
        let target = !favorite_photo.favorite();
        let ids = favorite_selection();
        for id in &ids {
            if let Err(error) = db::set_favorite(
                &favorite_context.connection.borrow(),
                *id,
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
        dismiss_menu_for_favorite();
        if let Some(gallery) = favorite_context.gallery.borrow().upgrade() {
            if favorite_context.filter.get() == sidebar::SidebarFilter::Favorites && !target {
                gallery.remove_photos(&ids);
                if let Some(lightbox) = favorite_context.lightbox.upgrade() {
                    if lightbox.root.is_visible() {
                        for id in &ids {
                            lightbox.remove_photo(*id);
                        }
                    }
                }
            } else {
                gallery.update_favorites(&ids, target);
            }
        }
        refresh_favorite_sidebar(&favorite_context);
    });

    if let sidebar::SidebarFilter::Album(album_id) = context.filter.get() {
        // The cover only makes sense inside an album, and always applies to
        // the album being viewed: no album list, no cross-album choices.
        let already_cover = db::albums(&context.connection.borrow())
            .unwrap_or_default()
            .into_iter()
            .find(|album| album.id == album_id)
            .and_then(|album| album.cover_photo_id)
            == Some(photo.id());
        let set_cover = add_action("Set as Album Cover");
        set_cover.set_sensitive(!already_cover);
        let set_cover_context = context.clone();
        let set_cover_photo_id = photo.id();
        let dismiss_menu_for_cover = dismiss_menu.clone();
        set_cover.connect_clicked(move |button| {
            dismiss_menu_for_cover();
            if let Err(error) = db::set_album_cover_photo(
                &set_cover_context.connection.borrow(),
                album_id,
                set_cover_photo_id,
            ) {
                show_error(
                    button.upcast_ref(),
                    "Could not set album cover",
                    &error.to_string(),
                );
                return;
            }
            refresh_album_ui(&set_cover_context);
        });

        let remove = add_action("Remove from Album");
        let remove_context = context.clone();
        let remove_selection = selection_provider.clone();
        let dismiss_menu_for_remove = dismiss_menu.clone();
        remove.connect_clicked(move |button| {
            dismiss_menu_for_remove();
            let ids = remove_selection();
            if let Err(error) = db::remove_photos_from_album(
                &remove_context.connection.borrow(),
                album_id,
                &ids,
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
            if let Some(gallery) = remove_context.gallery.borrow().upgrade() {
                gallery.remove_photos(&ids);
            }
            refresh_album_ui(&remove_context);
        });
    }
    {
        let separator = gtk::Separator::new(gtk::Orientation::Horizontal);
        separator.add_css_class("photo-context-separator");
        menu.append(&separator);
    }
    let copy = add_action("Copy");
    let copy_location = add_action("Copy File Location");
    {
        let separator = gtk::Separator::new(gtk::Orientation::Horizontal);
        separator.add_css_class("photo-context-separator");
        menu.append(&separator);
    }
    let move_file = add_action("Move…");
    let rename = add_action("Rename…");
    let file_manager = add_action("Open in File Manager");
    let wallpaper = add_action("Set as Wallpaper");
    let print = add_action("Print");
    let properties = add_action("Properties");
    {
        let separator = gtk::Separator::new(gtk::Orientation::Horizontal);
        separator.add_css_class("photo-context-separator");
        menu.append(&separator);
    }
    let delete = add_action("Delete");
    delete.add_css_class("destructive-action");

    let edit_context = context.clone();
    let edit_id = photo.id();
    let dismiss_menu_for_edit = dismiss_menu.clone();
    edit.connect_clicked(move |_| {
        dismiss_menu_for_edit();
        (edit_context.open_edit)(edit_id);
    });

    let photo_for_open = photo.clone();
    let dismiss_menu_for_open = dismiss_menu.clone();
    let lightbox_for_open = context.lightbox.clone();
    open.connect_clicked(move |_| {
        if let Some(lightbox) = lightbox_for_open.upgrade() {
            lightbox.open(vec![photo_for_open.clone()], 0);
        }
        dismiss_menu_for_open();
    });

    let file = crate::source::file(&photo.path());
    let open_with_file = file.clone();
    let open_with_window = context.window.clone();
    let dismiss_menu_for_open_with = dismiss_menu.clone();
    open_with.connect_clicked(move |_| {
        // The menu is removed before the chooser is presented, so never use
        // the menu button itself as the dialog parent.  It is unparented at
        // that point and AdwAlertDialog can fall back to a corner placement.
        dismiss_menu_for_open_with();
        if let Some(window) = open_with_window.upgrade() {
            show_open_with_dialog(window.upcast_ref(), &open_with_file);
        }
    });

    if let Some(open_in_folder) = open_in_folder {
        let folder_id = photo.folder_id();
        let navigate_to_folder = context.navigate_to_folder.clone();
        let dismiss_menu_for_folder = dismiss_menu.clone();
        let photo_id = photo.id();
        open_in_folder.connect_clicked(move |_| {
            if folder_id != 0 {
                navigate_to_folder(folder_id, photo_id);
            }
            dismiss_menu_for_folder();
        });
    }

    let path_for_copy = photo.path();
    let display = anchor.display();
    let dismiss_menu_for_copy = dismiss_menu.clone();
    copy_location.connect_clicked(move |_| {
        display.clipboard().set_text(&path_for_copy);
        dismiss_menu_for_copy();
    });

    let file_for_manager = file.clone();
    let dismiss_menu_for_manager = dismiss_menu.clone();
    file_manager.connect_clicked(move |_| {
        open_file_in_manager(&file_for_manager);
        dismiss_menu_for_manager();
    });

    let wallpaper_path = photo.path();
    let wallpaper_rotation = photo.rotation();
    let wallpaper_recipe = photo.edit_recipe();
    let wallpaper_width = photo.width();
    let wallpaper_height = photo.height();
    let wallpaper_mtime = photo.mtime();
    let wallpaper_size = photo.size_bytes();
    let wallpaper_window = context.window.clone();
    let dismiss_menu_for_wallpaper = dismiss_menu.clone();
    wallpaper.connect_clicked(move |_| {
        dismiss_menu_for_wallpaper();
        let target_size = wallpaper_window
            .upgrade()
            .and_then(|window| {
                let surface = window.surface()?;
                let display = gtk::prelude::WidgetExt::display(&window);
                let monitor = display.monitor_at_surface(&surface)?;
                let geometry = monitor.geometry();
                let scale = f64::from(monitor.scale_factor().max(1));
                Some((
                    (f64::from(geometry.width()) * scale).round() as u32,
                    (f64::from(geometry.height()) * scale).round() as u32,
                ))
            })
            .filter(|(width, height)| *width > 0 && *height > 0)
            .unwrap_or((1920, 1080));
        let path = wallpaper_path.clone();
        let recipe = wallpaper_recipe.clone();
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let result = prepare_wallpaper(
                &path,
                wallpaper_rotation,
                &recipe,
                wallpaper_width,
                wallpaper_height,
                wallpaper_mtime,
                wallpaper_size,
                target_size,
            )
            .map_err(|error| error.to_string());
            let _ = sender.send(result);
        });

        let window = wallpaper_window.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(50), move || {
            match receiver.try_recv() {
                Ok(Ok(path)) => {
                    if let Err(error) = apply_wallpaper(&path) {
                        if let Some(window) = window.upgrade() {
                            show_error(
                                window.upcast_ref(),
                                "Could not set wallpaper",
                                &error.to_string(),
                            );
                        }
                    }
                    glib::ControlFlow::Break
                }
                Ok(Err(error)) => {
                    if let Some(window) = window.upgrade() {
                        show_error(window.upcast_ref(), "Could not set wallpaper", &error);
                    }
                    glib::ControlFlow::Break
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
            }
        });
    });

    // These actions are not implemented yet; do not present them as working
    // commands that merely close the menu.
    copy.set_sensitive(false);
    move_file.set_sensitive(false);
    print.set_sensitive(false);

    let photo_for_rename = photo.clone();
    let anchor_for_rename = anchor.clone();
    let context_for_rename = context.clone();
    let dismiss_menu_for_rename = dismiss_menu.clone();
    rename.connect_clicked(move |_| {
        dismiss_menu_for_rename();
        show_rename_dialog(
            &anchor_for_rename,
            photo_for_rename.clone(),
            context_for_rename.clone(),
        );
    });

    let photo_for_properties = photo.clone();
    let anchor_for_properties = anchor.clone();
    let context_for_properties = context.clone();
    let dismiss_menu_for_properties = dismiss_menu.clone();
    properties.connect_clicked(move |_| {
        dismiss_menu_for_properties();
        show_properties_dialog(
            &anchor_for_properties,
            &photo_for_properties,
            &context_for_properties,
        );
    });

    let photo_for_delete = photo;
    let anchor_for_delete = anchor.clone();
    let context_for_delete = context;
    let dismiss_menu_for_delete = dismiss_menu.clone();
    delete.connect_clicked(move |_| {
        dismiss_menu_for_delete();
        show_delete_confirmation(
            &anchor_for_delete,
            photo_for_delete.clone(),
            context_for_delete.clone(),
        );
    });

    // Render the same menu UI for both grid thumbnails and lightbox photos.
    // A normal overlay child is always allocated by GtkOverlay, so there is
    // no GtkPopover 0x0/popup lifecycle to fail.
    let menu_host = gtk::ScrolledWindow::new();
    menu_host.add_css_class("photo-context-menu");
    menu_host.add_css_class("card");
    menu_host.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    menu_host.set_has_frame(false);
    menu_host.set_propagate_natural_height(true);
    // A context menu should never expand into a near full-height panel.
    // Keep enough room for the common actions and scroll only the overflow.
    menu_host.set_max_content_height((host.height() - 32).clamp(1, 460));
    menu_host.set_width_request(272);
    menu_host.set_halign(gtk::Align::Start);
    menu_host.set_valign(gtk::Align::Start);
    menu_host.set_can_target(true);
    menu_host.set_child(Some(&menu));

    // Centre the menu on the pointer instead of treating the pointer as the
    // menu's top-left corner.  Measure after the complete menu has been built
    // so the centring also follows dynamic items such as album actions.
    let (_, natural_width, _, _) = menu_host.measure(gtk::Orientation::Horizontal, -1);
    let measured_width = natural_width.max(272).min(host.width().max(1));
    let (_, natural_height, _, _) =
        menu_host.measure(gtk::Orientation::Vertical, measured_width);
    let measured_height = natural_height
        .max(1)
        .min(menu_host.max_content_height().max(1));

    let menu_x = (click_point.x().round() as i32 - measured_width / 2)
        .clamp(0, (host.width() - measured_width).max(0));
    let menu_y = (click_point.y().round() as i32 - measured_height / 2)
        .clamp(0, (host.height() - measured_height).max(0));
    menu_host.set_margin_start(menu_x);
    menu_host.set_margin_top(menu_y);

    let menu_widget = menu_host.clone().upcast::<gtk::Widget>();
    menu_to_hide.borrow_mut().replace(menu_widget.clone());
    ACTIVE_PHOTO_MENU.with(|active| {
        active.borrow_mut().replace(menu_widget);
    });

    if std::env::var_os("PICASA_TRACE").is_some() {
        eprintln!(
            "UI TRACE photo_context_menu_show host=GtkOverlay anchor={} host_size={}x{} point=({:.1},{:.1}) menu=({}, {}) measured={}x{}",
            anchor.type_().name(),
            host.width(),
            host.height(),
            click_point.x(),
            click_point.y(),
            menu_x,
            menu_y,
            measured_width,
            measured_height
        );
    }

    host.add_overlay(&menu_host);
    menu_host.set_visible(true);
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

fn prepare_wallpaper(
    reference: &str,
    rotation: i32,
    edit_recipe: &str,
    source_width: i64,
    source_height: i64,
    mtime: i64,
    size_bytes: i64,
    target_size: (u32, u32),
) -> anyhow::Result<std::path::PathBuf> {
    let source_path = crate::source::materialize(reference)?;
    std::fs::canonicalize(&source_path).map_err(|error| {
        anyhow::anyhow!(
            "Could not access {}: {error}",
            source_path.to_string_lossy()
        )
    })?;

    let directory = crate::thumbnail::cache_dir()?.join("wallpaper");
    std::fs::create_dir_all(&directory)?;
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"picasa-wallpaper-v2-fit\0");
    hasher.update(reference.as_bytes());
    hasher.update(b"\0");
    hasher.update(mtime.to_string().as_bytes());
    hasher.update(b"\0");
    hasher.update(size_bytes.to_string().as_bytes());
    hasher.update(b"\0");
    hasher.update(rotation.to_string().as_bytes());
    hasher.update(b"\0");
    hasher.update(edit_recipe.as_bytes());
    hasher.update(b"\0");
    hasher.update(target_size.0.to_string().as_bytes());
    hasher.update(b"x");
    hasher.update(target_size.1.to_string().as_bytes());
    let destination = directory.join(format!("{}.jpg", hasher.finalize().to_hex()));
    if !destination.is_file() {
        let image = crate::edit::render::render_for_export(
            reference,
            rotation,
            edit_recipe,
            source_width,
            source_height,
        )?;
        let wallpaper = compose_wallpaper(image, target_size.0, target_size.1);
        crate::edit::render::save_jpeg(&wallpaper, &destination, 95)?;
    }
    Ok(destination)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WallpaperLayout {
    Cover,
    PortraitBlur,
    PanoramaBlur,
}

fn wallpaper_layout(
    source_width: u32,
    source_height: u32,
    target_width: u32,
    target_height: u32,
) -> WallpaperLayout {
    if source_width < source_height {
        return WallpaperLayout::PortraitBlur;
    }
    let source_ratio = source_width as f64 / source_height.max(1) as f64;
    let target_ratio = target_width.max(1) as f64 / target_height.max(1) as f64;
    if source_ratio > target_ratio * 1.45 {
        WallpaperLayout::PanoramaBlur
    } else {
        WallpaperLayout::Cover
    }
}

fn compose_wallpaper(
    source: image::RgbaImage,
    target_width: u32,
    target_height: u32,
) -> image::RgbaImage {
    let target_width = target_width.max(1);
    let target_height = target_height.max(1);
    let layout = wallpaper_layout(
        source.width(),
        source.height(),
        target_width,
        target_height,
    );
    let source = image::DynamicImage::ImageRgba8(source);
    if layout == WallpaperLayout::Cover {
        return source
            .resize_to_fill(
                target_width,
                target_height,
                image::imageops::FilterType::Lanczos3,
            )
            .to_rgba8();
    }

    let preview_width = (target_width / 8).max(1);
    let preview_height = (target_height / 8).max(1);
    let background = source
        .resize_to_fill(
            preview_width,
            preview_height,
            image::imageops::FilterType::Triangle,
        )
        .to_rgba8();
    let background = image::imageops::blur(&background, 8.0);
    let mut canvas = image::DynamicImage::ImageRgba8(background)
        .resize_exact(
            target_width,
            target_height,
            image::imageops::FilterType::CatmullRom,
        )
        .to_rgba8();

    let foreground = if layout == WallpaperLayout::PanoramaBlur {
        // Keep at least 80% of a panorama. A small centre crop reduces the
        // letterbox area while retaining the wide composition.
        let crop_width = ((source.width() as f64 * 0.8).round() as u32)
            .max(1)
            .min(source.width());
        let left = source.width().saturating_sub(crop_width) / 2;
        source.crop_imm(left, 0, crop_width, source.height())
    } else {
        source
    };
    let foreground = foreground
        .resize(
            target_width,
            target_height,
            image::imageops::FilterType::Lanczos3,
        )
        .to_rgba8();
    let x = i64::from(target_width.saturating_sub(foreground.width()) / 2);
    let y = i64::from(target_height.saturating_sub(foreground.height()) / 2);
    image::imageops::overlay(&mut canvas, &foreground, x, y);
    canvas
}

fn apply_wallpaper(path: &std::path::Path) -> anyhow::Result<()> {
    let uri = gio::File::for_path(path).uri();

    let schema_source = gio::SettingsSchemaSource::default()
        .ok_or_else(|| anyhow::anyhow!("Desktop wallpaper settings are unavailable."))?;
    let schema = schema_source
        .lookup("org.gnome.desktop.background", true)
        .ok_or_else(|| anyhow::anyhow!("This desktop does not support setting the wallpaper here."))?;
    if !schema.has_key("picture-uri") {
        anyhow::bail!("This desktop does not expose a wallpaper setting.");
    }

    let settings = gio::Settings::new_full(
        &schema,
        None::<&gio::SettingsBackend>,
        None,
    );
    settings.set_string("picture-uri", uri.as_str())?;
    if schema.has_key("picture-uri-dark") {
        settings.set_string("picture-uri-dark", uri.as_str())?;
    }
    if schema.has_key("picture-options") {
        settings.set_string("picture-options", "zoom")?;
    }
    gio::Settings::sync();
    Ok(())
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

#[cfg(test)]
mod photo_actions_tests {
    use super::*;

    fn settle_gtk_layout() {
        let main_loop = gtk::glib::MainLoop::new(None, false);
        let loop_to_quit = main_loop.clone();
        gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(50), move || {
            loop_to_quit.quit();
        });
        main_loop.run();
    }

    fn find_action(root: &gtk::Widget, text: &str) -> Option<gtk::Button> {
        if let Some(button) = root.downcast_ref::<gtk::Button>() {
            let label = button.label().map(|label| label.to_string()).or_else(|| {
                button
                    .child()
                    .and_then(|child| child.downcast::<gtk::Label>().ok())
                    .map(|label| label.text().to_string())
            });
            if label.as_deref() == Some(text) {
                return Some(button.clone());
            }
        }
        let mut child = root.first_child();
        while let Some(widget) = child {
            let next = widget.next_sibling();
            if let Some(found) = find_action(&widget, text) {
                return Some(found);
            }
            child = next;
        }
        None
    }

    fn menu() -> gtk::Widget {
        ACTIVE_PHOTO_MENU
            .with(|active| active.borrow().clone())
            .expect("photo context menu is showing")
    }

    #[test]
    #[ignore = "requires a GTK display; run with --ignored --test-threads=1"]
    fn album_cover_action_is_only_offered_inside_an_album() {
        gtk::init().unwrap();
        let directory = std::env::temp_dir().join(format!(
            "pic-photo-context-cover-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let connection = Rc::new(RefCell::new(
            db::open(&directory.join("library.db")).unwrap(),
        ));
        connection
            .borrow()
            .execute_batch(
                "INSERT INTO photos (id, path) VALUES (1, 'samples/01-Start Up.jpg');",
            )
            .unwrap();
        let album = db::create_album(&connection.borrow(), "Holiday").unwrap();
        db::add_photos_to_album(&connection.borrow(), album.id, &[1]).unwrap();
        let photo = db::photo(&connection.borrow(), 1).unwrap().unwrap();
        let photo_object = crate::photo_object::PhotoObject::from_photo(&photo);

        let overlay = gtk::Overlay::new();
        let window = gtk::Window::builder()
            .default_width(640)
            .default_height(480)
            .child(&overlay)
            .build();
        window.present();
        settle_gtk_layout();

        let context_for = |filter: sidebar::SidebarFilter| PhotoActionContext {
            connection: connection.clone(),
            gallery: Rc::new(RefCell::new(std::rc::Weak::new())),
            filter: Rc::new(Cell::new(filter)),
            search: Rc::new(RefCell::new(String::new())),
            sort: Rc::new(Cell::new(PhotoSort {
                field: SortField::DateTaken,
                direction: SortDirection::Descending,
            })),
            info: Rc::new(InfoBar::new()),
            selected_photo: Rc::new(RefCell::new(None)),
            lightbox: std::rc::Weak::new(),
            sidebar: Rc::new(RefCell::new(None)),
            create_album: Rc::new(|| {}),
            import_folder: Rc::new(|| {}),
            delete_album: Rc::new(|_| {}),
            on_unavailable: Rc::new(|| {}),
            refresh_albums_home: Rc::new(|_| {}),
            navigate_to_folder: Rc::new(|_, _| {}),
            open_collage: Rc::new(|_| {}),
            open_edit: Rc::new(|_| {}),
            edit_clipboard: Rc::new(RefCell::new(None)),
            window: glib::WeakRef::new(),
            context_menu_host: Rc::new(RefCell::new(Some(overlay.clone().downgrade()))),
        };

        // Inside the album the action targets the album being viewed, with no
        // album list and no "Add to Album"-style chooser.
        show_photo_context_menu(
            photo_object.clone(),
            overlay.clone().upcast(),
            context_for(sidebar::SidebarFilter::Album(album.id)),
            10.0,
            10.0,
        );
        let open_menu = menu();
        assert!(find_action(&open_menu, "Set as Album Cover").is_some());
        assert!(find_action(&open_menu, "Remove from Album").is_some());

        find_action(&open_menu, "Set as Album Cover")
            .unwrap()
            .emit_clicked();
        assert_eq!(
            db::albums(&connection.borrow()).unwrap().remove(0).cover_photo_id,
            Some(1)
        );

        // The action greys out once the photo already is the album's cover.
        show_photo_context_menu(
            photo_object.clone(),
            overlay.clone().upcast(),
            context_for(sidebar::SidebarFilter::Album(album.id)),
            10.0,
            10.0,
        );
        assert!(
            !find_action(&menu(), "Set as Album Cover")
                .unwrap()
                .is_sensitive()
        );
        dismiss_active_photo_context_menu();

        // Outside an album the cover action is not offered at all.
        show_photo_context_menu(
            photo_object,
            overlay.clone().upcast(),
            context_for(sidebar::SidebarFilter::All),
            10.0,
            10.0,
        );
        let open_menu = menu();
        assert!(find_action(&open_menu, "Set as Album Cover").is_none());
        assert!(find_action(&open_menu, "Remove from Album").is_none());
        dismiss_active_photo_context_menu();

        window.close();
        drop(connection);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
