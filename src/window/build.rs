include!("dialogs.rs");
include!("navigation.rs");

pub fn build(app: &adw::Application, connection: Connection) -> adw::ApplicationWindow {
    let build_started = Instant::now();
    let window = adw::ApplicationWindow::new(app);
    // Test hook: fullscreen layout for UI automation.
    if std::env::var_os("PIC_TEST_FULLSCREEN").is_some() {
        window.fullscreen();
    }
    window.set_title(Some("PIC - Picasa iPhoto Clone"));
    window.set_default_size(1440, 900);

    crate::source::install_ui_heartbeat();

    install_close_confirmation(&window);

    let connection = Rc::new(RefCell::new(connection));
    let folders = db::folders_cached(&connection.borrow()).unwrap_or_default();
    let folder_cache = Rc::new(RefCell::new(folders.clone()));
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
    let mut all_startup_photos = db::photos(&connection.borrow(), None, false, None)
        .unwrap_or_default();
    retain_enabled_formats(&connection.borrow(), &mut all_startup_photos);
    let mut photos = all_startup_photos.clone();
    limit_recently_added(
        &connection.borrow(),
        sidebar::SidebarFilter::RecentlyAdded,
        &mut photos,
    );
    sort_photos(&mut photos, sort.get());
    let startup_photos = Rc::new(photos);
    eprintln!(
        "STARTUP cold_start_ms={} photos={} displayed={} folders={} albums={} scan=disabled rss_mb={}",
        build_started.elapsed().as_millis(),
        sidebar_counts.photos,
        startup_photos.len(),
        folders.len(),
        albums.len(),
        crate::diagnostics::rss_mb()
    );

    let info = Rc::new(InfoBar::new());
    info.set_photo(None);

    let selected_photo = Rc::new(RefCell::new(None));

    let lightbox = Rc::new(Lightbox::new());
    let info_for_lightbox = info.clone();
    let selected_photo_for_lightbox = selected_photo.clone();

    // Appearance themes are discovered from the themes folder at runtime; the
    // engine applies them, persists the choice, and is shared with the
    // Settings → Themes picker (the single theme list in the app).
    let display = gtk::gdk::Display::default().expect("a display is required");
    let theme_engine = crate::window::theme::ThemeEngine::new(
        display.clone(),
        connection.clone(),
        lightbox.clone(),
    );
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
    let edit_space_slot: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));
    let collection_navigation_slot: Rc<RefCell<Option<Rc<dyn Fn(i32)>>>> =
        Rc::new(RefCell::new(None));
    let search_popup_slot: Rc<RefCell<Option<gtk::Popover>>> = Rc::new(RefCell::new(None));
    let space_toggle_in_progress = Rc::new(Cell::new(false));

    include!("shortcuts.rs");

    let info_for_grid = info.clone();
    let selected_photo_for_grid = selected_photo.clone();
    let lightbox_for_grid = lightbox.clone();
    let filter = Rc::new(Cell::new(sidebar::SidebarFilter::RecentlyAdded));
    let search_text = Rc::new(RefCell::new(String::new()));
    let search_entry_slot: Rc<RefCell<Option<gtk::SearchEntry>>> = Rc::new(RefCell::new(None));
    let search_suppressed = Rc::new(Cell::new(false));
    let cleared_search_query = Rc::new(RefCell::new(None::<String>));
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
    let add_network_share_slot: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));
    let edit_open_slot: Rc<RefCell<Option<Rc<dyn Fn(i64)>>>> = Rc::new(RefCell::new(None));
    let edit_clipboard: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let open_edit: Rc<dyn Fn(i64)> = {
        let slot = edit_open_slot.clone();
        Rc::new(move |id| {
            if let Some(callback) = slot.borrow().as_ref() {
                callback(id);
            }
        })
    };
    let collage_open_slot: Rc<RefCell<Option<Rc<dyn Fn(Vec<i64>)>>>> =
        Rc::new(RefCell::new(None));
    let collage_add_mode_slot: Rc<RefCell<Option<Rc<dyn Fn()>>>> =
        Rc::new(RefCell::new(None));
    let collage_prepare_add_slot: Rc<RefCell<Option<Rc<dyn Fn()>>>> =
        Rc::new(RefCell::new(None));
    let collage_close_slot: Rc<RefCell<Option<Rc<dyn Fn()>>>> =
        Rc::new(RefCell::new(None));
    // True while the user is on the photos page picking images for the open
    // collage. The collage editor must survive that detour; every other
    // transition away from the collage page tears it down.
    let collage_add_mode: Rc<Cell<bool>> = Rc::new(Cell::new(false));
    // True while the photo editor is editing a photo that belongs to the
    // open collage. The collage editor survives that detour, and the
    // editor's Back/Done return to the collage instead of the photos page.
    let collage_editing: Rc<Cell<bool>> = Rc::new(Cell::new(false));
    let open_collage: Rc<dyn Fn(Vec<i64>)> = {
        let slot = collage_open_slot.clone();
        Rc::new(move |ids| {
            if let Some(callback) = slot.borrow().as_ref() {
                callback(ids);
            }
        })
    };
    let refresh_folder_slot: Rc<
        RefCell<Option<Rc<dyn Fn(PhotoScanRequestReason, String)>>>,
    > =
        Rc::new(RefCell::new(None));

    // Keep filesystem monitors alive for exactly the folders the user marked
    // for watching. Raw monitor callbacks only mark the owning imported root
    // dirty; they never start a scan directly.
    let folder_watch_monitors: Rc<RefCell<Vec<gio::FileMonitor>>> =
        Rc::new(RefCell::new(Vec::new()));
    let pending_watch_refreshes: Rc<
        RefCell<std::collections::HashMap<String, Instant>>,
    > = Rc::new(RefCell::new(std::collections::HashMap::new()));
    let rebuild_folder_watches: Rc<dyn Fn()> = {
        let connection = connection.clone();
        let monitors = folder_watch_monitors.clone();
        let pending = pending_watch_refreshes.clone();
        Rc::new(move || {
            for monitor in monitors.borrow_mut().drain(..) {
                monitor.cancel();
            }
            pending.borrow_mut().clear();

            if !db::folder_watching_enabled(&connection.borrow()) {
                
                return;
            }

            let Ok(folders) = db::folders_cached(&connection.borrow()) else {
                eprintln!("WATCH ERROR could not read folders");
                return;
            };
            for folder in folders.iter().filter(|folder| folder.watched && folder.available) {
                let Some(scan_root) = watch_scan_root(&folders, folder.id) else {
                    
                    continue;
                };
                let watched_path = folder.path.clone();
                let file = crate::source::file(&watched_path);
                let monitor = match file.monitor_directory(
                    gio::FileMonitorFlags::NONE,
                    gio::Cancellable::NONE,
                ) {
                    Ok(monitor) => monitor,
                    Err(error) => {
                        eprintln!(
                            "WATCH ERROR monitor path={} error={}",
                            watched_path, error
                        );
                        continue;
                    }
                };
                let pending_for_event = pending.clone();
                let scan_root_for_event = scan_root.clone();
                monitor.connect_changed(move |_, file, other_file, event| {
                    // Ignore metadata-only monitor noise. Content changes are
                    // coalesced below before they can authorize any scan.
                    if matches!(
                        event,
                        gio::FileMonitorEvent::AttributeChanged
                            | gio::FileMonitorEvent::PreUnmount
                            | gio::FileMonitorEvent::Unmounted
                    ) {
                        return;
                    }
                    pending_for_event
                        .borrow_mut()
                        .insert(scan_root_for_event.clone(), Instant::now());
                    
                });
                monitors.borrow_mut().push(monitor);
            }
            
        })
    };
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
        let folder_cache = folder_cache.clone();
        let gallery = gallery_for_actions.clone();
        let sidebar = sidebar_for_unavailable.clone();
        let slot = availability_refresh_slot.clone();
        let create_album = create_album.clone();
        let import_folder = import_folder.clone();
        let delete_album = delete_album.clone();
        Rc::new(move || {
            
            debug_assert!(PhotoScanRequestReason::AvailabilityUpdate
                .scan_kind()
                .is_none());
            refresh_availability_ui(
                &connection,
                &folder_cache,
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
    let folder_navigation_slot: Rc<RefCell<Option<Rc<dyn Fn(i64, i64)>>>> =
        Rc::new(RefCell::new(None));
    // Open in Folder keeps this exact target alive briefly so a sidebar
    // Tree-mode reorder cannot replace it with a generic folder-header scroll.
    let open_in_folder_exact_target: Rc<Cell<Option<i64>>> = Rc::new(Cell::new(None));
    let navigate_to_folder: Rc<dyn Fn(i64, i64)> = {
        let slot = folder_navigation_slot.clone();
        Rc::new(move |folder_id, photo_id| {
            if let Some(callback) = slot.borrow().as_ref() {
                callback(folder_id, photo_id);
            }
        })
    };
    let context_menu_host: Rc<RefCell<Option<glib::WeakRef<gtk::Overlay>>>> =
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
        navigate_to_folder: navigate_to_folder.clone(),
        open_collage: open_collage.clone(),
        open_edit: open_edit.clone(),
        edit_clipboard: edit_clipboard.clone(),
        refresh_albums_home: {
            let slot = albums_home_refresh_slot.clone();
            Rc::new(move |albums| {
                if let Some(refresh) = slot.borrow().as_ref() {
                    refresh(albums);
                }
            })
        },
        window: window.clone().upcast::<gtk::Window>().downgrade(),
        context_menu_host: context_menu_host.clone(),
    };
    let saved_grid_thumbnail_size = grid_thumbnail_size_from_setting(&connection.borrow());

    // Result activation should dismiss the visible search UI without running the
    // normal empty-query handler. Running that handler here would immediately
    // rebuild the current full library/folder model while the user is opening a
    // result, which is both unnecessary and can stall the GTK thread.
    let clear_search_after_result: Rc<dyn Fn()> = {
        let search_entry = search_entry_slot.clone();
        let search_text = search_text.clone();
        let suppressed = search_suppressed.clone();
        let debounce = search_debounce.clone();
        let cleared_query = cleared_search_query.clone();
        Rc::new(move || {
            if let Some(source) = debounce.borrow_mut().take() {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| source.remove()));
            }
            let query = search_text.borrow().clone();
            cleared_query.replace(Some(query.clone()));
            suppressed.set(true);
            if let Some(entry) = search_entry.borrow().as_ref() {
                entry.set_text("");
            }
            search_text.replace(String::new());
            suppressed.set(false);
            let cleared_query_for_timeout = cleared_query.clone();
            glib::timeout_add_local_once(Duration::from_millis(500), move || {
                if cleared_query_for_timeout.borrow().as_deref() == Some(query.as_str()) {
                    cleared_query_for_timeout.replace(None);
                }
            });
            
        })
    };

    let gallery = Rc::new(grid::Gallery::new(
        &[],
        saved_grid_thumbnail_size.unwrap_or(DEFAULT_GRID_THUMBNAIL_SIZE),
        move |photo| {
            info_for_grid.set_photo(photo.as_ref());
            selected_photo_for_grid.replace(photo);
        },
        {
            let availability_refresh = availability_refresh.clone();
            let clear_search_after_result = clear_search_after_result.clone();
            move |photos, selected_index| {
                if photos
                    .get(selected_index)
                    .is_some_and(|photo| !photo.original_available())
                {
                    availability_refresh();
                }
                lightbox_for_grid.open(photos, selected_index);
                clear_search_after_result();
            }
        },
        {
            let action_context = action_context.clone();
            move |photo, anchor, x, y| {
                show_photo_context_menu(photo, anchor, action_context.clone(), x, y);
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
    // No stored size (or only the untouched legacy default): adopt the
    // ~4-thumbnails-per-row view level on the first real layout.
    if saved_grid_thumbnail_size.is_none() {
        gallery.enable_auto_default_zoom();
    }
    // Resolved size for views that only need a number (album covers).
    let grid_thumbnail_size = saved_grid_thumbnail_size.unwrap_or(DEFAULT_GRID_THUMBNAIL_SIZE);

    // Thumbnail appearance (Settings > Library). Square corners toggle a CSS
    // class on the main window; whole-photo fit is applied to the gallery and
    // re-applied live when the toggles change.
    if crate::settings::saved_bool(
        &connection.borrow(),
        crate::db::THUMBNAIL_SQUARE_CORNERS_SETTING_KEY,
    )
    .unwrap_or(false)
    {
        window.add_css_class("square-corners");
    }
    gallery.set_fit_whole_photo(
        crate::settings::saved_bool(
            &connection.borrow(),
            crate::db::THUMBNAIL_FIT_WHOLE_PHOTO_SETTING_KEY,
        )
        .unwrap_or(false),
    );
    gallery_for_actions.replace(Rc::downgrade(&gallery));
    apply_gallery_grouping(&gallery, filter.get(), sort.get(), group_mode.get());

    // Reuse the gallery model for the Space shortcut. Re-querying the database
    // and rebuilding the complete library model here blocked the GTK thread.
    {
        let gallery = gallery.clone();
        let selected_photo = selected_photo.clone();
        let lightbox = lightbox.clone();
        let availability_refresh = availability_refresh.clone();

        space_open_slot.replace(Some(Rc::new(move || {
            let photos = gallery.photo_objects();
            // Scroll-then-open: when 1:1/Space is activated within a short
            // grace after a wheel/touchpad scroll and the pointer rests on a
            // thumbnail, that photo becomes the selection and opens. This is
            // the fast "scroll, then Space through photos" flow. Hover alone
            // never changes anything: the plain selection always wins.
            let scroll_hovered =
                gallery.hovered_photo_after_scroll(grid::SCROLL_HOVER_OPEN_GRACE);
            if let Some(hovered) = scroll_hovered.as_ref() {
                gallery.set_selected_photo_ids(&[hovered.id()]);
            }
            if std::env::var_os("PIC_DEBUG_SPACE").is_some() {
                match scroll_hovered.as_ref() {
                    Some(hovered) => eprintln!(
                        "[space-debug] slot: scroll-hover wins -> {}",
                        hovered.filename()
                    ),
                    None => eprintln!("[space-debug] slot: no scroll-hover -> selection decides"),
                }
            }
            let selected_id = scroll_hovered
                .as_ref()
                .map(|photo| photo.id())
                .or_else(|| {
                    selected_photo
                        .borrow()
                        .as_ref()
                        .map(|photo| photo.id())
                })
                .filter(|id| photos.iter().any(|photo| photo.id() == *id))
                .or_else(|| gallery.selected_photo_ids(None).into_iter().next());
            let Some(selected_id) = selected_id else {
                return;
            };
            let Some(index) = photos.iter().position(|photo| photo.id() == selected_id) else {
                return;
            };
            if !photos[index].original_available() {
                availability_refresh();
            }
            lightbox.open(photos, index);
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

    // Destructive maintenance actions for the Settings → Library page. The
    // settings window owns the buttons and confirmation dialogs; the actual
    // behaviour stays here where the gallery and refresh context live.
    let settings_maintenance = crate::settings::LibraryMaintenance {
        clear_thumbnails: {
            let connection = connection.clone();
            let gallery = gallery.clone();
            let filter = filter.clone();
            let search = search_text.clone();
            let sort = sort.clone();
            let availability_refresh = availability_refresh.clone();
            Rc::new(move || {
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
            })
        },
        clear_database: {
            let connection = connection.clone();
            let gallery = gallery.clone();
            let filter = filter.clone();
            let search = search_text.clone();
            let sort = sort.clone();
            let availability_refresh = availability_refresh.clone();
            Rc::new(move || {
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
            })
        },
        clear_all: {
            let connection = connection.clone();
            let gallery = gallery.clone();
            let filter = filter.clone();
            let search = search_text.clone();
            let sort = sort.clone();
            let availability_refresh = availability_refresh.clone();
            Rc::new(move || {
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
            })
        },
    };

    let settings_window = crate::settings::SettingsWindow::default();
    let settings_parent = window.clone();
    let settings_surface = window.clone();
    let settings_gallery_for_thumbs = gallery.clone();
    let settings_connection = connection.clone();
    let settings_gallery = gallery.clone();
    let settings_filter = filter.clone();
    let settings_search = search_text.clone();
    let settings_sort = sort.clone();
    let settings_lightbox = lightbox.clone();
    let settings_sidebar = sidebar_for_unavailable.clone();
    let settings_on_unavailable = availability_refresh.clone();
    let settings_albums_refresh = albums_home_refresh_slot.clone();
    let settings_rebuild_folder_watches = rebuild_folder_watches.clone();
    let settings_theme_engine = theme_engine.clone();
    let present_settings: Rc<dyn Fn(Option<&'static str>)> = Rc::new(move |initial_page| {
        let connection = settings_connection.clone();
        let gallery = settings_gallery.clone();
        let filter = settings_filter.clone();
        let search = settings_search.clone();
        let sort = settings_sort.clone();
        let lightbox = settings_lightbox.clone();
        let sidebar = settings_sidebar.clone();
        let on_unavailable = settings_on_unavailable.clone();
        let theme_connection = settings_connection.clone();
        let theme_albums_refresh = settings_albums_refresh.clone();
        let watch_connection = settings_connection.clone();
        let watch_sidebar = settings_sidebar.clone();
        let watch_on_unavailable = settings_on_unavailable.clone();
        let maintenance = settings_maintenance.clone();
        let thumbs_window = settings_surface.clone();
        let thumbs_gallery = settings_gallery_for_thumbs.clone();
        let thumbs_connection = settings_connection.clone();
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
                if let Some(sidebar) = sidebar.borrow().as_ref().cloned() {
                    if let Ok(folders) = db::folders_cached(&connection.borrow()) {
                        sidebar::refresh_folder_rows(&sidebar, &folders, &on_unavailable);
                    }
                    if let Ok(counts) = db::sidebar_counts(&connection.borrow()) {
                        sidebar::refresh_library_counts(&sidebar, counts, &on_unavailable);
                    }
                }
            }),
            Rc::new(move || {
                let albums = db::albums(&theme_connection.borrow()).unwrap_or_default();
                if let Some(refresh) = theme_albums_refresh.borrow().as_ref() {
                    refresh(&albums);
                }
            }),
            Rc::new(move || {
                crate::window::debug_log("THUMB SETTINGS: apply callback entered");
                let square = crate::settings::saved_bool(
                    &thumbs_connection.borrow(),
                    crate::db::THUMBNAIL_SQUARE_CORNERS_SETTING_KEY,
                )
                .unwrap_or(false);
                let fit = crate::settings::saved_bool(
                    &thumbs_connection.borrow(),
                    crate::db::THUMBNAIL_FIT_WHOLE_PHOTO_SETTING_KEY,
                )
                .unwrap_or(false);
                if square {
                    thumbs_window.add_css_class("square-corners");
                } else {
                    thumbs_window.remove_css_class("square-corners");
                }
                crate::window::debug_log(&format!(
                    "THUMB SETTINGS: css class applied (square={square}), deferring fit={fit} to idle"
                ));
                // Defer the tile walk out of the switch notification so the
                // settings UI settles before the gallery relayouts.
                let gallery_for_fit = thumbs_gallery.clone();
                glib::idle_add_local_once(move || {
                    crate::window::debug_log("THUMB SETTINGS: set_fit_whole_photo begin");
                    gallery_for_fit.set_fit_whole_photo(fit);
                    crate::window::debug_log("THUMB SETTINGS: set_fit_whole_photo end");
                });
                crate::window::debug_log("THUMB SETTINGS: apply callback exit");
            }),
            {
                let rebuild_folder_watches = settings_rebuild_folder_watches.clone();
                Rc::new(move || {
                    rebuild_folder_watches();
                    if let Some(sidebar) = watch_sidebar.borrow().as_ref().cloned() {
                        if let Ok(folders) = db::folders_cached(&watch_connection.borrow()) {
                            sidebar::refresh_folder_rows(
                                &sidebar,
                                &folders,
                                &watch_on_unavailable,
                            );
                        }
                    }
                })
            },
            maintenance,
            settings_theme_engine.clone(),
            initial_page,
        );
    });
    let present_settings_from_more = present_settings.clone();
    info.more.connect_clicked(move |_| {
        present_settings_from_more(None);
    });

    let action_context_for_lightbox = action_context.clone();
    lightbox.set_context_menu_handler(move |photo, anchor, x, y| {
        show_photo_context_menu(photo, anchor, action_context_for_lightbox.clone(), x, y);
    });

    // Existing collection navigation remains available to the non-Folder
    // lightbox views. Folder mode no longer switches collections: its gallery
    // already contains the complete continuous folder stream.
    let connection_for_collection_nav = connection.clone();
    let filter_for_collection_nav = filter.clone();
    let search_for_collection_nav = search_text.clone();
    let sort_for_collection_nav = sort.clone();
    let group_mode_for_collection_nav = group_mode.clone();
    let gallery_for_collection_nav = gallery.clone();
    let lightbox_for_collection_nav = lightbox.clone();
    let sidebar_selection_for_collection_nav = sidebar_selection_slot.clone();
    let selected_photo_for_collection_nav = selected_photo.clone();
    let collection_navigation: Rc<dyn Fn(i32)> = Rc::new(move |direction| {
        if direction == 0 {
            return;
        }

        let step: isize = if direction < 0 { -1 } else { 1 };
        let current_filter = filter_for_collection_nav.get();
        // Folder mode is already one continuous collection, so Up/Down must
        // not rebuild the grid into another single-folder page. Other existing
        // collection navigation (Albums/Library) keeps its previous behavior.
        if matches!(current_filter, sidebar::SidebarFilter::Folder(_)) {
            return;
        }
        let search = search_for_collection_nav.borrow().clone();

        match current_filter {
            sidebar::SidebarFilter::Folder(current_folder_id) => {
                // Navigate in the order the user can actually see in the
                // Folders sidebar. Tree mode therefore follows visible tree
                // rows, while Imported-only mode contains only imported roots.
                let folders = db::folders_cached(&connection_for_collection_nav.borrow())
                    .unwrap_or_default();
                let folder_ids = sidebar_selection_for_collection_nav
                    .borrow()
                    .as_ref()
                    .map(sidebar::visible_folder_ids)
                    .unwrap_or_else(|| folders.iter().map(|folder| folder.id).collect());
                let Some(current_index) = folder_ids
                    .iter()
                    .position(|folder_id| *folder_id == current_folder_id)
                else {
                    return;
                };

                let mut candidate = current_index as isize + step;
                while candidate >= 0 && candidate < folder_ids.len() as isize {
                    let folder_id = folder_ids[candidate as usize];
                    let Some(folder) = folders.iter().find(|folder| folder.id == folder_id) else {
                        candidate += step;
                        continue;
                    };
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
                            let sidebar = sidebar.clone();
                            let folder_id = folder.id;
                            glib::timeout_add_local_once(Duration::from_millis(100), move || {
                                sidebar::scroll_to_folder(&sidebar, folder_id);
                            });
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
                        if lightbox_for_collection_nav.root.is_visible() {
                            lightbox_for_collection_nav.open(objects, 0);
                        }
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
                let first_available_album = albums.iter().position(|album| {
                    let mut photos = db::photos_in_album(
                        &connection_for_collection_nav.borrow(),
                        album.id,
                        (!search.is_empty()).then_some(search.as_str()),
                    )
                    .unwrap_or_default();
                    retain_enabled_formats(&connection_for_collection_nav.borrow(), &mut photos);
                    !photos.is_empty()
                });
                let last_available_album = albums.iter().rposition(|album| {
                    let mut photos = db::photos_in_album(
                        &connection_for_collection_nav.borrow(),
                        album.id,
                        (!search.is_empty()).then_some(search.as_str()),
                    )
                    .unwrap_or_default();
                    retain_enabled_formats(&connection_for_collection_nav.borrow(), &mut photos);
                    !photos.is_empty()
                });

                // The album sequence is connected to Recently Added at its
                // upper boundary. Selecting the last item here makes reverse
                // navigation feel continuous instead of opening another
                // album unexpectedly.
                if direction < 0 && Some(current_index) == first_available_album {
                    let mut photos = db::photos(
                        &connection_for_collection_nav.borrow(),
                        None,
                        false,
                        (!search.is_empty()).then_some(search.as_str()),
                    )
                    .unwrap_or_default();
                    retain_enabled_formats(&connection_for_collection_nav.borrow(), &mut photos);
                    limit_recently_added(
                        &connection_for_collection_nav.borrow(),
                        sidebar::SidebarFilter::RecentlyAdded,
                        &mut photos,
                    );
                    sort_photos(&mut photos, sort_for_collection_nav.get());
                    if !photos.is_empty() {
                        let new_filter = sidebar::SidebarFilter::RecentlyAdded;
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
                        gallery_for_collection_nav.select_last_photo();
                        let objects = photos
                            .iter()
                            .map(crate::photo_object::PhotoObject::from_photo)
                            .collect::<Vec<_>>();
                        if lightbox_for_collection_nav.root.is_visible() {
                            lightbox_for_collection_nav.open(objects, photos.len() - 1);
                        }
                        return;
                    }
                }

                // The last album connects to the first available folder.
                if direction > 0 && Some(current_index) == last_available_album {
                    let folders = db::folders_cached(&connection_for_collection_nav.borrow())
                        .unwrap_or_default();
                    for folder in folders {
                        let mut photos = db::photos(
                            &connection_for_collection_nav.borrow(),
                            Some(folder.id),
                            false,
                            (!search.is_empty()).then_some(search.as_str()),
                        )
                        .unwrap_or_default();
                        retain_enabled_formats(&connection_for_collection_nav.borrow(), &mut photos);
                        sort_photos(&mut photos, sort_for_collection_nav.get());
                        if photos.is_empty() {
                            continue;
                        }

                        let new_filter = sidebar::SidebarFilter::Folder(folder.id);
                        filter_for_collection_nav.set(new_filter);
                        if let Some(sidebar) = sidebar_selection_for_collection_nav.borrow().as_ref() {
                            sidebar::set_active_filter(sidebar, new_filter);
                            let sidebar = sidebar.clone();
                            let folder_id = folder.id;
                            glib::timeout_add_local_once(Duration::from_millis(100), move || {
                                sidebar::scroll_to_folder(&sidebar, folder_id);
                            });
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
                        if lightbox_for_collection_nav.root.is_visible() {
                            lightbox_for_collection_nav.open(objects, 0);
                        }
                        return;
                    }
                }

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
                        if lightbox_for_collection_nav.root.is_visible() {
                            lightbox_for_collection_nav.open(objects, 0);
                        }
                        return;
                    }

                    candidate += step;
                }
            }
            sidebar::SidebarFilter::All
            | sidebar::SidebarFilter::Favorites
            | sidebar::SidebarFilter::RecentlyAdded => {
                // Photos connects forward to Favorites at its last item.
                if current_filter == sidebar::SidebarFilter::All && direction > 0 {
                    let mut photos = db::photos(
                        &connection_for_collection_nav.borrow(),
                        None,
                        true,
                        (!search.is_empty()).then_some(search.as_str()),
                    )
                    .unwrap_or_default();
                    retain_enabled_formats(&connection_for_collection_nav.borrow(), &mut photos);
                    sort_photos(&mut photos, sort_for_collection_nav.get());
                    if !photos.is_empty() {
                        let new_filter = sidebar::SidebarFilter::Favorites;
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
                        if lightbox_for_collection_nav.root.is_visible() {
                            lightbox_for_collection_nav.open(objects, 0);
                        }
                        return;
                    }
                }

                // Favorites sits between Photos and Recently Added in the
                // keyboard sequence. Keep both boundaries continuous and
                // select the correct end of the destination collection.
                if current_filter == sidebar::SidebarFilter::Favorites
                    && direction < 0
                {
                    let mut photos = db::photos(
                        &connection_for_collection_nav.borrow(),
                        None,
                        false,
                        (!search.is_empty()).then_some(search.as_str()),
                    )
                    .unwrap_or_default();
                    retain_enabled_formats(&connection_for_collection_nav.borrow(), &mut photos);
                    sort_photos(&mut photos, sort_for_collection_nav.get());
                    if !photos.is_empty() {
                        let new_filter = sidebar::SidebarFilter::All;
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
                        gallery_for_collection_nav.select_last_photo();
                        let objects = photos
                            .iter()
                            .map(crate::photo_object::PhotoObject::from_photo)
                            .collect::<Vec<_>>();
                        if lightbox_for_collection_nav.root.is_visible() {
                            lightbox_for_collection_nav.open(objects, photos.len() - 1);
                        }
                        return;
                    }
                }

                if current_filter == sidebar::SidebarFilter::Favorites
                    && direction > 0
                {
                    let mut photos = db::photos(
                        &connection_for_collection_nav.borrow(),
                        None,
                        false,
                        (!search.is_empty()).then_some(search.as_str()),
                    )
                    .unwrap_or_default();
                    retain_enabled_formats(&connection_for_collection_nav.borrow(), &mut photos);
                    limit_recently_added(
                        &connection_for_collection_nav.borrow(),
                        sidebar::SidebarFilter::RecentlyAdded,
                        &mut photos,
                    );
                    sort_photos(&mut photos, sort_for_collection_nav.get());
                    if !photos.is_empty() {
                        let new_filter = sidebar::SidebarFilter::RecentlyAdded;
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
                        if lightbox_for_collection_nav.root.is_visible() {
                            lightbox_for_collection_nav.open(objects, 0);
                        }
                        return;
                    }
                }

                if current_filter == sidebar::SidebarFilter::RecentlyAdded
                    && direction < 0
                {
                    let mut photos = db::photos(
                        &connection_for_collection_nav.borrow(),
                        None,
                        true,
                        (!search.is_empty()).then_some(search.as_str()),
                    )
                    .unwrap_or_default();
                    retain_enabled_formats(&connection_for_collection_nav.borrow(), &mut photos);
                    sort_photos(&mut photos, sort_for_collection_nav.get());
                    if !photos.is_empty() {
                        let new_filter = sidebar::SidebarFilter::Favorites;
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
                        gallery_for_collection_nav.select_last_photo();
                        let objects = photos
                            .iter()
                            .map(crate::photo_object::PhotoObject::from_photo)
                            .collect::<Vec<_>>();
                        if lightbox_for_collection_nav.root.is_visible() {
                            lightbox_for_collection_nav.open(objects, photos.len() - 1);
                        }
                        return;
                    }
                }

                // Recently Added connects forward to the first album. This
                // boundary must win over folder-based navigation, otherwise
                // the selected photo's folder can jump to an unrelated album.
                if current_filter == sidebar::SidebarFilter::RecentlyAdded && direction > 0 {
                    let albums = db::albums(&connection_for_collection_nav.borrow())
                        .unwrap_or_default();
                    for album in albums {
                        let mut photos = db::photos_in_album(
                            &connection_for_collection_nav.borrow(),
                            album.id,
                            (!search.is_empty()).then_some(search.as_str()),
                        )
                        .unwrap_or_default();
                        retain_enabled_formats(&connection_for_collection_nav.borrow(), &mut photos);
                        sort_photos(&mut photos, sort_for_collection_nav.get());
                        if photos.is_empty() {
                            continue;
                        }

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
                        if lightbox_for_collection_nav.root.is_visible() {
                            lightbox_for_collection_nav.open(objects, 0);
                        }
                        return;
                    }
                }

                // Library views contain photos from multiple folders. Use the
                // selected thumbnail's folder as the current position.
                let Some(current_folder_id) = selected_photo_for_collection_nav
                    .borrow()
                    .as_ref()
                    .map(|photo| photo.folder_id())
                    .filter(|folder_id| *folder_id > 0)
                else {
                    return;
                };
                let folders = db::folders_cached(&connection_for_collection_nav.borrow())
                    .unwrap_or_default();
                let Some(current_index) = folders
                    .iter()
                    .position(|folder| folder.id == current_folder_id)
                else {
                    return;
                };

                let mut candidate = current_index as isize + step;
                let favorites_only = current_filter == sidebar::SidebarFilter::Favorites;
                while candidate >= 0 && candidate < folders.len() as isize {
                    let folder = &folders[candidate as usize];
                    let mut photos = db::photos(
                        &connection_for_collection_nav.borrow(),
                        Some(folder.id),
                        favorites_only,
                        (!search.is_empty()).then_some(search.as_str()),
                    )
                    .unwrap_or_default();
                    retain_enabled_formats(&connection_for_collection_nav.borrow(), &mut photos);
                    limit_recently_added(
                        &connection_for_collection_nav.borrow(),
                        current_filter,
                        &mut photos,
                    );
                    sort_photos(&mut photos, sort_for_collection_nav.get());

                    if !photos.is_empty() {
                        let new_filter = sidebar::SidebarFilter::Folder(folder.id);
                        filter_for_collection_nav.set(new_filter);
                        if let Some(sidebar) = sidebar_selection_for_collection_nav.borrow().as_ref() {
                            sidebar::set_active_filter(sidebar, new_filter);
                            let sidebar = sidebar.clone();
                            let folder_id = folder.id;
                            glib::timeout_add_local_once(Duration::from_millis(100), move || {
                                sidebar::scroll_to_folder(&sidebar, folder_id);
                            });
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
                        if lightbox_for_collection_nav.root.is_visible() {
                            lightbox_for_collection_nav.open(objects, 0);
                        }
                        return;
                    }

                    candidate += step;
                }
            }
            sidebar::SidebarFilter::Albums => {
                // The Albums home view is not a photo thumbnail grid.
            }
        }
    });

    // Folder mode now uses native GridView keyboard behavior. Folder-to-folder
    // movement comes from the continuous model itself; there is no special
    // Up/Down collection switch here anymore.

    collection_navigation_slot.replace(Some(collection_navigation.clone()));
    let collection_navigation_for_lightbox = collection_navigation.clone();
    lightbox.set_collection_navigation_handler(move |direction| {
        collection_navigation_for_lightbox(direction)
    });

    gallery.root.add_css_class("photo-grid");

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);

    let refresh_status_box = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    refresh_status_box.set_margin_start(12);
    refresh_status_box.set_margin_end(12);
    refresh_status_box.set_margin_top(8);
    refresh_status_box.set_margin_bottom(8);
    refresh_status_box.add_css_class("toolbar");
    refresh_status_box.add_css_class("card");
    refresh_status_box.set_visible(false);
    let refresh_status_spinner = gtk::Spinner::new();
    refresh_status_spinner.set_spinning(false);
    refresh_status_box.append(&refresh_status_spinner);
    let refresh_status_label = gtk::Label::new(Some("Refreshing library…"));
    refresh_status_label.set_xalign(0.0);
    refresh_status_label.set_hexpand(true);
    refresh_status_box.append(&refresh_status_label);
    let refresh_status_stop = gtk::Button::with_label("Stop");
    refresh_status_box.append(&refresh_status_stop);
    content.append(&refresh_status_box);

    let grid_scroll = gtk::ScrolledWindow::new();
    grid_scroll.set_vexpand(true);
    grid_scroll.set_hexpand(true);
    // GtkGridView remains the direct scrollable child for every non-Folder
    // view, preserving its existing virtualization and selection behavior.
    grid_scroll.set_child(Some(&gallery.root));

    let folder_scroll = gtk::ScrolledWindow::new();
    folder_scroll.set_vexpand(true);
    folder_scroll.set_hexpand(true);
    // Folder mode uses its own virtualized ListView. Full-width folder headers
    // are ordinary ListView rows, so they move away naturally with the photos.
    folder_scroll.set_child(Some(&gallery.folder_root));
    let folder_scroll_overlay = gtk::Overlay::new();
    folder_scroll_overlay.set_hexpand(true);
    folder_scroll_overlay.set_vexpand(true);
    folder_scroll_overlay.set_child(Some(&folder_scroll));
    folder_scroll_overlay.add_overlay(&gallery.folder_rubberband);

    // A temporary date bubble makes a long chronological All Photos scrollbar
    // usable like a timeline. It is deliberately attached only to the GridView
    // scrollbar: Folder mode is not globally date-sorted.
    let scrub_date_label = gtk::Label::new(None);
    scrub_date_label.set_halign(gtk::Align::End);
    scrub_date_label.set_valign(gtk::Align::Center);
    scrub_date_label.set_margin_end(34);
    scrub_date_label.set_can_target(false);
    scrub_date_label.set_visible(false);
    scrub_date_label.add_css_class("scroll-scrub-date");
    let scrub_dragging = Rc::new(Cell::new(false));
    let scrub_hide_generation = Rc::new(Cell::new(0_u64));
    let update_scrub_date: Rc<dyn Fn(f64)> = Rc::new({
        let gallery = gallery.clone();
        let filter = filter.clone();
        let sort = sort.clone();
        let label = scrub_date_label.clone();
        move |scroll_y: f64| {
            let date = match sort.get().field {
                SortField::DateTaken => grid::GroupDate::Taken,
                SortField::DateAdded => grid::GroupDate::Added,
                _ => {
                    label.set_visible(false);
                    return;
                }
            };
            if filter.get() != sidebar::SidebarFilter::All {
                label.set_visible(false);
                return;
            }
            label.set_text(&gallery.month_label_for_scroll_position(scroll_y, date));
            label.set_visible(true);
        }
    });
    let scrollbar = grid_scroll.vscrollbar();
    let scrub_press = gtk::GestureClick::new();
    scrub_press.set_button(0);
    {
        let dragging = scrub_dragging.clone();
        let hide_generation = scrub_hide_generation.clone();
        let adjustment = grid_scroll.vadjustment();
        let update = update_scrub_date.clone();
        scrub_press.connect_pressed(move |_, _, _, _| {
            hide_generation.set(hide_generation.get().wrapping_add(1));
            dragging.set(true);
            update(adjustment.value());
        });
    }
    {
        let dragging = scrub_dragging.clone();
        let hide_generation = scrub_hide_generation.clone();
        let label = scrub_date_label.clone();
        scrub_press.connect_released(move |_, _, _, _| {
            if !dragging.replace(false) {
                return;
            }
            let generation = hide_generation.get().wrapping_add(1);
            hide_generation.set(generation);
            let label = label.clone();
            let hide_generation = hide_generation.clone();
            glib::timeout_add_local_once(Duration::from_millis(550), move || {
                if hide_generation.get() == generation {
                    label.set_visible(false);
                }
            });
        });
    }
    scrollbar.add_controller(scrub_press);
    {
        let dragging = scrub_dragging.clone();
        let update = update_scrub_date.clone();
        grid_scroll.vadjustment().connect_value_changed(move |adjustment| {
            if dragging.get() {
                update(adjustment.value());
            }
        });
    }

    let gallery_scroll_stack = gtk::Stack::new();
    gallery_scroll_stack.set_hexpand(true);
    gallery_scroll_stack.set_vexpand(true);
    gallery_scroll_stack.add_named(&grid_scroll, Some("grid"));
    gallery_scroll_stack.add_named(&folder_scroll_overlay, Some("folders"));
    {
        let gallery_scroll_stack = gallery_scroll_stack.clone();
        let gallery_for_folder_view = gallery.clone();
        gallery.set_folder_view_changed_handler(move |folder_mode| {
            gallery_scroll_stack.set_visible_child_name(if folder_mode {
                "folders"
            } else {
                "grid"
            });
            if folder_mode {
                // The Folder model rebuild is synchronous. This timeout runs
                // after that work returns to GTK and paints only the final
                // visible viewport instead of every intermediate bound row.
                let gallery = gallery_for_folder_view.clone();
                glib::timeout_add_local_once(Duration::from_millis(90), move || {
                    gallery.refresh_visible_folder_tiles();
                });
            }
        });
    }

    // Non-Folder GridView (Library, Favourites, Recently Added, Albums,
    // Search) needs the same current-viewport protection as Folder mode. Fast
    // scrollbar motion can otherwise leave hundreds of stale bind requests in
    // front of the final viewport and make thumbnails appear blank until idle.
    const GRID_THUMBNAIL_MOTION_PUMP_FRAMES: u8 = 12;
    let grid_thumbnail_motion_frames = Rc::new(Cell::new(0u8));
    let grid_thumbnail_motion_frames_for_event = grid_thumbnail_motion_frames.clone();
    let grid_thumbnail_motion_frames_for_tick = grid_thumbnail_motion_frames.clone();
    let grid_thumbnail_motion_phase = Rc::new(Cell::new(0u8));
    let grid_thumbnail_motion_phase_for_tick = grid_thumbnail_motion_phase.clone();
    let grid_scroll_direction = Rc::new(Cell::new(0.0_f64));
    let grid_scroll_direction_for_event = grid_scroll_direction.clone();
    let grid_scroll_direction_for_tick = grid_scroll_direction.clone();
    let last_grid_scroll_y = Rc::new(Cell::new(0.0_f64));
    let last_grid_scroll_y_for_event = last_grid_scroll_y.clone();
    // Large adjustment jumps are scrollbar/page teleports. For a few frames
    // after one, do not spend decoder capacity on speculative ahead-prefetch;
    // the newly targeted viewport must become useful first.
    let grid_scrub_frames = Rc::new(Cell::new(0u8));
    let grid_scrub_frames_for_event = grid_scrub_frames.clone();
    let grid_scrub_frames_for_tick = grid_scrub_frames.clone();
    let grid_scrub_generation = Rc::new(Cell::new(0u64));
    let grid_scrub_generation_for_event = grid_scrub_generation.clone();
    let grid_scrub_active = Rc::new(Cell::new(false));
    let grid_scrub_active_for_event = grid_scrub_active.clone();
    let grid_scrub_active_for_tick = grid_scrub_active.clone();
    // GtkAdjustment emits tiny correction events (often -1/0/+1) while the
    // scrollbar thumb is being dragged. Replacing the visible decode queue on
    // every correction cancels almost-finished work and leaves a wall of
    // placeholders. Coalesce scrub destinations so one viewport gets enough
    // time to finish before we replace it with a newer one.
    let grid_scrub_last_queue = Rc::new(RefCell::new(None::<std::time::Instant>));
    let grid_scrub_last_queue_for_event = grid_scrub_last_queue.clone();
    let grid_scrub_latest_value = Rc::new(Cell::new(0.0_f64));
    let grid_scrub_latest_value_for_event = grid_scrub_latest_value.clone();
    let grid_scrub_latest_page = Rc::new(Cell::new(0.0_f64));
    let grid_scrub_latest_page_for_event = grid_scrub_latest_page.clone();

    let gallery_for_group_scroll = gallery.clone();
    grid_scroll
        .vadjustment()
        .connect_value_changed(move |adjustment| {
            let value = adjustment.value();
            let previous = last_grid_scroll_y_for_event.replace(value);
            let delta = value - previous;
            grid_scroll_direction_for_event.set(delta.signum());
            grid_thumbnail_motion_frames_for_event.set(GRID_THUMBNAIL_MOTION_PUMP_FRAMES);

            // A scrollbar scrub can teleport farther than GTK can realize
            // GridView children in the same frame. Queue the destination from
            // the photo model immediately instead of waiting for widget binds.
            // Treat only multi-page motion as the *start* of a direct
            // scrollbar scrub. Once started, every following adjustment belongs
            // to that same drag until the bar has been quiet for 90 ms. This is
            // important because the user's final thumb movement can be much
            // smaller than the first large jump.
            let jump_threshold = (adjustment.page_size() * 1.75).max(900.0);
            if delta.abs() >= jump_threshold {
                grid_scrub_active_for_event.set(true);
                crate::grid::set_grid_scrub_active(true);
            }

            if grid_scrub_active_for_event.get() {
                grid_scrub_frames_for_event.set(6);
                grid_scrub_latest_value_for_event.set(value);
                grid_scrub_latest_page_for_event.set(adjustment.page_size());

                // Do not repeatedly replace the visible queue for GTK's tiny
                // adjustment corrections. At 40 ms the four/eight local-cache
                // workers get several decode slots per scrub sample, while the
                // newest scrollbar position still wins quickly enough to feel
                // live. A final forced sample is queued when the drag settles.
                let now = std::time::Instant::now();
                let should_queue = grid_scrub_last_queue_for_event
                    .borrow()
                    .as_ref()
                    .map(|last| now.duration_since(*last) >= Duration::from_millis(40))
                    .unwrap_or(true);
                if should_queue {
                    *grid_scrub_last_queue_for_event.borrow_mut() = Some(now);
                    gallery_for_group_scroll.queue_grid_scroll_target_cached_tiles_async(
                        value,
                        adjustment.page_size(),
                        192,
                    );
                }

                // End scrub mode only after the adjustment has stayed quiet for
                // a short interval. Old timeout callbacks are ignored by the
                // generation check, so an active drag cannot be ended early.
                let generation = grid_scrub_generation_for_event
                    .get()
                    .wrapping_add(1);
                grid_scrub_generation_for_event.set(generation);
                let generation_cell = grid_scrub_generation_for_event.clone();
                let scrub_active_cell = grid_scrub_active_for_event.clone();
                let last_queue_cell = grid_scrub_last_queue_for_event.clone();
                let latest_value_cell = grid_scrub_latest_value_for_event.clone();
                let latest_page_cell = grid_scrub_latest_page_for_event.clone();
                let gallery = gallery_for_group_scroll.clone();
                glib::timeout_add_local_once(Duration::from_millis(90), move || {
                    if generation_cell.get() != generation {
                        return;
                    }

                    // Force the final destination once more. The user may have
                    // released the thumb less than 40 ms after our last sample.
                    gallery.queue_grid_scroll_target_cached_tiles_async(
                        latest_value_cell.get(),
                        latest_page_cell.get(),
                        192,
                    );
                    *last_queue_cell.borrow_mut() = None;
                    scrub_active_cell.set(false);
                    crate::grid::set_grid_scrub_active(false);
                    gallery.refresh_visible_grid_tiles();
                });

            }


            gallery_for_group_scroll.update_group_header_for_scroll(value);
        });

    let gallery_for_grid_motion_tick = gallery.clone();
    grid_scroll.add_tick_callback(move |_, _| {
        let frames_left = grid_thumbnail_motion_frames_for_tick.get();
        if frames_left == 0 {
            return glib::ControlFlow::Continue;
        }
        grid_thumbnail_motion_frames_for_tick.set(frames_left.saturating_sub(1));

        // During direct scrollbar scrubbing the model-derived target queued by
        // the adjustment callback is authoritative. Do NOT replace it with
        // widget-derived visibility here: GtkGridView may still expose recycled
        // cells from the previous viewport for several frames. Applying the
        // paintables the workers already finished is still required, though:
        // during fast movement a tile binds once, usually before its decode
        // completes, and the completion drain misses tiles that were recycled
        // again before the decode arrived. Re-checking visible tiles against
        // the RAM cache every frame heals exactly those tiles.
        if grid_scrub_active_for_tick.get() {
            gallery_for_grid_motion_tick.apply_visible_grid_cached_paintables();
        } else {
            gallery_for_grid_motion_tick.queue_visible_grid_cached_tiles_async(128);
        }

        let scrub_left = grid_scrub_frames_for_tick.get();
        if scrub_left > 0 {
            grid_scrub_frames_for_tick.set(scrub_left.saturating_sub(1));
        }

        let phase = grid_thumbnail_motion_phase_for_tick.get().wrapping_add(1);
        grid_thumbnail_motion_phase_for_tick.set(phase);
        // During a scrollbar teleport, all decode capacity belongs to the
        // target viewport. Normal directional warming resumes once GTK has had
        // several frames to rebind the destination cells.
        if scrub_left == 0 && phase % 3 == 0 {
            gallery_for_grid_motion_tick
                .prefetch_grid_cached_tiles(32, grid_scroll_direction_for_tick.get());
        }

        glib::ControlFlow::Continue
    });

    let gallery_for_folder_scroll = gallery.clone();
    let latest_folder_scroll_y = Rc::new(Cell::new(0.0_f64));
    // Last scroll direction, used to warm thumbnails ahead of the user rather
    // than both sides equally.
    let folder_scroll_direction = Rc::new(Cell::new(0.0_f64));
    let folder_scroll_direction_for_event = folder_scroll_direction.clone();
    // Thumbnail loading is intentionally debounced until Folder motion stops.
    // GtkListView may rebind thousands of intermediate rows during a scrollbar
    // jump; loading thumbnails from each bind is pure wasted main-thread work.
    let folder_thumbnail_debounce: Rc<RefCell<Option<glib::SourceId>>> =
        Rc::new(RefCell::new(None));
    let folder_thumbnail_debounce_for_event = folder_thumbnail_debounce.clone();
    let folder_thumbnail_prefetch: Rc<RefCell<Option<glib::SourceId>>> =
        Rc::new(RefCell::new(None));
    let folder_thumbnail_prefetch_for_event = folder_thumbnail_prefetch.clone();
    // Coalesces scrub-target sampling: one sample in the jump frame itself,
    // then at most once per 50 ms while the drag continues. Frame-driven from
    // the motion tick below, not from a wall-clock timeout.
    let folder_scrub_sampler = Rc::new(RefCell::new(FolderScrollbarScrub::default()));
    let folder_scrub_sampler_for_event = folder_scrub_sampler.clone();
    let folder_scrub_sampler_for_tick = folder_scrub_sampler.clone();
    let latest_folder_scroll_y_for_tick = latest_folder_scroll_y.clone();
    let folder_vadjustment = folder_scroll.vadjustment();
    // True only for a real multi-page Folder scrollbar scrub. Page Up/Down is
    // deliberately excluded: Folder row bind now submits its own async visible
    // request, so page navigation must not repeatedly replace the queue.
    let folder_direct_scrub_active = Rc::new(Cell::new(false));
    let folder_direct_scrub_active_for_event = folder_direct_scrub_active.clone();
    let folder_direct_scrub_active_for_tick = folder_direct_scrub_active.clone();
    // The settled loader paints the final viewport after motion stops. During
    // active motion, drive thumbnail work from GTK frame ticks instead of a
    // 16 ms timeout. A timeout can run before GtkListView has rebound/allocated
    // the rows for a large scrollbar jump, which warms the old viewport and
    // leaves the new one blank. The short frame pump keeps retrying long enough
    // for recycled rows and async cache decodes to catch up.
    const FOLDER_THUMBNAIL_MOTION_PUMP_FRAMES: u8 = 12;
    let folder_thumbnail_motion_frames = Rc::new(Cell::new(0u8));
    let folder_thumbnail_motion_frames_for_event = folder_thumbnail_motion_frames.clone();
    let folder_thumbnail_motion_frames_for_tick = folder_thumbnail_motion_frames.clone();
    let folder_thumbnail_motion_phase = Rc::new(Cell::new(0u8));
    let folder_thumbnail_motion_phase_for_tick = folder_thumbnail_motion_phase.clone();
    let gallery_for_thumbnail_motion_tick = gallery.clone();
    let folder_scroll_direction_for_tick = folder_scroll_direction.clone();
    folder_scroll.add_tick_callback(move |_, _| {
        let frames_left = folder_thumbnail_motion_frames_for_tick.get();
        if frames_left == 0 {
            return glib::ControlFlow::Continue;
        }
        folder_thumbnail_motion_frames_for_tick.set(frames_left.saturating_sub(1));

        // Always prioritise the tiles actually visible in the frame GTK is
        // about to paint. queue_visible... uses reserved async capacity, so
        // stale prefetch requests cannot starve a scrollbar jump.
        if folder_direct_scrub_active_for_tick.get() {
            // The model-derived scrub target is authoritative while the thumb is
            // teleporting. GtkListView may still expose rows from the previous
            // viewport, so never let those recycled widgets replace the target
            // queue during an active direct scrub. Thumbnails the workers have
            // already finished must still be displayed: rows bind once, usually
            // before their decode completes, and the completion drain misses
            // rows recycled again mid-scrub. Applying RAM hits every frame
            // heals those rows without touching the decode queue.
            gallery_for_thumbnail_motion_tick.apply_visible_folder_cached_paintables();
        } else {
            gallery_for_thumbnail_motion_tick.queue_visible_folder_cached_tiles_async(96);
        }

        // Scrub decode targets are frame-driven: the first sample fires in the
        // jump frame itself (not a wall-clock interval later), then at most
        // every 50 ms while the drag continues. Coalescing keeps the decode
        // queue owned by one destination instead of one per ±1 px anchor
        // correction GtkListView emits during drags.
        let _scrub_target_queued = if folder_direct_scrub_active_for_tick.get()
            && folder_scrub_sampler_for_tick
                .borrow_mut()
                .sample_due(Instant::now())
        {
            let page = folder_vadjustment.page_size().max(1.0);
            gallery_for_thumbnail_motion_tick.queue_folder_scroll_target_cached_tiles_async(
                latest_folder_scroll_y_for_tick.get(),
                page,
                192,
            )
        } else {
            0
        };

        // Warming ahead is useful, but doing the larger offscreen scan on every
        // frame is unnecessary. Run it every third pump frame so visible work
        // remains dominant and GTK has plenty of time to render.
        let phase = folder_thumbnail_motion_phase_for_tick
            .get()
            .wrapping_add(1);
        folder_thumbnail_motion_phase_for_tick.set(phase);
        if !folder_direct_scrub_active_for_tick.get() && phase % 3 == 0 {
            gallery_for_thumbnail_motion_tick.prefetch_folder_cached_tiles(
                24,
                folder_scroll_direction_for_tick.get(),
            );
        }

        glib::ControlFlow::Continue
    });

    folder_scroll
        .vadjustment()
        .connect_value_changed(move |adjustment| {
            let raw_scroll_y = adjustment.value();
            // GtkListView keeps its scroll anchor on device-pixel boundaries
            // (same reason the wheel path quantizes in
            // install_smooth_gallery_scroll). Fractional scrollbar-drag values
            // make GTK immediately write a rounded value back, which reads as
            // a tiny bounce at drag end. Snap to whole pixels instead; the
            // re-entrant value_changed sees an integral value and no-ops.
            let scroll_y = raw_scroll_y.round();
            if scroll_y != raw_scroll_y {
                adjustment.set_value(scroll_y);
            }
            let previous_y = latest_folder_scroll_y.replace(scroll_y);
            let direction = scroll_y - previous_y;
            if direction != 0.0 {
                folder_scroll_direction_for_event.set(direction);
            }

            // Folder bind now submits its own visible-priority async request on
            // every RAM miss. That is sufficient for wheel motion and Page
            // Up/Down, and avoids the old failure mode where each ~one-page
            // adjustment step replaced another 30-60 useful requests.
            //
            // Only true multi-page scrollbar teleports use model-derived target
            // preloading. Coalesce those samples so workers can finish useful
            // work instead of decoding every intermediate thumb position.
            let page_size = adjustment.page_size().max(1.0);
            let direct_scrub = direction.abs() > page_size * 1.60;
            if direct_scrub {
                folder_direct_scrub_active_for_event.set(true);
                // Folder and grid views are never visible at the same time,
                // so reuse the shared scrub thread-local: it keeps recycled
                // folder tiles from being blanked at unbind while rows are
                // rebound faster than thumbnail decodes can land. The folder
                // settle callback clears it again.
                crate::grid::set_grid_scrub_active(true);
                // First target sample fires in the next frame tick; further
                // samples are coalesced to one per 50 ms by the sampler.
                folder_scrub_sampler_for_event.borrow_mut().begin();
            }

            // Keep only the cheap position bookkeeping in the raw adjustment
            // callback. Widget picking and sidebar work are throttled below so
            // wheel/touchpad/scrollbar motion cannot spend a frame walking GTK
            // widgets merely to update a visual location marker.
            gallery_for_folder_scroll.update_group_header_for_scroll(scroll_y);

            // Cancel the previous settle callback and arm a new one. Only the
            // final viewport after ~90 ms of idle motion gets a full visible
            // refresh. During sustained scrolling, separately warm a tiny
            // budgeted batch so the viewport does not remain blank until the
            // user fully stops.
            if let Some(source) = folder_thumbnail_debounce_for_event.borrow_mut().take() {
                source.remove();
            }
            if let Some(source) = folder_thumbnail_prefetch_for_event.borrow_mut().take() {
                source.remove();
            }
            // Keep a frame-synchronised thumbnail pump alive after every
            // movement. Re-arming it is cheap and covers wheel/touchpad motion,
            // kinetic scrolling, and direct scrollbar jumps with the same path.
            // The pump runs after GtkListView has had frame opportunities to
            // recycle/rebind rows, so it follows the new viewport instead of a
            // stale one captured by a wall-clock timeout.
            folder_thumbnail_motion_frames_for_event
                .set(FOLDER_THUMBNAIL_MOTION_PUMP_FRAMES);
            let gallery_for_visible = gallery_for_folder_scroll.clone();
            let debounce_slot = folder_thumbnail_debounce_for_event.clone();
            let prefetch_slot = folder_thumbnail_prefetch_for_event.clone();
            let direction_for_settle = folder_scroll_direction_for_event.get();
            let final_scroll_y = latest_folder_scroll_y.clone();
            let final_page_size = adjustment.page_size().max(1.0);
            let direct_scrub_active_for_settle = folder_direct_scrub_active_for_event.clone();
            let folder_scrub_sampler_for_settle = folder_scrub_sampler_for_event.clone();
            let source = glib::timeout_add_local(Duration::from_millis(110), move || {
                debounce_slot.borrow_mut().take();
                direct_scrub_active_for_settle.set(false);
                folder_scrub_sampler_for_settle.borrow_mut().end();
                // End the shared scrub window before the visible refresh so
                // tiles that never received their thumbnail drop the stale
                // backstop image and return to the normal placeholder state.
                crate::grid::set_grid_scrub_active(false);
                gallery_for_visible.queue_folder_scroll_target_cached_tiles_async(
                    final_scroll_y.get(),
                    final_page_size,
                    192,
                );
                // Defer the visible refresh to the next main-loop pass so GTK
                // can finish this frame's post-jump anchor/allocation work
                // before tiles drop their stale backstop paintables. Unloading
                // in the same callback interleaves with the ListView's row
                // re-positioning and reads as a small bounce at drag end.
                // Queueing decodes stays inline; it does not touch widgets.
                let gallery_for_settle_refresh = gallery_for_visible.clone();
                glib::timeout_add_local_once(Duration::ZERO, move || {
                    gallery_for_settle_refresh.refresh_visible_folder_tiles();
                });

                // GtkListView can realize the last destination row a few frames
                // *after* the settle callback above. Give those newly-created
                // tiles a short visible-only catch-up window before spending
                // worker capacity on speculative ahead/behind prefetch. This is
                // deliberately additive: it never replaces queued visible work.
                let gallery_for_prefetch = gallery_for_visible.clone();
                let prefetch_slot_for_tick = prefetch_slot.clone();
                let catchup_frames = Rc::new(Cell::new(6u8));
                let catchup_frames_for_tick = catchup_frames.clone();
                let prefetch_source = glib::timeout_add_local(
                    Duration::from_millis(16),
                    move || {
                        let frames_left = catchup_frames_for_tick.get();
                        if frames_left > 0 {
                            gallery_for_prefetch.refresh_visible_folder_tiles();
                            catchup_frames_for_tick.set(frames_left - 1);
                            return glib::ControlFlow::Continue;
                        }

                        let loaded =
                            gallery_for_prefetch.prefetch_folder_cached_tiles(24, direction_for_settle);
                        if loaded == 0 && !gallery_for_prefetch.thumbnail_display_work_pending() {
                            prefetch_slot_for_tick.borrow_mut().take();
                            glib::ControlFlow::Break
                        } else {
                            glib::ControlFlow::Continue
                        }
                    },
                );
                prefetch_slot.replace(Some(prefetch_source));
                glib::ControlFlow::Break
            });
            folder_thumbnail_debounce_for_event.replace(Some(source));

        });

    

    install_smooth_gallery_scroll(&grid_scroll, gallery.clone(), true);
    // Folder mode uses a variable-height GtkListView. Use relative wheel
    // easing rather than an absolute spring target so GTK anchor corrections
    // cannot pull the viewport backwards. Precision touchpads remain native.
    install_folder_smooth_gallery_scroll(&folder_scroll, gallery.clone());

    // While the sidebar divider is being dragged, keep the gallery column
    // count fixed. Otherwise every few pixels can cross a column threshold
    // and repeatedly rebuild visible rows. Apply the final width once after
    // the drag ends.
    let sidebar_resize_active = Rc::new(Cell::new(false));
    // Temporary hover-autohide is presentation only. While that overlay is
    // sliding in or out, keep the gallery's logical width/column model frozen
    // so Folder mode does not rebuild rows just because the pointer touched
    // the left edge. Pin/unpin remains a real layout change and uses the settle
    // gate below to perform one final responsive update after the animation.
    let sidebar_hover_layout_freeze = Rc::new(Cell::new(false));
    // Generation protects against a delayed hide callback unfreezing a newer
    // hover reveal when the pointer returns to the edge very quickly.
    let sidebar_hover_freeze_generation = Rc::new(Cell::new(0u64));
    let sidebar_layout_settle = Rc::new(RefCell::new(WidthSettleGate::new(3)));
    let gallery_for_resize = gallery.clone();
    let sidebar_resize_active_for_tick = sidebar_resize_active.clone();
    let sidebar_hover_layout_freeze_for_tick = sidebar_hover_layout_freeze.clone();
    let sidebar_layout_settle_for_tick = sidebar_layout_settle.clone();
    gallery_scroll_stack.add_tick_callback(move |surface, _clock| {
        gallery_for_resize.drain_thumbnail_display_completions();
        if should_observe_width(
            sidebar_resize_active_for_tick.get(),
            sidebar_hover_layout_freeze_for_tick.get(),
        ) {
            let width = surface.width();
            if width > 100
                && sidebar_layout_settle_for_tick
                    .borrow_mut()
                    .observe(width)
            {
                gallery_for_resize.update_width(width);
            }
        }
        glib::ControlFlow::Continue
    });

    let grid_surface = gtk::Box::new(gtk::Orientation::Vertical, 0);
    grid_surface.set_hexpand(true);
    grid_surface.set_vexpand(true);
    grid_surface.add_css_class("photo-grid");
    // Date grouping keeps its existing heading. Folder grouping hides this
    // widget and renders its headers inside the folder scroller instead.
    grid_surface.append(&gallery.group_header);
    grid_surface.append(&gallery_scroll_stack);

    let grid_overlay = gtk::Overlay::new();
    grid_overlay.set_hexpand(true);
    grid_overlay.set_vexpand(true);
    grid_overlay.set_child(Some(&grid_surface));
    grid_overlay.add_overlay(&scrub_date_label);
    grid_overlay.add_overlay(&lightbox.root);
    context_menu_host.borrow_mut().replace(grid_overlay.downgrade());

    // The context menu is a normal GtkOverlay child, so give it the
    // autohide behaviour GtkPopover used to provide. Any pointer press
    // outside the active menu dismisses it. Presses on menu buttons are
    // left alone so their normal clicked handlers still run.
    let context_menu_autohide = gtk::GestureClick::new();
    context_menu_autohide.set_button(0);
    context_menu_autohide.set_propagation_phase(gtk::PropagationPhase::Capture);
    let window_for_menu_autohide = window.clone();
    let lightbox_for_menu_autohide = lightbox.clone();
    context_menu_autohide.connect_pressed(move |_, _, x, y| {
        let inside_menu = window_for_menu_autohide
            .pick(x, y, gtk::PickFlags::DEFAULT)
            .is_some_and(|picked| photo_context_menu_contains(&picked));
        if !inside_menu && dismiss_active_photo_context_menu() {
            let root = lightbox_for_menu_autohide.root.clone();
            glib::idle_add_local_once(move || {
                if root.is_visible() {
                    root.grab_focus();
                }
            });
        }
    });
    window.add_controller(context_menu_autohide);

    let photo_page = gtk::Box::new(gtk::Orientation::Vertical, 0);
    photo_page.set_hexpand(true);
    photo_page.set_vexpand(true);
    photo_page.append(&grid_overlay);

    let album_theme_changed: Rc<dyn Fn()> = {
        let connection = connection.clone();
        let refresh_slot = albums_home_refresh_slot.clone();
        Rc::new(move || {
            let albums = db::albums(&connection.borrow()).unwrap_or_default();
            if let Some(refresh) = refresh_slot.borrow().as_ref() {
                refresh(&albums);
            }
        })
    };
    // Clone for the theme engine: the original is captured by the layout.rs
    // include below.
    let theme_post_apply = album_theme_changed.clone();
    let albums_home = albums_view::build(
        &albums,
        connection.clone(),
        grid_thumbnail_size,
        {
            let slot = album_home_click_slot.clone();
            Rc::new(move |album_id| {
                if let Some(open) = slot.borrow().as_ref() {
                    open(album_id);
                }
            })
        },
        album_theme_changed.clone(),
        {
            let present_settings = present_settings.clone();
            Rc::new(move || present_settings(Some("themes")))
        },
    );
    // Albums home is a separate ScrolledWindow, so it needs the same wheel
    // easing as the photo views. Ctrl+wheel must not zoom the hidden gallery.
    install_smooth_gallery_scroll(&albums_home, gallery.clone(), false);
    albums_view::connect_create_album(&albums_home, create_album.clone());
    let main_stack = gtk::Stack::new();
    main_stack.set_hexpand(true);
    main_stack.set_vexpand(true);
    main_stack.add_named(&photo_page, Some("photos"));
    main_stack.add_named(&albums_home, Some("albums"));
    let collage_page = gtk::Box::new(gtk::Orientation::Vertical, 0);
    collage_page.set_hexpand(true);
    collage_page.set_vexpand(true);
    main_stack.add_named(&collage_page, Some("collage"));
    let edit_page = gtk::Box::new(gtk::Orientation::Vertical, 0);
    edit_page.set_hexpand(true);
    edit_page.set_vexpand(true);
    main_stack.add_named(&edit_page, Some("edit"));
    main_stack.set_visible_child_name("photos");
    content.append(&main_stack);
    content.append(&info.root);

    let collage_editor: Rc<RefCell<Option<crate::collage::CollageEditor>>> =
        Rc::new(RefCell::new(None));
    {
        let parent = window.clone().upcast::<gtk::Window>();
        let connection = connection.clone();
        let main_stack = main_stack.clone();
        let collage_page = collage_page.clone();
        let collage_editor = collage_editor.clone();
        let collage_add_mode_slot = collage_add_mode_slot.clone();
        let collage_close_slot = collage_close_slot.clone();
        let collage_add_mode = collage_add_mode.clone();
        // Opens the photo editor while marking the trip as collage-owned so
        // the collage editor survives and Back/Done return to the collage.
        let open_edit_from_collage = {
            let open_edit = open_edit.clone();
            let collage_editing = collage_editing.clone();
            Rc::new(move |id: i64| {
                collage_editing.set(true);
                open_edit(id);
            }) as Rc<dyn Fn(i64)>
        };
        collage_open_slot.replace(Some(Rc::new(move |ids| {
            let add_mode_slot = collage_add_mode_slot.clone();
            let close_slot = collage_close_slot.clone();
            let collage_add_mode = collage_add_mode.clone();
            crate::collage::open(
                &parent,
                connection.clone(),
                ids,
                Rc::new({
                    let parent = parent.clone();
                    let main_stack = main_stack.clone();
                    let collage_page = collage_page.clone();
                    let collage_editor = collage_editor.clone();
                    let collage_add_mode = collage_add_mode.clone();
                    let open_edit_from_collage = open_edit_from_collage.clone();
                    move |photos, draft| {
                        while let Some(child) = collage_page.first_child() {
                            collage_page.remove(&child);
                        }
                        // A fresh editor starts with no add-photos detour
                        // pending; clear any stale picking state so leaving
                        // the collage page tears it down normally.
                        collage_add_mode.set(false);
                        let add_photos = {
                            let slot = add_mode_slot.clone();
                            Rc::new(move || {
                                if let Some(add_mode) = slot.borrow().as_ref() {
                                    add_mode();
                                }
                            }) as Rc<dyn Fn()>
                        };
                        let close = {
                            let slot = close_slot.clone();
                            Rc::new(move || {
                                if let Some(close) = slot.borrow().as_ref() {
                                    close();
                                }
                            }) as Rc<dyn Fn()>
                        };
                        let editor = crate::collage::build_editor(
                            &parent,
                            photos,
                            add_photos,
                            close,
                            draft,
                            open_edit_from_collage.clone(),
                        );
                        collage_page.append(&editor.root);
                        collage_editor.replace(Some(editor));
                        main_stack.set_visible_child_name("collage");
                    }
                }),
            );
        })));
    }

    let edit_editor: Rc<RefCell<Option<crate::edit::EditEditor>>> = Rc::new(RefCell::new(None));
    // Catch-all teardown when the collage/edit pages are left by any path
    // (sidebar navigation, search, opening the editor mid-collage, ...).
    // GtkStack reports the maximum minimum size of all its pages, hidden
    // ones included, so an editor left attached keeps demanding its tall
    // minimum and pushes the shared bottom bar offscreen on short windows.
    // The Exit/Done handlers tear down directly; this runs idempotently for
    // every other transition. The collage editor survives transitions to
    // the photos page while the user is picking images to add.
    {
        let main_stack_for_teardown = main_stack.clone();
        let collage_page = collage_page.clone();
        let collage_editor = collage_editor.clone();
        let edit_page = edit_page.clone();
        let edit_editor = edit_editor.clone();
        let collage_add_mode = collage_add_mode.clone();
        let collage_editing = collage_editing.clone();
        let gallery = gallery.clone();
        let connection_for_teardown = connection.clone();
        main_stack_for_teardown.connect_visible_child_notify(move |stack| {
            let visible = stack.visible_child_name();
            if visible.as_deref() != Some("collage")
                && !collage_add_mode.get()
                && !collage_editing.get()
            {
                // try_borrow: the add-photos handler holds the editor borrow
                // while switching pages; skip rather than panic.
                if collage_editor.try_borrow().map(|editor| editor.is_some()).unwrap_or(false) {
                    // Persist the draft on every path out of the editor.
                    if let Ok(editor_handle) = collage_editor.try_borrow() {
                        if let Some(editor) = editor_handle.as_ref() {
                            let json = editor.draft_json();
                            let guard = connection_for_teardown.borrow();
                            let _ =
                                db::set_setting(&guard, crate::collage::DRAFT_SETTING_KEY, &json);
                        }
                    }
                    // Clear the grid selection so the next toolbar collage
                    // click is a blank start and can offer the resume prompt.
                    gallery.set_selected_photo_ids(&[]);
                    while let Some(child) = collage_page.first_child() {
                        collage_page.remove(&child);
                    }
                    collage_editor.replace(None);
                }
            }
            if visible.as_deref() != Some("edit")
                && edit_editor.try_borrow().map(|editor| editor.is_some()).unwrap_or(false)
            {
                // Any exit from the photo editor ends the collage detour.
                collage_editing.set(false);
                while let Some(child) = edit_page.first_child() {
                    edit_page.remove(&child);
                }
                edit_editor.replace(None);
            }
        });
    }
    {
        let parent = window.clone().upcast::<gtk::Window>();
        let connection = connection.clone();
        let main_stack = main_stack.clone();
        let edit_page = edit_page.clone();
        let edit_editor = edit_editor.clone();
        let gallery = gallery.clone();
        let selected_photo = selected_photo.clone();
        let info = info.clone();
        let lightbox = lightbox.clone();
        let edit_space_slot = edit_space_slot.clone();
        let collage_editing = collage_editing.clone();
        let collage_editor = collage_editor.clone();
        edit_open_slot.replace(Some(Rc::new(move |id| {
            let Some(db_photo) = db::photo(&connection.borrow(), id).ok().flatten() else {
                // The open failed; drop any pending collage-edit marker so
                // the flag cannot get stuck and spare the collage forever.
                collage_editing.set(false);
                return;
            };
            edit_space_slot.borrow_mut().take();
            lightbox.close();
            let library_scroll_y = gallery.scroll_position();
            while let Some(child) = edit_page.first_child() {
                edit_page.remove(&child);
            }
            let photo = crate::photo_object::PhotoObject::from_photo(&db_photo);
            info.one_to_one.set_active(false);
            let close = {
                let main_stack = main_stack.clone();
                let one_to_one = info.one_to_one.clone();
                let gallery = gallery.clone();
                let edit_space_slot = edit_space_slot.clone();
                let edit_page = edit_page.clone();
                let edit_editor = edit_editor.clone();
                let collage_editing = collage_editing.clone();
                let collage_editor = collage_editor.clone();
                let connection = connection.clone();
                Rc::new(move || {
                    edit_space_slot.borrow_mut().take();
                    one_to_one.set_active(false);
                    let returning_to_collage = collage_editing.get();
                    if returning_to_collage {
                        // Bring the collage tile up to date with any edits
                        // saved during this detour.
                        if let Ok(editor_handle) = collage_editor.try_borrow() {
                            if let Some(collage) = editor_handle.as_ref() {
                                if let Some(updated) =
                                    db::photo(&connection.borrow(), id).ok().flatten()
                                {
                                    collage.refresh_photo_metadata(&[
                                        crate::photo_object::PhotoObject::from_photo(&updated),
                                    ]);
                                }
                            }
                        }
                        main_stack.set_visible_child_name("collage");
                    } else {
                        main_stack.set_visible_child_name("photos");
                        gallery.restore_view(id, library_scroll_y);
                    }
                    // Detach the editor so it stops contributing to the
                    // stack's size request and its preview memory is
                    // released; the editor is rebuilt on the next open.
                    while let Some(child) = edit_page.first_child() {
                        edit_page.remove(&child);
                    }
                    edit_editor.replace(None);
                }) as Rc<dyn Fn()>
            };
            let saved = {
                let gallery = gallery.clone();
                let selected_photo = selected_photo.clone();
                let info = info.clone();
                Rc::new(move |photo: crate::photo_object::PhotoObject| {
                    gallery.update_edit_recipe(photo.id(), &photo.edit_recipe());
                    selected_photo.replace(Some(photo.clone()));
                    info.set_photo(Some(&photo));
                }) as Rc<dyn Fn(crate::photo_object::PhotoObject)>
            };
            let editor = crate::edit::build_editor(
                &parent,
                connection.clone(),
                photo,
                close,
                saved,
            );
            {
                let one_to_one = info.one_to_one.clone();
                editor.set_one_to_one_sync_handler(move |enabled| {
                    one_to_one.set_active(enabled);
                });
            }
            // Make the way back explicit when the editor was opened from a
            // collage tile; Done saves and follows the same return path.
            if collage_editing.get() {
                editor.set_back_label("Back to Collage", "Return to the collage");
            }
            edit_page.append(&editor.root);
            edit_editor.replace(Some(editor));
            {
                let main_stack = main_stack.clone();
                let one_to_one = info.one_to_one.clone();
                let edit_space_slot = edit_space_slot.clone();
                edit_space_slot.replace(Some(Rc::new(move || {
                    if main_stack.visible_child_name().as_deref() == Some("edit") {
                        one_to_one.set_active(!one_to_one.is_active());
                    }
                })));
            }
            main_stack.set_visible_child_name("edit");
        })));
    }

    let selected_for_favorite = selected_photo.clone();
    let db_for_favorite = connection.clone();
    let info_favorite = info.clone();
    let gallery_for_favorite = gallery.clone();
    let filter_for_favorite = filter.clone();
    let lightbox_for_favorite = lightbox.clone();
    let sidebar_refresh_for_favorite = availability_refresh.clone();
    let sidebar_for_favorite = sidebar_for_unavailable.clone();

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
        if filter_for_favorite.get() == sidebar::SidebarFilter::Favorites && !favorite {
            gallery_for_favorite.remove_photos(&[photo.id()]);
            if lightbox_for_favorite.root.is_visible() {
                lightbox_for_favorite.remove_photo(photo.id());
            }
        } else {
            gallery_for_favorite.update_favorites(&[photo.id()], favorite);
            info_favorite.set_photo(Some(&photo));
        }
        if let Some(sidebar) = sidebar_for_favorite.borrow().as_ref().cloned() {
            if let Ok(counts) = db::sidebar_counts(&db_for_favorite.borrow()) {
                sidebar::refresh_library_counts(
                    &sidebar,
                    counts,
                    &sidebar_refresh_for_favorite,
                );
            }
        }
        
    });

    let lightbox_for_one_to_one = lightbox.clone();
    {
        // Keep the toolbar 1:1 toggle in sync when the lightbox changes mode
        // itself (e.g. Ctrl+wheel leaves 1:1 for a manual zoom).
        let one_to_one_button = info.one_to_one.clone();
        lightbox.set_one_to_one_sync_handler(move |enabled| {
            one_to_one_button.set_active(enabled);
        });
    }
    let edit_editor_for_one_to_one = edit_editor.clone();
    let main_stack_for_one_to_one = main_stack.clone();
    let space_toggle_in_progress_for_toggle = space_toggle_in_progress.clone();
    let space_open_slot_for_one_to_one = space_open_slot.clone();
    info.one_to_one.connect_toggled(move |button| {
        let enabled = button.is_active();
        if main_stack_for_one_to_one.visible_child_name().as_deref() == Some("edit") {
            if let Some(editor) = edit_editor_for_one_to_one.borrow().as_ref() {
                editor.set_one_to_one(enabled);
            }
        } else if enabled && !lightbox_for_one_to_one.root.is_visible() {
            if let Some(open_selected) = space_open_slot_for_one_to_one.borrow().as_ref() {
                open_selected();
            }
            let lightbox = lightbox_for_one_to_one.clone();
            glib::idle_add_local_once(move || {
                if lightbox.root.is_visible() {
                    lightbox.set_one_to_one(true);
                    lightbox.root.grab_focus();
                }
            });
        } else {
            lightbox_for_one_to_one.set_one_to_one(enabled);
        }
        if space_toggle_in_progress_for_toggle.get() {
            space_toggle_in_progress_for_toggle.set(false);
        }
    });

    // Keep the UI state reset when the lightbox is closed through any path.
    // Restore the current photo into the shared gallery selection and focus the
    // view that is actually visible (GridView normally, Folder ListView while
    // browsing the continuous Picasa-style Folder stream).
    let one_to_one_for_visibility = info.one_to_one.clone();
    let gallery_for_lightbox_close = gallery.clone();
    let selected_photo_for_lightbox_close = selected_photo.clone();
    lightbox.root.connect_visible_notify(move |root| {
        if !root.is_visible() {
            one_to_one_for_visibility.set_active(false);

            let gallery = gallery_for_lightbox_close.clone();
            let selected_photo = selected_photo_for_lightbox_close.borrow().clone();
            glib::idle_add_local_once(move || {
                if let Some(photo) = selected_photo {
                    gallery.select_photo(photo.id());
                }
                gallery.grab_focus();
            });
        }
    });

    let gallery_for_zoom_out = gallery.clone();
    let edit_editor_for_zoom_out = edit_editor.clone();
    let main_stack_for_zoom_out = main_stack.clone();
    info.grid_zoom_out.connect_clicked(move |_| {
        if main_stack_for_zoom_out.visible_child_name().as_deref() == Some("edit") {
            if let Some(editor) = edit_editor_for_zoom_out.borrow().as_ref() {
                editor.zoom_out();
            }
        } else {
            gallery_for_zoom_out.zoom_out();
        }
    });
    let gallery_for_zoom_reset = gallery.clone();
    let edit_editor_for_zoom_reset = edit_editor.clone();
    let main_stack_for_zoom_reset = main_stack.clone();
    info.grid_zoom_reset.connect_clicked(move |_| {
        if main_stack_for_zoom_reset.visible_child_name().as_deref() == Some("edit") {
            if let Some(editor) = edit_editor_for_zoom_reset.borrow().as_ref() {
                editor.fit();
            }
        } else {
            gallery_for_zoom_reset.reset_zoom();
        }
    });
    let gallery_for_zoom_in = gallery.clone();
    let edit_editor_for_zoom_in = edit_editor.clone();
    let main_stack_for_zoom_in = main_stack.clone();
    info.grid_zoom_in.connect_clicked(move |_| {
        if main_stack_for_zoom_in.visible_child_name().as_deref() == Some("edit") {
            if let Some(editor) = edit_editor_for_zoom_in.borrow().as_ref() {
                editor.zoom_in();
            }
        } else {
            gallery_for_zoom_in.zoom_in();
        }
    });

    let selected_for_rotate = selected_photo.clone();
    let db_for_rotate = connection.clone();
    let info_rotate = info.clone();
    let gallery_for_rotate = gallery.clone();
    let lightbox_for_rotate = lightbox.clone();
    let edit_editor_for_rotate = edit_editor.clone();
    let main_stack_for_rotate = main_stack.clone();

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
        if main_stack_for_rotate.visible_child_name().as_deref() == Some("edit") {
            if let Some(editor) = edit_editor_for_rotate.borrow().as_ref() {
                if editor.photo_id() == photo.id() {
                    editor.set_library_rotation(rotation);
                }
            }
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

        let filename = std::path::Path::new(&photo.filename())
            .file_stem()
            .and_then(|value| value.to_str())
            .map(|stem| format!("{stem}.jpg"))
            .unwrap_or_else(|| "export.jpg".to_string());
        dialog.set_current_name(&filename);

        dialog.connect_response(move |dialog, response| {
            if response == gtk::ResponseType::Accept {
                if let Some(file) = dialog.file() {
                    if let Some(destination) = file.path() {
                        // PhotoObject is a GTK object and must stay on the GTK
                        // thread. Copy only Send-safe scalar/string values into
                        // the export worker.
                        let reference = photo.path();
                        let rotation = photo.rotation();
                        let edit_recipe = photo.edit_recipe();
                        let source_width = photo.width();
                        let source_height = photo.height();
                        std::thread::spawn(move || {
                            let result = crate::edit::render::render_for_export(
                                &reference,
                                rotation,
                                &edit_recipe,
                                source_width,
                                source_height,
                            )
                            .and_then(|image| {
                                crate::edit::render::save_jpeg(&image, &destination, 92)
                            });
                            if let Err(error) = result {
                                eprintln!("Could not export photo: {error:#}");
                            }
                        });
                    }
                }
            }

            dialog.destroy();
        });

        dialog.show();
    });

    let selected_for_print = selected_photo.clone();
    let window_for_print = window.clone();
    let connection_for_print = connection.clone();
    let gallery_for_print = gallery.clone();

    info.print.connect_clicked(move |_| {
        let Some(photo) = selected_for_print.borrow().clone() else {
            return;
        };

        // Print every selected photo (the full multi-selection, falling back
        // to the anchor photo when nothing extra is selected). PhotoObject is
        // a GTK object and must stay on the GTK thread: copy only Send-safe
        // scalar/string values into the print requests; the renders happen on
        // a worker thread while the dialog runs here.
        let ids = gallery_for_print.selected_photo_ids(Some(photo.id()));
        let requests: Vec<PhotoPrintRequest> = ids
            .iter()
            .filter_map(|id| db::photo(&connection_for_print.borrow(), *id).ok().flatten())
            .map(|record| PhotoPrintRequest {
                job_name: record
                    .path
                    .rsplit('/')
                    .next()
                    .unwrap_or("photo")
                    .to_string(),
                reference: record.path,
                rotation: record.rotation,
                edit_recipe: record.edit_recipe,
                source_width: record.width.unwrap_or(0),
                source_height: record.height.unwrap_or(0),
            })
            .collect();

        print_photos(&window_for_print, &connection_for_print, requests);
    });

    let (
        main_split,
        main_surface,
        sidebar,
        thumbnail_recovery_requested,
        thumbnail_recovery_deferred,
        reconnected_sources,
        sidebar_for_events,
        right_column,
        right_header,
        search,
    ) = include!("layout.rs");

    // Appearance button: opens Settings → Themes, where the theme list is
    // built from the theme folders on disk. toolbar.rs appends the
    // `settings` button to the header tools.
    let settings = crate::window::theme::appearance_button();
    theme_engine.set_appearance_button(&settings);
    // Theme switches redraw the albums home so covers pick up the palette.
    // Window chrome (traffic lights vs native title buttons) is wired inside
    // layout.rs, where the headers and the placement closure live.
    theme_engine.set_post_apply(theme_post_apply);
    {
        let present_settings_for_appearance = present_settings.clone();
        settings.connect_clicked(move |_| {
            present_settings_for_appearance(Some("themes"));
        });
    }
    let refresh = include!("toolbar.rs");

    right_column.append(&right_header);
    right_column.append(&content);

    window.set_content(Some(&main_surface));

    let startup_gallery = gallery.clone();
    let startup_photos_for_idle = startup_photos.clone();
    let startup_total = startup_photos_for_idle.len();
    let mut startup_offset = 0usize;
    const STARTUP_BATCH_SIZE: usize = 500;
    glib::idle_add_local(move || {
        if startup_offset >= startup_total {
            return glib::ControlFlow::Break;
        }

        let end = (startup_offset + STARTUP_BATCH_SIZE).min(startup_total);
        let batch = &startup_photos_for_idle[startup_offset..end];
        if startup_offset == 0 {
            startup_gallery.replace(batch);
        } else {
            startup_gallery.append_photos(batch);
        }
        startup_offset = end;

        if startup_offset >= startup_total {
            glib::ControlFlow::Break
        } else {
            glib::ControlFlow::Continue
        }
    });

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

    crate::css::install_foundation(&display);
    // Apply the persisted theme (and the base layer) before the window's
    // first rendered frame. Must run after the foundation so the base theme
    // is added after base.css at the same priority and wins.
    theme_engine.startup();

    let (scan_sender, scan_receiver) = std::sync::mpsc::channel::<ScanUiEvent>();
    let (refresh_prepare_sender, refresh_prepare_receiver) =
        std::sync::mpsc::channel::<RefreshPrepareEvent>();

    let scan_job = Rc::new(RefCell::new(ScanJobState::default()));
    sidebar::bind_refresh_gate(&sidebar, &refresh);
    {
        let job = scan_job.clone();
        let refresh = refresh.downgrade();
        let stop = refresh_status_stop.clone();
        // Includes preparation and cancellation acknowledgement, not merely
        // the time an individual folder worker is active.
        glib::timeout_add_local(Duration::from_millis(50), move || {
            let Some(refresh) = refresh.upgrade() else {
                return glib::ControlFlow::Break;
            };
            let busy = job.borrow().kind.is_some();
            refresh.set_sensitive(!busy);
            stop.set_sensitive(busy);
            glib::ControlFlow::Continue
        });
    }
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
                if job.active.is_some() {
                    None
                } else {
                    job.pending
                        .pop_front()
                        .map(|root| (root, job.generation, job.kind))
                }
            };
            let Some((root, generation, kind)) = next else {
                return;
            };
            
            let control = spawn_tagged_scan(root, generation, scan_sender.clone());
            scan_job.borrow_mut().active = Some(control);
        })
    };

    refresh_folder_slot.replace(Some({
        let scan_job = scan_job.clone();
        let refresh_prepare_sender = refresh_prepare_sender.clone();
        let refresh_status_box = refresh_status_box.clone();
        let refresh_status_spinner = refresh_status_spinner.clone();
        let refresh_status_label = refresh_status_label.clone();
        let stop_scan = stop_scan.clone();
        let refresh = refresh.clone();
        Rc::new(move |reason: PhotoScanRequestReason, path: String| {
            
            if reason.scan_kind().is_none() {
                
                return;
            }
            if scan_job.borrow().kind.is_some() {
                
                refresh_status_label.set_text("Refresh already running…");
                refresh_status_spinner.set_spinning(true);
                refresh_status_box.set_visible(true);
                stop_scan.set_visible(true);
                return;
            }
            // A full Refresh All may still have a background availability/sidebar
            // rebuild pending. Do not let that stale worker mutate the sidebar
            // while a targeted folder refresh is starting.
            refresh.set_sensitive(false);
            invalidate_availability_refreshes();
            refresh_status_label.set_text(&format!("Refreshing {}…", crate::source::filename(&path)));
            refresh_status_spinner.set_spinning(true);
            refresh_status_box.set_visible(true);
            stop_scan.set_visible(true);
            let generation = {
                let mut job = scan_job.borrow_mut();
                job.authorize_photo_scan(reason)
                    .expect("folder refresh reason was authorized")
            };
            let sender = refresh_prepare_sender.clone();
            std::thread::spawn(move || {
                let imported_root = db::open_default()
                    .and_then(|connection| db::folders_cached(&connection))
                    .map(|folders| {
                        folders
                            .into_iter()
                            .any(|folder| folder.path == path && folder.imported_root)
                    })
                    .map_err(|error| error.to_string());
                let _ = sender.send(RefreshPrepareEvent::FolderReady {
                    generation,
                    path,
                    imported_root,
                });
            });
        })
    }));

    // Hover to focus: once the pointer rests on a thumbnail, it becomes the
    // selection - so Space/1:1 always opens exactly what is under it.
    {
        let gallery_for_hover = gallery.clone();
        gallery.set_hover_select_handler(Rc::new(move || {
            gallery_for_hover.select_photo_under_pointer();
        }));
    }


    // Debounce/coalesce monitor activity independently from scan authorization.
    // One busy scan cannot be interrupted by a watch event; the dirty root
    // remains pending and is submitted after the current job becomes idle.
    {
        let pending = pending_watch_refreshes.clone();
        let scan_job = scan_job.clone();
        let refresh_folder_slot = refresh_folder_slot.clone();
        glib::timeout_add_local(Duration::from_millis(250), move || {
            if scan_job.borrow().kind.is_some() {
                return glib::ControlFlow::Continue;
            }
            let now = Instant::now();
            let ready = pending
                .borrow()
                .iter()
                .find_map(|(path, changed_at)| {
                    (now.duration_since(*changed_at) >= Duration::from_millis(750))
                        .then(|| path.clone())
                });
            if let Some(path) = ready {
                pending.borrow_mut().remove(&path);
                
                if let Some(callback) = refresh_folder_slot.borrow().as_ref() {
                    callback(PhotoScanRequestReason::DebouncedWatchRefresh, path);
                }
            }
            glib::ControlFlow::Continue
        });
    }
    rebuild_folder_watches();

    let cancel_scan_job: Rc<dyn Fn()> = {
        let scan_job = scan_job.clone();
        let refresh_status_label = refresh_status_label.clone();
        Rc::new(move || {
            let mut job = scan_job.borrow_mut();
            // Stop means the whole current job. In particular, a library refresh
            // must not continue with the next queued folder after cancellation.
            job.stop_requested = true;
            job.pending.clear();
            if matches!(job.kind, Some(ScanJobKind::Refresh | ScanJobKind::FolderRefresh)) {
                refresh_status_label.set_text("Stopping refresh…");
            }
            if let Some(control) = job.active.as_ref() {
                control.cancel();
            }
        })
    };
    {
        let cancel = cancel_scan_job.clone();
        stop_scan.connect_clicked(move |_| cancel());
    }
    {
        let cancel = cancel_scan_job.clone();
        refresh_status_stop.connect_clicked(move |_| cancel());
    }

    // Recover existing indexed photos at startup, on mount changes, or after
    // manual Refresh. Offline drives may stay disconnected for hours; leave
    // them idle until one of those events requests another recovery pass.
    let startup_paths = std::sync::Arc::new(
        startup_photos
            .iter()
            .map(|photo| photo.path.clone())
            .collect::<std::collections::HashSet<_>>(),
    );
    let start_thumbnail_recovery: Rc<dyn Fn()> = {
        let connection = connection.clone();
        let scan_job = scan_job.clone();
        let sender = scan_sender.clone();
        let requested = thumbnail_recovery_requested.clone();
        let reconnected_sources = reconnected_sources.clone();
        Rc::new(move || {
            if scan_job.borrow().kind.is_some()
                || (!requested.get() && reconnected_sources.borrow().pending.is_empty())
            {
                return;
            }
            let Ok(mut photos) = db::photos(&connection.borrow(), None, false, None) else {
                return;
            };
            if !requested.get() {
                let sources = reconnected_sources.borrow();
                photos.retain(|photo| sources.includes(&photo.path));
            }
            retain_enabled_formats(&connection.borrow(), &mut photos);
            requested.set(false);
            reconnected_sources.borrow_mut().pending.clear();
            // Unrelated mounts need no cache checks, worker, or progress UI.
            if photos.is_empty() {
                return;
            }
            let mut job = scan_job.borrow_mut();
            job.generation = job.generation.wrapping_add(1);
            job.kind = Some(ScanJobKind::Maintenance);
            job.imported_total = 0;
            job.failed_total = 0;
            job.stop_requested = false;
            let control = scanner::ScanControl::default();
            job.active = Some(control.clone());
            spawn_thumbnail_recovery(
                photos,
                startup_paths.clone(),
                job.generation,
                sender.clone(),
                control,
            );
        })
    };
    start_thumbnail_recovery();

    // Test hook: auto-open the Add Network Share dialog for UI automation.
    if std::env::var_os("PIC_TEST_OPEN_SHARE").is_some() {
        let slot = add_network_share_slot.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(2000), move || {
            if let Some(callback) = slot.borrow().as_ref() {
                callback();
            }
        });
    }
    let add_network_share_slot_for_import = add_network_share_slot.clone();
    let connection_for_share = connection.clone();
    let scan_job_for_share = scan_job.clone();
    let start_next_scan_for_share = start_next_scan.clone();
    let sidebar_refresh_for_share = availability_refresh.clone();
    let window_for_share = window.clone();
    add_network_share_slot.replace(Some(Rc::new(move || {
        let connection = connection_for_share.clone();
        let scan_job = scan_job_for_share.clone();
        let start_next_scan = start_next_scan_for_share.clone();
        let sidebar_refresh = sidebar_refresh_for_share.clone();
        let parent_window = window_for_share.clone();
        let parent_for_dialog = window_for_share.clone();
        let on_connect: Rc<dyn Fn(String, String)> = Rc::new(move |name, browse_root| {
            let parent = parent_window.clone().upcast::<gtk::Widget>();
            crate::source::net_trace(format!("connect_requested uri={browse_root}"));
            // Normalize the connection root BEFORE mounting: resolve network://
            // discovery shortcuts, guarantee a trailing slash, and for NFS
            // strip the advertised service port (nfs://host:2049/export is a
            // service endpoint, not a mountable export - GIO mounts
            // nfs://host/export).
            let chooser_root = crate::source::network_browse_root(&browse_root);
            if !chooser_root.contains("://") || !chooser_root.ends_with('/') {
                let message = format!(
                    "could not browse {browse_root}: not a valid network location"
                );
                crate::source::net_trace(format!(
                    "browse_failed uri={browse_root} error={message}"
                ));
                show_error(&parent, "Could not open network share", &message);
                sidebar_refresh();
                return;
            }
            crate::source::net_trace(format!("connect_root normalized={chooser_root}"));
            let connection = connection.clone();
            let scan_job = scan_job.clone();
            let start_next_scan = start_next_scan.clone();
            let sidebar_refresh = sidebar_refresh.clone();
            let parent_window = parent_window.clone();
            let mount_parent = parent_window.clone().upcast::<gtk::Window>();
            let root_for_mount = chooser_root.clone();
            let parent_for_mount_error = parent.clone();
            let parent_widget = parent_window.clone().upcast::<gtk::Widget>();
            let parent_for_register = parent.clone();
            crate::source::mount_share_async(&root_for_mount, Some(&mount_parent), move |result| {
                if let Err(message) = result {
                    // The real GIO/GVfs error text is included in `message`, so
                    // a failed NFS mount shows why instead of silently falling
                    // through to an empty browser.
                    show_error(&parent_for_mount_error, "Could not connect to network share", &message);
                    sidebar_refresh();
                    return;
                }
                crate::source::net_trace(format!("browse_opened root={chooser_root}"));

                // Browse the mounted server (shares first, then folders) with
                // the in-app GIO browser and register only the folder the user
                // actually picks. The GTK file chooser cannot display remote
                // gvfs locations (it falls back to $HOME).
                let connection = connection.clone();
                let scan_job = scan_job.clone();
                let start_next_scan = start_next_scan.clone();
                let sidebar_refresh = sidebar_refresh.clone();
                let parent = parent_for_register.clone();
                show_network_folder_browser(
                    parent_widget,
                    chooser_root,
                    Rc::new(move |selected_uri: String| {
                        let display_name = if name.is_empty() {
                            crate::source::filename(&selected_uri)
                        } else {
                            name.clone()
                        };
                        crate::source::net_trace(format!(
                            "register_start uri={selected_uri} name={display_name}"
                        ));
                        if let Err(error) = db::insert_network_share(
                            &connection.borrow(),
                            &selected_uri,
                            &display_name,
                        ) {
                            show_error(&parent, "Could not add network share", &error.to_string());
                            return;
                        }
                        crate::source::net_trace(format!("registered uri={selected_uri}"));
                        sidebar_refresh();
                        // The share is mounted and the folder was verified by
                        // browsing into it, so the scan goes straight out.
                        if let Ok(mut job) = scan_job.try_borrow_mut() {
                            job.authorize_photo_scan(PhotoScanRequestReason::ImportFolder)
                                .expect("import is an authorized scan reason");
                            job.pending.push_back(selected_uri.clone());
                        }
                        start_next_scan();
                        crate::source::net_trace(format!("scan_queued uri={selected_uri}"));
                    }),
                );
            });
        });
        show_add_network_share_dialog(parent_for_dialog.upcast::<gtk::Widget>(), on_connect);
    })));

    let parent = window.clone();
    let connection_for_import = connection.clone();
    let sidebar_refresh_for_import = availability_refresh.clone();
    let scan_job_for_import = scan_job.clone();
    let start_next_scan_for_import = start_next_scan.clone();

    import_folder_slot.replace(Some(Rc::new(move || {
        let scan_job = scan_job_for_import.clone();
        let start_next_scan = start_next_scan_for_import.clone();
        let connection = connection_for_import.clone();
        let sidebar_refresh = sidebar_refresh_for_import.clone();

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
                    
                    if let Err(error) = db::mark_import_root(&connection.borrow(), &root) {
                        eprintln!("Could not register imported folder {root}: {error}");
                        return;
                    }
                    // Re-read the folder hierarchy immediately after the
                    // selected root is registered. The scan worker may emit
                    // its first event later, but the sidebar must already show
                    // the correct parent/child relationship before scanning.
                    sidebar_refresh();
                    {
                        let mut job = scan_job.borrow_mut();
                        job.authorize_photo_scan(PhotoScanRequestReason::ImportFolder)
                            .expect("import is an authorized scan reason");
                        job.pending.push_back(root);
                    }
                    start_next_scan();
                }
            }

            dialog.destroy();
        });

        dialog.show();
    })));

    let scan_job_for_refresh = scan_job.clone();
    let refresh_prepare_sender_for_click = refresh_prepare_sender.clone();
    let recovery_requested_for_refresh = thumbnail_recovery_requested.clone();
    let refresh_status_box_for_click = refresh_status_box.clone();
    let refresh_status_spinner_for_click = refresh_status_spinner.clone();
    let refresh_status_label_for_click = refresh_status_label.clone();
    let stop_scan_for_refresh_click = stop_scan.clone();

    refresh.connect_clicked(move |button| {
        
        let maintenance_was_active = scan_job_for_refresh
            .borrow()
            .kind
            == Some(ScanJobKind::Maintenance);
        if maintenance_was_active {
            scan_job_for_refresh.borrow_mut().preempt_maintenance();
            
        } else if scan_job_for_refresh.borrow().kind.is_some() {
            
            refresh_status_label_for_click.set_text("Refresh already running…");
            refresh_status_spinner_for_click.set_spinning(true);
            refresh_status_box_for_click.set_visible(true);
            stop_scan_for_refresh_click.set_visible(true);
            return;
        }
        button.set_sensitive(false);
        // Do not request a full post-refresh thumbnail recovery pass here.
        // Refresh scans already create thumbnails for changed items; a recovery
        // pass over the entire 66k-photo library immediately after repeated
        // refreshes can monopolize the app. Startup/mount recovery remains.
        recovery_requested_for_refresh.set(false);
        refresh_status_label_for_click.set_text("Refreshing library…");
        refresh_status_spinner_for_click.set_spinning(true);
        refresh_status_box_for_click.set_visible(true);
        stop_scan_for_refresh_click.set_visible(true);
        

        let generation = {
            let mut job = scan_job_for_refresh.borrow_mut();
            job.authorize_photo_scan(PhotoScanRequestReason::ManualLibraryRefresh)
                .expect("manual library refresh is an authorized scan reason")
        };

        let sender = refresh_prepare_sender_for_click.clone();
        std::thread::spawn(move || {
            
            let roots = db::open_default()
                .and_then(|connection| db::imported_root_paths(&connection))
                .map_err(|error| error.to_string());
            let count = roots.as_ref().map(|roots| roots.len()).unwrap_or(0);
            
            let _ = sender.send(RefreshPrepareEvent::LibraryReady {
                generation,
                roots,
            });
        });
    });

    let gallery_for_events = gallery.clone();
    let connection_for_events = connection.clone();
    let info_for_events = info.clone();
    let selected_photo_for_events = selected_photo.clone();
    let availability_refresh_for_events = availability_refresh.clone();
    let filter_for_events = filter.clone();
    let search_for_events = search_text.clone();
    let sort_for_events = sort.clone();
    let scan_job_for_events = scan_job.clone();
    let start_next_scan_for_events = start_next_scan.clone();
    let stop_scan_for_events = stop_scan.clone();
    let refresh_status_box_for_events = refresh_status_box.clone();
    let refresh_status_spinner_for_events = refresh_status_spinner.clone();
    let refresh_status_label_for_events = refresh_status_label.clone();
    let mut displayed_generation: Option<u64> = None;
    let mut scan_count: usize = 0;
    let mut pending_photos: VecDeque<db::Photo> = VecDeque::new();
    let mut thumbnail_dirty_paths = VecDeque::<std::path::PathBuf>::new();
    let mut thumbnail_dirty_seen = std::collections::HashSet::<std::path::PathBuf>::new();
    let mut priority_thumbnail_paths = Vec::new();
    let mut failure_message_shown = false;
    let mut thumbnail_total: usize = 0;
    let mut last_progress_update = Instant::now();

    glib::timeout_add_local(Duration::from_millis(250), move || {
        // Drain event-triggered recovery requests once the current scan ends.
        // With no request, this checks only a flag and performs no disk probes.
        start_thumbnail_recovery();
        priority_thumbnail_paths.extend(crate::thumbnail::take_priority_completions());
        let priority_completions = priority_thumbnail_paths.len();
        let priority_pending = crate::thumbnail::priority_pending_count();
        
        while let Ok(prepare_event) = refresh_prepare_receiver.try_recv() {
            match prepare_event {
                RefreshPrepareEvent::LibraryReady {
                    generation,
                    roots,
                } => {
                    if generation != scan_job_for_events.borrow().generation {
                        continue;
                    }
                    let roots = match roots {
                        Ok(roots) => roots.into_iter().collect::<VecDeque<_>>(),
                        Err(error) => {
                            eprintln!("Could not refresh library folders: {error}");
                            let mut job = scan_job_for_events.borrow_mut();
                            job.kind = None;
                            job.active = None;
                            job.pending.clear();
                            refresh_status_spinner_for_events.set_spinning(false);
                            stop_scan_for_events.set_visible(false);
                            refresh_status_label_for_events.set_text("Could not read library folders");
                            refresh_status_box_for_events.set_visible(true);
                            let panel = refresh_status_box_for_events.clone();
                            glib::timeout_add_local_once(Duration::from_millis(3000), move || {
                                panel.set_visible(false);
                            });
                            continue;
                        }
                    };
                    
                    if roots.is_empty() {
                        let mut job = scan_job_for_events.borrow_mut();
                        job.kind = None;
                        job.active = None;
                        job.pending.clear();
                        refresh_status_spinner_for_events.set_spinning(false);
                        stop_scan_for_events.set_visible(false);
                        refresh_status_label_for_events.set_text("No library folders to refresh");
                        refresh_status_box_for_events.set_visible(true);
                        let panel = refresh_status_box_for_events.clone();
                        glib::timeout_add_local_once(Duration::from_millis(2500), move || {
                            panel.set_visible(false);
                        });
                        continue;
                    }
                    let should_start = {
                        let mut job = scan_job_for_events.borrow_mut();
                        if job.stop_requested {
                            job.kind = None;
                            job.active = None;
                            job.pending.clear();
                            false
                        } else {
                            job.pending = roots;
                            true
                        }
                    };
                    if should_start {
                        start_next_scan_for_events();
                    } else {
                        refresh_status_spinner_for_events.set_spinning(false);
                        stop_scan_for_events.set_visible(false);
                        refresh_status_label_for_events.set_text("Library refresh stopped");
                        refresh_status_box_for_events.set_visible(true);
                        let panel = refresh_status_box_for_events.clone();
                        glib::timeout_add_local_once(Duration::from_millis(2500), move || {
                            panel.set_visible(false);
                        });
                    }
                }
                RefreshPrepareEvent::FolderReady {
                    generation,
                    path,
                    imported_root,
                } => {
                    if generation != scan_job_for_events.borrow().generation {
                        continue;
                    }
                    
                    let ok = match imported_root {
                        Ok(ok) => ok,
                        Err(error) => {
                            eprintln!("Could not validate refresh folder: {error}");
                            false
                        }
                    };
                    let should_start = {
                        let mut job = scan_job_for_events.borrow_mut();
                        if !ok || job.stop_requested {
                            job.kind = None;
                            job.active = None;
                            job.pending.clear();
                            false
                        } else {
                            job.pending.clear();
                            job.pending.push_back(path);
                            true
                        }
                    };
                    if should_start {
                        start_next_scan_for_events();
                    } else {
                        refresh_status_spinner_for_events.set_spinning(false);
                        stop_scan_for_events.set_visible(false);
                        refresh_status_label_for_events.set_text("Folder refresh stopped");
                        refresh_status_box_for_events.set_visible(true);
                        let panel = refresh_status_box_for_events.clone();
                        glib::timeout_add_local_once(Duration::from_millis(2500), move || {
                            panel.set_visible(false);
                        });
                    }
                }
            }
        }

        if priority_pending > 0 && thumbnail_total == 0 {
            refresh_status_label_for_events
                .set_text(&format!("Creating visible thumbnails ({priority_pending} queued)"));
            refresh_status_box_for_events.set_visible(true);
        } else if priority_completions > 0 && thumbnail_total == 0 {
            refresh_status_box_for_events.set_visible(false);
        }
        // Never monopolize the GTK loop when a fast scanner has queued many
        // results. Leaving some events queued lets GTK process input, redraws,
        // scrolling, and folder changes between import batches.
        // Keep scan/photo events from monopolizing GTK while a large refresh
        // or thumbnail recovery is active. A smaller batch lets input,
        // redraws, and the Stop button run between worker updates.
        const MAX_EVENTS_PER_TICK: usize = 16;
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
                
                continue;
            }

            if displayed_generation != Some(ui_event.generation) {
                displayed_generation = Some(ui_event.generation);
                scan_count = 0;
                thumbnail_total = 0;
                failure_message_shown = false;
            }

            let event = ui_event.event;
            match &event {
                scanner::ScanEvent::Started { root } => {
                    
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
                    refresh_status_label_for_events.set_text(&format!("Scanning {folder_name}…"));
                    refresh_status_spinner_for_events.set_spinning(true);
                    refresh_status_box_for_events.set_visible(true);
                }

                scanner::ScanEvent::FolderStarted { folder } => {
                    
                    // Only imports should append rows immediately. Refreshing an
                    // existing library/folder after Refresh All was appending
                    // duplicate sidebar rows; right-clicking those stale rows
                    // could start another refresh against duplicated UI state.
                    if scan_job_for_events.borrow().kind == Some(ScanJobKind::Import) {
                        run_ui_guarded("sidebar folder append", || {
                            sidebar::append_folder(
                                &sidebar_for_events,
                                folder,
                                availability_refresh_for_events.clone(),
                            )
                        });
                    }
                }

                scanner::ScanEvent::PhotoIndexed {
                    path,
                    photo,
                    newly_discovered,
                    ..
                } => {
                    scan_count += 1;
                    
                    let search_active = !search_for_events.borrow().is_empty();
                    if !search_active {
                        if *newly_discovered
                            && crate::image_format::path_is_enabled(
                                &connection_for_events.borrow(),
                                &photo.path,
                            )
                            && (matches!(filter_for_events.get(), sidebar::SidebarFilter::All)
                                || matches!(filter_for_events.get(), sidebar::SidebarFilter::Folder(id) if Some(id) == photo.folder_id))
                        {
                            pending_photos.push_back(photo.clone());
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
                    }

                    if last_progress_update.elapsed() >= Duration::from_millis(150) {
                        let text = format!("Indexed {scan_count} photos");
                        refresh_status_label_for_events.set_text(&text);
                        refresh_status_box_for_events.set_visible(true);
                        last_progress_update = Instant::now();
                    }
                }

                scanner::ScanEvent::IndexingFinished { imported } => {
                    // Progressive imports append quickly; refresh jobs do one
                    // final rebuild after the whole serialized multi-folder job.
                    if !matches!(
                        scan_job_for_events.borrow().kind,
                        Some(ScanJobKind::Refresh | ScanJobKind::FolderRefresh)
                    ) {
                        refresh_grid(
                            &connection_for_events,
                            filter_for_events.get(),
                            &search_for_events.borrow(),
                            sort_for_events.get(),
                            &gallery_for_events,
                        );
                    }
                    let text = format!("Indexed {imported} photos");
                    refresh_status_label_for_events.set_text(&text);
                    refresh_status_box_for_events.set_visible(true);
                }

                scanner::ScanEvent::ThumbnailsDeferred { total } => {
                    thumbnail_recovery_deferred.set(*total > 0);
                    
                }
                scanner::ScanEvent::ThumbnailsStarted { total } => {
                    
                    scan_count = 0;
                    thumbnail_total = *total;
                    refresh_status_label_for_events
                        .set_text(&format!("Creating thumbnails 0 / {total}"));
                    refresh_status_spinner_for_events.set_spinning(true);
                    refresh_status_box_for_events.set_visible(true);
                    stop_scan_for_events.set_visible(true);
                }

                scanner::ScanEvent::ThumbnailCreated { path } => {
                    scan_count += 1;
                    
                    // Coalesce source paths received in this timer tick and
                    // refresh only matching realized tiles. Non-realized rows
                    // will discover the new cache entry through the background
                    // presentation loader when they bind later.
                    if thumbnail_dirty_seen.insert(path.clone()) {
                        thumbnail_dirty_paths.push_back(path.clone());
                    }
                    if last_progress_update.elapsed() >= Duration::from_millis(150) {
                        let text = format!("Creating thumbnails {scan_count} / {thumbnail_total}");
                        refresh_status_label_for_events.set_text(&text);
                        refresh_status_box_for_events.set_visible(true);
                        last_progress_update = Instant::now();
                    }
                }

                scanner::ScanEvent::Failed { path, error } => {
                    eprintln!("SCAN FAILED: {}: {}", path.display(), error);
                    if !failure_message_shown {
                        failure_message_shown = true;
                        refresh_status_label_for_events
                            .set_text(&format!("Some files could not be added: {error}"));
                    } else {
                        refresh_status_label_for_events.set_text("Scanning… some files failed");
                    }
                    refresh_status_box_for_events.set_visible(true);
                }

                scanner::ScanEvent::Finished { imported, failed } => {
                    
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

                    // An entirely offline (or already cached) pass did no work.
                    // Leave it quiet while waiting for the next reconnect.
                    if kind == Some(ScanJobKind::Maintenance) && thumbnail_total == 0 {
                        scan_job_for_events.borrow_mut().kind = None;
                        continue;
                    }

                    if matches!(kind, Some(ScanJobKind::Refresh | ScanJobKind::FolderRefresh))
                        && !matches!(filter_for_events.get(), sidebar::SidebarFilter::Folder(_))
                    {
                        refresh_grid(
                            &connection_for_events,
                            filter_for_events.get(),
                            &search_for_events.borrow(),
                            sort_for_events.get(),
                            &gallery_for_events,
                        );
                    }

                    if kind != Some(ScanJobKind::FolderRefresh) {
                        crate::settings::refresh_library_availability_stats(
                            connection_for_events.clone(),
                        );
                    }

                    stop_scan_for_events.set_visible(false);
                    refresh_status_spinner_for_events.set_spinning(false);
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
                        Some(ScanJobKind::FolderRefresh) => {
                            if total_failed == 0 {
                                format!("Folder refresh complete · {total_imported} photos updated")
                            } else {
                                format!(
                                    "Folder refresh complete · {total_imported} updated · {total_failed} failed"
                                )
                            }
                        }
                        Some(ScanJobKind::Maintenance) => {
                            if total_failed == 0 {
                                if thumbnail_recovery_deferred.get() {
                                    "Available thumbnails recovered · waiting for offline sources".to_string()
                                } else {
                                    "Thumbnail recovery complete".to_string()
                                }
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
                    if kind != Some(ScanJobKind::FolderRefresh) {
                        availability_refresh_for_events();
                    }
                    // New photos outside Folder mode never touch the cached
                    // Folder stream, so a restored stream would silently miss
                    // the share that was just imported. Drop it; the next
                    // folder click rebuilds (scoped preview, then background).
                    if total_imported > 0 {
                        gallery_for_events.invalidate_folder_cache();
                    }
                    
                    refresh_status_label_for_events.set_text(&message);
                    refresh_status_box_for_events.set_visible(true);
                    let panel = refresh_status_box_for_events.clone();
                    glib::timeout_add_local_once(Duration::from_millis(2500), move || {
                        panel.set_visible(false);
                    });
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
                    refresh_status_spinner_for_events.set_spinning(false);
                    let message = match kind {
                        Some(ScanJobKind::Refresh) => {
                            format!("Library refresh stopped · {imported} photos updated")
                        }
                        Some(ScanJobKind::FolderRefresh) => {
                            format!("Folder refresh stopped · {imported} photos updated")
                        }
                        _ => format!("Import stopped · {imported} photos added"),
                    };
                    if *imported > 0 {
                        gallery_for_events.invalidate_folder_cache();
                    }
                    
                    refresh_status_label_for_events.set_text(&message);
                    refresh_status_box_for_events.set_visible(true);
                    let panel = refresh_status_box_for_events.clone();
                    glib::timeout_add_local_once(Duration::from_millis(2500), move || {
                        panel.set_visible(false);
                    });
                }
            }
        }

        

        const PHOTO_APPEND_BATCH: usize = 192;
        if !search_for_events.borrow().is_empty() {
            // Search results supersede progressive scan appends. The next
            // debounced refresh will replace the model from the DB.
            pending_photos.clear();
        }
        if !pending_photos.is_empty() {
            let mut batch = Vec::with_capacity(PHOTO_APPEND_BATCH.min(pending_photos.len()));
            for _ in 0..PHOTO_APPEND_BATCH {
                let Some(photo) = pending_photos.pop_front() else {
                    break;
                };
                batch.push(photo);
            }
            run_ui_guarded("photo batch append", || {
                gallery_for_events.append_photos(&batch)
            });
            
        }
        for path in priority_thumbnail_paths.drain(..) {
            if thumbnail_dirty_seen.insert(path.clone()) {
                thumbnail_dirty_paths.push_back(path);
            }
        }
        const THUMBNAIL_REFRESH_BATCH: usize = 128;
        if !thumbnail_dirty_paths.is_empty() {
            let mut paths = Vec::with_capacity(THUMBNAIL_REFRESH_BATCH.min(thumbnail_dirty_paths.len()));
            for _ in 0..THUMBNAIL_REFRESH_BATCH {
                let Some(path) = thumbnail_dirty_paths.pop_front() else {
                    break;
                };
                thumbnail_dirty_seen.remove(&path);
                paths.push(path);
            }
            run_ui_guarded("targeted thumbnail refresh", || {
                gallery_for_events.refresh_thumbnails_for_paths(&paths)
            });
            
        }

        glib::ControlFlow::Continue
    });

    window
}

