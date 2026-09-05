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

pub fn build(app: &adw::Application, connection: Connection) -> adw::ApplicationWindow {
    let window = adw::ApplicationWindow::new(app);
    window.set_title(Some("PIC - Picasa iPhoto Clone"));
    window.set_default_size(1440, 900);

    let connection = Rc::new(RefCell::new(connection));
    let folders = db::folders(&connection.borrow()).unwrap_or_default();
    let albums = db::albums(&connection.borrow()).unwrap_or_default();
    let sidebar_counts = db::sidebar_counts(&connection.borrow()).unwrap_or_default();
    let sort = Rc::new(Cell::new(PhotoSort {
        field: SortField::from_key(
            &db::setting(&connection.borrow(), SORT_FIELD_SETTING_KEY)
                .ok()
                .flatten()
                .unwrap_or_default(),
        ),
        direction: SortDirection::from_key(
            &db::setting(&connection.borrow(), SORT_DIRECTION_SETTING_KEY)
                .ok()
                .flatten()
                .unwrap_or_default(),
        ),
    }));
    let saved_group_mode = group_mode_from_key(
        &db::setting(&connection.borrow(), GROUP_MODE_SETTING_KEY)
            .ok()
            .flatten()
            .unwrap_or_default(),
    );
    let group_mode = Rc::new(Cell::new(if group_date_for_sort(sort.get()).is_some() {
        saved_group_mode
    } else {
        grid::GroupMode::None
    }));
    let mut photos = db::photos(&connection.borrow(), None, false, None).unwrap_or_default();
    retain_enabled_formats(&connection.borrow(), &mut photos);
    sort_photos(&mut photos, sort.get());

    let info = Rc::new(InfoBar::new());
    info.set_photo(None);

    let selected_photo = Rc::new(RefCell::new(None));

    let lightbox = Rc::new(Lightbox::new());
    let info_for_lightbox = info.clone();
    let selected_photo_for_lightbox = selected_photo.clone();
    lightbox.set_photo_changed_handler(move |photo| {
        info_for_lightbox.set_photo(Some(&photo));
        // Navigating to another photo restores the viewer's normal fit state.
        info_for_lightbox.one_to_one.set_active(false);
        selected_photo_for_lightbox.replace(Some(photo));
    });

    // Space opens the fullscreen viewer from the gallery. While the viewer
    // is open, it toggles between fit and 1:1 viewing.
    // The actual open action is installed after Gallery exists.
    let space_open_slot: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));
    let space_toggle_in_progress = Rc::new(Cell::new(false));

    // Handle viewer keyboard shortcuts at the window boundary as well as
    // inside the lightbox. Capture prevents GtkGridView from interpreting
    // Space as a selection toggle.
    let window_escape = gtk::EventControllerKey::new();
    window_escape.set_propagation_phase(gtk::PropagationPhase::Capture);
    let lightbox_for_window_escape = lightbox.clone();
    let space_open_slot_for_key = space_open_slot.clone();
    let one_to_one_for_key = info.one_to_one.clone();
    let space_toggle_in_progress_for_key = space_toggle_in_progress.clone();
    let window_for_fullscreen_key = window.clone();
    window_escape.connect_key_pressed(move |_, key, _, _| {
        if (key == gtk::gdk::Key::Escape || key == gtk::gdk::Key::BackSpace)
            && lightbox_for_window_escape.root.is_visible()
        {
            if std::env::var_os("PICASA_TRACE").is_some() {
                eprintln!("UI TRACE lightbox_escape_window_close");
            }
            lightbox_for_window_escape.close();
            glib::Propagation::Stop
        } else if key == gtk::gdk::Key::F11 {
            if window_for_fullscreen_key.is_fullscreen() {
                window_for_fullscreen_key.unfullscreen();
            } else {
                window_for_fullscreen_key.fullscreen();
            }
            glib::Propagation::Stop
        } else if lightbox_for_window_escape.root.is_visible()
            && (key == gtk::gdk::Key::Left || key == gtk::gdk::Key::Right)
        {
            lightbox_for_window_escape.navigate_photo(if key == gtk::gdk::Key::Left { -1 } else { 1 });
            glib::Propagation::Stop
        } else if lightbox_for_window_escape.root.is_visible()
            && (key == gtk::gdk::Key::Up || key == gtk::gdk::Key::Down)
        {
            lightbox_for_window_escape.navigate_collection(if key == gtk::gdk::Key::Up { -1 } else { 1 });
            glib::Propagation::Stop
        } else if key == gtk::gdk::Key::space {
            if lightbox_for_window_escape.root.is_visible() {
                if one_to_one_for_key.is_active() {
                    space_toggle_in_progress_for_key.set(true);
                    one_to_one_for_key.set_active(false);
                    if std::env::var_os("PICASA_TRACE").is_some() {
                        eprintln!("UI TRACE lightbox_space_fit");
                    }
                } else {
                    space_toggle_in_progress_for_key.set(true);
                    one_to_one_for_key.set_active(true);
                    if std::env::var_os("PICASA_TRACE").is_some() {
                        eprintln!("UI TRACE lightbox_space_one_to_one");
                    }
                }
            } else if let Some(open_selected) = space_open_slot_for_key.borrow().as_ref() {
                open_selected();
                if std::env::var_os("PICASA_TRACE").is_some() {
                    eprintln!("UI TRACE lightbox_space_open_selected");
                }
            }
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    window.add_controller(window_escape);

    let info_for_grid = info.clone();
    let selected_photo_for_grid = selected_photo.clone();
    let lightbox_for_grid = lightbox.clone();
    let filter = Rc::new(Cell::new(sidebar::SidebarFilter::All));
    let search_text = Rc::new(RefCell::new(String::new()));
    let search_entry_slot: Rc<RefCell<Option<gtk::SearchEntry>>> = Rc::new(RefCell::new(None));
    let search_suppressed = Rc::new(Cell::new(false));
    let search_debounce: Rc<RefCell<Option<glib::SourceId>>> = Rc::new(RefCell::new(None));
    let gallery_for_actions: Rc<RefCell<Weak<grid::Gallery>>> = Rc::new(RefCell::new(Weak::new()));
    let sidebar_for_unavailable: Rc<RefCell<Option<gtk::ScrolledWindow>>> =
        Rc::new(RefCell::new(None));
    let sidebar_selection_slot: Rc<RefCell<Option<gtk::ScrolledWindow>>> =
        Rc::new(RefCell::new(None));
    let create_album_slot: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));
    let create_album: Rc<dyn Fn()> = {
        let slot = create_album_slot.clone();
        Rc::new(move || {
            if let Some(callback) = slot.borrow().as_ref() {
                callback();
            }
        })
    };
    let import_folder_slot: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));
    let refresh_folder_slot: Rc<RefCell<Option<Rc<dyn Fn(String)>>>> =
        Rc::new(RefCell::new(None));
    let import_folder: Rc<dyn Fn()> = {
        let slot = import_folder_slot.clone();
        Rc::new(move || {
            if let Some(callback) = slot.borrow().as_ref() {
                callback();
            }
        })
    };
    let delete_album_slot: Rc<RefCell<Option<Rc<dyn Fn(i64)>>>> = Rc::new(RefCell::new(None));
    let delete_album: Rc<dyn Fn(i64)> = {
        let slot = delete_album_slot.clone();
        Rc::new(move |album_id| {
            if let Some(callback) = slot.borrow().as_ref() {
                callback(album_id);
            }
        })
    };
    let availability_refresh_slot: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));
    let availability_refresh: Rc<dyn Fn()> = {
        let connection = connection.clone();
        let gallery = gallery_for_actions.clone();
        let sidebar = sidebar_for_unavailable.clone();
        let slot = availability_refresh_slot.clone();
        let create_album = create_album.clone();
        let import_folder = import_folder.clone();
        let delete_album = delete_album.clone();
        Rc::new(move || {
            refresh_availability_ui(
                &connection,
                &gallery,
                &sidebar,
                &slot,
                create_album.clone(),
                import_folder.clone(),
                delete_album.clone(),
            );
        })
    };
    availability_refresh_slot.replace(Some(availability_refresh.clone()));
    let albums_home_refresh_slot: Rc<RefCell<Option<Rc<dyn Fn(&[db::Album])>>>> =
        Rc::new(RefCell::new(None));
    let album_home_click_slot: Rc<RefCell<Option<Rc<dyn Fn(i64)>>>> =
        Rc::new(RefCell::new(None));
    let action_context = PhotoActionContext {
        connection: connection.clone(),
        gallery: gallery_for_actions.clone(),
        filter: filter.clone(),
        search: search_text.clone(),
        sort: sort.clone(),
        info: info.clone(),
        selected_photo: selected_photo.clone(),
        lightbox: Rc::downgrade(&lightbox),
        sidebar: sidebar_for_unavailable.clone(),
        create_album: create_album.clone(),
        import_folder: import_folder.clone(),
        delete_album: delete_album.clone(),
        on_unavailable: availability_refresh.clone(),
        refresh_albums_home: {
            let slot = albums_home_refresh_slot.clone();
            Rc::new(move |albums| {
                if let Some(refresh) = slot.borrow().as_ref() {
                    refresh(albums);
                }
            })
        },
    };
    let grid_thumbnail_size = grid_thumbnail_size_from_setting(&connection.borrow());

    let gallery = Rc::new(grid::Gallery::new(
        &photos,
        grid_thumbnail_size,
        move |photo| {
            info_for_grid.set_photo(photo.as_ref());
            selected_photo_for_grid.replace(photo);
        },
        {
            let availability_refresh = availability_refresh.clone();
            move |photos, selected_index| {
                if photos
                    .get(selected_index)
                    .is_some_and(|photo| !photo.original_available())
                {
                    availability_refresh();
                }
                lightbox_for_grid.open(photos, selected_index);
            }
        },
        {
            let action_context = action_context.clone();
            move |photo, anchor| {
                show_photo_context_menu(photo, anchor, action_context.clone(), 0.0, 0.0);
            }
        },
        {
            let connection = connection.clone();
            let availability_refresh = availability_refresh.clone();
            move |photo, anchor| {
                show_unavailable_dialog(
                    &anchor,
                    photo,
                    connection.clone(),
                    availability_refresh.clone(),
                );
            }
        },
        {
            let connection = connection.clone();
            move |width| {
                if let Err(error) = db::set_setting(
                    &connection.borrow(),
                    GRID_THUMBNAIL_SIZE_SETTING_KEY,
                    &width.to_string(),
                ) {
                    eprintln!("Could not save grid thumbnail size: {error}");
                }
            }
        },
    ));
    gallery_for_actions.replace(Rc::downgrade(&gallery));
    apply_gallery_grouping(&gallery, filter.get(), sort.get(), group_mode.get());

    // Complete the Space shortcut now that the gallery/current filter state
    // exists. Rebuild the same current photo collection used by the grid, then
    // open the currently selected photo at its matching index.
    {
        let connection = connection.clone();
        let filter = filter.clone();
        let search = search_text.clone();
        let sort = sort.clone();
        let selected_photo = selected_photo.clone();
        let lightbox = lightbox.clone();
        let availability_refresh = availability_refresh.clone();

        space_open_slot.replace(Some(Rc::new(move || {
            let Some(selected) = selected_photo.borrow().clone() else {
                return;
            };

            if !selected.original_available() {
                availability_refresh();
            }

            let search_text = search.borrow().clone();
            let current_filter = filter.get();

            let mut photos = match current_filter {
                sidebar::SidebarFilter::Albums => Vec::new(),
                sidebar::SidebarFilter::Album(album_id) => db::photos_in_album(
                    &connection.borrow(),
                    album_id,
                    (!search_text.is_empty()).then_some(search_text.as_str()),
                )
                .unwrap_or_default(),
                sidebar::SidebarFilter::Folder(folder_id) => db::photos(
                    &connection.borrow(),
                    Some(folder_id),
                    false,
                    (!search_text.is_empty()).then_some(search_text.as_str()),
                )
                .unwrap_or_default(),
                sidebar::SidebarFilter::Favorites => db::photos(
                    &connection.borrow(),
                    None,
                    true,
                    (!search_text.is_empty()).then_some(search_text.as_str()),
                )
                .unwrap_or_default(),
                sidebar::SidebarFilter::All | sidebar::SidebarFilter::RecentlyAdded => db::photos(
                    &connection.borrow(),
                    None,
                    false,
                    (!search_text.is_empty()).then_some(search_text.as_str()),
                )
                .unwrap_or_default(),
            };

            retain_enabled_formats(&connection.borrow(), &mut photos);
            sort_photos(&mut photos, sort.get());

            let Some(index) = photos.iter().position(|photo| photo.id == selected.id()) else {
                return;
            };

            let objects = photos
                .iter()
                .map(crate::photo_object::PhotoObject::from_photo)
                .collect::<Vec<_>>();
            lightbox.open(objects, index);
        })));
    }

    let create_context = action_context.clone();
    let create_parent = window.clone();
    create_album_slot.replace(Some(Rc::new(move || {
        show_create_album_dialog(
            create_parent.clone().upcast(),
            Vec::new(),
            create_context.clone(),
        );
    })));
    let delete_context = action_context.clone();
    let delete_parent = window.clone();
    delete_album_slot.replace(Some(Rc::new(move |album_id| {
        show_delete_album_confirmation(
            delete_parent.clone().upcast(),
            album_id,
            delete_context.clone(),
        );
    })));

    configure_infobar_album_menu(&info.add_to_album, action_context.clone());

    let settings_window = crate::settings::SettingsWindow::default();
    let settings_parent = window.clone();
    let settings_connection = connection.clone();
    let settings_gallery = gallery.clone();
    let settings_filter = filter.clone();
    let settings_search = search_text.clone();
    let settings_sort = sort.clone();
    let settings_lightbox = lightbox.clone();
    info.more.connect_clicked(move |_| {
        let connection = settings_connection.clone();
        let gallery = settings_gallery.clone();
        let filter = settings_filter.clone();
        let search = settings_search.clone();
        let sort = settings_sort.clone();
        let lightbox = settings_lightbox.clone();
        settings_window.present(
            &settings_parent,
            settings_connection.clone(),
            Rc::new(move || {
                lightbox.close();
                refresh_grid(
                    &connection,
                    filter.get(),
                    &search.borrow(),
                    sort.get(),
                    &gallery,
                );
            }),
        );
    });

    let action_context_for_lightbox = action_context.clone();
    lightbox.set_context_menu_handler(move |photo, anchor, x, y| {
        show_photo_context_menu(photo, anchor, action_context_for_lightbox.clone(), x, y);
    });

    // In fullscreen, Left/Right and the mouse wheel stay inside the currently
    // opened collection. Up/Down moves to the adjacent Folder when browsing a
    // folder, or to the adjacent Album when browsing an album. The window owns
    // collection ordering and database access; the lightbox only asks for a
    // direction.
    let connection_for_collection_nav = connection.clone();
    let filter_for_collection_nav = filter.clone();
    let search_for_collection_nav = search_text.clone();
    let sort_for_collection_nav = sort.clone();
    let group_mode_for_collection_nav = group_mode.clone();
    let gallery_for_collection_nav = gallery.clone();
    let lightbox_for_collection_nav = lightbox.clone();
    let sidebar_selection_for_collection_nav = sidebar_selection_slot.clone();
    lightbox.set_collection_navigation_handler(move |direction| {
        if direction == 0 {
            return;
        }

        let step: isize = if direction < 0 { -1 } else { 1 };
        let current_filter = filter_for_collection_nav.get();
        let search = search_for_collection_nav.borrow().clone();

        match current_filter {
            sidebar::SidebarFilter::Folder(current_folder_id) => {
                // db::folders() is already path-ordered, which matches the
                // sidebar's folder-tree traversal closely enough for keyboard
                // Up/Down navigation. Skip empty/filtered-out folders.
                let folders = db::folders(&connection_for_collection_nav.borrow())
                    .unwrap_or_default();
                let Some(current_index) = folders
                    .iter()
                    .position(|folder| folder.id == current_folder_id)
                else {
                    return;
                };

                let mut candidate = current_index as isize + step;
                while candidate >= 0 && candidate < folders.len() as isize {
                    let folder = &folders[candidate as usize];
                    let mut photos = db::photos(
                        &connection_for_collection_nav.borrow(),
                        Some(folder.id),
                        false,
                        (!search.is_empty()).then_some(search.as_str()),
                    )
                    .unwrap_or_default();
                    retain_enabled_formats(&connection_for_collection_nav.borrow(), &mut photos);
                    sort_photos(&mut photos, sort_for_collection_nav.get());

                    if !photos.is_empty() {
                        let new_filter = sidebar::SidebarFilter::Folder(folder.id);
                        filter_for_collection_nav.set(new_filter);
                        if let Some(sidebar) = sidebar_selection_for_collection_nav.borrow().as_ref() {
                            sidebar::set_active_filter(sidebar, new_filter);
                        }
                        apply_gallery_grouping(
                            &gallery_for_collection_nav,
                            new_filter,
                            sort_for_collection_nav.get(),
                            group_mode_for_collection_nav.get(),
                        );
                        gallery_for_collection_nav.replace(&photos);
                        let objects = photos
                            .iter()
                            .map(crate::photo_object::PhotoObject::from_photo)
                            .collect::<Vec<_>>();
                        lightbox_for_collection_nav.open(objects, 0);
                        return;
                    }

                    candidate += step;
                }
            }
            sidebar::SidebarFilter::Album(current_album_id) => {
                let albums = db::albums(&connection_for_collection_nav.borrow())
                    .unwrap_or_default();
                let Some(current_index) = albums
                    .iter()
                    .position(|album| album.id == current_album_id)
                else {
                    return;
                };

                let mut candidate = current_index as isize + step;
                while candidate >= 0 && candidate < albums.len() as isize {
                    let album = &albums[candidate as usize];
                    let mut photos = db::photos_in_album(
                        &connection_for_collection_nav.borrow(),
                        album.id,
                        (!search.is_empty()).then_some(search.as_str()),
                    )
                    .unwrap_or_default();
                    retain_enabled_formats(&connection_for_collection_nav.borrow(), &mut photos);
                    sort_photos(&mut photos, sort_for_collection_nav.get());

                    if !photos.is_empty() {
                        let new_filter = sidebar::SidebarFilter::Album(album.id);
                        filter_for_collection_nav.set(new_filter);
                        if let Some(sidebar) = sidebar_selection_for_collection_nav.borrow().as_ref() {
                            sidebar::set_active_filter(sidebar, new_filter);
                        }
                        apply_gallery_grouping(
                            &gallery_for_collection_nav,
                            new_filter,
                            sort_for_collection_nav.get(),
                            group_mode_for_collection_nav.get(),
                        );
                        gallery_for_collection_nav.replace(&photos);
                        let objects = photos
                            .iter()
                            .map(crate::photo_object::PhotoObject::from_photo)
                            .collect::<Vec<_>>();
                        lightbox_for_collection_nav.open(objects, 0);
                        return;
                    }

                    candidate += step;
                }
            }
            _ => {
                // Library destinations (Photos/Favourites/Recently Added) do
                // not have a folder/album Up/Down relationship.
            }
        }
    });

    gallery.root.add_css_class("photo-grid");

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);

    let grid_scroll = gtk::ScrolledWindow::new();
    grid_scroll.set_vexpand(true);
    grid_scroll.set_hexpand(true);
    // GtkGridView implements GtkScrollable and must remain the direct child of
    // GtkScrolledWindow. Wrapping it in a Box/Viewport caused the restored vs
    // maximized allocation bug and destroyed visible-item virtualization.
    grid_scroll.set_child(Some(&gallery.root));

    let gallery_for_group_scroll = gallery.clone();
    grid_scroll
        .vadjustment()
        .connect_value_changed(move |adjustment| {
            gallery_for_group_scroll.update_group_header_for_scroll(adjustment.value());
        });

    let grid_zoom_scroll =
        gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
    let gallery_for_zoom_scroll = gallery.clone();
    grid_zoom_scroll.connect_scroll(move |controller, _, dy| {
        if controller
            .current_event_state()
            .contains(gtk::gdk::ModifierType::CONTROL_MASK)
        {
            if dy < 0.0 {
                gallery_for_zoom_scroll.zoom_in();
            } else if dy > 0.0 {
                gallery_for_zoom_scroll.zoom_out();
            }
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    grid_scroll.add_controller(grid_zoom_scroll);

    // While the sidebar divider is being dragged, keep the gallery column
    // count fixed. Otherwise every few pixels can cross a column threshold
    // and GtkGridView repeatedly reflows all visible thumbnails, which looks
    // like the grid is juggling back and forth. The final width is applied
    // once when the drag ends.
    let sidebar_resize_active = Rc::new(Cell::new(false));
    let gallery_for_resize = gallery.clone();
    let sidebar_resize_active_for_tick = sidebar_resize_active.clone();
    grid_scroll.add_tick_callback(move |scrolled, _clock| {
        if !sidebar_resize_active_for_tick.get() {
            let width = scrolled.width();
            if width > 100 {
                gallery_for_resize.update_width(width);
            }
        }
        glib::ControlFlow::Continue
    });

    let grid_surface = gtk::Box::new(gtk::Orientation::Vertical, 0);
    grid_surface.set_hexpand(true);
    grid_surface.set_vexpand(true);
    grid_surface.add_css_class("photo-grid");
    // The group heading sits outside the scroller, so it cannot change
    // GtkGridView's item/row measurements. With grouping disabled it is hidden
    // and the gallery has the same geometry as the pre-grouping implementation.
    grid_surface.append(&gallery.group_header);
    grid_surface.append(&grid_scroll);

    let grid_overlay = gtk::Overlay::new();
    grid_overlay.set_hexpand(true);
    grid_overlay.set_vexpand(true);
    grid_overlay.set_child(Some(&grid_surface));
    grid_overlay.add_overlay(&lightbox.root);

    let photo_page = gtk::Box::new(gtk::Orientation::Vertical, 0);
    photo_page.set_hexpand(true);
    photo_page.set_vexpand(true);
    photo_page.append(&grid_overlay);

    let albums_home = albums_view::build(
        &albums,
        &connection.borrow(),
        grid_thumbnail_size,
        {
            let slot = album_home_click_slot.clone();
            Rc::new(move |album_id| {
                if let Some(open) = slot.borrow().as_ref() {
                    open(album_id);
                }
            })
        },
    );
    let main_stack = gtk::Stack::new();
    main_stack.set_hexpand(true);
    main_stack.set_vexpand(true);
    main_stack.add_named(&photo_page, Some("photos"));
    main_stack.add_named(&albums_home, Some("albums"));
    main_stack.set_visible_child_name("photos");
    content.append(&main_stack);
    content.append(&info.root);

    let selected_for_favorite = selected_photo.clone();
    let db_for_favorite = connection.clone();
    let info_favorite = info.clone();
    let gallery_for_favorite = gallery.clone();
    let filter_for_favorite = filter.clone();
    let search_for_favorite = search_text.clone();
    let sort_for_favorite = sort.clone();
    let lightbox_for_favorite = lightbox.clone();
    let sidebar_refresh_for_favorite = availability_refresh.clone();

    info.favorite.connect_clicked(move |_| {
        let Some(photo) = selected_for_favorite.borrow().clone() else {
            return;
        };
        let favorite = !photo.favorite();

        if let Err(error) = db::set_favorite(&db_for_favorite.borrow(), photo.id(), favorite) {
            eprintln!("Could not update favourite: {error}");
            return;
        }
        photo.set_favorite(favorite);

        if !favorite && filter_for_favorite.get() == sidebar::SidebarFilter::Favorites {
            lightbox_for_favorite.close();
            selected_for_favorite.replace(None);
            info_favorite.set_photo(None);
            let search = search_for_favorite.borrow().clone();
            refresh_grid(
                &db_for_favorite,
                filter_for_favorite.get(),
                &search,
                sort_for_favorite.get(),
                &gallery_for_favorite,
            );
        } else {
            info_favorite.set_photo(Some(&photo));
        }
        sidebar_refresh_for_favorite();

        if std::env::var_os("PICASA_TRACE").is_some() {
            eprintln!(
                "UI TRACE photo_favourite_changed id={} favourite={} filter={:?} lightbox_visible={}",
                photo.id(),
                favorite,
                filter_for_favorite.get(),
                lightbox_for_favorite.root.is_visible()
            );
        }
    });

    let lightbox_for_one_to_one = lightbox.clone();
    let space_toggle_in_progress_for_toggle = space_toggle_in_progress.clone();
    info.one_to_one.connect_toggled(move |button| {
        lightbox_for_one_to_one.set_one_to_one(button.is_active());
        if space_toggle_in_progress_for_toggle.get() {
            space_toggle_in_progress_for_toggle.set(false);
        }
    });

    // Keep the UI state reset when the lightbox is closed through any path.
    // Restore both the GridView selection and keyboard focus. Merely focusing
    // the GridView container is not enough after the lightbox owned focus:
    // GTK can lose the active list cursor, so arrow keys appear dead.
    let one_to_one_for_visibility = info.one_to_one.clone();
    let gallery_for_lightbox_close = gallery.root.clone();
    let selected_photo_for_lightbox_close = selected_photo.clone();
    lightbox.root.connect_visible_notify(move |root| {
        if !root.is_visible() {
            one_to_one_for_visibility.set_active(false);

            let gallery = gallery_for_lightbox_close.clone();
            let selected_photo = selected_photo_for_lightbox_close.borrow().clone();
            glib::idle_add_local_once(move || {
                if let (Some(photo), Some(model)) = (selected_photo, gallery.model()) {
                    for position in 0..model.n_items() {
                        let matches = model
                            .item(position)
                            .and_downcast::<crate::photo_object::PhotoObject>()
                            .is_some_and(|item| item.id() == photo.id());
                        if matches {
                            model.select_item(position, true);
                            break;
                        }
                    }
                }
                gallery.grab_focus();
            });
        }
    });

    let gallery_for_zoom_out = gallery.clone();
    info.grid_zoom_out
        .connect_clicked(move |_| gallery_for_zoom_out.zoom_out());
    let gallery_for_zoom_reset = gallery.clone();
    info.grid_zoom_reset
        .connect_clicked(move |_| gallery_for_zoom_reset.set_zoom(DEFAULT_GRID_THUMBNAIL_SIZE));
    let gallery_for_zoom_in = gallery.clone();
    info.grid_zoom_in
        .connect_clicked(move |_| gallery_for_zoom_in.zoom_in());

    let selected_for_rotate = selected_photo.clone();
    let db_for_rotate = connection.clone();
    let info_rotate = info.clone();
    let gallery_for_rotate = gallery.clone();
    let lightbox_for_rotate = lightbox.clone();

    info.rotate.connect_clicked(move |_| {
        let Some(photo) = selected_for_rotate.borrow().clone() else {
            return;
        };
        let rotation = (photo.rotation() + 90) % 360;

        if let Err(error) = db::set_rotation(&db_for_rotate.borrow(), photo.id(), rotation) {
            eprintln!("Could not rotate photo: {error}");
            return;
        }
        photo.set_rotation(rotation);
        info_rotate.set_photo(Some(&photo));
        gallery_for_rotate.refresh_thumbnails();
        lightbox_for_rotate.refresh_current();
        if std::env::var_os("PICASA_TRACE").is_some() {
            eprintln!(
                "UI TRACE photo_rotated id={} rotation={}",
                photo.id(),
                rotation
            );
        }
    });

    let selected_for_export = selected_photo.clone();
    let parent_for_export = window.clone();

    info.export.connect_clicked(move |_| {
        let Some(photo) = selected_for_export.borrow().clone() else {
            return;
        };

        let dialog = gtk::FileChooserNative::new(
            Some("Export Photo"),
            Some(&parent_for_export),
            gtk::FileChooserAction::Save,
            Some("Export"),
            Some("Cancel"),
        );

        let filename = photo.filename();
        dialog.set_current_name(&filename);

        dialog.connect_response(move |dialog, response| {
            if response == gtk::ResponseType::Accept {
                if let Some(file) = dialog.file() {
                    if let Some(destination) = file.path() {
                        let source = photo.path();
                        let rotation = photo.rotation();

                        std::thread::spawn(move || {
                            let result = image::open(&source).and_then(|image| {
                                let rotated = match rotation {
                                    90 => image::DynamicImage::ImageRgba8(
                                        image::imageops::rotate90(&image.to_rgba8()),
                                    ),
                                    180 => image::DynamicImage::ImageRgba8(
                                        image::imageops::rotate180(&image.to_rgba8()),
                                    ),
                                    270 => image::DynamicImage::ImageRgba8(
                                        image::imageops::rotate270(&image.to_rgba8()),
                                    ),
                                    _ => image,
                                };

                                rotated.save(&destination).map_err(image::ImageError::from)
                            });

                            if let Err(error) = result {
                                eprintln!("Could not export photo: {error}");
                            }
                        });
                    }
                }
            }

            dialog.destroy();
        });

        dialog.show();
    });

    let main_split = adw::OverlaySplitView::new();

    let destination_click: Rc<dyn Fn(sidebar::SidebarFilter)> = {
        let search_entry = search_entry_slot.clone();
        let search_text = search_text.clone();
        let suppressed = search_suppressed.clone();
        let debounce = search_debounce.clone();
        let filter = filter.clone();
        let connection = connection.clone();
        let gallery = gallery.clone();
        let lightbox = lightbox.clone();
        let sort = sort.clone();
        let group_mode = group_mode.clone();
        let sidebar_selection = sidebar_selection_slot.clone();
        let main_stack = main_stack.clone();
        let albums_home = albums_home.clone();
        let connection_for_albums = connection.clone();
        let album_home_click_slot = album_home_click_slot.clone();
        Rc::new(move |new_filter| {
            if let Some(source) = debounce.borrow_mut().take() {
                source.remove();
            }
            suppressed.set(true);
            if let Some(entry) = search_entry.borrow().as_ref() {
                entry.set_text("");
            }
            suppressed.set(false);
            search_text.replace(String::new());
            eprintln!("SEARCH TRACE destination_click clear_search filter={:?}", new_filter);
            lightbox.close();
            filter.set(new_filter);
            if let Some(sidebar) = sidebar_selection.borrow().as_ref() {
                sidebar::set_active_filter(sidebar, new_filter);
            }
            if new_filter == sidebar::SidebarFilter::Albums {
                main_stack.set_visible_child_name("albums");
                if let Ok(albums) = db::albums(&connection_for_albums.borrow()) {
                    let on_album = album_home_click_slot
                        .borrow()
                        .as_ref()
                        .cloned()
                        .unwrap_or_else(|| Rc::new(|_| {}));
                    albums_view::refresh(
                        &albums_home,
                        &albums,
                        &connection_for_albums.borrow(),
                        grid_thumbnail_size,
                        on_album,
                    );
                }
                return;
            }
            main_stack.set_visible_child_name("photos");
            apply_gallery_grouping(&gallery, new_filter, sort.get(), group_mode.get());
            refresh_grid(&connection, new_filter, "", sort.get(), &gallery);
        })
    };

    album_home_click_slot.replace(Some({
        let destination_click = destination_click.clone();
        Rc::new(move |album_id| destination_click(sidebar::SidebarFilter::Album(album_id)))
    }));
    albums_home_refresh_slot.replace(Some({
        let albums_home = albums_home.clone();
        let connection = connection.clone();
        let click_slot = album_home_click_slot.clone();
        Rc::new(move |albums| {
            let on_album = click_slot
                .borrow()
                .as_ref()
                .cloned()
                .unwrap_or_else(|| Rc::new(|_| {}));
            eprintln!("ALBUM UI TRACE index_refresh albums={}", albums.len());
            // Album cards keep using existing cached thumbnails; this only
            // replaces the index data after an album mutation.
            albums_view::refresh(
                &albums_home,
                albums,
                &connection.borrow(),
                grid_thumbnail_size,
                on_album,
            );
        })
    }));

    // Wrap the split view in an overlay so sidebar resizing can show a live
    // preview divider without reallocating the actual sidebar/content panes.
    // The real width is committed only when the drag finishes, keeping the
    // GtkGridView completely stable during pointer motion.
    let main_surface = gtk::Overlay::new();
    main_surface.set_hexpand(true);
    main_surface.set_vexpand(true);
    main_surface.set_child(Some(&main_split));

    let sidebar_resize_preview = gtk::Box::new(gtk::Orientation::Vertical, 0);
    sidebar_resize_preview.set_width_request(2);
    sidebar_resize_preview.set_vexpand(true);
    sidebar_resize_preview.set_halign(gtk::Align::Start);
    sidebar_resize_preview.set_valign(gtk::Align::Fill);
    sidebar_resize_preview.set_can_target(false);
    sidebar_resize_preview.set_visible(false);
    sidebar_resize_preview.add_css_class("sidebar-resize-preview");
    main_surface.add_overlay(&sidebar_resize_preview);

    let sidebar = sidebar::build(
        &folders,
        &albums,
        sidebar_counts,
        {
            let main_split = main_split.clone();
            let destination_click = destination_click.clone();
            move |new_filter| {
                destination_click(new_filter);
                // In the compact overlay layout, selecting a destination should
                // immediately return the available width to the photo grid.
                if main_split.is_collapsed() {
                    main_split.set_show_sidebar(false);
                }
            }
        },
        create_album.clone(),
        import_folder.clone(),
        delete_album.clone(),
        availability_refresh.clone(),
        {
            let slot = refresh_folder_slot.clone();
            Rc::new(move |path| {
                if let Some(callback) = slot.borrow().as_ref() {
                    callback(path);
                }
            })
        },
        {
            let connection = connection.clone();
            let parent: gtk::Widget = window.clone().upcast();
            Rc::new(move |folder| {
                show_folder_statistics(&parent, connection.clone(), folder);
            })
        },
        {
            let parent: gtk::Widget = window.clone().upcast();
            let context = action_context.clone();
            Rc::new(move |folder| {
                show_remove_folder_confirmation(parent.clone(), folder, context.clone());
            })
        },
        {
            let context = action_context.clone();
            Rc::new(move |folder, favorite| {
                match db::set_favorite_for_folder(
                    &context.connection.borrow(),
                    folder.id,
                    favorite,
                ) {
                    Ok(changed) => {
                        eprintln!(
                            "FAVORITE TRACE folder={} favorite={} changed={}",
                            folder.id, favorite, changed
                        );
                        refresh_photo_actions_grid(&context);
                        (context.on_unavailable)();
                    }
                    Err(error) => {
                        show_error(
                            context.info.root.upcast_ref(),
                            "Could not update folder favourites",
                            &error.to_string(),
                        );
                    }
                }
            })
        },
    );
    sidebar_for_unavailable.replace(Some(sidebar.clone()));
    sidebar_selection_slot.replace(Some(sidebar.clone()));
    sidebar::set_active_filter(&sidebar, filter.get());

    // Refresh availability when the desktop reports a mount/unmount. This is
    // deliberately limited to cache/UI updates; it does not start a scan.
    let volume_monitor = gio::VolumeMonitor::get();
    let mount_refresh_pending = Rc::new(Cell::new(false));
    let schedule_mount_refresh: Rc<dyn Fn()> = {
        let availability_refresh = availability_refresh.clone();
        let pending = mount_refresh_pending.clone();
        Rc::new(move || {
            if pending.replace(true) {
                return;
            }
            let availability_refresh = availability_refresh.clone();
            let pending = pending.clone();
            glib::timeout_add_local_once(Duration::from_millis(250), move || {
                pending.set(false);
                availability_refresh();
            });
        })
    };
    let schedule_mount_refresh_for_mount = schedule_mount_refresh.clone();
    volume_monitor.connect_mount_added(move |_, mount| {
        if std::env::var_os("PICASA_TRACE").is_some() {
            eprintln!(
                "UI TRACE availability_mount_added uri={}",
                mount.root().uri()
            );
        }
        schedule_mount_refresh_for_mount();
    });
    let schedule_mount_refresh_for_unmount = schedule_mount_refresh.clone();
    volume_monitor.connect_mount_removed(move |_, mount| {
        if std::env::var_os("PICASA_TRACE").is_some() {
            eprintln!(
                "UI TRACE availability_mount_removed uri={}",
                mount.root().uri()
            );
        }
        schedule_mount_refresh_for_unmount();
    });
    let schedule_mount_refresh_for_change = schedule_mount_refresh.clone();
    volume_monitor.connect_mount_changed(move |_, mount| {
        if std::env::var_os("PICASA_TRACE").is_some() {
            eprintln!(
                "UI TRACE availability_mount_changed uri={}",
                mount.root().uri()
            );
        }
        schedule_mount_refresh_for_change();
    });
    unsafe {
        window.set_data("picasa-volume-monitor", volume_monitor);
        #[cfg(unix)]
        {
            let unix_mount_monitor = gio::UnixMountMonitor::get();
            let schedule_unix_mount_refresh = schedule_mount_refresh.clone();
            unix_mount_monitor.connect_mountpoints_changed(move |_| {
                schedule_unix_mount_refresh();
            });
            let schedule_unix_mount_refresh = schedule_mount_refresh.clone();
            unix_mount_monitor.connect_mounts_changed(move |_| {
                schedule_unix_mount_refresh();
            });
            window.set_data("picasa-unix-mount-monitor", unix_mount_monitor);
        }
    }
    let sidebar_for_events = sidebar.clone();

    let left_header = adw::HeaderBar::new();
    left_header.set_height_request(46);
    left_header.set_show_start_title_buttons(false);
    left_header.set_show_end_title_buttons(false);
    left_header.add_css_class("layout-left-header");

    let display_for_sidebar_toggle = gtk::gdk::Display::default().expect("a display is required");
    let sidebar_toggle_icon_theme = gtk::IconTheme::for_display(&display_for_sidebar_toggle);
    let sidebar_toggle_icon = if sidebar_toggle_icon_theme.has_icon("sidebar-hide-symbolic") {
        "sidebar-hide-symbolic"
    } else if sidebar_toggle_icon_theme.has_icon("view-sidebar-symbolic") {
        "view-sidebar-symbolic"
    } else {
        "pan-start-symbolic"
    };
    let menu = gtk::Button::from_icon_name(sidebar_toggle_icon);
    menu.set_tooltip_text(Some("Hide sidebar"));
    menu.add_css_class("flat");
    menu.add_css_class("sidebar-toggle-button");
    menu.set_size_request(28, 28);
    left_header.pack_start(&menu);

    let left_column = gtk::Box::new(gtk::Orientation::Vertical, 0);
    left_column.set_vexpand(true);
    left_column.add_css_class("layout-left-column");
    sidebar.set_vexpand(true);
    left_column.append(&left_header);
    left_column.append(&sidebar);

    // Overlay a narrow drag handle on the sidebar's right edge.
    // AdwOverlaySplitView does not expose a built-in draggable divider, so we
    // adjust its sidebar_width_fraction ourselves while preserving its compact
    // overlay behaviour. Long folder names can therefore be given more room
    // without permanently making the sidebar wide.
    let sidebar_shell = gtk::Overlay::new();
    sidebar_shell.set_hexpand(true);
    sidebar_shell.set_vexpand(true);
    sidebar_shell.set_child(Some(&left_column));

    let sidebar_resize_handle = gtk::Box::new(gtk::Orientation::Vertical, 0);
    sidebar_resize_handle.set_width_request(8);
    sidebar_resize_handle.set_hexpand(false);
    sidebar_resize_handle.set_vexpand(true);
    sidebar_resize_handle.set_halign(gtk::Align::End);
    sidebar_resize_handle.set_valign(gtk::Align::Fill);
    sidebar_resize_handle.set_cursor_from_name(Some("col-resize"));
    sidebar_resize_handle.add_css_class("sidebar-resize-handle");
    sidebar_shell.add_overlay(&sidebar_resize_handle);

    let right_column = gtk::Box::new(gtk::Orientation::Vertical, 0);
    right_column.set_hexpand(true);
    right_column.set_vexpand(true);

    main_split.set_sidebar(Some(&sidebar_shell));
    main_split.set_content(Some(&right_column));
    main_split.set_min_sidebar_width(200.0);
    main_split.set_max_sidebar_width(600.0);
    main_split.set_sidebar_width_fraction(0.22);
    main_split.set_enable_show_gesture(true);
    main_split.set_enable_hide_gesture(true);

    // Sidebar pin/hover state lives in sidebar.rs. Keep one authoritative
    // state store so manual pinning and temporary hover-open behavior cannot
    // drift apart.
    sidebar::set_pinned(&sidebar, true);
    sidebar::clear_hover_open(&sidebar);

    // When the sidebar is hidden, expose a very small hover target at the
    // far-left edge. Hover-opening does not change the pinned state.
    let sidebar_hover_reveal = gtk::Box::new(gtk::Orientation::Vertical, 0);
    sidebar_hover_reveal.set_width_request(10);
    sidebar_hover_reveal.set_hexpand(false);
    sidebar_hover_reveal.set_vexpand(true);
    sidebar_hover_reveal.set_halign(gtk::Align::Start);
    sidebar_hover_reveal.set_valign(gtk::Align::Fill);
    sidebar_hover_reveal.set_visible(!main_split.shows_sidebar());
    sidebar_hover_reveal.set_cursor_from_name(Some("default"));
    main_surface.add_overlay(&sidebar_hover_reveal);

    let sidebar_hover_motion = gtk::EventControllerMotion::new();
    let main_split_for_hover_reveal = main_split.clone();
    let sidebar_for_hover_reveal = sidebar.clone();
    sidebar_hover_motion.connect_enter(move |_, _, _| {
        if !main_split_for_hover_reveal.shows_sidebar()
            && !sidebar::is_pinned(&sidebar_for_hover_reveal)
        {
            sidebar::set_hover_open(&sidebar_for_hover_reveal, true);
            main_split_for_hover_reveal.set_show_sidebar(true);
        }
    });
    sidebar_hover_reveal.add_controller(sidebar_hover_motion);

    // Auto-close only a sidebar that was opened by hover. A pinned sidebar
    // must remain open when the pointer leaves.
    let sidebar_leave_motion = gtk::EventControllerMotion::new();
    let main_split_for_hover_hide = main_split.clone();
    let sidebar_for_hover_hide = sidebar.clone();
    sidebar_leave_motion.connect_leave(move |_| {
        if sidebar::is_hover_open(&sidebar_for_hover_hide) {
            sidebar::clear_hover_open(&sidebar_for_hover_hide);
            main_split_for_hover_hide.set_show_sidebar(false);
        }
    });
    sidebar_shell.add_controller(sidebar_leave_motion);

    let sidebar_hover_reveal_for_state = sidebar_hover_reveal.clone();
    main_split.connect_show_sidebar_notify(move |split| {
        sidebar_hover_reveal_for_state.set_visible(!split.shows_sidebar());
    });

    // Keep resize geometry stable for the full drag gesture. Recomputing the
    // starting width from sidebar_width_fraction() * the *current* split width
    // on every motion made the denominator move while GTK was reallocating the
    // two panes, which produced the visible jumping/jerking.
    //
    // Capture actual allocated pixels once at drag begin, then derive every
    // subsequent fraction from that fixed geometry.
    let sidebar_drag_start_width = Rc::new(Cell::new(0.0f64));
    let sidebar_drag_split_width = Rc::new(Cell::new(1.0f64));
    let sidebar_drag = gtk::GestureDrag::new();
    sidebar_drag.set_button(1);

    let sidebar_shell_for_drag_begin = sidebar_shell.clone();
    let main_split_for_drag_begin = main_split.clone();
    let sidebar_drag_start_width_begin = sidebar_drag_start_width.clone();
    let sidebar_drag_split_width_begin = sidebar_drag_split_width.clone();
    let sidebar_resize_active_for_begin = sidebar_resize_active.clone();
    sidebar_drag.connect_drag_begin(move |_, _, _| {
        sidebar_resize_active_for_begin.set(true);
        sidebar_drag_start_width_begin
            .set(sidebar_shell_for_drag_begin.width().max(1) as f64);
        sidebar_drag_split_width_begin
            .set(main_split_for_drag_begin.width().max(1) as f64);
    });

    // Keep the real split allocation unchanged during pointer motion. Instead
    // move a 2px preview divider across the full window. This gives immediate
    // resize feedback without making every GtkGridView cell reallocate.
    let pending_sidebar_fraction = Rc::new(Cell::new(main_split.sidebar_width_fraction()));
    let pending_sidebar_fraction_update = pending_sidebar_fraction.clone();
    let sidebar_drag_start_width_update = sidebar_drag_start_width.clone();
    let sidebar_drag_split_width_update = sidebar_drag_split_width.clone();
    let sidebar_resize_preview_update = sidebar_resize_preview.clone();
    sidebar_drag.connect_drag_update(move |_, offset_x, _| {
        let split_width = sidebar_drag_split_width_update.get().max(1.0);
        let target_width = (sidebar_drag_start_width_update.get() + offset_x)
            .clamp(200.0, 600.0)
            .min(split_width * 0.70);
        let fraction = (target_width / split_width).clamp(0.10, 0.70);
        pending_sidebar_fraction_update.set(fraction);

        sidebar_resize_preview_update.set_margin_start(target_width.round() as i32 - 1);
        sidebar_resize_preview_update.set_visible(true);
    });

    let main_split_for_drag_end = main_split.clone();
    let pending_sidebar_fraction_end = pending_sidebar_fraction.clone();
    let sidebar_resize_active_for_end = sidebar_resize_active.clone();
    let sidebar_resize_preview_end = sidebar_resize_preview.clone();
    let gallery_for_sidebar_drag_end = gallery.clone();
    let grid_scroll_for_sidebar_drag_end = grid_scroll.clone();
    sidebar_drag.connect_drag_end(move |_, _, _| {
        sidebar_resize_preview_end.set_visible(false);
        main_split_for_drag_end
            .set_sidebar_width_fraction(pending_sidebar_fraction_end.get());
        sidebar_resize_active_for_end.set(false);

        // Wait until the split view has received its single final allocation,
        // then perform exactly one responsive grid update.
        let gallery = gallery_for_sidebar_drag_end.clone();
        let scrolled = grid_scroll_for_sidebar_drag_end.clone();
        glib::idle_add_local_once(move || {
            let width = scrolled.width();
            if width > 100 {
                gallery.update_width(width);
            }
        });
    });
    sidebar_resize_handle.add_controller(sidebar_drag);

    let main_split_for_hide = main_split.clone();
    let sidebar_for_hide = sidebar.clone();
    menu.connect_clicked(move |_| {
        sidebar::set_pinned(&sidebar_for_hide, false);
        sidebar::clear_hover_open(&sidebar_for_hide);
        main_split_for_hide.set_show_sidebar(false);
    });

    let right_header = adw::HeaderBar::new();
    right_header.set_height_request(46);
    right_header.set_hexpand(true);
    right_header.set_show_start_title_buttons(true);
    right_header.set_show_end_title_buttons(true);
    right_header.add_css_class("layout-right-header");

    let show_sidebar = gtk::Button::from_icon_name("sidebar-show-symbolic");
    show_sidebar.set_tooltip_text(Some("Show sidebar"));
    show_sidebar.add_css_class("flat");
    show_sidebar.set_visible(false);
    let main_split_for_show = main_split.clone();
    let sidebar_for_show = sidebar.clone();
    show_sidebar.connect_clicked(move |_| {
        sidebar::set_pinned(&sidebar_for_show, true);
        main_split_for_show.set_show_sidebar(true);
    });
    right_header.pack_start(&show_sidebar);

    let show_sidebar_for_state = show_sidebar.clone();
    main_split.connect_show_sidebar_notify(move |split| {
        show_sidebar_for_state.set_visible(!split.shows_sidebar());
    });

    let search = gtk::SearchEntry::new();
    search.set_placeholder_text(Some("Search photos"));
    search.set_width_chars(22);
    // Do not force a 320px minimum. The fixed minimum was wider than the
    // available header centre area in smaller windows and pushed toolbar
    // buttons outside the visible allocation.
    search.set_size_request(160, -1);
    search.set_hexpand(true);
    search.add_css_class("search-field");
    search_entry_slot.replace(Some(search.clone()));
    let search_area = gtk::Box::new(gtk::Orientation::Vertical, 0);
    search_area.set_valign(gtk::Align::Center);
    search_area.set_hexpand(true);
    search_area.append(&search);
    let suggestion_revealer = gtk::Revealer::new();
    suggestion_revealer.set_transition_type(gtk::RevealerTransitionType::SlideDown);
    suggestion_revealer.set_reveal_child(false);
    let suggestion_list = gtk::Box::new(gtk::Orientation::Vertical, 0);
    suggestion_list.set_size_request(320, -1);
    suggestion_revealer.set_child(Some(&suggestion_list));
    search_area.append(&suggestion_revealer);
    right_header.set_title_widget(Some(&search_area));

    let gallery_for_search = gallery.clone();
    let connection_for_search = connection.clone();
    let filter_for_search = filter.clone();
    let search_text_for_search = search_text.clone();
    let sort_for_search = sort.clone();
    let search_suppressed_for_search = search_suppressed.clone();
    let search_debounce_for_search = search_debounce.clone();
    let destination_click_for_search = destination_click.clone();
    let suggestion_revealer_for_search = suggestion_revealer.clone();
    let suggestion_list_for_search = suggestion_list.clone();
    let folders_for_search = folders.clone();

    search.connect_search_changed(move |entry| {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let callback_started = Instant::now();
            eprintln!("SEARCH TRACE changed start text={:?}", entry.text());
            if search_suppressed_for_search.get() {
                eprintln!("SEARCH TRACE changed suppressed");
                return;
            }
            eprintln!("SEARCH TRACE changed cancel_previous_debounce");
            if let Some(source) = search_debounce_for_search.borrow_mut().take() {
                let removal = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    source.remove();
                }));
                if removal.is_err() {
                    eprintln!("SEARCH TRACE debounce_remove_failed source_already_removed");
                }
            }
            let query = entry.text().to_string();
            eprintln!("SEARCH TRACE changed query_captured query={:?}", query);
            search_text_for_search.replace(query.clone());
            eprintln!("SEARCH TRACE changed shared_text_replaced");
            eprintln!(
                "SEARCH TRACE changed folders_cached count={}",
                folders_for_search.len()
            );
            update_folder_suggestions(
                &suggestion_revealer_for_search,
                &suggestion_list_for_search,
                &folders_for_search,
                &query,
                Rc::new({
                    let destination_click = destination_click_for_search.clone();
                    move |folder_id| destination_click(sidebar::SidebarFilter::Folder(folder_id))
                }),
            );
            // Updating the inline suggestion list must never move typing focus
            // away from the SearchEntry.
            entry.grab_focus();
            eprintln!("SEARCH TRACE changed suggestions_updated");

            if query.is_empty() {
                eprintln!("SEARCH TRACE changed empty_refresh_start");
                refresh_grid(&connection_for_search, filter_for_search.get(), "", sort_for_search.get(), &gallery_for_search);
                eprintln!("SEARCH TRACE changed empty_refresh_done");
            } else if query.chars().count() < 2 {
                eprintln!("SEARCH TRACE photo_refresh_deferred query={:?}", query);
            } else {
                eprintln!("SEARCH TRACE photo_refresh_deferred query={:?}", query);
                let connection = connection_for_search.clone();
                let filter = filter_for_search.clone();
                let search_text = search_text_for_search.clone();
                let sort = sort_for_search.clone();
                let gallery = gallery_for_search.clone();
                let debounce_slot = search_debounce_for_search.clone();
                let query_for_refresh = query.clone();
                let source = glib::timeout_add_local(Duration::from_millis(300), move || {
                    let refresh_started = Instant::now();
                    eprintln!("SEARCH TRACE debounce_fired query={:?}", query_for_refresh);
                    // The source removes itself after returning Break. Clear
                    // the slot now so a later keystroke never tries to remove
                    // an already-finished SourceId.
                    debounce_slot.borrow_mut().take();
                    if search_text.borrow().as_str() != query_for_refresh {
                        eprintln!("SEARCH TRACE debounce_stale query={:?}", query_for_refresh);
                        return glib::ControlFlow::Break;
                    }
                    eprintln!("SEARCH TRACE global_refresh query={:?}", query_for_refresh);
                    refresh_grid(&connection, filter.get(), &query_for_refresh, sort.get(), &gallery);
                    eprintln!(
                        "SEARCH TRACE global_refresh_done query={:?} elapsed_ms={}",
                        query_for_refresh,
                        refresh_started.elapsed().as_millis()
                    );
                    glib::ControlFlow::Break
                });
                search_debounce_for_search.replace(Some(source));
                eprintln!("SEARCH TRACE changed debounce_scheduled");
            }
            eprintln!(
                "SEARCH TRACE changed done query={:?} elapsed_ms={}",
                query,
                callback_started.elapsed().as_millis()
            );
        }));
        if let Err(payload) = result {
            let message = payload
                .downcast_ref::<&str>()
                .copied()
                .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
                .unwrap_or("non-string panic payload");
            eprintln!("SEARCH TRACE PANIC callback message={message}");
        }
    });

    let search_text_for_activate = search_text.clone();
    let search_suppressed_for_activate = search_suppressed.clone();
    let search_debounce_for_activate = search_debounce.clone();
    let filter_for_activate = filter.clone();
    let connection_for_activate = connection.clone();
    let sort_for_activate = sort.clone();
    let gallery_for_activate = gallery.clone();
    let suggestion_revealer_for_activate = suggestion_revealer.clone();
    search.connect_activate(move |entry| {
        eprintln!("SEARCH TRACE activate text={:?}", entry.text());
        if let Some(source) = search_debounce_for_activate.borrow_mut().take() {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| source.remove()));
        }
        search_suppressed_for_activate.set(true);
        entry.set_text("");
        search_suppressed_for_activate.set(false);
        search_text_for_activate.replace(String::new());
        suggestion_revealer_for_activate.set_reveal_child(false);
        eprintln!("SEARCH TRACE activate clear_search");
        refresh_grid(
            &connection_for_activate,
            filter_for_activate.get(),
            "",
            sort_for_activate.get(),
            &gallery_for_activate,
        );
        entry.grab_focus();
    });

    let import = gtk::Button::from_icon_name("folder-open-symbolic");
    import.set_tooltip_text(Some("Import Folder"));
    right_header.pack_end(&import);

    let refresh = gtk::Button::from_icon_name("view-refresh-symbolic");
    refresh.set_tooltip_text(Some("Refresh library"));
    right_header.pack_end(&refresh);

    let settings = gtk::MenuButton::new();
    settings.set_icon_name("emblem-system-symbolic");
    settings.set_tooltip_text(Some("Settings"));

    // The iPhone presentation is the app's default. Keep the stock GTK4 /
    // libadwaita presentation immediately available as an opt-in overlay,
    // rather than making users restart or changing their system theme.
    let standard_theme_provider = gtk::CssProvider::new();
    standard_theme_provider.load_from_data(STANDARD_GTK4_CSS);
    let display = gtk::gdk::Display::default().expect("a display is required");

    let settings_popover = gtk::Popover::new();
    let settings_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    settings_box.set_margin_top(8);
    settings_box.set_margin_bottom(8);
    settings_box.set_margin_start(8);
    settings_box.set_margin_end(8);

    let appearance = gtk::Label::new(Some("Appearance"));
    appearance.set_xalign(0.0);
    appearance.add_css_class("heading");

    let saved_theme = db::setting(&connection.borrow(), THEME_SETTING_KEY)
        .ok()
        .flatten()
        .unwrap_or_else(|| "standard".to_string());

    let iphone_theme = gtk::CheckButton::with_label("iPhoto Dark");
    iphone_theme.set_tooltip_text(Some("Dark iPhoto-inspired gallery"));

    let standard_theme = gtk::CheckButton::with_label("Standard GTK4");
    standard_theme.set_group(Some(&iphone_theme));
    if saved_theme == "iphone" {
        iphone_theme.set_active(true);
    } else {
        standard_theme.set_active(true);
    }
    standard_theme.set_tooltip_text(Some("Use the regular GTK4 / libadwaita appearance"));

    let style_manager = adw::StyleManager::default();
    if saved_theme == "iphone" {
        lightbox.use_iphone_backdrop();
    } else {
        lightbox.use_standard_backdrop(style_manager.is_dark());
    }

    let display_for_standard = display.clone();
    let provider_for_standard = standard_theme_provider.clone();
    let connection_for_standard = connection.clone();
    let lightbox_for_standard = lightbox.clone();
    let style_manager_for_standard = style_manager.clone();
    standard_theme.connect_toggled(move |button| {
        if button.is_active() {
            gtk::style_context_add_provider_for_display(
                &display_for_standard,
                &provider_for_standard,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
            );
            lightbox_for_standard.use_standard_backdrop(style_manager_for_standard.is_dark());
            if let Err(error) = db::set_setting(
                &connection_for_standard.borrow(),
                THEME_SETTING_KEY,
                "standard",
            ) {
                eprintln!("Could not save appearance theme: {error}");
            }
        }
    });

    let display_for_iphone = display.clone();
    let provider_for_iphone = standard_theme_provider.clone();
    let connection_for_iphone = connection.clone();
    let lightbox_for_iphone = lightbox.clone();
    iphone_theme.connect_toggled(move |button| {
        if button.is_active() {
            gtk::style_context_remove_provider_for_display(
                &display_for_iphone,
                &provider_for_iphone,
            );
            lightbox_for_iphone.use_iphone_backdrop();
            if let Err(error) =
                db::set_setting(&connection_for_iphone.borrow(), THEME_SETTING_KEY, "iphone")
            {
                eprintln!("Could not save appearance theme: {error}");
            }
        }
    });

    let standard_theme_for_dark = standard_theme.clone();
    let lightbox_for_dark = lightbox.clone();
    style_manager.connect_dark_notify(move |manager| {
        if standard_theme_for_dark.is_active() {
            lightbox_for_dark.use_standard_backdrop(manager.is_dark());
        }
    });

    let clear_thumbnails = gtk::Button::with_label("Clear thumbnails");
    clear_thumbnails.set_halign(gtk::Align::Fill);
    clear_thumbnails.add_css_class("clear-action-button");
    let clear_database = gtk::Button::with_label("Clear database");
    clear_database.set_halign(gtk::Align::Fill);
    clear_database.add_css_class("clear-action-button");
    let clear_all = gtk::Button::with_label("Clear all");
    clear_all.set_halign(gtk::Align::Fill);
    clear_all.add_css_class("clear-action-button");
    settings_box.append(&appearance);
    settings_box.append(&iphone_theme);
    settings_box.append(&standard_theme);
    settings_box.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    settings_box.append(&clear_thumbnails);
    settings_box.append(&clear_database);
    settings_box.append(&clear_all);
    settings_popover.set_child(Some(&settings_box));
    settings.set_popover(Some(&settings_popover));

    let sort_button = gtk::MenuButton::new();
    sort_button.set_icon_name(match sort.get().direction {
        SortDirection::Ascending => "view-sort-ascending-symbolic",
        SortDirection::Descending => "view-sort-descending-symbolic",
    });
    sort_button.set_tooltip_text(Some("Sort photos"));

    let sort_popover = gtk::Popover::new();
    let sort_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    sort_box.set_margin_top(8);
    sort_box.set_margin_bottom(8);
    sort_box.set_margin_start(8);
    sort_box.set_margin_end(8);

    let sort_heading = gtk::Label::new(Some("Sort by"));
    sort_heading.set_xalign(0.0);
    sort_heading.add_css_class("heading");
    sort_box.append(&sort_heading);

    let date_taken_sort = gtk::CheckButton::with_label("Date taken");
    let name_sort = gtk::CheckButton::with_label("Name");
    let file_size_sort = gtk::CheckButton::with_label("File size");
    let dimensions_sort = gtk::CheckButton::with_label("Dimensions");
    let date_added_sort = gtk::CheckButton::with_label("Date added");
    for button in [
        &name_sort,
        &file_size_sort,
        &dimensions_sort,
        &date_added_sort,
    ] {
        button.set_group(Some(&date_taken_sort));
    }
    match sort.get().field {
        SortField::DateTaken => date_taken_sort.set_active(true),
        SortField::Name => name_sort.set_active(true),
        SortField::FileSize => file_size_sort.set_active(true),
        SortField::Dimensions => dimensions_sort.set_active(true),
        SortField::DateAdded => date_added_sort.set_active(true),
    }
    for button in [
        &date_taken_sort,
        &name_sort,
        &file_size_sort,
        &dimensions_sort,
        &date_added_sort,
    ] {
        sort_box.append(button);
    }

    sort_box.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    let order_heading = gtk::Label::new(Some("Order"));
    order_heading.set_xalign(0.0);
    order_heading.add_css_class("heading");
    sort_box.append(&order_heading);
    let ascending_sort = gtk::CheckButton::with_label("Ascending");
    let descending_sort = gtk::CheckButton::with_label("Descending");
    descending_sort.set_group(Some(&ascending_sort));
    match sort.get().direction {
        SortDirection::Ascending => ascending_sort.set_active(true),
        SortDirection::Descending => descending_sort.set_active(true),
    }
    sort_box.append(&ascending_sort);
    sort_box.append(&descending_sort);

    sort_box.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    let group_heading = gtk::Label::new(Some("Group by"));
    group_heading.set_xalign(0.0);
    group_heading.add_css_class("heading");
    sort_box.append(&group_heading);

    let group_none = gtk::CheckButton::with_label("None");
    let group_day = gtk::CheckButton::with_label("Day");
    let group_month = gtk::CheckButton::with_label("Month");
    group_day.set_group(Some(&group_none));
    group_month.set_group(Some(&group_none));
    match group_mode.get() {
        grid::GroupMode::None => group_none.set_active(true),
        grid::GroupMode::Day => group_day.set_active(true),
        grid::GroupMode::Month => group_month.set_active(true),
    }
    // Grouping belongs to the Library section, not to a particular sort field.
    // Date grouping will use Date Taken (or Date Added when that sort is active).
    // Albums and Folders keep the controls disabled.
    let group_available = is_library_filter(filter.get());
    group_none.set_sensitive(group_available);
    group_day.set_sensitive(group_available);
    group_month.set_sensitive(group_available);
    sort_box.append(&group_none);
    sort_box.append(&group_day);
    sort_box.append(&group_month);

    let connect_sort_field = |button: &gtk::CheckButton, field: SortField| {
        let sort = sort.clone();
        let group_mode = group_mode.clone();
        let group_none = group_none.clone();
        let group_day = group_day.clone();
        let group_month = group_month.clone();
        let connection = connection.clone();
        let filter = filter.clone();
        let search = search_text.clone();
        let gallery = gallery.clone();
        button.connect_toggled(move |button| {
            if !button.is_active() {
                return;
            }
            let value = PhotoSort {
                field,
                ..sort.get()
            };
            sort.set(value);
            // Name/size/dimensions sorting cannot form a coherent chronological
            // group sequence. Keep Group by available in Library, but selecting a
            // non-date sort while grouped returns the grouping mode to None.
            if group_date_for_sort(value).is_none()
                && group_mode.get() != grid::GroupMode::None
            {
                group_none.set_active(true);
            }
            apply_gallery_grouping(&gallery, filter.get(), value, group_mode.get());
            if let Err(error) =
                db::set_setting(&connection.borrow(), SORT_FIELD_SETTING_KEY, field.key())
            {
                eprintln!("Could not save photo sort field: {error}");
            }
            refresh_grid(&connection, filter.get(), &search.borrow(), value, &gallery);
        });
    };
    connect_sort_field(&date_taken_sort, SortField::DateTaken);
    connect_sort_field(&name_sort, SortField::Name);
    connect_sort_field(&file_size_sort, SortField::FileSize);
    connect_sort_field(&dimensions_sort, SortField::Dimensions);
    connect_sort_field(&date_added_sort, SortField::DateAdded);

    let connect_sort_direction = |button: &gtk::CheckButton, direction: SortDirection| {
        let sort = sort.clone();
        let sort_button = sort_button.clone();
        let connection = connection.clone();
        let filter = filter.clone();
        let search = search_text.clone();
        let gallery = gallery.clone();
        button.connect_toggled(move |button| {
            if !button.is_active() {
                return;
            }
            let value = PhotoSort {
                direction,
                ..sort.get()
            };
            sort.set(value);
            sort_button.set_icon_name(match direction {
                SortDirection::Ascending => "view-sort-ascending-symbolic",
                SortDirection::Descending => "view-sort-descending-symbolic",
            });
            if let Err(error) = db::set_setting(
                &connection.borrow(),
                SORT_DIRECTION_SETTING_KEY,
                direction.key(),
            ) {
                eprintln!("Could not save photo sort direction: {error}");
            }
            refresh_grid(&connection, filter.get(), &search.borrow(), value, &gallery);
        });
    };
    connect_sort_direction(&ascending_sort, SortDirection::Ascending);
    connect_sort_direction(&descending_sort, SortDirection::Descending);

    let connect_group_mode = |button: &gtk::CheckButton, mode: grid::GroupMode| {
        let group_mode = group_mode.clone();
        let sort = sort.clone();
        let date_taken_sort = date_taken_sort.clone();
        let connection = connection.clone();
        let filter = filter.clone();
        let gallery = gallery.clone();
        button.connect_toggled(move |button| {
            if !button.is_active() {
                return;
            }
            if !is_library_filter(filter.get()) {
                return;
            }
            // Day/Month are chronological groups. If the user was sorting by
            // Name, Size or Dimensions, switch to Date Taken automatically
            // instead of greying out Group by.
            if mode != grid::GroupMode::None && group_date_for_sort(sort.get()).is_none() {
                date_taken_sort.set_active(true);
            }
            group_mode.set(mode);
            if let Err(error) = db::set_setting(
                &connection.borrow(),
                GROUP_MODE_SETTING_KEY,
                group_mode_key(mode),
            ) {
                eprintln!("Could not save photo group mode: {error}");
            }
            apply_gallery_grouping(&gallery, filter.get(), sort.get(), mode);
        });
    };
    connect_group_mode(&group_none, grid::GroupMode::None);
    connect_group_mode(&group_day, grid::GroupMode::Day);
    connect_group_mode(&group_month, grid::GroupMode::Month);

    sort_popover.set_child(Some(&sort_box));

    // The active sidebar destination can change after this popover is built.
    // Re-evaluate Group by every time it is opened so all Library destinations
    // stay enabled and Albums/Folders are visibly disabled.
    let filter_for_group_controls = filter.clone();
    let group_none_for_visibility = group_none.clone();
    let group_day_for_visibility = group_day.clone();
    let group_month_for_visibility = group_month.clone();
    sort_popover.connect_visible_notify(move |popover| {
        if !popover.is_visible() {
            return;
        }
        let available = is_library_filter(filter_for_group_controls.get());
        group_none_for_visibility.set_sensitive(available);
        group_day_for_visibility.set_sensitive(available);
        group_month_for_visibility.set_sensitive(available);
    });

    sort_button.set_popover(Some(&sort_popover));
    let header_tools = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    header_tools.append(&sort_button);
    header_tools.append(&settings);
    right_header.pack_end(&header_tools);

    right_column.append(&right_header);
    right_column.append(&content);

    // Toasts for import/scan progress and results.
    let toast_overlay = adw::ToastOverlay::new();
    toast_overlay.set_child(Some(&main_surface));

    window.set_content(Some(&toast_overlay));

    // Below this width the sidebar becomes an overlay instead of permanently
    // consuming grid space. The breakpoint restores the expanded split view
    // automatically when the window grows again.
    let compact = adw::Breakpoint::new(
        adw::BreakpointCondition::parse("max-width: 1050px")
            .expect("valid compact sidebar breakpoint"),
    );
    compact.add_setter(&main_split, "collapsed", Some(&true.to_value()));
    compact.add_setter(&main_split, "show-sidebar", Some(&false.to_value()));
    window.add_breakpoint(compact);

    let provider = gtk::CssProvider::new();
    provider.load_from_data("\
        window, .layout-left-column, .navigation-sidebar { background: #252525; color: #f2f2f2; }\
        .layout-left-column { min-width: 200px; border-right: 1px solid #101010; }\
        .sidebar-resize-handle { background: transparent; }\
        .sidebar-resize-handle:hover { background: rgba(120,185,232,0.35); }\
        .sidebar-resize-preview { background: rgba(120,185,232,0.95); }\
        .layout-left-header, .layout-right-header { min-height: 46px; background: linear-gradient(to bottom, #404040, #2d2d2d); border-bottom: 1px solid #171717; color: #f5f5f5; }\
        .app-title { font-weight: 700; font-size: 15px; margin-left: 4px; color: #f5f5f5; text-shadow: 0 1px #111; }\
        .layout-left-header button, .layout-right-header button { color: #eeeeee; border-radius: 6px; }\
        .layout-left-header .sidebar-toggle-button, .layout-left-header .sidebar-toggle-button image { color: #f5f5f5; opacity: 1; }\
        .layout-left-header button:hover, .layout-right-header button:hover { background: rgba(255,255,255,0.12); }\
        .search-field { min-width: 160px; min-height: 30px; padding: 0 10px; border-radius: 7px; background: #202020; border: 1px solid #151515; color: #f5f5f5; box-shadow: inset 0 1px rgba(0,0,0,0.55), 0 1px rgba(255,255,255,0.10); }\
        .search-field image { color: #bdbdbd; }\
        .search-field entry { background: transparent; border: none; box-shadow: none; color: #f5f5f5; }\
        .photo-grid { background: #292929; }\
        scrolledwindow undershoot { background: transparent; }\
        gridview.section-grid { background: transparent; padding: 0; }\
        gridview.section-grid > child, gridview.section-grid > item { padding: 6px; margin: 0; background: transparent; background-image: none; box-shadow: none; border-radius: 10px; }\
        gridview.section-grid > child:hover, gridview.section-grid > child:selected, gridview.section-grid > child:focus, gridview.section-grid > child:active, gridview.section-grid > item:hover, gridview.section-grid > item:selected, gridview.section-grid > item:focus, gridview.section-grid > item:active { background: transparent; background-image: none; outline: none; box-shadow: none; }\
        .photo-frame { box-shadow: 0 2px 5px rgba(0,0,0,0.62), 0 0 0 1px rgba(255,255,255,0.12); }\
        .photo-tile { border-radius: 7px; border: 2px solid transparent; background: #3a3a3a; transition: border-color 150ms ease, box-shadow 150ms ease; }\
        .photo-tile:hover { border-color: rgba(140,196,237,0.70); box-shadow: 0 3px 10px rgba(0,0,0,0.75), 0 0 0 1px rgba(255,255,255,0.18); }\
        .albums-home-grid > flowboxchild { padding: 0; margin: 0; min-height: 0; }\
        button.album-card { min-width: 0; padding: 0; margin: 0; background: transparent; background-image: none; border: none; box-shadow: none; }\
        button.album-card:hover, button.album-card:focus, button.album-card:active { background: transparent; background-image: none; box-shadow: none; }\
        gridview.section-grid > child:selected .photo-tile, gridview.section-grid > item:selected .photo-tile { border-color: #78b9e8; box-shadow: 0 0 0 1px #c6e6ff, 0 3px 10px rgba(0,0,0,0.75); }\
        .selection-badge { opacity: 0; transition: opacity 150ms ease; background: #4d9fdb; color: white; border-radius: 9999px; padding: 3px; box-shadow: 0 1px 3px rgba(0,0,0,0.55); }\
        gridview.section-grid > child:selected .selection-badge, gridview.section-grid > item:selected .selection-badge { opacity: 1; }\
        .offline-badge { min-width: 20px; min-height: 20px; padding: 0; border-radius: 9999px; color: #2a1a00; background: #f2c14e; font-weight: 700; }\
        .sidebar-offline-badge { min-width: 16px; min-height: 16px; padding: 0; border-radius: 9999px; color: #2a1a00; background: #f2c14e; font-weight: 700; font-size: 10px; }\
        .folder-disclosure { opacity: 0.68; }\
        .navigation-sidebar .folder-count { color: #969696; }\
        .thumbnail { border-radius: 5px; }\
        .missing-thumbnail { background: #383838; }\
        .section-heading-box { margin-top: 18px; margin-bottom: 7px; }\
        .section-heading { color: #f0f0f0; font-weight: 700; font-size: 14px; text-shadow: 0 1px #151515; }\
        .section-more-btn { opacity: 0.6; color: #d8d8d8; border-radius: 6px; min-width: 28px; min-height: 28px; padding: 2px 4px; }\
        .section-more-btn:hover { opacity: 1.0; background: rgba(255,255,255,0.10); }\
        .photo-info-bar { background: linear-gradient(to bottom, #3b3b3b, #2c2c2c); border-top: 1px solid #151515; min-height: 58px; color: #efefef; }\
        .info-preview { min-width: 40px; min-height: 40px; border-radius: 6px; background: #333; border: 1px solid #555; }\
        .info-title { font-weight: 700; font-size: 13px; color: #f4f4f4; }\
        .metric-key { font-size: 11px; color: #bcbcbc; font-weight: 600; }\
        .metric-val { font-size: 13px; font-weight: 500; color: #eeeeee; }\
        .photo-action-button { min-width: 34px; min-height: 34px; padding: 0; border-radius: 7px; color: #ededed; background: #3a3a3a; border: 1px solid #1b1b1b; box-shadow: inset 0 1px rgba(255,255,255,0.12); transition: background 150ms ease; }\
        .photo-action-button:hover { background: #505050; }\
        button.clear-action-button { color: #2e3436; background: #e6e6e6; border: 1px solid #9a9a9a; }\
        button.clear-action-button:hover { color: #1f2325; background: #f0f0f0; border-color: #777777; }\
        button.clear-action-button:active { background: #d2d2d2; }\
        .favorite-btn.active, .favorite-btn.active image { color: #ff453a; }\
        .one-to-one-btn:checked { color: #ffffff; background: #4d9fdb; }\
        .sidebar-count { min-width: 38px; font-variant-numeric: tabular-nums; color: #a9a9a9; }\
        .section-count { font-size: 13px; color: #bcbcbc; }\
        .group-heading-bar { background: transparent; padding: 0; }\
        .navigation-sidebar row { border-radius: 6px; color: #e8e8e8; }\
        .navigation-sidebar row:hover { background: rgba(255,255,255,0.07); }\
        .navigation-sidebar .sidebar-section-heading { margin-top: 8px; padding-top: 0; }\
        .navigation-sidebar .sidebar-section-heading-title { color: #f0f0f0; font-size: inherit; font-weight: 700; }\
        .navigation-sidebar row:selected { background: #4b7d9e; color: white; }\
        .navigation-sidebar .heading { color: #a9a9a9; font-size: 11px; font-weight: 700; margin-top: 14px; margin-bottom: 4px; }\
        .navigation-sidebar .dim-label, .photo-info-bar .dim-label { color: #bcbcbc; }\
        .lightbox-backdrop { background: #292929; }\
        .lightbox-backdrop.standard-light { background: @view_bg_color; }\
        .lightbox-backdrop.standard-dark { background: #000000; }\
        .lightbox-picture { background: transparent; }\
    ");

    gtk::style_context_add_provider_for_display(
        &gtk::gdk::Display::default().expect("a display is required"),
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
    // Apply the persisted choice before the window's first rendered frame.
    if saved_theme != "iphone" {
        gtk::style_context_add_provider_for_display(
            &display,
            &standard_theme_provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
        );
    }

    let (scan_sender, scan_receiver) = std::sync::mpsc::channel::<ScanUiEvent>();

    let scan_job = Rc::new(RefCell::new(ScanJobState::default()));
    let stop_scan = gtk::Button::from_icon_name("process-stop-symbolic");
    stop_scan.set_tooltip_text(Some("Stop current scan"));
    stop_scan.set_visible(false);
    right_header.pack_end(&stop_scan);

    let start_next_scan: Rc<dyn Fn()> = {
        let scan_job = scan_job.clone();
        let scan_sender = scan_sender.clone();
        Rc::new(move || {
            let next = {
                let mut job = scan_job.borrow_mut();
                job.pending
                    .pop_front()
                    .map(|root| (root, job.generation))
            };
            let Some((root, generation)) = next else {
                return;
            };
            let control = spawn_tagged_scan(root, generation, scan_sender.clone());
            scan_job.borrow_mut().active = Some(control);
        })
    };

    refresh_folder_slot.replace(Some({
        let scan_job = scan_job.clone();
        let start_next_scan = start_next_scan.clone();
        Rc::new(move |path: String| {
            let mut job = scan_job.borrow_mut();
            if let Some(previous) = job.active.take() {
                previous.cancel();
            }
            job.generation = job.generation.wrapping_add(1);
            job.kind = Some(ScanJobKind::Refresh);
            job.pending.clear();
            job.pending.push_back(path);
            job.imported_total = 0;
            job.failed_total = 0;
            drop(job);
            start_next_scan();
        })
    }));

    let scan_job_for_stop = scan_job.clone();
    stop_scan.connect_clicked(move |_| {
        let mut job = scan_job_for_stop.borrow_mut();
        // Stop means the whole current job. In particular, a library refresh
        // must not continue with the next queued folder after cancellation.
        job.pending.clear();
        if let Some(control) = job.active.as_ref() {
            control.cancel();
        }
    });

    // A previously interrupted import can leave valid DB records without
    // previews. Rebuild those previews off the GTK thread at startup.
    let missing_thumbnails: Vec<_> = photos
        .iter()
        .filter_map(|photo| {
            let cache =
                crate::thumbnail::cache_path(&photo.path, photo.mtime, photo.size_bytes).ok()?;
            (!cache.is_file()).then(|| (photo.path.clone(), photo.mtime, photo.size_bytes))
        })
        .collect();
    if !missing_thumbnails.is_empty() {
        scan_job.borrow_mut().kind = Some(ScanJobKind::Maintenance);
        let sender = scan_sender.clone();
        let generation = scan_job.borrow().generation;
        std::thread::spawn(move || {
            let _ = sender.send(ScanUiEvent {
                generation,
                event: scanner::ScanEvent::ThumbnailsStarted {
                    total: missing_thumbnails.len(),
                },
            });
            let results = crate::thumbnail::create_many(&missing_thumbnails, |path| {
                let _ = sender.send(ScanUiEvent {
                    generation,
                    event: scanner::ScanEvent::ThumbnailCreated {
                        path: std::path::PathBuf::from(path),
                    },
                });
            });
            let mut failed = 0;
            for ((path, _, _), result) in missing_thumbnails.into_iter().zip(results) {
                if let Err(error) = result {
                    failed += 1;
                    let _ = sender.send(ScanUiEvent {
                        generation,
                        event: scanner::ScanEvent::Failed {
                            path: std::path::PathBuf::from(path),
                            error: format!("thumbnail: {error}"),
                        },
                    });
                }
            }
            let _ = sender.send(ScanUiEvent {
                generation,
                event: scanner::ScanEvent::Finished {
                    imported: 0,
                    failed,
                },
            });
        });
    }

    let parent = window.clone();
    let scan_job_for_import = scan_job.clone();
    let start_next_scan_for_import = start_next_scan.clone();

    import_folder_slot.replace(Some(Rc::new(move || {
        let scan_job = scan_job_for_import.clone();
        let start_next_scan = start_next_scan_for_import.clone();

        let dialog = gtk::FileChooserNative::new(
            Some("Import Folder"),
            Some(&parent),
            gtk::FileChooserAction::SelectFolder,
            Some("Import"),
            Some("Cancel"),
        );

        dialog.connect_response(move |dialog, response| {
            if response == gtk::ResponseType::Accept {
                if let Some(file) = dialog.file() {
                    let root = crate::source::reference(&file);
                    {
                        let mut job = scan_job.borrow_mut();
                        if let Some(previous) = job.active.take() {
                            previous.cancel();
                        }
                        job.generation = job.generation.wrapping_add(1);
                        job.kind = Some(ScanJobKind::Import);
                        job.pending.clear();
                        job.pending.push_back(root);
                        job.imported_total = 0;
                        job.failed_total = 0;
                    }
                    start_next_scan();
                }
            }

            dialog.destroy();
        });

        dialog.show();
    })));

    let import_folder_for_header = import_folder.clone();
    import.connect_clicked(move |_| import_folder_for_header());

    let scan_job_for_refresh = scan_job.clone();
    let start_next_scan_for_refresh = start_next_scan.clone();
    let connection_for_refresh = connection.clone();
    let availability_refresh_for_app_refresh = availability_refresh.clone();
    let toast_overlay_for_refresh = toast_overlay.clone();

    refresh.connect_clicked(move |_| {
        availability_refresh_for_app_refresh();

        // Always read the live folder list. The list captured when the window
        // was created becomes stale as soon as the user imports a new folder.
        let folders = match db::folders(&connection_for_refresh.borrow()) {
            Ok(folders) => folders,
            Err(error) => {
                eprintln!("Could not refresh library folders: {error}");
                toast_overlay_for_refresh
                    .add_toast(adw::Toast::new("Could not read library folders"));
                return;
            }
        };
        if folders.is_empty() {
            toast_overlay_for_refresh.add_toast(adw::Toast::new("No library folders to refresh"));
            return;
        }

        {
            let mut job = scan_job_for_refresh.borrow_mut();
            if let Some(previous) = job.active.take() {
                previous.cancel();
            }
            job.generation = job.generation.wrapping_add(1);
            job.kind = Some(ScanJobKind::Refresh);
            job.pending = folders
                .into_iter()
                .filter(|folder| folder.imported_root)
                .map(|folder| folder.path)
                .collect();
            if std::env::var_os("PICASA_TRACE").is_some() {
                eprintln!("FOLDER TRACE refresh_roots pending={:?}", job.pending);
            }
            job.imported_total = 0;
            job.failed_total = 0;
        }
        // Refresh folders sequentially. The next one starts only after the
        // current folder's thumbnail pass emits Finished, so scan progress
        // from different folders cannot overwrite one another.
        start_next_scan_for_refresh();
    });

    let parent_for_settings = window.clone();
    let connection_for_clear_thumbnails = connection.clone();
    let gallery_for_clear_thumbnails = gallery.clone();
    let filter_for_clear_thumbnails = filter.clone();
    let search_for_clear_thumbnails = search_text.clone();
    let sort_for_clear_thumbnails = sort.clone();
    let availability_refresh_for_clear_thumbnails = availability_refresh.clone();
    clear_thumbnails.connect_clicked(move |_| {
        let availability_refresh = availability_refresh_for_clear_thumbnails.clone();
        let connection = connection_for_clear_thumbnails.clone();
        let gallery = gallery_for_clear_thumbnails.clone();
        let filter = filter_for_clear_thumbnails.clone();
        let search = search_for_clear_thumbnails.clone();
        let sort = sort_for_clear_thumbnails.clone();
        confirm_action(
            &parent_for_settings,
            "Clear thumbnails?",
            "Cached thumbnails will be deleted. Your photos and database will remain.",
            move || {
                if let Err(error) = crate::thumbnail::clear_cache() {
                    eprintln!("Could not clear thumbnails: {error}");
                }
                refresh_grid(
                    &connection,
                    filter.get(),
                    &search.borrow(),
                    sort.get(),
                    &gallery,
                );
                availability_refresh();
            },
        );
    });

    let parent_for_clear_database = window.clone();
    let connection_for_clear_database = connection.clone();
    let gallery_for_clear_database = gallery.clone();
    let filter_for_clear_database = filter.clone();
    let search_for_clear_database = search_text.clone();
    let sort_for_clear_database = sort.clone();
    let availability_refresh_for_clear_database = availability_refresh.clone();
    clear_database.connect_clicked(move |_| {
        let availability_refresh = availability_refresh_for_clear_database.clone();
        let connection = connection_for_clear_database.clone();
        let gallery = gallery_for_clear_database.clone();
        let filter = filter_for_clear_database.clone();
        let search = search_for_clear_database.clone();
        let sort = sort_for_clear_database.clone();
        confirm_action(
            &parent_for_clear_database,
            "Clear database?",
            "Indexed photos and album links will be removed. Registered folders will remain.",
            move || {
                if let Err(error) = db::clear_photos(&connection.borrow()) {
                    eprintln!("Could not clear database: {error}");
                }
                refresh_grid(
                    &connection,
                    filter.get(),
                    &search.borrow(),
                    sort.get(),
                    &gallery,
                );
                availability_refresh();
            },
        );
    });

    let parent_for_clear_all = window.clone();
    let connection_for_clear_all = connection.clone();
    let gallery_for_clear_all = gallery.clone();
    let filter_for_clear_all = filter.clone();
    let search_for_clear_all = search_text.clone();
    let sort_for_clear_all = sort.clone();
    let availability_refresh_for_clear_all = availability_refresh.clone();
    clear_all.connect_clicked(move |_| {
        let availability_refresh = availability_refresh_for_clear_all.clone();
        let connection = connection_for_clear_all.clone();
        let gallery = gallery_for_clear_all.clone();
        let filter = filter_for_clear_all.clone();
        let search = search_for_clear_all.clone();
        let sort = sort_for_clear_all.clone();
        confirm_action(
            &parent_for_clear_all,
            "Clear everything?",
            "Indexed photos, albums, registered folders, and cached thumbnails will be deleted.",
            move || {
                if let Err(error) = db::clear_all(&connection.borrow()) {
                    eprintln!("Could not clear database: {error}");
                }
                if let Err(error) = crate::thumbnail::clear_cache() {
                    eprintln!("Could not clear thumbnails: {error}");
                }
                refresh_grid(
                    &connection,
                    filter.get(),
                    &search.borrow(),
                    sort.get(),
                    &gallery,
                );
                availability_refresh();
            },
        );
    });

    let gallery_for_events = gallery.clone();
    let connection_for_events = connection.clone();
    let info_for_events = info.clone();
    let selected_photo_for_events = selected_photo.clone();
    let availability_refresh_for_events = availability_refresh.clone();
    let filter_for_events = filter.clone();
    let search_for_events = search_text.clone();
    let sort_for_events = sort.clone();
    let toast_overlay_for_events = toast_overlay.clone();
    let scan_job_for_events = scan_job.clone();
    let start_next_scan_for_events = start_next_scan.clone();
    let stop_scan_for_events = stop_scan.clone();
    let mut displayed_generation: Option<u64> = None;
    let mut scan_count: usize = 0;
    let mut pending_photos: Vec<db::Photo> = Vec::new();
    let mut thumbnails_dirty = false;
    let mut failure_toast_shown = false;
    let mut progress_toast: Option<adw::Toast> = None;
    let mut thumbnail_total: usize = 0;
    let event_started = std::time::Instant::now();

    glib::timeout_add_local(Duration::from_millis(250), move || {
        // Never monopolize the GTK loop when a fast scanner has queued many
        // results. Leaving some events queued lets GTK process input, redraws,
        // scrolling, and folder changes between import batches.
        const MAX_EVENTS_PER_TICK: usize = 128;
        let mut handled_events = 0;
        while handled_events < MAX_EVENTS_PER_TICK {
            let Ok(ui_event) = scan_receiver.try_recv() else {
                break;
            };
            handled_events += 1;

            // Events from a cancelled/superseded scan are still allowed to
            // finish in their worker threads, but they must never overwrite
            // the progress UI or completion state of the newer job.
            if ui_event.generation != scan_job_for_events.borrow().generation {
                if std::env::var_os("PICASA_TRACE").is_some() {
                    eprintln!(
                        "SCAN stale event ignored generation={} current={}",
                        ui_event.generation,
                        scan_job_for_events.borrow().generation
                    );
                }
                continue;
            }

            if displayed_generation != Some(ui_event.generation) {
                displayed_generation = Some(ui_event.generation);
                scan_count = 0;
                thumbnail_total = 0;
                failure_toast_shown = false;
                if let Some(toast) = progress_toast.take() {
                    toast.dismiss();
                }
            }

            let event = ui_event.event;
            match &event {
                scanner::ScanEvent::Started { root } => {
                    if std::env::var_os("PICASA_TRACE").is_some() {
                        eprintln!("SCAN Started root={}", root.display());
                    }
                    scan_count = 0;
                    thumbnail_total = 0;
                    let is_user_job = !matches!(
                        scan_job_for_events.borrow().kind,
                        Some(ScanJobKind::Maintenance) | None
                    );
                    stop_scan_for_events.set_visible(is_user_job);

                    let folder_name = root
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or_else(|| root.to_str().unwrap_or("folder"));
                    if let Some(toast) = progress_toast.as_ref() {
                        toast.set_title(&format!("Scanning {folder_name}…"));
                    } else {
                        let toast = adw::Toast::new(&format!("Scanning {folder_name}…"));
                        toast.set_timeout(0);
                        toast_overlay_for_events.add_toast(toast.clone());
                        progress_toast = Some(toast);
                    }
                }

                scanner::ScanEvent::FolderStarted { folder } => {
                    if std::env::var_os("PICASA_TRACE").is_some() {
                        eprintln!("SCAN FolderStarted id={} path={}", folder.id, folder.path);
                    }
                    run_ui_guarded("sidebar folder append", || {
                        sidebar::append_folder(
                            &sidebar_for_events,
                            folder,
                            availability_refresh_for_events.clone(),
                        )
                    });
                }

                scanner::ScanEvent::PhotoIndexed {
                    path,
                    photo,
                    newly_discovered,
                    ..
                } => {
                    scan_count += 1;
                    if std::env::var_os("PICASA_TRACE").is_some() && scan_count.is_multiple_of(128)
                    {
                        eprintln!(
                            "SCAN PhotoIndexed count={} latest={}",
                            scan_count,
                            path.display()
                        );
                    }
                    if *newly_discovered
                        && crate::image_format::path_is_enabled(
                            &connection_for_events.borrow(),
                            &photo.path,
                        )
                        && (matches!(filter_for_events.get(), sidebar::SidebarFilter::All)
                            || matches!(filter_for_events.get(), sidebar::SidebarFilter::Folder(id) if Some(id) == photo.folder_id))
                    {
                        pending_photos.push(photo.clone());
                    } else if !newly_discovered {
                        gallery_for_events.update_dimensions(photo.id, photo.width, photo.height);
                        if selected_photo_for_events
                            .borrow()
                            .as_ref()
                            .is_some_and(|selected| selected.id() == photo.id)
                        {
                            let selected = selected_photo_for_events.borrow().clone();
                            info_for_events.set_photo(selected.as_ref());
                        }
                    }

                    if let Some(toast) = progress_toast.as_ref() {
                        toast.set_title(&format!("Indexed {scan_count} photos"));
                    }
                }

                scanner::ScanEvent::IndexingFinished { imported } => {
                    // Progressive imports append quickly; rebuild once when
                    // indexing settles so the chosen ordering is restored.
                    refresh_grid(
                        &connection_for_events,
                        filter_for_events.get(),
                        &search_for_events.borrow(),
                        sort_for_events.get(),
                        &gallery_for_events,
                    );
                    if let Some(toast) = progress_toast.as_ref() {
                        toast.set_title(&format!("Indexed {imported} photos"));
                    }
                }

                scanner::ScanEvent::ThumbnailsStarted { total } => {
                    if std::env::var_os("PICASA_TRACE").is_some() {
                        eprintln!("SCAN ThumbnailsStarted total={total}");
                    }
                    scan_count = 0;
                    thumbnail_total = *total;
                    if let Some(toast) = progress_toast.as_ref() {
                        toast.set_title(&format!("Creating thumbnails 0 / {total}"));
                    } else {
                        let toast = adw::Toast::new(&format!("Creating thumbnails 0 / {total}"));
                        toast.set_timeout(0);
                        toast_overlay_for_events.add_toast(toast.clone());
                        progress_toast = Some(toast);
                    }
                }

                scanner::ScanEvent::ThumbnailCreated { path } => {
                    scan_count += 1;
                    if std::env::var_os("PICASA_TRACE").is_some() && scan_count.is_multiple_of(128)
                    {
                        eprintln!(
                            "SCAN ThumbnailCreated count={} latest={}",
                            scan_count,
                            path.display()
                        );
                    }
                    // Coalesce all thumbnail completions received in this
                    // timer tick into one virtualized-grid traversal.
                    thumbnails_dirty = true;
                    if let Some(toast) = progress_toast.as_ref() {
                        toast.set_title(&format!(
                            "Creating thumbnails {scan_count} / {thumbnail_total}"
                        ));
                    }
                }

                scanner::ScanEvent::Failed { path, error } => {
                    eprintln!("SCAN FAILED: {}: {}", path.display(), error);
                    if let Some(toast) = progress_toast.as_ref() {
                        toast.set_title("Scanning… some files failed");
                    }

                    // Keep one failure visible in the toast area, but do not
                    // enqueue thousands of toasts for a damaged folder.
                    if !failure_toast_shown {
                        failure_toast_shown = true;
                        toast_overlay_for_events.add_toast(adw::Toast::new(&format!(
                            "Some files could not be added: {}",
                            error
                        )));
                    }
                }

                scanner::ScanEvent::Finished { imported, failed } => {
                    if std::env::var_os("PICASA_TRACE").is_some() {
                        eprintln!("SCAN Finished imported={imported} failed={failed}");
                    }
                    eprintln!(
                        "===== SCAN COMPLETE: imported={} failed={} | progressive gallery updates complete =====",
                        imported,
                        failed
                    );

                    let (kind, has_more, total_imported, total_failed) = {
                        let mut job = scan_job_for_events.borrow_mut();
                        job.active = None;
                        job.imported_total += *imported;
                        job.failed_total += *failed;
                        (
                            job.kind,
                            !job.pending.is_empty(),
                            job.imported_total,
                            job.failed_total,
                        )
                    };

                    if kind == Some(ScanJobKind::Refresh) && has_more {
                        // Start exactly one next folder. Because scanner::Finished
                        // arrives after that folder's thumbnail pass, refresh
                        // progress remains serialized end-to-end.
                        start_next_scan_for_events();
                        continue;
                    }

                    stop_scan_for_events.set_visible(false);
                    if let Some(toast) = progress_toast.take() {
                        toast.dismiss();
                    }

                    let message = match kind {
                        Some(ScanJobKind::Refresh) => {
                            if total_failed == 0 {
                                format!("Library refresh complete · {total_imported} photos updated")
                            } else {
                                format!(
                                    "Library refresh complete · {total_imported} updated · {total_failed} failed"
                                )
                            }
                        }
                        Some(ScanJobKind::Maintenance) => {
                            if total_failed == 0 {
                                "Thumbnail recovery complete".to_string()
                            } else {
                                format!("Thumbnail recovery complete · {total_failed} failed")
                            }
                        }
                        _ => {
                            if total_failed == 0 {
                                format!("Added {total_imported} photos")
                            } else {
                                format!("Added {total_imported} photos · {total_failed} failed")
                            }
                        }
                    };
                    scan_job_for_events.borrow_mut().kind = None;
                    availability_refresh_for_events();
                    toast_overlay_for_events.add_toast(adw::Toast::new(&message));
                }

                scanner::ScanEvent::Cancelled { imported } => {
                    let kind = {
                        let mut job = scan_job_for_events.borrow_mut();
                        job.active = None;
                        job.pending.clear();
                        job.imported_total += *imported;
                        let kind = job.kind;
                        job.kind = None;
                        kind
                    };
                    stop_scan_for_events.set_visible(false);
                    let message = match kind {
                        Some(ScanJobKind::Refresh) => {
                            format!("Library refresh stopped · {imported} photos updated")
                        }
                        _ => format!("Import stopped · {imported} photos added"),
                    };
                    if let Some(toast) = progress_toast.take() {
                        toast.dismiss();
                    }
                    toast_overlay_for_events.add_toast(adw::Toast::new(&message));
                }
            }
        }

        if std::env::var_os("PICASA_TRACE").is_some() && handled_events > 0 {
            eprintln!(
                "UI PERF event_tick events={} elapsed_ms={}",
                handled_events,
                event_started.elapsed().as_millis()
            );
        }

        if !pending_photos.is_empty() {
            if std::env::var_os("PICASA_TRACE").is_some() {
                eprintln!(
                    "UI PERF photo_batch={} elapsed_ms={}",
                    pending_photos.len(),
                    event_started.elapsed().as_millis()
                );
            }
            run_ui_guarded("photo batch append", || {
                gallery_for_events.append_photos(&pending_photos)
            });
            pending_photos.clear();
        }
        if thumbnails_dirty {
            run_ui_guarded("thumbnail refresh", || {
                gallery_for_events.refresh_thumbnails()
            });
            thumbnails_dirty = false;
        }

        glib::ControlFlow::Continue
    });

    window
}
