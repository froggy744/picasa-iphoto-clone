{
    let main_split = adw::OverlaySplitView::new();

    let album_theme_changed_for_destination = album_theme_changed.clone();
    let destination_click_with_target: Rc<dyn Fn(sidebar::SidebarFilter, bool)> = {
        let search_entry = search_entry_slot.clone();
        let search_text = search_text.clone();
        let suppressed = search_suppressed.clone();
        let debounce = search_debounce.clone();
        let cleared_query = cleared_search_query.clone();
        let filter = filter.clone();
        let connection = connection.clone();
        let folder_cache = folder_cache.clone();
        let gallery = gallery.clone();
        let lightbox = lightbox.clone();
        let sort = sort.clone();
        let group_mode = group_mode.clone();
        let sidebar_selection = sidebar_selection_slot.clone();
        let main_stack = main_stack.clone();
        let albums_home = albums_home.clone();
        let connection_for_albums = connection.clone();
        let album_home_click_slot = album_home_click_slot.clone();
        let open_in_folder_exact_target = open_in_folder_exact_target.clone();
        Rc::new(move |new_filter, exact_photo_target| {
            // Cancel stale async refresh/folder-scroll work before this new
            // destination is established. This also covers Folder-to-Folder
            // reuse, which otherwise would not bump the refresh generation.
            invalidate_pending_grid_navigation();
            gallery.clear_pending_folder_target();
            let query = search_text.borrow().clone();
            cleared_query.replace(Some(query.clone()));
            if !exact_photo_target {
                open_in_folder_exact_target.set(None);
            }
            let folder_target = if let sidebar::SidebarFilter::Folder(folder_id) = new_filter {
                folder_cache
                    .borrow()
                    .iter()
                    .find(|folder| folder.id == folder_id)
                    .map(|folder| (folder_id, folder.path.clone()))
            } else {
                None
            };
            // Once the continuous Folder stream is loaded, clicking another
            // folder should be a scroll operation, not another database query
            // and model rebuild. An active global search is the exception: its
            // grid model is not the Folder stream, so it must be reloaded.
            let reuse_folder_stream = can_reuse_folder_stream_for_destination(
                folder_target.is_some(),
                gallery.can_restore_folder_cache(),
            );

            if let Some(source) = debounce.borrow_mut().take() {
                source.remove();
            }
            suppressed.set(true);
            if let Some(entry) = search_entry.borrow().as_ref() {
                entry.set_text("");
            }
            suppressed.set(false);
            search_text.replace(String::new());
            lightbox.close();
            let cleared_query_for_timeout = cleared_query.clone();
            glib::timeout_add_local_once(Duration::from_millis(500), move || {
                if cleared_query_for_timeout.borrow().as_deref() == Some(query.as_str()) {
                    cleared_query_for_timeout.replace(None);
                }
            });
            filter.set(new_filter);
            if let Err(error) = db::set_setting(
                &connection.borrow(),
                LAST_VIEW_SETTING_KEY,
                &sidebar_filter_setting(new_filter),
            ) {
                eprintln!("Could not save current view: {error}");
            }
            if let Some(sidebar) = sidebar_selection.borrow().as_ref() {
                sidebar::set_active_filter(sidebar, new_filter);
            }
            if new_filter == sidebar::SidebarFilter::Library {
                gallery.set_selected_photo_ids(&[]);
                main_stack.set_visible_child_name("library");
                return;
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
                        connection_for_albums.clone(),
                        grid_thumbnail_size,
                        on_album,
                        album_theme_changed_for_destination.clone(),
                    );
                }
                return;
            }
            main_stack.set_visible_child_name("photos");
            // destination_click clears the search entry above, so History
            // grouping can safely re-enable here.
            apply_gallery_grouping(&gallery, new_filter, sort.get(), group_mode.get(), true);
            match folder_destination_plan(exact_photo_target, reuse_folder_stream) {
                FolderDestinationPlan::ReuseWithoutFolderScroll => {
                    // Open in Folder will select/scroll the exact photo below.
                    // Do not also queue the generic folder-header destination.
                    return;
                }
                FolderDestinationPlan::RefreshWithoutFolderScroll => {
                    // Load the continuous Folder stream, but deliberately omit
                    // folder_target so refresh_grid_inner does not schedule a
                    // later scroll_to_folder() that can overwrite the exact photo.
                    refresh_grid_inner(
                        &connection,
                        new_filter,
                        "",
                        sort.get(),
                        &gallery,
                        None,
                    );
                }
                FolderDestinationPlan::Normal => {
                    if let Some((folder_id, folder_path)) = folder_target {
                        if reuse_folder_stream
                            && gallery.scroll_to_folder(folder_id, &folder_path)
                        {
                            return;
                        }
                        refresh_grid_to_folder(
                            &connection,
                            new_filter,
                            "",
                            sort.get(),
                            &gallery,
                            folder_id,
                            folder_path,
                        );
                    } else {
                        refresh_grid(&connection, new_filter, "", sort.get(), &gallery);
                    }
                }
            }
        })
    };

    let destination_click: Rc<dyn Fn(sidebar::SidebarFilter)> = {
        let destination_click_with_target = destination_click_with_target.clone();
        Rc::new(move |new_filter| destination_click_with_target(new_filter, false))
    };
    library_navigation_slot.replace(Some(destination_click.clone()));

    album_home_click_slot.replace(Some({
        let destination_click = destination_click.clone();
        Rc::new(move |album_id| destination_click(sidebar::SidebarFilter::Album(album_id)))
    }));
    folder_navigation_slot.replace(Some({
        let destination_click_with_target = destination_click_with_target.clone();
        let sidebar_selection = sidebar_selection_slot.clone();
        let gallery = gallery.clone();
        let exact_target = open_in_folder_exact_target.clone();
        Rc::new(move |folder_id, photo_id| {
            exact_target.set(Some(photo_id));
            destination_click_with_target(sidebar::SidebarFilter::Folder(folder_id), true);
            let gallery = gallery.clone();
            let exact_target_for_timer = exact_target.clone();
            let attempts = Rc::new(Cell::new(0u32));
            let attempts_for_timer = attempts.clone();
            glib::timeout_add_local(Duration::from_millis(25), move || {
                let attempt = attempts_for_timer.get() + 1;
                attempts_for_timer.set(attempt);
                let building = gallery.stream_building();
                let revealed = if building {
                    false
                } else {
                    gallery.select_photo(photo_id)
                };

                if open_in_folder_should_stop(building, revealed, attempt) {
                    // Keep the exact target around long enough for the sidebar's
                    // own Tree-mode/ancestor expansion callbacks to settle.
                    // Those callbacks may reorder Folder rows, but will reassert
                    // this photo instead of jumping to the folder header.
                    let exact_target_for_clear = exact_target_for_timer.clone();
                    glib::timeout_add_local_once(Duration::from_millis(750), move || {
                        if exact_target_for_clear.get() == Some(photo_id) {
                            exact_target_for_clear.set(None);
                        }
                    });
                    glib::ControlFlow::Break
                } else {
                    glib::ControlFlow::Continue
                }
            });
            if let Some(sidebar) = sidebar_selection.borrow().as_ref().cloned() {
                // scroll_to_folder() already retries internally if Tree mode or
                // ancestor expansion is required. Do not hammer it 24 times.
                sidebar::scroll_to_folder(&sidebar, folder_id);
            }
        })
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
            // Album cards keep using existing cached thumbnails; this only
            // replaces the index data after an album mutation.
            albums_view::refresh(
                &albums_home,
                albums,
                connection.clone(),
                grid_thumbnail_size,
                on_album,
                album_theme_changed.clone(),
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

    let folder_display_mode = sidebar::FolderDisplayMode::from_setting(
        db::setting(
            &connection.borrow(),
            sidebar::FOLDER_DISPLAY_MODE_SETTING_KEY,
        )
        .ok()
        .flatten()
        .as_deref(),
    );
    let initial_folder_order = folder_stream_order(&folders, folder_display_mode);
    gallery.set_folder_catalog(&folders, &initial_folder_order);

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
        add_network_share.clone(),
        delete_album.clone(),
        availability_refresh.clone(),
        {
            let slot = refresh_folder_slot.clone();
            Rc::new(move |path| {
                if let Some(callback) = slot.borrow().as_ref() {
                    callback(PhotoScanRequestReason::UserFolderRefresh, path);
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
        {
            let connection = connection.clone();
            let sidebar = sidebar_for_unavailable.clone();
            let on_unavailable = availability_refresh.clone();
            let rebuild_folder_watches = rebuild_folder_watches.clone();
            let parent: gtk::Widget = window.clone().upcast();
            Rc::new(move |folder, watched| {
                match db::set_folder_watched(&connection.borrow(), folder.id, watched) {
                    Ok(true) => {
                        rebuild_folder_watches();
                        if let Some(sidebar) = sidebar.borrow().as_ref().cloned() {
                            if let Ok(folders) = db::folders(&connection.borrow()) {
                                sidebar::refresh_folder_rows(
                                    &sidebar,
                                    &folders,
                                    &on_unavailable,
                                );
                            }
                        }
                    }
                    Ok(false) => show_error(
                        &parent,
                        "Could not change folder watch state",
                        "That folder is no longer registered in the library.",
                    ),
                    Err(error) => show_error(
                        &parent,
                        "Could not change folder watch state",
                        &error.to_string(),
                    ),
                }
            })
        },
        folder_display_mode,
        {
            let connection = connection.clone();
            let filter = filter.clone();
            let sort = sort.clone();
            let gallery = gallery.clone();
            let group_mode = group_mode.clone();
            let open_in_folder_exact_target = open_in_folder_exact_target.clone();
            Rc::new(move |mode| {
                if let Err(error) = db::set_setting(
                    &connection.borrow(),
                    sidebar::FOLDER_DISPLAY_MODE_SETTING_KEY,
                    mode.setting_value(),
                ) {
                    eprintln!("Could not save folder display mode: {error}");
                }

                let current_filter = filter.get();
                if let sidebar::SidebarFilter::Folder(folder_id) = current_filter {
                    let folders = db::folders(&connection.borrow()).unwrap_or_default();
                    let folder_path = folders
                        .iter()
                        .find(|folder| folder.id == folder_id)
                        .map(|folder| folder.path.clone());
                    apply_gallery_grouping(
                        &gallery,
                        current_filter,
                        sort.get(),
                        group_mode.get(),
                        true,
                    );
                    // A sidebar tree-mode change only reorders folder sections.
                    // Reorder the existing stream instead of a full database
                    // refresh and 66k PhotoObject rebuild (measured 2565 ms).
                    let order = folder_stream_order(&folders, mode);
                    gallery.set_folder_catalog(&folders, &order);

                    // Rebuilding/reordering the virtual Folder rows is
                    // synchronous, but GTK realizes the new ListView layout on
                    // the next main-loop turn. Reassert the destination only
                    // after that turn so an Albums/Photos -> Folder/Tree
                    // transition cannot leave the viewport at the restored
                    // cache position. This does not alter smooth scrolling.
                    if let Some(photo_id) = open_in_folder_exact_target.get() {
                        let gallery = gallery.clone();
                        glib::idle_add_local_once(move || {
                            if !gallery.stream_building() {
                                gallery.select_photo(photo_id);
                            }
                        });
                    } else if let Some(folder_path) = folder_path {
                        let gallery = gallery.clone();
                        glib::idle_add_local_once(move || {
                            gallery.scroll_to_folder(folder_id, &folder_path);
                        });
                    }
                }
            })
        },
        sidebar::SidebarVisibility::from_connection(&connection.borrow()),
    );
    {
        let gallery_for_tab = gallery.clone();
        sidebar::set_keyboard_grid_target(
            &sidebar,
            Rc::new(move || gallery_for_tab.visible_root()),
            &[
                gallery.root.clone().upcast::<gtk::Widget>(),
                gallery.folder_root.clone().upcast::<gtk::Widget>(),
            ],
        );
    }
    sidebar_for_unavailable.replace(Some(sidebar.clone()));
    sidebar_selection_slot.replace(Some(sidebar.clone()));
    sidebar::set_active_filter(&sidebar, filter.get());

    // Sidebar folder tracking is intentionally click-driven. Passive gallery
    // scrolling must never move the sidebar or probe the visible folder. A
    // primary click on a thumbnail only updates the sidebar's folder-location
    // marker; it must never change the active gallery filter. "Open in Folder"
    // remains the explicit action that navigates the gallery into Folder mode.
    let install_thumbnail_sidebar_focus = |root: &gtk::Widget| {
        let click = gtk::GestureClick::new();
        click.set_button(1);
        click.set_propagation_phase(gtk::PropagationPhase::Capture);
        let root = root.clone();
        let root_for_pick = root.clone();
        let sidebar = sidebar.clone();
        click.connect_pressed(move |_, _, x, y| {
            let Some(picked) = root_for_pick.pick(x, y, gtk::PickFlags::DEFAULT) else {
                return;
            };
            let Some(tile) = picked
                .ancestor(crate::grid::SquareTile::static_type())
                .and_downcast::<crate::grid::SquareTile>()
            else {
                return;
            };
            let Some(photo) = tile.photo() else {
                return;
            };
            sidebar::set_scroll_location(&sidebar, Some(photo.folder_id()));
        });
        root.add_controller(click);
    };
    install_thumbnail_sidebar_focus(gallery.root.upcast_ref());
    install_thumbnail_sidebar_focus(gallery.folder_root.upcast_ref());

    // Reconnecting sources also resumes previews for already indexed photos.
    let thumbnail_recovery_requested = Rc::new(Cell::new(true));
    let thumbnail_recovery_deferred = Rc::new(Cell::new(false));
    let reconnected_sources = Rc::new(RefCell::new(ReconnectedSources {
        mounted: mounted_source_roots(),
        ..Default::default()
    }));
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
                // Mount changes only re-evaluate folder availability for the
                // offline badge. Cached thumbnails stay usable; do not start
                // thumbnail recovery, scanning, or per-photo file checks.
                
                availability_refresh();
            });
        })
    };
    let schedule_mount_refresh_for_mount = schedule_mount_refresh.clone();
    volume_monitor.connect_mount_added(move |_, mount| {
        
        schedule_mount_refresh_for_mount();
    });
    let schedule_mount_refresh_for_unmount = schedule_mount_refresh.clone();
    volume_monitor.connect_mount_removed(move |_, mount| {
        
        schedule_mount_refresh_for_unmount();
    });
    let schedule_mount_refresh_for_change = schedule_mount_refresh.clone();
    volume_monitor.connect_mount_changed(move |_, mount| {
        
        schedule_mount_refresh_for_change();
    });
    unsafe {
        window.set_data("picasa-volume-monitor", volume_monitor);
    }
    crate::platform::install_native_mount_monitor(&window, schedule_mount_refresh.clone());
    let sidebar_for_events = sidebar.clone();

    let left_header = adw::HeaderBar::new();
    left_header.set_height_request(46);
    // Left-side window controls (macOS traffic lights, Linux left button
    // layout) must live in the sidebar header while the sidebar is visible so
    // they stay at the window's top-left corner. While the sidebar is hidden
    // the show-sidebar handler below moves them to the right header, which
    // then spans the full window width. End-side buttons (Windows-style top
    // right) stay in the right header only.
    left_header.set_show_start_title_buttons(true);
    left_header.set_show_end_title_buttons(false);
    left_header.add_css_class("layout-left-header");

    // Theme-overridable window controls: a theme opts in with
    // `window-controls: traffic-light` in its picasa-theme header and the
    // ThemeEngine shows this box while hiding the native start title
    // buttons. Packed first so the lights sit left of everything else.
    let window_controls = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    window_controls.add_css_class("window-controls");
    window_controls.set_margin_start(8);
    window_controls.set_margin_end(8);
    window_controls.set_valign(gtk::Align::Center);
    let window_controls_close = gtk::Button::new();
    window_controls_close.add_css_class("traffic-light");
    window_controls_close.add_css_class("traffic-light-close");
    window_controls_close.set_tooltip_text(Some("Close"));
    window_controls_close.set_focus_on_click(false);
    {
        let window = window.clone();
        window_controls_close.connect_clicked(move |_| window.close());
    }
    let window_controls_minimize = gtk::Button::new();
    window_controls_minimize.add_css_class("traffic-light");
    window_controls_minimize.add_css_class("traffic-light-minimize");
    window_controls_minimize.set_tooltip_text(Some("Minimize"));
    window_controls_minimize.set_focus_on_click(false);
    {
        let window = window.clone();
        window_controls_minimize.connect_clicked(move |_| window.minimize());
    }
    let window_controls_zoom = gtk::Button::new();
    window_controls_zoom.add_css_class("traffic-light");
    window_controls_zoom.add_css_class("traffic-light-zoom");
    window_controls_zoom.set_tooltip_text(Some("Maximize"));
    window_controls_zoom.set_focus_on_click(false);
    {
        let window = window.clone();
        window_controls_zoom.connect_clicked(move |_| {
            if window.is_maximized() {
                window.unmaximize();
            } else {
                window.maximize();
            }
        });
    }
    for light in [
        &window_controls_close,
        &window_controls_minimize,
        &window_controls_zoom,
    ] {
        light.add_css_class("flat");
        window_controls.append(light);
    }
    window_controls.set_visible(false);
    left_header.pack_start(&window_controls);

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
    // Keep the sidebar pin on the right edge of the sidebar header so the
    // left edge remains clear for macOS-style or Linux-left window controls.
    left_header.pack_end(&menu);

    // The pin button is state-aware. A pinned sidebar offers "Hide sidebar";
    // a sidebar that was only revealed by the left-edge hover offers "Always
    // Show" so clicking it converts the temporary reveal into a persistent
    // pin instead of closing it again. Pin state lives in plain cells without
    // change notifications, so callers sync the button on every transition.
    let update_pin_button: Rc<dyn Fn(bool)> = {
        let menu = menu.clone();
        Rc::new(move |pinned| {
            if pinned {
                menu.set_icon_name(sidebar_toggle_icon);
                menu.set_tooltip_text(Some("Hide sidebar"));
            } else {
                menu.set_icon_name("view-pin-symbolic");
                menu.set_tooltip_text(Some("Always Show"));
            }
        })
    };
    update_pin_button(true);

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
    // Keep a forgiving hit area around the visible divider. The handle is
    // intentionally transparent, so this does not change the sidebar's
    // appearance.
    sidebar_resize_handle.set_width_request(12);
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
    main_split.set_min_sidebar_width(SIDEBAR_MIN_WIDTH);
    main_split.set_max_sidebar_width(SIDEBAR_MAX_WIDTH);
    let saved_sidebar_fraction = numeric_setting::<f64>(
        &connection.borrow(),
        SIDEBAR_WIDTH_FRACTION_SETTING_KEY,
    )
    .unwrap_or(0.22)
    .clamp(0.10, 0.70);
    main_split.set_sidebar_width_fraction(saved_sidebar_fraction);
    // When collapsed, libadwaita clamps the sidebar to max-sidebar-width and
    // ignores the fraction, so the compact overlay swaps in its own (narrower)
    // cap. Expanding restores the normal maximum, leaving the wide-window
    // layout untouched.
    let collapsed_sidebar_width = Rc::new(Cell::new(
        numeric_setting::<f64>(
            &connection.borrow(),
            SIDEBAR_COLLAPSED_WIDTH_SETTING_KEY,
        )
        .unwrap_or(SIDEBAR_COLLAPSED_DEFAULT_WIDTH)
        .clamp(SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH),
    ));
    {
        let collapsed_sidebar_width_for_notify = collapsed_sidebar_width.clone();
        let sidebar_for_collapse_notify = sidebar.clone();
        main_split.connect_collapsed_notify(move |split| {
            if split.is_collapsed() {
                split.set_max_sidebar_width(collapsed_sidebar_width_for_notify.get());
            } else {
                split.set_max_sidebar_width(SIDEBAR_MAX_WIDTH);
                // A collapsed hover peek that was still open when the window
                // widened must not leave stale auto-hide state behind: the
                // expanded sidebar is persistent again (pin gate resumes).
                sidebar::clear_hover_open(&sidebar_for_collapse_notify);
            }
        });
        if main_split.is_collapsed() {
            main_split.set_max_sidebar_width(collapsed_sidebar_width.get());
        }
    }
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
    let sidebar_hover_layout_freeze_for_reveal = sidebar_hover_layout_freeze.clone();
    let sidebar_hover_freeze_generation_for_reveal = sidebar_hover_freeze_generation.clone();
    sidebar_hover_motion.connect_enter(move |_, _, _| {
        // The breakpoint auto-hides even a pinned sidebar on narrow windows,
        // so edge-hover must still peek it open while collapsed. In the
        // expanded layout the pin gate still applies: a pinned sidebar the
        // user explicitly hid stays hidden until they show it again.
        if !main_split_for_hover_reveal.shows_sidebar()
            && (!sidebar::is_pinned(&sidebar_for_hover_reveal)
                || main_split_for_hover_reveal.is_collapsed())
        {
            // Hover reveal is intentionally presentation-only. Freeze the
            // gallery model before OverlaySplitView starts changing width.
            sidebar_hover_freeze_generation_for_reveal.set(
                sidebar_hover_freeze_generation_for_reveal
                    .get()
                    .wrapping_add(1),
            );
            sidebar_hover_layout_freeze_for_reveal.set(true);
            sidebar::set_hover_open(&sidebar_for_hover_reveal, true);
            main_split_for_hover_reveal.set_show_sidebar(true);
        }
    });
    sidebar_hover_reveal.add_controller(sidebar_hover_motion);

    // Auto-close only a sidebar that was opened by hover (including a
    // collapsed-mode peek of a pinned sidebar). A pinned sidebar in the
    // expanded layout never hover-opens, so it never auto-hides here.
    let sidebar_leave_motion = gtk::EventControllerMotion::new();
    let main_split_for_hover_hide = main_split.clone();
    let sidebar_for_hover_hide = sidebar.clone();
    sidebar_leave_motion.connect_leave(move |_| {
        if sidebar::is_hover_open(&sidebar_for_hover_hide) {
            // The show-sidebar notify handler below owns release of the layout
            // freeze after the hide animation. This also covers other UI paths
            // that can close a temporarily revealed sidebar.
            main_split_for_hover_hide.set_show_sidebar(false);
            sidebar::clear_hover_open(&sidebar_for_hover_hide);
        }
    });
    sidebar_shell.add_controller(sidebar_leave_motion);

    let sidebar_hover_reveal_for_state = sidebar_hover_reveal.clone();
    let sidebar_for_show_state = sidebar.clone();
    let sidebar_hover_layout_freeze_for_state = sidebar_hover_layout_freeze.clone();
    let sidebar_hover_freeze_generation_for_state = sidebar_hover_freeze_generation.clone();
    let pin_button_for_state = update_pin_button.clone();
    main_split.connect_show_sidebar_notify(move |split| {
        let sidebar_visible = split.shows_sidebar();
        sidebar_hover_reveal_for_state.set_visible(!sidebar_visible);
        // Every reveal path (hover, pin button, breakpoint restore) funnels
        // through this notify, so this is the one place that keeps the pin
        // button's tooltip/icon in sync with how the sidebar was opened.
        if sidebar_visible {
            pin_button_for_state(sidebar::is_pinned(&sidebar_for_show_state));
        }

        if !split.shows_sidebar() && sidebar_hover_layout_freeze_for_state.get() {
            // A hover-open sidebar may be closed by mouse-leave, destination
            // selection, search, or another compact-layout action. Keep the
            // gallery frozen through the slide-out, then release without a
            // forced reflow. If another hover reveal starts first, generation
            // matching prevents this old timeout from unfreezing the new one.
            sidebar::clear_hover_open(&sidebar_for_show_state);
            let generation = sidebar_hover_freeze_generation_for_state
                .get()
                .wrapping_add(1);
            sidebar_hover_freeze_generation_for_state.set(generation);
            let freeze = sidebar_hover_layout_freeze_for_state.clone();
            let active_generation = sidebar_hover_freeze_generation_for_state.clone();
            glib::timeout_add_local_once(Duration::from_millis(400), move || {
                if active_generation.get() == generation {
                    freeze.set(false);
                }
            });
        }
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
    sidebar_drag.set_propagation_phase(gtk::PropagationPhase::Capture);
    sidebar_drag.set_exclusive(true);

    let sidebar_shell_for_drag_begin = sidebar_shell.clone();
    let main_split_for_drag_begin = main_split.clone();
    let sidebar_drag_start_width_begin = sidebar_drag_start_width.clone();
    let sidebar_drag_split_width_begin = sidebar_drag_split_width.clone();
    let sidebar_resize_active_for_begin = sidebar_resize_active.clone();
    let sidebar_layout_settle_for_drag_begin = sidebar_layout_settle.clone();
    sidebar_drag.connect_drag_begin(move |_, _, _| {
        sidebar_layout_settle_for_drag_begin.borrow_mut().cancel();
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
    let pending_collapsed_width = Rc::new(Cell::new(collapsed_sidebar_width.get()));
    let pending_collapsed_width_update = pending_collapsed_width.clone();
    let main_split_for_drag_update = main_split.clone();
    let sidebar_drag_start_width_update = sidebar_drag_start_width.clone();
    let sidebar_drag_split_width_update = sidebar_drag_split_width.clone();
    let sidebar_resize_preview_update = sidebar_resize_preview.clone();
    sidebar_drag.connect_drag_update(move |_, offset_x, _| {
        let split_width = sidebar_drag_split_width_update.get().max(1.0);
        let target_width = (sidebar_drag_start_width_update.get() + offset_x)
            .clamp(SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH)
            .min(split_width * 0.70);
        if main_split_for_drag_update.is_collapsed() {
            // The collapsed allocator reads max-sidebar-width only; writing
            // the fraction here would have no effect on the overlay width.
            pending_collapsed_width_update.set(target_width);
        } else {
            let fraction =
                (target_width / split_width).clamp(0.10, 0.70);
            pending_sidebar_fraction_update.set(fraction);
        }

        sidebar_resize_preview_update.set_margin_start(target_width.round() as i32 - 1);
        sidebar_resize_preview_update.set_visible(true);
    });

    let main_split_for_drag_end = main_split.clone();
    let pending_sidebar_fraction_end = pending_sidebar_fraction.clone();
    let pending_collapsed_width_end = pending_collapsed_width.clone();
    let collapsed_sidebar_width_for_end = collapsed_sidebar_width.clone();
    let connection_for_drag_end = connection.clone();
    let sidebar_resize_active_for_end = sidebar_resize_active.clone();
    let sidebar_resize_preview_end = sidebar_resize_preview.clone();
    let gallery_for_sidebar_drag_end = gallery.clone();
    let gallery_surface_for_sidebar_drag_end = gallery_scroll_stack.clone();
    sidebar_drag.connect_drag_end(move |_, _, _| {
        sidebar_resize_preview_end.set_visible(false);
        if main_split_for_drag_end.is_collapsed() {
            let width = pending_collapsed_width_end.get();
            collapsed_sidebar_width_for_end.set(width);
            main_split_for_drag_end.set_max_sidebar_width(width);
            if let Err(error) = db::set_setting(
                &connection_for_drag_end.borrow(),
                SIDEBAR_COLLAPSED_WIDTH_SETTING_KEY,
                &width.to_string(),
            ) {
                eprintln!("Could not save sidebar overlay width: {error}");
            }
        } else {
            main_split_for_drag_end
                .set_sidebar_width_fraction(pending_sidebar_fraction_end.get());
        }
        sidebar_resize_active_for_end.set(false);

        // Wait until the split view has received its single final allocation,
        // then perform exactly one responsive grid update.
        let gallery = gallery_for_sidebar_drag_end.clone();
        let surface = gallery_surface_for_sidebar_drag_end.clone();
        glib::idle_add_local_once(move || {
            let width = surface.width();
            if width > 100 {
                gallery.update_width(width);
            }
        });
    });
    sidebar_resize_handle.add_controller(sidebar_drag);

    let main_split_for_hide = main_split.clone();
    let sidebar_for_hide = sidebar.clone();
    let sidebar_layout_settle_for_hide = sidebar_layout_settle.clone();
    let sidebar_hover_layout_freeze_for_unpin = sidebar_hover_layout_freeze.clone();
    let sidebar_hover_freeze_generation_for_unpin = sidebar_hover_freeze_generation.clone();
    let pin_button_for_menu = update_pin_button.clone();
    menu.connect_clicked(move |_| {
        // A hover-revealed sidebar is temporary. This button must pin it so
        // it stays open (mouse-leave stops auto-hiding once pinned), not
        // close it again.
        if !sidebar::is_pinned(&sidebar_for_hide) {
            // Release the reveal's layout freeze the same way the pin button
            // in the content header does: bump the generation so any pending
            // unfreeze timeout becomes a no-op, then unfreeze immediately —
            // pinning does not change the sidebar width.
            sidebar_hover_freeze_generation_for_unpin.set(
                sidebar_hover_freeze_generation_for_unpin
                    .get()
                    .wrapping_add(1),
            );
            sidebar_hover_layout_freeze_for_unpin.set(false);
            // set_pinned(true) also clears hover_open, so the shell's
            // mouse-leave handler no longer auto-hides the sidebar.
            sidebar::set_pinned(&sidebar_for_hide, true);
            pin_button_for_menu(true);
            return;
        }
        // Unpinning is a persistent layout change. Allow the animation to run
        // without intermediate Folder rebuilds, then reflow once at its final
        // width through WidthSettleGate.
        sidebar_hover_freeze_generation_for_unpin.set(
            sidebar_hover_freeze_generation_for_unpin
                .get()
                .wrapping_add(1),
        );
        sidebar_hover_layout_freeze_for_unpin.set(false);
        sidebar_layout_settle_for_hide.borrow_mut().begin();
        sidebar::set_pinned(&sidebar_for_hide, false);
        sidebar::clear_hover_open(&sidebar_for_hide);
        main_split_for_hide.set_show_sidebar(false);
    });

    let right_header = adw::HeaderBar::new();
    right_header.set_height_request(46);
    right_header.set_hexpand(true);
    right_header.set_show_start_title_buttons(!main_split.shows_sidebar());
    right_header.set_show_end_title_buttons(true);
    right_header.add_css_class("layout-right-header");

    let show_sidebar = gtk::Button::from_icon_name("sidebar-show-symbolic");
    show_sidebar.set_tooltip_text(Some("Show sidebar"));
    show_sidebar.add_css_class("flat");
    show_sidebar.set_visible(false);
    let main_split_for_show = main_split.clone();
    let sidebar_for_show = sidebar.clone();
    let sidebar_layout_settle_for_show = sidebar_layout_settle.clone();
    let sidebar_hover_layout_freeze_for_pin = sidebar_hover_layout_freeze.clone();
    let sidebar_hover_freeze_generation_for_pin = sidebar_hover_freeze_generation.clone();
    show_sidebar.connect_clicked(move |_| {
        // Pinning is the point where the sidebar becomes part of the persistent
        // layout. Recalculate only once after the slide has settled.
        sidebar_hover_freeze_generation_for_pin.set(
            sidebar_hover_freeze_generation_for_pin
                .get()
                .wrapping_add(1),
        );
        sidebar_hover_layout_freeze_for_pin.set(false);
        sidebar_layout_settle_for_show.borrow_mut().begin();
        sidebar::set_pinned(&sidebar_for_show, true);
        main_split_for_show.set_show_sidebar(true);
    });
    right_header.pack_start(&show_sidebar);

    let show_sidebar_for_state = show_sidebar.clone();
    // Theme-overridable window controls: the mode cell is written by the
    // ThemeEngine hook (build.rs) whenever a theme activates; placement runs
    // on every sidebar transition and mirrors the native title buttons'
    // left/right header movement.
    let window_controls_mode: Rc<Cell<crate::css::theme_discovery::WindowControls>> = Rc::new(
        Cell::new(crate::css::theme_discovery::WindowControls::Native),
    );
    let place_window_controls: Rc<dyn Fn()> = {
        let window_controls = window_controls.clone();
        let left_header = left_header.clone();
        let right_header = right_header.clone();
        let main_split = main_split.clone();
        let mode = window_controls_mode.clone();
        // Which header currently hosts the lights. get_parent() of a
        // headerbar-packed child is the headerbar's internal start box, not
        // the headerbar itself, so host tracking goes through this cell
        // instead of comparing parents.
        let host: Rc<RefCell<Option<adw::HeaderBar>>> = Rc::new(RefCell::new(None));
        Rc::new(move || {
            let traffic = mode.get()
                == crate::css::theme_discovery::WindowControls::TrafficLight;
            let sidebar_visible = main_split.shows_sidebar();
            // Native start title buttons only exist while traffic lights are
            // off; the traffic-light box replaces them wherever they live.
            left_header.set_show_start_title_buttons(!traffic && sidebar_visible);
            right_header.set_show_start_title_buttons(!traffic && !sidebar_visible);
            let mut host = host.borrow_mut();
            if traffic {
                let target = if sidebar_visible {
                    left_header.clone()
                } else {
                    right_header.clone()
                };
                if host.as_ref() != Some(&target) {
                    // Unparent from whatever holds the box (the previous
                    // header's internal start box) before repacking.
                    if window_controls.parent().is_some() {
                        window_controls.unparent();
                    }
                    target.pack_start(&window_controls);
                    *host = Some(target);
                }
                window_controls.set_visible(true);
            } else {
                if host.take().is_some() && window_controls.parent().is_some() {
                    window_controls.unparent();
                }
                window_controls.set_visible(false);
            }
        })
    };
    let place_for_notify = place_window_controls.clone();
    main_split.connect_show_sidebar_notify(move |split| {
        let sidebar_visible = split.shows_sidebar();
        show_sidebar_for_state.set_visible(!sidebar_visible);
        // Keep window controls (native title buttons or theme traffic lights)
        // at the window's top-left corner in every sidebar state, and never
        // render them in both headers at once (which would also duplicate
        // them mid-slide during the animation).
        place_for_notify();
    });
    // The engine reports the active theme's window-control style on every
    // activation (startup + switches); placement reacts here where the
    // headers and the mode cell live.
    theme_engine.set_on_window_controls_changed({
        let window_controls_mode = window_controls_mode.clone();
        let place_for_hook = place_window_controls.clone();
        Rc::new(move |controls: crate::css::theme_discovery::WindowControls| {
            window_controls_mode.set(controls);
            place_for_hook();
        })
    });
    place_window_controls();

    let search = gtk::SearchEntry::new();
    search.set_placeholder_text(Some("Search photos"));
    search.set_width_chars(18);
    // Do not force a 320px minimum. The fixed minimum was wider than the
    // available header centre area in smaller windows and pushed toolbar
    // buttons outside the visible allocation.
    search.set_size_request(220, -1);
    search.set_hexpand(true);
    search.add_css_class("search-field");
    search_entry_slot.replace(Some(search.clone()));
    let search_area = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    search_area.set_valign(gtk::Align::Center);
    search_area.set_size_request(220, -1);
    search_area.set_hexpand(true);
    search_area.append(&search);
    // Refresh/export/batch status lives in the header title area beside the
    // search entry (hidden when idle), so it never resizes the photo grid.
    // The search entry keeps hexpand and its 220px floor; the status row
    // degrades (bar → label → all) before that floor is threatened.
    search_area.append(operation_progress.slot());
    operation_progress.watch_space(&search_area);
    let (suggestion_popover, suggestion_list) = folder_suggestion_popup(&search);
    search_popup_slot.replace(Some(suggestion_popover.clone()));
    connect_search_popup_dismissal(window.upcast_ref(), &search, &suggestion_popover);

    let add_selected_to_collage = gtk::Button::with_label("Add Selected to Collage");
    add_selected_to_collage.set_visible(false);
    add_selected_to_collage.add_css_class("suggested-action");
    add_selected_to_collage
        .set_tooltip_text(Some("Add the selected library photos to the collage"));
    right_header.pack_end(&add_selected_to_collage);
    collage_add_mode_slot.replace(Some({
        let prepare = collage_prepare_add_slot.clone();
        Rc::new(move || {
            if let Some(prepare) = prepare.borrow().as_ref() {
                prepare();
            }
        })
    }));
    collage_close_slot.replace(Some({
        let main_stack = main_stack.clone();
        let button = add_selected_to_collage.clone();
        let gallery = gallery.clone();
        let collage_page = collage_page.clone();
        let collage_editor = collage_editor.clone();
        let collage_add_mode = collage_add_mode.clone();
        let connection = connection.clone();
        Rc::new(move || {
            gallery.set_collage_selection_mode(false);
            // Drop the grid selection too: otherwise the collage photos
            // stay selected and the next toolbar click reads as an
            // explicit selection, silently skipping "Resume Collage?".
            gallery.set_selected_photo_ids(&[]);
            collage_add_mode.set(false);
            // Persist the draft before teardown so "Resume Collage?" can
            // restore it the next time the editor opens.
            {
                let editor_handle = collage_editor.borrow();
                if let Some(editor) = editor_handle.as_ref() {
                    let json = editor.draft_json();
                    let guard = connection.borrow();
                    let _ =
                        db::set_setting(&guard, crate::collage::DRAFT_SETTING_KEY, &json);
                }
            }
            main_stack.set_visible_child_name("photos");
            button.set_visible(false);
            // Detach the editor so the stack's size request drops back to
            // normal. GtkStack reports the maximum minimum size of all its
            // pages (hidden ones included), so a collage page left attached
            // keeps demanding its tall minimum and pushes the shared bottom
            // bar offscreen on short windows.
            while let Some(child) = collage_page.first_child() {
                collage_page.remove(&child);
            }
            collage_editor.replace(None);
        })
    }));
    collage_prepare_add_slot.replace(Some({
        let main_stack = main_stack.clone();
        let button = add_selected_to_collage.clone();
        let gallery = gallery.clone();
        let collage_editor = collage_editor.clone();
        let collage_add_mode = collage_add_mode.clone();
        let filter = filter.clone();
        let connection = connection.clone();
        let sort = sort.clone();
        Rc::new(move || {
            let editor_handle = collage_editor.borrow();
            let Some(editor) = editor_handle.as_ref() else {
                return;
            };
            gallery.set_collage_selection_mode(true);
            gallery.set_selected_photo_ids(&editor.photo_ids());
            // Mark the detour before switching pages so the stack's
            // visible-child teardown spares the live editor.
            collage_add_mode.set(true);
            if filter.get() == sidebar::SidebarFilter::Library {
                apply_gallery_grouping(
                    &gallery,
                    sidebar::SidebarFilter::All,
                    sort.get(),
                    grid::GroupMode::None,
                    true,
                );
                refresh_grid(&connection, sidebar::SidebarFilter::All, "", sort.get(), &gallery);
            }
            main_stack.set_visible_child_name("photos");
            button.set_visible(true);
        })
    }));
    {
        let main_stack = main_stack.clone();
        let button = add_selected_to_collage.clone();
        let gallery = gallery.clone();
        let connection = connection.clone();
        let collage_editor = collage_editor.clone();
        let collage_add_mode = collage_add_mode.clone();
        add_selected_to_collage.connect_clicked(move |_| {
            let photos = gallery
                .selected_photo_ids(None)
                .into_iter()
                .filter_map(|id| db::photo(&connection.borrow(), id).ok().flatten())
                .map(|photo| crate::photo_object::PhotoObject::from_photo(&photo))
                .collect::<Vec<_>>();
            if photos.len() < 2 {
                return;
            }
            if let Some(editor) = collage_editor.borrow().as_ref() {
                editor.set_photos(photos);
                gallery.set_collage_selection_mode(false);
                collage_add_mode.set(false);
                main_stack.set_visible_child_name("collage");
                button.set_visible(false);
            }
        });
    }
    {
        let info = info.clone();
        main_stack.connect_visible_child_notify(move |stack| {
            info.set_collage_active(stack.visible_child_name().as_deref() == Some("collage"));
        });
    }
    {
        let open_edit = open_edit.clone();
        let selected_photo = selected_photo.clone();
        let main_stack = main_stack.clone();
        let one_to_one = info.one_to_one.clone();
        let collage_editing = collage_editing.clone();
        let collage_editor = collage_editor.clone();
        let edit_space_slot_for_toggle = edit_space_slot.clone();
        info.edit.connect_clicked(move |_| {
            // The bottom Edit button is only for normal photo editing. Collage
            // photos use the collage editor's own Edit Photo action, which
            // preserves the return-to-collage state.
            if main_stack.visible_child_name().as_deref() == Some("collage") {
                return;
            }

            // The bottom Edit button is a true open/close toggle. This keeps
            // the editing workspace optional instead of forcing users to use
            // Back/Done just to return to normal browsing.
            if main_stack.visible_child_name().as_deref() == Some("edit") {
                one_to_one.set_active(false);
                // Drop the edit Space slot so gallery/lightbox Space works
                // again once the toggle leaves the edit page.
                edit_space_slot_for_toggle.borrow_mut().take();
                if collage_editing.get() {
                    // The edit session belongs to a collage: return there.
                    collage_editing.set(false);
                    if let Some(photo) = selected_photo.borrow().as_ref() {
                        if let Ok(editor_handle) = collage_editor.try_borrow() {
                            if let Some(collage) = editor_handle.as_ref() {
                                collage.refresh_photo_metadata(std::slice::from_ref(photo));
                            }
                        }
                    }
                    main_stack.set_visible_child_name("collage");
                } else {
                    main_stack.set_visible_child_name("photos");
                }
                return;
            }
            // Copy the id out before open_edit(). That call changes the stack
            // synchronously, which may trigger a gallery selection callback
            // that mutably borrows selected_photo.
            let photo_id = selected_photo.borrow().as_ref().map(|photo| photo.id());
            if let Some(id) = photo_id {
                open_edit(id);
            }
        });
    }
    {
        let open_collage = open_collage.clone();
        let gallery = gallery.clone();
        let main_stack = main_stack.clone();
        let collage_editor = collage_editor.clone();
        let connection = connection.clone();
        info.collage.connect_clicked(move |_| {
            // Re-entering Collage while its editor is still visible happens
            // before the normal stack teardown can save the live project.
            // Persist it here so Resume always refers to what the user is
            // actually looking at, not an older database draft.
            if main_stack.visible_child_name().as_deref() == Some("collage") {
                if let Ok(editor_handle) = collage_editor.try_borrow() {
                    if let Some(editor) = editor_handle.as_ref() {
                        let json = editor.draft_json();
                        let guard = connection.borrow();
                        let _ = db::set_setting(
                            &guard,
                            crate::collage::DRAFT_SETTING_KEY,
                            &json,
                        );
                    }
                }
            }

            open_collage(gallery.selected_photo_ids(None));
        });
    }
    // Keep the search field centered in the header.
    right_header.set_title_widget(Some(&search_area));

    

    // The lightbox takes keyboard focus while it is open and covers the
    // header, so the search entry cannot be clicked or receive typed input.
    // Keep the existing search entry and handlers, but provide the standard
    // shortcut to close the overlay and return focus to search.
    let search_keyboard = gtk::EventControllerKey::new();
    search_keyboard.set_propagation_phase(gtk::PropagationPhase::Capture);
    let search_for_keyboard = search.clone();
    let lightbox_for_search_keyboard = lightbox.clone();
    search_keyboard.connect_key_pressed(move |_, key, _, modifiers| {
        if key == gtk::gdk::Key::f
            && modifiers.contains(gtk::gdk::ModifierType::CONTROL_MASK)
        {
            if lightbox_for_search_keyboard.root.is_visible() {
                lightbox_for_search_keyboard.close();
            }
            search_for_keyboard.grab_focus();
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    window.add_controller(search_keyboard);

    let begin_typed_search: Rc<dyn Fn() -> bool> = {
        let stack = main_stack.downgrade();
        let split = main_split.downgrade();
        Rc::new(move || {
            let (Some(stack), Some(split)) = (stack.upgrade(), split.upgrade()) else {
                return false;
            };
            // The current photo viewer remains open while the gallery behind
            // it updates to the new search results.
            if !matches!(stack.visible_child_name().as_deref(), Some("photos" | "library")) {
                return false;
            }
            if split.is_collapsed() && split.shows_sidebar() {
                split.set_show_sidebar(false);
            }
            true
        })
    };
    // Attach this at the window boundary so typing still starts a search when
    // the fullscreen photo viewer is the widget currently receiving input.
    connect_type_to_search(window.upcast_ref::<gtk::Widget>(), &search, begin_typed_search);

    // Once a search has been entered, the entry retains focus. Forward the
    // gallery navigation keys so they do not only move the text cursor after
    // a folder suggestion has been selected.
    let gallery_for_search_navigation = gallery.clone();
    let search_for_navigation = search.clone();
    let popup_for_navigation = suggestion_popover.clone();
    let search_navigation = gtk::EventControllerKey::new();
    search_navigation.set_propagation_phase(gtk::PropagationPhase::Capture);
    search_navigation.connect_key_pressed(move |controller, key, _, modifiers| {
        if modifiers.intersects(
            gtk::gdk::ModifierType::CONTROL_MASK
                | gtk::gdk::ModifierType::ALT_MASK
                | gtk::gdk::ModifierType::SUPER_MASK
                | gtk::gdk::ModifierType::META_MASK,
        ) || !matches!(
            key,
            gtk::gdk::Key::Left
                | gtk::gdk::Key::Right
                | gtk::gdk::Key::Up
                | gtk::gdk::Key::Down
        )
        {
            return glib::Propagation::Proceed;
        }
        let Some(root) = search_for_navigation.root() else {
            return glib::Propagation::Proceed;
        };
        let focused_in_search = root.focus().is_some_and(|focus| {
            focus == search_for_navigation.upcast_ref::<gtk::Widget>().clone()
                || focus.is_ancestor(&search_for_navigation)
        });
        if !focused_in_search
            || (popup_for_navigation.is_visible()
                && matches!(key, gtk::gdk::Key::Up | gtk::gdk::Key::Down))
        {
            return glib::Propagation::Proceed;
        }
        let grid_target = gallery_for_search_navigation.visible_root();
        grid_target.grab_focus();
        let _ = controller.forward(&grid_target);
        glib::Propagation::Stop
    });
    window.add_controller(search_navigation);

    // The split layout has two in-content header bars instead of one native
    // titlebar. Preserve the usual titlebar double-click behavior on both:
    // maximize when normal, and restore the previous window size when maximized.
    for header in [&left_header, &right_header] {
        let window_for_titlebar = window.clone();
        let titlebar_double_click = gtk::GestureClick::new();
        titlebar_double_click.set_button(1);
        titlebar_double_click.connect_pressed(move |gesture, n_press, _, _| {
            if n_press != 2 {
                return;
            }
            if window_for_titlebar.is_maximized() {
                window_for_titlebar.unmaximize();
            } else {
                window_for_titlebar.maximize();
            }
            gesture.set_state(gtk::EventSequenceState::Claimed);
        });
        header.add_controller(titlebar_double_click);
    }

    let gallery_for_search = gallery.clone();
    let connection_for_search = connection.clone();
    let filter_for_search = filter.clone();
    let search_text_for_search = search_text.clone();
    let sort_for_search = sort.clone();
    let group_mode_for_search = group_mode.clone();
    let search_suppressed_for_search = search_suppressed.clone();
    let cleared_search_query_for_search = cleared_search_query.clone();
    let search_debounce_for_search = search_debounce.clone();
    let destination_click_for_search = destination_click.clone();
    let sidebar_selection_for_search = sidebar_selection_slot.clone();
    let suggestion_popover_for_search = suggestion_popover.clone();
    let suggestion_list_for_search = suggestion_list.clone();
    let stack_for_home_search = main_stack.clone();

    search.connect_search_changed(move |entry| {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if search_suppressed_for_search.get() {
                return;
            }
            if let Some(source) = search_debounce_for_search.borrow_mut().take() {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    source.remove();
                }));
            }
            let query = entry.text().to_string();
            if should_ignore_cleared_search_event(
                false,
                cleared_search_query_for_search.borrow().as_deref(),
                &query,
            ) {
                cleared_search_query_for_search.replace(None);
                return;
            }
            // Folder suggestions come directly from the library database.
            // This is intentionally a lightweight lookup: do not call db::folders()
            // here because it also calculates recursive counts and availability.
            let folders_for_search = db::search_folders(
                &connection_for_search.borrow(),
                &query,
                24,
            )
            .unwrap_or_default();
            search_text_for_search.replace(query.clone());
            if filter_for_search.get() == sidebar::SidebarFilter::Library {
                stack_for_home_search.set_visible_child_name(
                    if query.is_empty() { "library" } else { "photos" },
                );
                gallery_for_search.set_grouping(grid::GroupMode::None, grid::GroupDate::Taken);
            }
            if !query.is_empty() {
                gallery_for_search.cancel_progressive_build();
            }
            // Search is a global results view even when it was started from a
            // folder. Temporarily leave the Folder/History stream while text
            // is active; clearing the query restores the grouped view.
            if matches!(filter_for_search.get(), sidebar::SidebarFilter::Folder(_))
                || filter_for_search.get() == sidebar::SidebarFilter::History
            {
                if query.is_empty() {
                    apply_gallery_grouping(
                        &gallery_for_search,
                        filter_for_search.get(),
                        sort_for_search.get(),
                        group_mode_for_search.get(),
                        true,
                    );
                } else {
                    gallery_for_search.set_grouping(
                        grid::GroupMode::None,
                        grid::GroupDate::Taken,
                    );
                    if matches!(filter_for_search.get(), sidebar::SidebarFilter::Folder(_)) {
                        if let Some(sidebar) = sidebar_selection_for_search.borrow().as_ref() {
                            sidebar::set_scroll_location(sidebar, None);
                        }
                    }
                }
            }
            
            update_folder_suggestions(
                &suggestion_popover_for_search,
                &suggestion_list_for_search,
                &folders_for_search,
                &query,
                Rc::new({
                    let destination_click = destination_click_for_search.clone();
                    let sidebar_selection = sidebar_selection_for_search.clone();
                    let gallery = gallery_for_search.clone();
                    let filter = filter_for_search.clone();
                    let search_text = search_text_for_search.clone();
                    let folders = folders_for_search.clone();
                    move |folder_id| {
                        
                        let folder_path = folders
                            .iter()
                            .find(|folder| folder.id == folder_id)
                            .map(|folder| folder.path.clone());

                        // Folder suggestions are navigation results. Clear the search and
                        // enter the normal continuous Folder view, then focus this folder.
                        // destination_click restores the cached Folder stream when available.
                        destination_click(sidebar::SidebarFilter::Folder(folder_id));

                        // The first Folder navigation after startup has no cache yet. The
                        // continuous stream is built progressively, so preserve the requested
                        // folder on the Gallery until its rows are ready. This also lets a
                        // valid cache restore focus immediately without reloading the stream.
                        if let Some(folder_path) = folder_path {
                            gallery.set_pending_folder_target(folder_id, folder_path);
                            
                            let focused_immediately = gallery.try_focus_pending_folder();
                            let gallery = gallery.clone();
                            let filter = filter.clone();
                            let search_text = search_text.clone();
                            let attempts = Rc::new(Cell::new(0u32));
                            let attempts_for_timer = attempts.clone();
                            if !focused_immediately {
                                glib::timeout_add_local(Duration::from_millis(25), move || {
                                let attempt = attempts_for_timer.get() + 1;
                                attempts_for_timer.set(attempt);
                                let still_on_target = filter.get()
                                    == sidebar::SidebarFilter::Folder(folder_id);
                                let search_is_clear = search_text.borrow().is_empty();
                                let building = gallery.stream_building();
                                let focused = if still_on_target && search_is_clear {
                                    gallery.try_focus_pending_folder()
                                } else {
                                    false
                                };
                                let pending = gallery.has_pending_folder_target();

                                if search_folder_focus_should_stop(
                                    still_on_target,
                                    search_is_clear,
                                    building,
                                    pending,
                                    focused,
                                    attempt,
                                ) {
                                    glib::ControlFlow::Break
                                } else {
                                    glib::ControlFlow::Continue
                                }
                                });
                            }
                        }

                        if let Some(sidebar) = sidebar_selection.borrow().as_ref().cloned() {
                            glib::timeout_add_local_once(
                                Duration::from_millis(100),
                                move || sidebar::scroll_to_folder(&sidebar, folder_id),
                            );
                        }
                    }
                }),
            );

            if query.is_empty() {
                refresh_grid(
                    &connection_for_search,
                    filter_for_search.get(),
                    "",
                    sort_for_search.get(),
                    &gallery_for_search,
                );
            } else {
                let connection = connection_for_search.clone();
                let filter = filter_for_search.clone();
                let search_text = search_text_for_search.clone();
                let sort = sort_for_search.clone();
                let gallery = gallery_for_search.clone();
                let debounce_slot = search_debounce_for_search.clone();
                let query_for_refresh = query.clone();
                let source = glib::timeout_add_local(
                    Duration::from_millis(SEARCH_DEBOUNCE_MS),
                    move || {
                        // The source removes itself after returning Break. Clear
                        // the slot now so a later keystroke never tries to remove
                        // an already-finished SourceId.
                        debounce_slot.borrow_mut().take();
                        if search_text.borrow().as_str() != query_for_refresh {
                            return glib::ControlFlow::Break;
                        }
                        refresh_grid(
                            &connection,
                            filter.get(),
                            &query_for_refresh,
                            sort.get(),
                            &gallery,
                        );
                        
                        glib::ControlFlow::Break
                    },
                );
                search_debounce_for_search.replace(Some(source));
            }
            
        }));
        if let Err(payload) = result {
            let message = payload
                .downcast_ref::<&str>()
                .copied()
                .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
                .unwrap_or("non-string panic payload");
            eprintln!("SEARCH ERROR changed handler: {message}");
        }
    });

    let search_text_for_activate = search_text.clone();
    let search_debounce_for_activate = search_debounce.clone();
    let filter_for_activate = filter.clone();
    let connection_for_activate = connection.clone();
    let sort_for_activate = sort.clone();
    let group_mode_for_activate = group_mode.clone();
    let gallery_for_activate = gallery.clone();
    let suggestion_popover_for_activate = suggestion_popover.clone();
    let stack_for_home_activate = main_stack.clone();
    search.connect_activate(move |entry| {
        if let Some(source) = search_debounce_for_activate.borrow_mut().take() {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| source.remove()));
        }
        let query = entry.text().to_string();
        search_text_for_activate.replace(query.clone());
        if filter_for_activate.get() == sidebar::SidebarFilter::Library {
            stack_for_home_activate.set_visible_child_name(
                if query.is_empty() { "library" } else { "photos" },
            );
            gallery_for_activate.set_grouping(grid::GroupMode::None, grid::GroupDate::Taken);
        }
        suggestion_popover_for_activate.popdown();
        if matches!(filter_for_activate.get(), sidebar::SidebarFilter::Folder(_))
            || filter_for_activate.get() == sidebar::SidebarFilter::History
        {
            if query.is_empty() {
                apply_gallery_grouping(
                    &gallery_for_activate,
                    filter_for_activate.get(),
                    sort_for_activate.get(),
                    group_mode_for_activate.get(),
                    true,
                );
            } else {
                gallery_for_activate.set_grouping(grid::GroupMode::None, grid::GroupDate::Taken);
            }
        }
        refresh_grid(
            &connection_for_activate,
            filter_for_activate.get(),
            &query,
            sort_for_activate.get(),
            &gallery_for_activate,
        );
    });

    (
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
    )
}
