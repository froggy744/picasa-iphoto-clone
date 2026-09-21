pub(crate) fn set_grid_scrub_active(active: bool) {
    GRID_SCRUB_ACTIVE.with(|flag| flag.set(active));
}

fn grid_scrub_active() -> bool {
    GRID_SCRUB_ACTIVE.with(Cell::get)
}

fn folder_thumbnail_cache_get(path: &str) -> Option<gtk::gdk::Paintable> {
    FOLDER_THUMBNAIL_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        let index = cache
            .iter()
            .position(|(cached_path, _)| cached_path == path)?;
        let entry = cache.remove(index)?;
        let paintable = entry.1.clone();
        cache.push_back(entry);
        Some(paintable)
    })
}

fn folder_thumbnail_cache_insert(path: String, paintable: gtk::gdk::Paintable) {
    FOLDER_THUMBNAIL_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        cache.retain(|(cached_path, _)| cached_path != &path);
        cache.push_back((path, paintable));
        while cache.len() > FOLDER_THUMBNAIL_CACHE_CAPACITY {
            cache.pop_front();
        }
    });
}

fn folder_thumbnail_cache_remove(key: &str) {
    FOLDER_THUMBNAIL_CACHE.with(|cache| {
        cache
            .borrow_mut()
            .retain(|(cached_key, _)| cached_key != key);
    });
}

fn photo_presentation_key(photo: &PhotoObject) -> Option<String> {
    let cached = photo.cached_thumbnail_path()?;
    Some(crate::thumbnail_display::presentation_key(
        &cached,
        &photo.path(),
        photo.rotation(),
        &photo.edit_recipe(),
        photo.width(),
        photo.height(),
    ))
}

fn photo_presentation_request(
    photo: &PhotoObject,
    visible_priority: bool,
) -> Option<crate::thumbnail_display::DisplayRequest> {
    let cached = photo.cached_thumbnail_path()?;
    Some(crate::thumbnail_display::request_for(
        cached,
        photo.path(),
        photo.mtime(),
        photo.size_bytes(),
        photo.rotation(),
        photo.edit_recipe(),
        photo.width(),
        photo.height(),
        visible_priority,
    ))
}

fn queue_photo_presentation_async(photo: &PhotoObject, visible_priority: bool) -> bool {
    let Some(request) = photo_presentation_request(photo, visible_priority) else {
        return false;
    };
    if folder_thumbnail_cache_get(&request.key).is_some() {
        return false;
    }
    crate::thumbnail_display::submit(request)
}

mod square_tile {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use glib::subclass::prelude::*;
    use gtk::prelude::*;
    use gtk::subclass::prelude::*;
    use gtk4 as gtk;

    use crate::photo_object::PhotoObject;

    #[derive(Default)]
    pub struct SquareTile {
        pub width: Cell<i32>,
        pub height: Cell<i32>,
        pub favorite_indicators_visible: Cell<bool>,
        pub photo: RefCell<Option<PhotoObject>>,
        pub visual_loaded: Cell<bool>,
        pub applied_visual_key: RefCell<Option<String>>,
        // Folder mode stores the backing photo index on each realized tile so
        // prefetch can warm the photo model ahead of the viewport rather than
        // being limited to GTK's currently realized widget pool.
        pub photo_index: Cell<Option<usize>>,
        // Stored so the offline badge can be created lazily on first use
        // (folder tiles no longer build it eagerly).
        pub unavailable: RefCell<Option<Rc<dyn Fn(PhotoObject, gtk::Widget)>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SquareTile {
        const NAME: &'static str = "PicasaSquareTile";
        type Type = super::SquareTile;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for SquareTile {
        fn dispose(&self) {
            self.photo.take();
            while let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for SquareTile {
        fn measure(&self, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            let requested = match orientation {
                gtk::Orientation::Horizontal => self.width.get().max(1),
                gtk::Orientation::Vertical => self.height.get().max(1),
                _ => self.width.get().max(1),
            };

            (requested, requested, -1, -1)
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            let child_width = self.width.get().min(width).max(1);
            let child_height = self.height.get().min(height).max(1);

            if let Some(child) = self.obj().first_child() {
                let x = ((width - child_width) / 2).max(0) as f32;
                let y = ((height - child_height) / 2).max(0) as f32;

                let transform =
                    gtk::gsk::Transform::new().translate(&gtk::graphene::Point::new(x, y));

                child.allocate(child_width, child_height, baseline, Some(transform));
            }
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            if let Some(child) = self.obj().first_child() {
                self.obj().snapshot_child(&child, snapshot);
            }
        }
    }
}

glib::wrapper! {
    pub struct SquareTile(ObjectSubclass<square_tile::SquareTile>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl SquareTile {
    pub(crate) fn new(width: i32, height: i32, child: &impl IsA<gtk::Widget>) -> Self {
        let tile: Self = glib::Object::new();
        tile.imp().width.set(width.max(1));
        tile.imp().height.set(height.max(1));
        tile.imp().favorite_indicators_visible.set(true);
        child.as_ref().set_parent(&tile);
        tile
    }

    pub(crate) fn photo(&self) -> Option<PhotoObject> {
        self.imp().photo.borrow().as_ref().cloned()
    }

    fn set_tile_size(&self, width: i32, height: i32) {
        let width = width.max(1);
        let height = height.max(1);
        if self.imp().width.get() == width && self.imp().height.get() == height {
            return;
        }
        self.imp().width.set(width);
        self.imp().height.set(height);
        self.queue_resize();
    }

    /// Switch between cropping photos to the tile (Cover, the default) and
    /// letterboxing the whole photo (Contain) so portrait and landscape
    /// thumbnails both show the complete picture.
    fn set_content_fit(&self, fit: gtk::ContentFit) {
        let Some(frame) = self.first_child().and_downcast::<gtk::Overlay>() else {
            return;
        };
        if let Some(picture) = frame.child().and_downcast::<gtk::Picture>() {
            picture.set_content_fit(fit);
        }
    }

    /// Remember how to open the unavailable dialog so the offline badge can be
    /// created lazily instead of on every folder tile.
    fn set_unavailable_handler(&self, handler: Rc<dyn Fn(PhotoObject, gtk::Widget)>) {
        self.imp().unavailable.replace(Some(handler));
    }

    fn ensure_offline_badge(&self, frame: &gtk::Overlay) -> gtk::Button {
        if let Some(existing) = find_overlay_child(frame, "offline-badge") {
            if let Ok(button) = existing.downcast::<gtk::Button>() {
                return button;
            }
        }
        let badge = gtk::Button::with_label("!");
        badge.set_halign(gtk::Align::Start);
        badge.set_valign(gtk::Align::Start);
        badge.set_margin_top(8);
        badge.set_margin_start(8);
        badge.add_css_class("offline-badge");
        badge.set_tooltip_text(Some("Original photo unavailable"));
        badge.set_visible(false);
        frame.add_overlay(&badge);
        if let Some(handler) = self.imp().unavailable.borrow().clone() {
            let tile = self.downgrade();
            badge.connect_clicked(move |badge| {
                let Some(tile) = tile.upgrade() else {
                    return;
                };
                let Some(photo) = tile.imp().photo.borrow().as_ref().cloned() else {
                    return;
                };
                handler(photo, badge.clone().upcast());
            });
        }
        badge
    }

    fn set_favorite_indicator_visible(&self, visible: bool) {
        self.imp().favorite_indicators_visible.set(visible);
        self.refresh_favorite_indicator();
    }

    fn refresh_favorite_indicator(&self) {
        let Some(photo) = self.imp().photo.borrow().clone() else {
            return;
        };
        let Some(frame) = self.first_child().and_downcast::<gtk::Overlay>() else {
            return;
        };
        let visible = self.imp().favorite_indicators_visible.get() && photo.favorite();
        if visible {
            ensure_favorite_badge(&frame).set_visible(true);
        } else if let Some(child) = find_overlay_child(&frame, "favorite-badge") {
            child.set_visible(false);
        }
    }

    fn set_photo_deferred(&self, photo: &PhotoObject) {
        let same_photo = self
            .imp()
            .photo
            .borrow()
            .as_ref()
            .is_some_and(|current| current.id() == photo.id());
        if same_photo {
            return;
        }
        self.unload_visual();
        self.imp().photo.replace(Some(photo.clone()));
    }

    fn apply_presentation_paintable(
        &self,
        expected_key: &str,
        paintable: &gtk::gdk::Paintable,
    ) -> bool {
        let started = std::time::Instant::now();
        let Some(photo) = self.imp().photo.borrow().as_ref().cloned() else {
            return false;
        };
        if photo_presentation_key(&photo).as_deref() != Some(expected_key) {
            return false;
        }
        if self.imp().visual_loaded.get()
            && self.imp().applied_visual_key.borrow().as_deref() == Some(expected_key)
        {
            return true;
        }
        let Some(frame) = self.first_child().and_downcast::<gtk::Overlay>() else {
            return false;
        };
        let Some(picture) = frame.child().and_downcast::<gtk::Picture>() else {
            return false;
        };
        picture.set_paintable(Some(paintable));
        if picture.has_css_class("missing-thumbnail") {
            picture.remove_css_class("missing-thumbnail");
        }
        if let Some(placeholder) = picture.next_sibling().and_downcast::<gtk::Image>() {
            if placeholder.is_visible() {
                placeholder.set_visible(false);
            }
        }
        photo.set_thumbnail_available(true);
        self.imp().visual_loaded.set(true);
        *self.imp().applied_visual_key.borrow_mut() = Some(expected_key.to_owned());
        if std::env::var_os("PICASA_TRACE").is_some() { eprintln!("PIC_THUMBNAIL paintable_assign elapsed_us={}", started.elapsed().as_micros()); }
        true
    }

    fn mark_presentation_missing(&self, expected_key: &str) -> bool {
        let Some(photo) = self.imp().photo.borrow().as_ref().cloned() else {
            return false;
        };
        if photo_presentation_key(&photo).as_deref() != Some(expected_key) {
            return false;
        }
        let Some(frame) = self.first_child().and_downcast::<gtk::Overlay>() else {
            return false;
        };
        let Some(picture) = frame.child().and_downcast::<gtk::Picture>() else {
            return false;
        };
        picture.set_paintable(gtk::gdk::Paintable::NONE);
        if !picture.has_css_class("missing-thumbnail") {
            picture.add_css_class("missing-thumbnail");
        }
        if let Some(placeholder) = picture.next_sibling().and_downcast::<gtk::Image>() {
            if !placeholder.is_visible() {
                placeholder.set_visible(true);
            }
        }
        photo.set_thumbnail_available(false);
        // The presentation worker has already requested background regeneration.
        // Treat the placeholder as the current settled state so a frame pump does
        // not requeue the same missing cache file continuously. A targeted
        // ThumbnailCreated/priority completion invalidates this state.
        self.imp().visual_loaded.set(true);
        true
    }

    fn refresh_badges_from_model(&self) {
        let Some(photo) = self.imp().photo.borrow().as_ref().cloned() else {
            return;
        };
        let Some(frame) = self.first_child().and_downcast::<gtk::Overlay>() else {
            return;
        };

        let unavailable = !photo.original_available();
        if unavailable {
            let badge = self.ensure_offline_badge(&frame);
            if !badge.is_visible() {
                badge.set_visible(true);
            }
        } else if let Some(child) = find_overlay_child(&frame, "offline-badge") {
            if child.is_visible() {
                child.set_visible(false);
            }
        }

        let favorite = self.imp().favorite_indicators_visible.get() && photo.favorite();
        if favorite {
            ensure_favorite_badge(&frame).set_visible(true);
        } else if let Some(child) = find_overlay_child(&frame, "favorite-badge") {
            child.set_visible(false);
        }

        let edited = !crate::edit::EditRecipe::decode(&photo.edit_recipe()).is_default();
        if edited {
            ensure_edited_badge(&frame).set_visible(true);
        } else if let Some(child) = find_overlay_child(&frame, "edited-badge") {
            child.set_visible(false);
        }
    }

    fn queue_presentation_visual_async(&self, visible_priority: bool) -> bool {
        let Some(photo) = self.imp().photo.borrow().as_ref().cloned() else {
            return false;
        };
        let Some(request) = photo_presentation_request(&photo, visible_priority) else {
            return false;
        };

        if let Some(paintable) = folder_thumbnail_cache_get(&request.key) {
            self.apply_presentation_paintable(&request.key, &paintable);
            return true;
        }

        // During a direct scrollbar scrub GTK can recycle the same handful of
        // GridView cells hundreds of times per second. Keep the previous
        // paintable as a transient visual backstop instead of flashing the
        // empty template on every rebind. The correct thumbnail replaces it
        // atomically when the worker completion arrives. Once scrubbing
        // settles, refresh_visible_grid_tiles() clears any unresolved stale
        // image and restores the normal placeholder behavior.
        if !grid_scrub_active() {
            if let Some(frame) = self.first_child().and_downcast::<gtk::Overlay>() {
                if let Some(picture) = frame.child().and_downcast::<gtk::Picture>() {
                    picture.set_paintable(gtk::gdk::Paintable::NONE);
                    if !picture.has_css_class("missing-thumbnail") {
                        picture.add_css_class("missing-thumbnail");
                    }
                    if let Some(placeholder) = picture.next_sibling().and_downcast::<gtk::Image>() {
                        if !placeholder.is_visible() {
                            placeholder.set_visible(true);
                        }
                    }
                }
            }
        }

        crate::thumbnail_display::submit(request)
    }

    fn load_visual(&self) {
        if self.imp().visual_loaded.get() {
            return;
        }
        let Some(photo) = self.imp().photo.borrow().as_ref().cloned() else {
            return;
        };
        self.refresh_badges_from_model();
        let cache_hit = if let Some(key) = photo_presentation_key(&photo) {
            if let Some(paintable) = folder_thumbnail_cache_get(&key) {
                self.apply_presentation_paintable(&key, &paintable);
                true
            } else {
                false
            }
        } else {
            false
        };
        if !cache_hit {
            self.queue_presentation_visual_async(true);
        }
    }

    fn unload_visual(&self) {
        self.imp().visual_loaded.set(false);
        self.imp().applied_visual_key.borrow_mut().take();
        if !grid_scrub_active() {
            if let Some(frame) = self.first_child().and_downcast::<gtk::Overlay>() {
                if let Some(picture) = frame.child().and_downcast::<gtk::Picture>() {
                    picture.set_paintable(gtk::gdk::Paintable::NONE);
                }
            }
        }
    }

    fn bind_photo(&self, photo: &PhotoObject) {
        let started = std::time::Instant::now();
        self.set_photo_deferred(photo);
        self.load_visual();
        if std::env::var_os("PICASA_TRACE").is_some() { eprintln!("PIC_THUMBNAIL gtk_bind elapsed_us={} id={}", started.elapsed().as_micros(), photo.id()); }
    }

    /// Folder ListView bind must stay strictly presentation-only.
    ///
    /// GtkListView can recycle hundreds or thousands of rows during a large
    /// scrollbar jump. Do not stat files, probe originals, request thumbnail
    /// generation, or render RAW/edit transforms here. Those operations turn
    /// harmless widget recycling into synchronous main-thread work.
    fn bind_photo_folder_fast(&self, photo: &PhotoObject, photo_index: usize) {
        // Folder bind immediately paints the correct paintable, so replace the
        // photo in place instead of running the full unload path. The old path
        // cleared the paintable and toggled CSS on every recycled tile, which
        // invalidated GTK's style tree for the whole realized list on each
        // scroll frame (measured: gtk_css_node_validate_internal dominated).
        let same_photo = self
            .imp()
            .photo
            .borrow()
            .as_ref()
            .is_some_and(|current| current.id() == photo.id());
        if !same_photo {
            self.imp().photo.replace(Some(photo.clone()));
        }
        self.imp().photo_index.set(Some(photo_index));

        let Some(bound) = self.imp().photo.borrow().as_ref().cloned() else {
            return;
        };
        let Some(frame) = self.first_child().and_downcast::<gtk::Overlay>() else {
            return;
        };
        let Some(picture) = frame.child().and_downcast::<gtk::Picture>() else {
            return;
        };

        // RAM-only fast path. Never touch the filesystem from ListView bind,
        // but reuse a paintable that the settled-viewport loader has already
        // decoded. This keeps nearby thumbnails visible during wheel scrolling
        // without restoring the old per-bind I/O/RAW/edit work.
        let memory_hit = photo_presentation_key(&bound)
            .as_deref()
            .and_then(folder_thumbnail_cache_get);
        if let Some(paintable) = memory_hit.as_ref() {
            picture.set_paintable(Some(paintable));
            if picture.has_css_class("missing-thumbnail") {
                picture.remove_css_class("missing-thumbnail");
            }
        } else {
            // Do not blank a recycled Folder tile just because the destination
            // thumbnail is not in RAM yet. Keep the previous paintable as a
            // temporary visual backstop and immediately submit the *current*
            // photo to the async presentation queue. The completion drain
            // validates the presentation key before replacing the paintable, so
            // a tile that gets recycled again cannot receive the wrong image.
            //
            // This is intentionally different from the old Folder design,
            // which displayed an empty template here and waited for a later
            // viewport pump to rediscover the tile. During Page Up/Down and
            // scrollbar scrubbing that rediscovery can lag several frames.
            if picture.paintable().is_none() {
                if !picture.has_css_class("missing-thumbnail") {
                    picture.add_css_class("missing-thumbnail");
                }
            }
            if let Some(request) = photo_presentation_request(&bound, true) {
                crate::thumbnail_display::submit_latest_visible(request);
            }
        }

        let unavailable = !bound.original_available();
        if unavailable {
            let badge = self.ensure_offline_badge(&frame);
            if !badge.is_visible() {
                badge.set_visible(true);
            }
            if badge.tooltip_text().as_deref() != Some("Original photo unavailable") {
                badge.set_tooltip_text(Some("Original photo unavailable"));
            }
        } else if let Some(child) = find_overlay_child(&frame, "offline-badge") {
            if child.is_visible() {
                child.set_visible(false);
            }
        }
        let favorite = self.imp().favorite_indicators_visible.get() && bound.favorite();
        if favorite {
            ensure_favorite_badge(&frame).set_visible(true);
        } else if let Some(child) = find_overlay_child(&frame, "favorite-badge") {
            child.set_visible(false);
        }
        let edited = !crate::edit::EditRecipe::decode(&bound.edit_recipe()).is_default();
        if edited {
            let badge = ensure_edited_badge(&frame);
            badge.set_visible(true);
            if badge.tooltip_text().as_deref() != Some("Edited") {
                badge.set_tooltip_text(Some("Edited"));
            }
        } else if let Some(child) = find_overlay_child(&frame, "edited-badge") {
            child.set_visible(false);
        }
        if let Some(placeholder) = picture.next_sibling().and_downcast::<gtk::Image>() {
            // Only show the empty template when there is genuinely no paintable
            // to keep on screen. A recycled tile with an old paintable stays
            // visually populated until its correct async result arrives.
            let visible = memory_hit.is_none() && picture.paintable().is_none();
            if placeholder.is_visible() != visible {
                placeholder.set_visible(visible);
            }
        }
        self.imp().visual_loaded.set(memory_hit.is_some());
    }

    /// Queue a cached folder thumbnail without blocking the GTK thread.
    ///
    /// Fast Folder scrolling can recycle rows faster than the settled viewport
    /// loader runs. The bind path remains RAM-only; cache-file probing, JPEG
    /// reading, and decoding happen on a bounded worker set here. The result is
    /// applied only if this recycled tile still represents the same photo.
    fn queue_folder_cached_visual_async(&self, visible_priority: bool) -> bool {
        let Some(photo) = self.imp().photo.borrow().as_ref().cloned() else {
            return false;
        };
        let Some(request) = photo_presentation_request(&photo, visible_priority) else {
            return false;
        };

        if let Some(paintable) = folder_thumbnail_cache_get(&request.key) {
            self.apply_presentation_paintable(&request.key, &paintable);
            return true;
        }

        // Folder ListView rows are recycled aggressively when their model is
        // rebound. Keep any existing paintable as a temporary visual backstop
        // until the key-validated async result arrives. Missing/failed results
        // still clear the tile through mark_presentation_missing().
        crate::thumbnail_display::submit(request)
    }

    /// Load the final viewport thumbnail after Folder scrolling settles.
    ///
    /// Even after motion settles, thumbnail cache probing, decoding and visual
    /// transforms stay on the shared worker queue. The GTK thread only updates
    /// tooltip/badge state and submits a request when RAM has no paintable.
    fn load_folder_cached_visual(&self) {
        let Some(photo) = self.imp().photo.borrow().as_ref().cloned() else {
            return;
        };
        let Some(frame) = self.first_child().and_downcast::<gtk::Overlay>() else {
            return;
        };
        let Some(picture) = frame.child().and_downcast::<gtk::Picture>() else {
            return;
        };

        // Tooltip work stays out of the high-frequency bind path. Once motion
        // settles this is presentation-only and does not touch the original or
        // thumbnail file.
        let filename = photo.filename();
        if picture.tooltip_text().as_deref() != Some(filename.as_str()) {
            picture.set_tooltip_text(Some(&filename));
        }

        // Scrolling never probes originals. Offline state comes from the
        // folder/mount snapshot plus explicit background availability refreshes.
        if !self.imp().visual_loaded.get() {
            self.queue_folder_cached_visual_async(true);
        }
    }

    /// Reset a recycled Folder tile without blanking its current paintable.
    ///
    /// GtkListView can unbind/rebind the same tile pool during a Folder model
    /// splice. Clearing every image at unbind produces a wall of placeholders
    /// while async cache decodes catch up. The next bind immediately changes
    /// the photo identity and queues the correct presentation key; completion
    /// application validates that key before replacing this temporary backstop.
    fn clear_photo_folder_recycle(&self) {
        self.imp().visual_loaded.set(false);
        self.imp().applied_visual_key.borrow_mut().take();
        self.imp().photo.take();
        self.imp().photo_index.set(None);
        if let Some(frame) = self.first_child().and_downcast::<gtk::Overlay>() {
            if let Some(picture) = frame.child().and_downcast::<gtk::Picture>() {
                picture.set_tooltip_text(None);
                if let Some(placeholder) = picture.next_sibling().and_downcast::<gtk::Image>() {
                    let visible = picture.paintable().is_none();
                    if placeholder.is_visible() != visible {
                        placeholder.set_visible(visible);
                    }
                }
            }
            for class_name in ["manual-selected", "folder-photo-selected"] {
                frame.remove_css_class(class_name);
            }
            let mut child = frame.first_child();
            while let Some(current) = child {
                if let Some(image) = current.downcast_ref::<gtk::Image>() {
                    if image.has_css_class("favorite-badge")
                        || image.has_css_class("edited-badge")
                        || image.has_css_class("selection-badge")
                    {
                        image.set_visible(false);
                    }
                }
                if let Some(button) = current.downcast_ref::<gtk::Button>() {
                    if button.has_css_class("offline-badge") {
                        button.set_visible(false);
                    }
                }
                child = current.next_sibling();
            }
        }
    }

    fn clear_photo(&self) {
        self.unload_visual();
        self.imp().photo.take();
        self.imp().photo_index.set(None);
        if let Some(frame) = self.first_child().and_downcast::<gtk::Overlay>() {
            if let Some(picture) = frame.child().and_downcast::<gtk::Picture>() {
                // A fast Folder scrollbar scrub unbinds/rebinds rows faster
                // than thumbnail decodes can land. Blanking the paintable
                // here would leave the recycled tile with nothing to show
                // when bind_photo_folder_fast tries to keep the previous
                // image as a transient backstop. The settle refresh clears
                // any unresolved stale image once scrubbing stops; the
                // completion drain validates presentation keys before
                // applying, so no wrong image can persist on screen.
                if !grid_scrub_active() {
                    picture.set_paintable(gtk::gdk::Paintable::NONE);
                }
                picture.set_tooltip_text(None);
            }
            for class_name in ["manual-selected", "folder-photo-selected"] {
                frame.remove_css_class(class_name);
            }
            let mut child = frame.first_child();
            while let Some(current) = child {
                if let Some(image) = current.downcast_ref::<gtk::Image>() {
                    if image.has_css_class("favorite-badge")
                        || image.has_css_class("edited-badge")
                        || image.has_css_class("selection-badge")
                    {
                        image.set_visible(false);
                    }
                }
                if let Some(button) = current.downcast_ref::<gtk::Button>() {
                    if button.has_css_class("offline-badge") {
                        button.set_visible(false);
                    }
                }
                child = current.next_sibling();
            }
        }
    }

    fn set_manual_selected(&self, selected: bool) {
        let Some(frame) = self.first_child().and_downcast::<gtk::Overlay>() else {
            return;
        };
        // `clear_photo` hides the selection badge when a tile is recycled.
        // Opacity is the real on/off switch via CSS, so make the badge visible
        // again here or recycled tiles lose their indicator.
        if selected {
            let badge = ensure_selection_badge(&frame);
            if !badge.is_visible() {
                badge.set_visible(true);
            }
        }
        let has_class = frame.has_css_class("folder-photo-selected");
        if selected && !has_class {
            frame.add_css_class("folder-photo-selected");
        } else if !selected && has_class {
            frame.remove_css_class("folder-photo-selected");
        }
    }

    fn refresh_thumbnail(&self) {
        self.refresh_thumbnail_with_probe(true);
    }

    fn refresh_availability(&self) {
        let Some(photo) = self.imp().photo.borrow().clone() else {
            return;
        };
        // Read the value the async probe or the availability worker already
        // stored; never stat the original on the GTK thread here.
        let available = photo.original_available();

        if let Some(frame) = self.first_child().and_downcast::<gtk::Overlay>() {
            if !available {
                self.ensure_offline_badge(&frame).set_visible(true);
            } else if let Some(child) = find_overlay_child(&frame, "offline-badge") {
                child.set_visible(false);
            }
        }
    }

    fn refresh_thumbnail_with_probe(&self, _probe_thumbnail: bool) {
        let Some(photo) = self.imp().photo.borrow().clone() else {
            return;
        };
        if let Some(key) = photo_presentation_key(&photo) {
            folder_thumbnail_cache_remove(&key);
        }
        self.imp().visual_loaded.set(false);
        self.imp().applied_visual_key.borrow_mut().take();
        self.refresh_badges_from_model();
        self.queue_presentation_visual_async(true);
    }
}

fn find_overlay_child(frame: &gtk::Overlay, css_class: &str) -> Option<gtk::Widget> {
    let mut child = frame.first_child();
    while let Some(current) = child {
        if current.has_css_class(css_class) {
            return Some(current);
        }
        child = current.next_sibling();
    }
    None
}

/// Folder tiles are built without badges; almost no photo needs one, and
/// creating four extra widgets per tile was the dominant cost of rebuilding the
/// Folder rows (~250-520 ms per column change). These helpers create a badge the
/// first time it is actually needed.
fn ensure_favorite_badge(frame: &gtk::Overlay) -> gtk::Image {
    if let Some(existing) = find_overlay_child(frame, "favorite-badge") {
        if let Ok(image) = existing.downcast::<gtk::Image>() {
            return image;
        }
    }
    let badge = gtk::Image::from_icon_name("emote-love-symbolic");
    badge.set_pixel_size(18);
    badge.set_halign(gtk::Align::End);
    badge.set_valign(gtk::Align::End);
    badge.set_margin_bottom(8);
    badge.set_margin_end(8);
    badge.add_css_class("favorite-badge");
    badge.set_visible(false);
    frame.add_overlay(&badge);
    badge
}

fn ensure_edited_badge(frame: &gtk::Overlay) -> gtk::Image {
    if let Some(existing) = find_overlay_child(frame, "edited-badge") {
        if let Ok(image) = existing.downcast::<gtk::Image>() {
            return image;
        }
    }
    let badge = gtk::Image::from_icon_name("document-edit-symbolic");
    badge.set_pixel_size(18);
    badge.set_halign(gtk::Align::Start);
    badge.set_valign(gtk::Align::End);
    badge.set_margin_bottom(8);
    badge.set_margin_start(8);
    badge.add_css_class("edited-badge");
    badge.set_tooltip_text(Some("Edited"));
    badge.set_visible(false);
    frame.add_overlay(&badge);
    badge
}

fn ensure_selection_badge(frame: &gtk::Overlay) -> gtk::Image {
    if let Some(existing) = find_overlay_child(frame, "selection-badge") {
        if let Ok(image) = existing.downcast::<gtk::Image>() {
            return image;
        }
    }
    let badge = gtk::Image::from_icon_name("object-select-symbolic");
    badge.set_pixel_size(18);
    badge.set_halign(gtk::Align::End);
    badge.set_valign(gtk::Align::Start);
    badge.set_margin_top(8);
    badge.set_margin_end(8);
    badge.add_css_class("selection-badge");
    badge.set_visible(false);
    frame.add_overlay(&badge);
    badge
}

/// Nikon RAW thumbnails can contain a letterboxed embedded preview whose
/// pixel aspect ratio does not match the camera image dimensions. GTK cannot
/// crop bars that are already part of the cached pixels, so crop the small
/// cached image before handing it to the normal Cover-rendered Picture.
pub(crate) fn raw_cached_thumbnail(photo: &PhotoObject, path: &str) -> Option<gtk::gdk::Paintable> {
    let source_path = photo.path();
    if !crate::image_format::uses(&source_path, crate::image_format::DecoderKind::Raw) {
        return None;
    }

    let source_width = photo.width();
    let source_height = photo.height();
    if source_width <= 0 || source_height <= 0 {
        return None;
    }

    let rotation = photo.rotation().rem_euclid(360);
    let edit_recipe = photo.edit_recipe();
    let recipe = crate::edit::EditRecipe::decode(&edit_recipe);
    if let Some(paintable) = raw_thumbnail_cache_get(&source_path, rotation, &edit_recipe) {
        return Some(paintable);
    }

    let mut image = image::open(path).ok()?.to_rgba8();
    let image_width = image.width();
    let image_height = image.height();
    if image_width == 0 || image_height == 0 {
        return None;
    }

    // Cached thumbnails have already had EXIF orientation applied during
    // generation. Raw dimensions are reported in sensor orientation, so
    // orientations 5-8 require the target axes to be swapped as well.
    let orientation = crate::thumbnail::exif_orientation(&source_path);
    let (display_width, display_height) = if matches!(orientation, 5..=8) {
        (source_height, source_width)
    } else {
        (source_width, source_height)
    };
    let target_ratio = display_width as f64 / display_height as f64;
    let image_ratio = image_width as f64 / image_height as f64;
    if image_ratio > target_ratio {
        let crop_width =
            ((image_height as f64 * target_ratio).round() as u32).clamp(1, image_width);
        let left = (image_width - crop_width) / 2;
        image = image::imageops::crop_imm(&image, left, 0, crop_width, image_height).to_image();
    } else if image_ratio < target_ratio {
        let crop_height =
            ((image_width as f64 / target_ratio).round() as u32).clamp(1, image_height);
        let top = (image_height - crop_height) / 2;
        image = image::imageops::crop_imm(&image, 0, top, image_width, crop_height).to_image();
    }

    image = match rotation {
        90 => image::imageops::rotate90(&image),
        180 => image::imageops::rotate180(&image),
        270 => image::imageops::rotate270(&image),
        _ => image,
    };
    if !recipe.is_default() {
        image = crate::edit::render::apply_recipe(image, &recipe);
    }

    let width = image.width() as i32;
    let height = image.height() as i32;
    let bytes = glib::Bytes::from_owned(image.into_raw());
    let texture = gtk::gdk::MemoryTexture::new(
        width,
        height,
        gtk::gdk::MemoryFormat::R8g8b8a8,
        &bytes,
        width as usize * 4,
    );
    let paintable: gtk::gdk::Paintable = texture.upcast();
    raw_thumbnail_cache_insert(source_path, rotation, edit_recipe, paintable.clone());
    Some(paintable)
}

fn raw_thumbnail_cache_get(path: &str, rotation: i32, recipe: &str) -> Option<gtk::gdk::Paintable> {
    RAW_THUMBNAIL_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        let index = cache
            .iter()
            .position(|(cached_path, cached_rotation, cached_recipe, _)| {
                cached_path == path && *cached_rotation == rotation && cached_recipe == recipe
            })?;
        let entry = cache.remove(index)?;
        let paintable = entry.3.clone();
        cache.push_back(entry);
        Some(paintable)
    })
}

fn raw_thumbnail_cache_insert(
    path: String,
    rotation: i32,
    recipe: String,
    paintable: gtk::gdk::Paintable,
) {
    RAW_THUMBNAIL_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        cache.retain(|(cached_path, cached_rotation, cached_recipe, _)| {
            cached_path != &path || *cached_rotation != rotation || cached_recipe != &recipe
        });
        cache.push_back((path, rotation, recipe, paintable));
        while cache.len() > RAW_THUMBNAIL_CACHE_CAPACITY {
            cache.pop_front();
        }
    });
}
