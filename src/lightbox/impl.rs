impl Lightbox {
    pub fn new() -> Self {
        let root = gtk::Overlay::new();
        root.set_hexpand(true);
        root.set_vexpand(true);
        root.set_halign(gtk::Align::Fill);
        root.set_valign(gtk::Align::Fill);
        root.set_focusable(true);
        root.set_can_target(true);
        root.set_visible(false);

        let backdrop = gtk::Box::new(gtk::Orientation::Vertical, 0);
        backdrop.set_hexpand(true);
        backdrop.set_vexpand(true);
        backdrop.set_can_target(true);
        backdrop.add_css_class("lightbox-backdrop");
        root.set_child(Some(&backdrop));

        let picture = gtk::Picture::new();
        picture.set_can_shrink(true);
        picture.set_content_fit(gtk::ContentFit::Contain);
        picture.set_hexpand(false);
        picture.set_vexpand(false);
        picture.set_halign(gtk::Align::Center);
        picture.set_valign(gtk::Align::Center);
        picture.set_can_target(true);
        picture.add_css_class("lightbox-picture");

        // Keep the image in a viewport so native-size presentation can be
        // larger than the window and remain pannable instead of being forced
        // back into the overlay's allocation.
        let picture_viewport = gtk::ScrolledWindow::new();
        picture_viewport.set_hexpand(true);
        picture_viewport.set_vexpand(true);
        picture_viewport.set_halign(gtk::Align::Fill);
        picture_viewport.set_valign(gtk::Align::Fill);
        picture_viewport.set_can_target(true);
        // Keep scrollbars hidden while keeping the viewport constrained to
        // the lightbox allocation. External gives us real scroll ranges
        // without drawing normal scrollbar UI.
        picture_viewport.set_policy(gtk::PolicyType::External, gtk::PolicyType::External);
        picture_viewport.set_child(Some(&picture));
        root.add_overlay(&picture_viewport);

        let one_to_one_active = Rc::new(Cell::new(false));
        let native_texture: Rc<RefCell<Option<NativeTextureCache>>> = Rc::new(RefCell::new(None));
        let display_texture_cache: DisplayTextureCache = Rc::new(RefCell::new(VecDeque::new()));

        // Native-size 1:1 panning. Keep the drag gesture on the picture itself:
        // that is the widget under the pointer, while the ScrolledWindow owns
        // the adjustments that actually move the visible region.
        //
        // Do not manually claim the sequence here. The drag gesture is grouped
        // with the existing double-click gesture below, so a double-click can
        // still close the lightbox while a genuine drag pans the native image.
        let drag_start_h = Rc::new(Cell::new(0.0));
        let drag_start_v = Rc::new(Cell::new(0.0));
        let pan_drag = gtk::GestureDrag::new();
        pan_drag.set_button(1);
        pan_drag.set_propagation_phase(gtk::PropagationPhase::Capture);

        let one_to_one_for_drag_begin = one_to_one_active.clone();
        let root_for_drag_begin = root.clone();
        let viewport_for_drag_begin = picture_viewport.clone();
        let drag_start_h_begin = drag_start_h.clone();
        let drag_start_v_begin = drag_start_v.clone();
        pan_drag.connect_drag_begin(move |gesture, x, y| {
            if !one_to_one_for_drag_begin.get() {
                gesture.set_state(gtk::EventSequenceState::Denied);
                return;
            }

            // The drag controller is attached to the stationary root overlay.
            // Only start panning if the drag began inside the image viewport.
            let inside_viewport = viewport_for_drag_begin
                .compute_bounds(&root_for_drag_begin)
                .map(|bounds| {
                    x >= bounds.x() as f64
                        && y >= bounds.y() as f64
                        && x < (bounds.x() + bounds.width()) as f64
                        && y < (bounds.y() + bounds.height()) as f64
                })
                .unwrap_or(false);

            if !inside_viewport {
                gesture.set_state(gtk::EventSequenceState::Denied);
                return;
            }

            let hadj = viewport_for_drag_begin.hadjustment();
            let vadj = viewport_for_drag_begin.vadjustment();
            drag_start_h_begin.set(hadj.value());
            drag_start_v_begin.set(vadj.value());
            viewport_for_drag_begin.set_cursor_from_name(Some("grabbing"));

            if std::env::var_os("PICASA_TRACE").is_some() {
                eprintln!(
                    "UI TRACE lightbox_pan_begin x={} y={} h={} h_upper={} h_page={} v={} v_upper={} v_page={}",
                    x,
                    y,
                    hadj.value(),
                    hadj.upper(),
                    hadj.page_size(),
                    vadj.value(),
                    vadj.upper(),
                    vadj.page_size()
                );
            }
        });

        let one_to_one_for_drag_update = one_to_one_active.clone();
        let viewport_for_drag_update = picture_viewport.clone();
        let drag_start_h_update = drag_start_h.clone();
        let drag_start_v_update = drag_start_v.clone();
        pan_drag.connect_drag_update(move |_, offset_x, offset_y| {
            if !one_to_one_for_drag_update.get() {
                return;
            }

            let hadj = viewport_for_drag_update.hadjustment();
            let vadj = viewport_for_drag_update.vadjustment();
            let max_h = (hadj.upper() - hadj.page_size()).max(hadj.lower());
            let max_v = (vadj.upper() - vadj.page_size()).max(vadj.lower());

            let new_h = (drag_start_h_update.get() - offset_x).clamp(hadj.lower(), max_h);
            let new_v = (drag_start_v_update.get() - offset_y).clamp(vadj.lower(), max_v);
            hadj.set_value(new_h);
            vadj.set_value(new_v);

            if std::env::var_os("PICASA_TRACE").is_some() {
                eprintln!(
                    "UI TRACE lightbox_pan_update dx={} dy={} h={} v={}",
                    offset_x, offset_y, new_h, new_v
                );
            }
        });

        let one_to_one_for_drag_end = one_to_one_active.clone();
        let viewport_for_drag_end = picture_viewport.clone();
        pan_drag.connect_drag_end(move |_, _, _| {
            viewport_for_drag_end.set_cursor_from_name(if one_to_one_for_drag_end.get() {
                Some("grab")
            } else {
                None
            });
        });
        let photos = Rc::new(RefCell::new(Vec::<PhotoObject>::new()));
        let index = Rc::new(Cell::new(0usize));
        let last_width = Rc::new(Cell::new(0i32));
        let last_height = Rc::new(Cell::new(0i32));
        let zoom = Rc::new(Cell::new(0.0)); // 0 means fit-to-window
        let zoom_before_one_to_one = Rc::new(Cell::new(0.0));
        let load_generation = Rc::new(Cell::new(0u64));
        let decode_cancel: Rc<RefCell<Option<Arc<AtomicBool>>>> = Rc::new(RefCell::new(None));
        let photo_changed: PhotoChangedHandler = Rc::new(RefCell::new(None));
        let context_menu: ContextMenuHandler = Rc::new(RefCell::new(None));
        let collection_navigation: CollectionNavigationHandler = Rc::new(RefCell::new(None));

        let double_click = gtk::GestureClick::new();
        double_click.set_button(1);
        let root_for_double = root.clone();
        let generation_for_double = load_generation.clone();
        let cancel_for_double = decode_cancel.clone();
        double_click.connect_pressed(move |gesture, n_press, _, _| {
            if n_press == 2 {
                generation_for_double.set(generation_for_double.get().wrapping_add(1));
                if let Some(active) = cancel_for_double.borrow_mut().take() {
                    active.store(true, Ordering::Release);
                }
                root_for_double.set_visible(false);
                gesture.set_state(gtk::EventSequenceState::Claimed);
            }
        });
        // Double-click stays on the picture. The pan gesture is attached to
        // the stationary root overlay so its coordinates do not move while
        // the ScrolledWindow adjustments pan the image.
        picture.add_controller(double_click);
        root.add_controller(pan_drag);

        // Some close paths intentionally hide the overlay directly (outside
        // click and double-click). Reset the internal presentation state for
        // those paths as well as for Lightbox::close().
        let one_to_one_for_visibility = one_to_one_active.clone();
        let zoom_for_visibility = zoom.clone();
        let picture_for_visibility = picture.clone();
        let viewport_for_visibility = picture_viewport.clone();
        root.connect_visible_notify(move |root| {
            if !root.is_visible() {
                one_to_one_for_visibility.set(false);
                zoom_for_visibility.set(0.0);
                picture_for_visibility.set_can_shrink(true);
                viewport_for_visibility.set_cursor_from_name(None);
                reset_viewport(&viewport_for_visibility);
            }
        });

        let outside_click = gtk::GestureClick::new();
        outside_click.set_button(1);
        outside_click.set_propagation_phase(gtk::PropagationPhase::Capture);

        let root_for_outside = root.clone();
        let picture_for_outside = picture.clone();

        outside_click.connect_pressed(move |gesture, n_press, x, y| {
            if n_press > 1 {
                return;
            }

            let inside_picture = picture_for_outside
                .compute_bounds(&root_for_outside)
                .map(|bounds| {
                    x >= bounds.x() as f64
                        && y >= bounds.y() as f64
                        && x < (bounds.x() + bounds.width()) as f64
                        && y < (bounds.y() + bounds.height()) as f64
                })
                .unwrap_or(false);

            if !inside_picture {
                root_for_outside.set_visible(false);
                gesture.set_state(gtk::EventSequenceState::Claimed);
            }
        });
        root.add_controller(outside_click);

        // Capture secondary clicks on the stable photo viewport.
        let make_context_click = {
            let photos = photos.clone();
            let index = index.clone();
            let context_menu = context_menu.clone();
            let root = root.clone();
            let picture_viewport = picture_viewport.clone();
            let menu_anchor: gtk::Widget = picture_viewport.clone().upcast();
            move || {
                let right_click = gtk::GestureClick::new();
                right_click.set_button(3);
                right_click.set_propagation_phase(gtk::PropagationPhase::Capture);
                let photos = photos.clone();
                let index = index.clone();
                let context_menu = context_menu.clone();
                let menu_anchor = menu_anchor.clone();
                right_click.connect_pressed(move |gesture, _, x, y| {
                    if std::env::var_os("PICASA_TRACE").is_some() {
                        eprintln!(
                            "UI TRACE lightbox_context_click received visible={} targetable={}",
                            menu_anchor.is_visible(),
                            menu_anchor.can_target()
                        );
                    }
                    let Some(photo) = photos.borrow().get(index.get()).cloned() else {
                        if std::env::var_os("PICASA_TRACE").is_some() {
                            eprintln!("UI TRACE lightbox_context_click no_current_photo");
                        }
                        return;
                    };
                    if let Some(handler) = context_menu.borrow().as_ref() {
                        if std::env::var_os("PICASA_TRACE").is_some() {
                            eprintln!(
                                "UI TRACE lightbox_context_menu index={} path={}",
                                index.get(),
                                photo.path()
                            );
                        }
                        let point = gtk::graphene::Point::new(x as f32, y as f32);
                        let Some(root_point) = picture_viewport.compute_point(&root, &point) else {
                            return;
                        };
                        handler(
                            photo,
                            root.clone().upcast::<gtk::Widget>(),
                            root_point.x() as f64,
                            root_point.y() as f64,
                        );
                        gesture.set_state(gtk::EventSequenceState::Claimed);
                    } else if std::env::var_os("PICASA_TRACE").is_some() {
                        eprintln!("UI TRACE lightbox_context_click no_handler");
                    }
                });
                right_click
            }
        };
        picture_viewport.add_controller(make_context_click());

        let picture_for_fit = picture.clone();
        let photos_for_fit = photos.clone();
        let index_for_fit = index.clone();
        let last_width_for_fit = last_width.clone();
        let last_height_for_fit = last_height.clone();
        let zoom_for_fit = zoom.clone();

        root.add_tick_callback(move |root, _| {
            if !root.is_visible() {
                return glib::ControlFlow::Continue;
            }

            let width = root.width();
            let height = root.height();

            if width > 0
                && height > 0
                && (width != last_width_for_fit.get() || height != last_height_for_fit.get())
            {
                last_width_for_fit.set(width);
                last_height_for_fit.set(height);

                fit_picture(
                    &picture_for_fit,
                    &photos_for_fit.borrow(),
                    index_for_fit.get(),
                    width,
                    height,
                    zoom_for_fit.get(),
                );
            }

            glib::ControlFlow::Continue
        });

        let scroll = gtk::EventControllerScroll::new(
            gtk::EventControllerScrollFlags::VERTICAL | gtk::EventControllerScrollFlags::DISCRETE,
        );
        // Capture wheel events before GtkScrolledWindow can consume them.
        // Mouse wheel is reserved for previous/next photo navigation in both
        // fit mode and 1:1 mode. Panning in 1:1 is mouse-drag only.
        scroll.set_propagation_phase(gtk::PropagationPhase::Capture);

        let photos_for_scroll = photos.clone();
        let index_for_scroll = index.clone();
        let picture_for_scroll = picture.clone();
        let root_for_scroll = root.clone();
        let zoom_for_scroll = zoom.clone();
        let generation_for_scroll = load_generation.clone();
        let cancel_for_scroll = decode_cancel.clone();
        let photo_changed_for_scroll = photo_changed.clone();
        let viewport_for_scroll = picture_viewport.clone();
        let native_texture_for_scroll = native_texture.clone();
        let display_cache_for_scroll = display_texture_cache.clone();
        let one_to_one_for_scroll = one_to_one_active.clone();

        scroll.connect_scroll(move |controller, _, dy| {
            if std::env::var_os("PICASA_TRACE").is_some() {
                eprintln!(
                    "UI TRACE lightbox_scroll dy={} visible={} focus={} index={}",
                    dy,
                    root_for_scroll.is_visible(),
                    root_for_scroll.has_focus(),
                    index_for_scroll.get()
                );
            }
            // Ctrl+wheel zooms the image; ordinary wheel keeps navigation.
            // The controller's modifier state is sampled on the GTK thread.
            if controller
                .current_event_state()
                .contains(gtk::gdk::ModifierType::CONTROL_MASK)
            {
                let current = if zoom_for_scroll.get() <= 0.0 {
                    1.0
                } else {
                    zoom_for_scroll.get()
                };
                zoom_for_scroll
                    .set((current * if dy < 0.0 { 1.12 } else { 0.89 }).clamp(0.25, 4.0));
                fit_picture(
                    &picture_for_scroll,
                    &photos_for_scroll.borrow(),
                    index_for_scroll.get(),
                    root_for_scroll.width(),
                    root_for_scroll.height(),
                    zoom_for_scroll.get(),
                );
                return glib::Propagation::Stop;
            }
            if dy == 0.0 || !root_for_scroll.is_visible() {
                return glib::Propagation::Proceed;
            }

            let len = photos_for_scroll.borrow().len();
            if len == 0 {
                return glib::Propagation::Stop;
            }

            let current = index_for_scroll.get();
            let next = if dy < 0.0 {
                current.saturating_sub(1)
            } else {
                (current + 1).min(len - 1)
            };

            if next != current {
                index_for_scroll.set(next);
                zoom_for_scroll.set(0.0);
                one_to_one_for_scroll.set(false);
                native_texture_for_scroll.borrow_mut().take();
                reset_viewport(&viewport_for_scroll);
                let (fit_geometry_fixed, cache_hit) = prepare_navigation_photo(
                    &picture_for_scroll,
                    photos_for_scroll.borrow().get(next),
                    &root_for_scroll,
                    zoom_for_scroll.get(),
                    &display_cache_for_scroll,
                );
                notify_photo_changed(&photo_changed_for_scroll, &photos_for_scroll.borrow(), next);
                let generation = generation_for_scroll.get().wrapping_add(1);
                generation_for_scroll.set(generation);

                show_photo(
                    &picture_for_scroll,
                    &photos_for_scroll.borrow(),
                    next,
                    &root_for_scroll,
                    zoom_for_scroll.clone(),
                    generation_for_scroll.clone(),
                    generation,
                    cancel_for_scroll.clone(),
                    &viewport_for_scroll,
                    native_texture_for_scroll.clone(),
                    display_cache_for_scroll.clone(),
                    fit_geometry_fixed,
                    cache_hit,
                );
            }

            glib::Propagation::Stop
        });
        root.add_controller(scroll);

        let key = gtk::EventControllerKey::new();
        // Capture before focused children can consume the event, so Escape always closes the lightbox.
        key.set_propagation_phase(gtk::PropagationPhase::Capture);
        let root_for_escape = root.clone();
        let picture_for_key = picture.clone();
        let photos_for_key = photos.clone();
        let index_for_key = index.clone();
        let zoom_for_key = zoom.clone();
        let generation_for_key = load_generation.clone();
        let cancel_for_key = decode_cancel.clone();
        let photo_changed_for_key = photo_changed.clone();
        let viewport_for_key = picture_viewport.clone();
        let native_texture_for_key = native_texture.clone();
        let display_cache_for_key = display_texture_cache.clone();
        let one_to_one_for_key = one_to_one_active.clone();
        let collection_navigation_for_key = collection_navigation.clone();

        key.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape && root_for_escape.is_visible() {
                root_for_escape.set_visible(false);
                glib::Propagation::Stop
            } else if [gtk::gdk::Key::plus, gtk::gdk::Key::equal].contains(&key) {
                let current = if zoom_for_key.get() <= 0.0 {
                    1.0
                } else {
                    zoom_for_key.get()
                };
                zoom_for_key.set((current * 1.15).min(4.0));
                fit_picture(
                    &picture_for_key,
                    &photos_for_key.borrow(),
                    index_for_key.get(),
                    root_for_escape.width(),
                    root_for_escape.height(),
                    zoom_for_key.get(),
                );
                glib::Propagation::Stop
            } else if key == gtk::gdk::Key::minus {
                let current = if zoom_for_key.get() <= 0.0 {
                    1.0
                } else {
                    zoom_for_key.get()
                };
                zoom_for_key.set((current * 0.87).max(0.25));
                fit_picture(
                    &picture_for_key,
                    &photos_for_key.borrow(),
                    index_for_key.get(),
                    root_for_escape.width(),
                    root_for_escape.height(),
                    zoom_for_key.get(),
                );
                glib::Propagation::Stop
            } else if key == gtk::gdk::Key::_0 {
                zoom_for_key.set(0.0);
                fit_picture(
                    &picture_for_key,
                    &photos_for_key.borrow(),
                    index_for_key.get(),
                    root_for_escape.width(),
                    root_for_escape.height(),
                    0.0,
                );
                glib::Propagation::Stop
            } else if key == gtk::gdk::Key::_1 {
                zoom_for_key.set(1.0);
                fit_picture(
                    &picture_for_key,
                    &photos_for_key.borrow(),
                    index_for_key.get(),
                    root_for_escape.width(),
                    root_for_escape.height(),
                    1.0,
                );
                glib::Propagation::Stop
            } else if key == gtk::gdk::Key::Left || key == gtk::gdk::Key::Right {
                let len = photos_for_key.borrow().len();
                if len == 0 {
                    return glib::Propagation::Stop;
                }

                let current = index_for_key.get();
                let next = if key == gtk::gdk::Key::Left {
                    current.saturating_sub(1)
                } else {
                    (current + 1).min(len - 1)
                };

                if next != current {
                    index_for_key.set(next);
                    zoom_for_key.set(0.0);
                    one_to_one_for_key.set(false);
                    native_texture_for_key.borrow_mut().take();
                    reset_viewport(&viewport_for_key);
                    let (fit_geometry_fixed, cache_hit) = prepare_navigation_photo(
                        &picture_for_key,
                        photos_for_key.borrow().get(next),
                        &root_for_escape,
                        zoom_for_key.get(),
                        &display_cache_for_key,
                    );
                    notify_photo_changed(&photo_changed_for_key, &photos_for_key.borrow(), next);
                    let generation = generation_for_key.get().wrapping_add(1);
                    generation_for_key.set(generation);
                    show_photo(
                        &picture_for_key,
                        &photos_for_key.borrow(),
                        next,
                        &root_for_escape,
                        zoom_for_key.clone(),
                        generation_for_key.clone(),
                        generation,
                        cancel_for_key.clone(),
                        &viewport_for_key,
                        native_texture_for_key.clone(),
                        display_cache_for_key.clone(),
                        fit_geometry_fixed,
                        cache_hit,
                    );
                }
                glib::Propagation::Stop
            } else if key == gtk::gdk::Key::Up || key == gtk::gdk::Key::Down {
                let direction = if key == gtk::gdk::Key::Up { -1 } else { 1 };
                if let Some(handler) = collection_navigation_for_key.borrow().as_ref() {
                    handler(direction);
                    glib::Propagation::Stop
                } else {
                    glib::Propagation::Proceed
                }
            } else {
                glib::Propagation::Proceed
            }
        });
        root.add_controller(key);

        Self {
            root,
            backdrop,
            picture,
            picture_viewport,
            photos,
            index,
            last_width,
            last_height,
            zoom,
            zoom_before_one_to_one,
            one_to_one_active,
            native_texture,
            display_texture_cache,
            load_generation,
            decode_cancel,
            photo_changed,
            context_menu,
            collection_navigation,
        }
    }

    pub fn set_photo_changed_handler(&self, handler: impl Fn(PhotoObject) + 'static) {
        self.photo_changed.replace(Some(Box::new(handler)));
    }

    pub fn set_context_menu_handler(
        &self,
        handler: impl Fn(PhotoObject, gtk::Widget, f64, f64) + 'static,
    ) {
        self.context_menu.replace(Some(Box::new(handler)));
    }

    pub fn set_collection_navigation_handler(&self, handler: impl Fn(i32) + 'static) {
        self.collection_navigation.replace(Some(Box::new(handler)));
    }

    /// Toggle native-pixel presentation while remembering the previous zoom.
    /// A negative zoom is reserved for this temporary 1:1 mode.
    pub fn set_one_to_one(&self, enabled: bool) {
        if std::env::var_os("PICASA_TRACE").is_some() {
            eprintln!("UI TRACE lightbox_one_to_one enabled={enabled}");
        }

        if self.one_to_one_active.get() == enabled {
            return;
        }

        self.one_to_one_active.set(enabled);
        self.picture.set_can_shrink(!enabled);
        self.picture_viewport
            .set_cursor_from_name(if enabled { Some("grab") } else { None });

        if enabled {
            if self.zoom.get() != -1.0 {
                self.zoom_before_one_to_one.set(self.zoom.get());
                self.zoom.set(-1.0);
            }

            // Reuse the already decoded native texture when possible. This
            // makes repeated 1:1 toggles instantaneous instead of decoding
            // the same source again every time.
            let cached = self.native_texture.borrow().clone();
            let current = self
                .photos
                .borrow()
                .get(self.index.get())
                .map(|photo| (photo.path(), photo.rotation()));

            if let (Some(cache), Some((path, rotation))) = (cached, current) {
                if cache.path == path && cache.rotation == rotation {
                    self.picture.set_paintable(Some(&cache.texture));
                    fit_picture(
                        &self.picture,
                        &self.photos.borrow(),
                        self.index.get(),
                        self.root.width(),
                        self.root.height(),
                        -1.0,
                    );
                    self.picture.queue_resize();
                    self.picture_viewport.queue_resize();
                    center_viewport_soon(&self.picture_viewport);
                    return;
                }
            }
        } else {
            if self.zoom.get() == -1.0 {
                self.zoom.set(self.zoom_before_one_to_one.get());
            }

            fit_picture(
                &self.picture,
                &self.photos.borrow(),
                self.index.get(),
                self.root.width(),
                self.root.height(),
                self.zoom.get(),
            );
            self.picture.queue_resize();
            self.picture_viewport.queue_resize();
            reset_viewport(&self.picture_viewport);
            return;
        }

        if self.root.is_visible() {
            self.refresh_current();
        }
    }

    pub fn use_iphone_backdrop(&self) {
        self.backdrop.remove_css_class("standard-light");
        self.backdrop.remove_css_class("standard-dark");
    }

    pub fn use_standard_backdrop(&self, dark: bool) {
        self.backdrop.remove_css_class(if dark {
            "standard-light"
        } else {
            "standard-dark"
        });
        self.backdrop.add_css_class(if dark {
            "standard-dark"
        } else {
            "standard-light"
        });
    }

    pub fn open(&self, photos: Vec<PhotoObject>, selected: usize) {
        self.photos.replace(photos);

        let len = self.photos.borrow().len();
        if len == 0 {
            return;
        }

        self.index.set(selected.min(len - 1));
        self.zoom.set(0.0);
        self.zoom_before_one_to_one.set(0.0);
        self.one_to_one_active.set(false);
        self.native_texture.borrow_mut().take();
        reset_viewport(&self.picture_viewport);
        notify_photo_changed(&self.photo_changed, &self.photos.borrow(), self.index.get());
        self.last_width.set(0);
        self.last_height.set(0);
        let generation = self.load_generation.get().wrapping_add(1);
        self.load_generation.set(generation);

        // Make the overlay allocatable before selecting a decode target. On
        // the first open it was previously still 0x0 here, so the initial
        // photo used a fallback size and appeared smaller until navigation.
        self.root.set_visible(true);
        if std::env::var_os("PICASA_TRACE").is_some() {
            eprintln!(
                "UI TRACE lightbox_open photos={} index={} root_targetable={} viewport_targetable={} picture_targetable={}",
                len,
                self.index.get(),
                self.root.can_target(),
                self.picture_viewport.can_target(),
                self.picture.can_target()
            );
        }
        // The overlay can still be unmapped/unallocated at this exact point,
        // so the immediate focus request is not always enough. That left the
        // underlying GtkGridView owning the arrow keys until another action
        // (such as Enter) happened to move focus. Request focus now and again
        // on the next idle iteration, once GTK has mapped the lightbox.
        self.root.grab_focus();
        let root_for_focus = self.root.clone();
        glib::idle_add_local_once(move || {
            if root_for_focus.is_visible() {
                root_for_focus.grab_focus();
            }
        });

        if let Some(photo) = self.photos.borrow().get(self.index.get()) {
            show_cached_preview(&self.picture, photo);
        }

        // GTK does not allocate an overlay synchronously when it becomes
        // visible. Start the first full decode on its first allocated frame,
        // otherwise viewer_decode_target sees 0x0 and permanently uses the
        // fallback target until the user navigates.
        let picture = self.picture.clone();
        let photos = self.photos.clone();
        let index = self.index.clone();
        let zoom = self.zoom.clone();
        let current_generation = self.load_generation.clone();
        let decode_cancel = self.decode_cancel.clone();
        let picture_viewport = self.picture_viewport.clone();
        let native_texture = self.native_texture.clone();
        let display_texture_cache = self.display_texture_cache.clone();
        self.root.add_tick_callback(move |root, _| {
            if !root.is_visible() || current_generation.get() != generation {
                return glib::ControlFlow::Break;
            }
            if root.width() <= 0 || root.height() <= 0 {
                return glib::ControlFlow::Continue;
            }

            show_photo(
                &picture,
                &photos.borrow(),
                index.get(),
                root,
                zoom.clone(),
                current_generation.clone(),
                generation,
                decode_cancel.clone(),
                &picture_viewport,
                native_texture.clone(),
                display_texture_cache.clone(),
                false,
                false,
            );
            fit_picture(
                &picture,
                &photos.borrow(),
                index.get(),
                root.width(),
                root.height(),
                zoom.get(),
            );
            glib::ControlFlow::Break
        });
    }


    pub fn navigate_photo(&self, direction: i32) {
        if !self.root.is_visible() {
            return;
        }
        let len = self.photos.borrow().len();
        if len == 0 {
            return;
        }

        let current = self.index.get();
        let next = if direction < 0 {
            current.saturating_sub(1)
        } else if direction > 0 {
            (current + 1).min(len - 1)
        } else {
            current
        };
        if next == current {
            return;
        }

        self.index.set(next);
        self.zoom.set(0.0);
        self.one_to_one_active.set(false);
        self.native_texture.borrow_mut().take();
        reset_viewport(&self.picture_viewport);

        let (fit_geometry_fixed, cache_hit) = prepare_navigation_photo(
            &self.picture,
            self.photos.borrow().get(next),
            &self.root,
            self.zoom.get(),
            &self.display_texture_cache,
        );

        notify_photo_changed(&self.photo_changed, &self.photos.borrow(), next);

        let generation = self.load_generation.get().wrapping_add(1);
        self.load_generation.set(generation);
        show_photo(
            &self.picture,
            &self.photos.borrow(),
            next,
            &self.root,
            self.zoom.clone(),
            self.load_generation.clone(),
            generation,
            self.decode_cancel.clone(),
            &self.picture_viewport,
            self.native_texture.clone(),
            self.display_texture_cache.clone(),
            fit_geometry_fixed,
            cache_hit,
        );
    }

    pub fn navigate_collection(&self, direction: i32) {
        if !self.root.is_visible() || direction == 0 {
            return;
        }
        if let Some(handler) = self.collection_navigation.borrow().as_ref() {
            handler(direction);
        }
    }

    pub fn close(&self) {
        // Invalidate an in-flight full-resolution decode as well as hiding
        // the viewer. A late worker result must not repopulate a closed view.
        self.load_generation
            .set(self.load_generation.get().wrapping_add(1));
        if let Some(active) = self.decode_cancel.borrow_mut().take() {
            active.store(true, Ordering::Release);
        }
        self.one_to_one_active.set(false);
        self.zoom.set(0.0);
        reset_viewport(&self.picture_viewport);
        self.root.set_visible(false);
    }

    /// Re-decode the visible photo after presentation metadata such as the
    /// user's rotation changes. The current full image stays in place until
    /// its correctly rotated replacement is ready.
    pub fn refresh_current(&self) {
        if !self.root.is_visible() || self.photos.borrow().is_empty() {
            return;
        }
        let generation = self.load_generation.get().wrapping_add(1);
        self.load_generation.set(generation);
        show_photo(
            &self.picture,
            &self.photos.borrow(),
            self.index.get(),
            &self.root,
            self.zoom.clone(),
            self.load_generation.clone(),
            generation,
            self.decode_cancel.clone(),
            &self.picture_viewport,
            self.native_texture.clone(),
            self.display_texture_cache.clone(),
            false,
            false,
        );
    }
}
