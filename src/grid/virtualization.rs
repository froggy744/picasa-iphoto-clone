/// Lightweight folder metadata used only to keep Folder sections in the same
/// hierarchy order as the sidebar.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FolderCatalogEntry {
    folder_id: i64,
    parent_id: Option<i64>,
    photo_count: usize,
}

impl Gallery {
    pub fn zoom_in(self: &Rc<Self>) {
        let base = self
            .pending_zoom_width
            .get()
            .unwrap_or_else(|| self.tile_width.get());
        self.request_zoom(next_zoom_level(base));
    }

    pub fn zoom_out(self: &Rc<Self>) {
        let base = self
            .pending_zoom_width
            .get()
            .unwrap_or_else(|| self.tile_width.get());
        self.request_zoom(prev_zoom_level(base));
    }

    /// Reset to the default view level: ~4 thumbnails per row for the current
    /// surface, matching the startup default for users who never picked a
    /// size. Falls back to the ladder level nearest the legacy fixed default
    /// before the first real layout is known.
    pub fn reset_zoom(self: &Rc<Self>) {
        let width = if self.group_mode.get() == GroupMode::Folder {
            self.folder_root.width()
        } else {
            self.root.width()
        };
        let target = if width > 0 {
            zoom_level_for_four_columns(width)
        } else {
            nearest_zoom_level(DEFAULT_TILE_WIDTH)
        };
        self.request_zoom(target);
    }

    /// Adopt the ~4-thumbnails-per-row default on the first real layout.
    /// Only set when no user-chosen thumbnail size is stored; the size stays
    /// session-local until the user zooms manually, so resizing the window
    /// later keeps re-targeting the default instead of freezing an old width.
    pub fn enable_auto_default_zoom(&self) {
        self.auto_default_zoom.set(true);
    }

    /// Letterbox whole photos (Contain) instead of cropping them to the tile
    /// (Cover), so portrait thumbnails show portrait, not a centre strip.
    /// Applies to realized tiles immediately and to future tiles via the
    /// factory.
    pub fn set_fit_whole_photo(self: &Rc<Self>, fit: bool) {
        if self.fit_whole_photo.get() == fit {
            return;
        }
        self.fit_whole_photo.set(fit);
        self.v2.set_fit_whole_photo(fit);
        self.v2_folder.set_fit_whole_photo(fit);
        let content_fit = if fit {
            gtk::ContentFit::Contain
        } else {
            gtk::ContentFit::Cover
        };
        let mut tiles = Vec::new();
        collect_tiles(self.root.upcast_ref(), &mut tiles);
        collect_tiles(self.folder_root.upcast_ref(), &mut tiles);
        crate::window::debug_log(&format!(
            "GALLERY: set_fit_whole_photo({fit}) applying to {} realized tiles",
            tiles.len()
        ));
        for tile in tiles {
            tile.set_content_fit(content_fit);
        }
    }

    /// Record a zoom request. Isolated clicks apply immediately; a rapid
    /// Ctrl+wheel spin coalesces its extra notches into one trailing reflow so
    /// crossing several column boundaries does not rebuild the Folder rows per
    /// notch.
    pub fn request_zoom(self: &Rc<Self>, width: i32) {
        // Snap every request onto the canonical ladder so +/-, Ctrl+wheel and
        // Reset converge on the same sizes no matter where they start.
        let width = nearest_zoom_level(width).clamp(MIN_TILE_WIDTH, MAX_TILE_WIDTH);
        // An explicit zoom always wins over the pending startup default.
        self.auto_default_zoom.set(false);
        let base = self
            .pending_zoom_width
            .get()
            .unwrap_or_else(|| self.tile_width.get());
        if width == base {
            return;
        }
        // A burst is already in progress if a trailing source exists.
        let leading = self.zoom_reflow_source.borrow().is_none();
        self.pending_zoom_width.set(Some(width));
        if leading {
            let this = self.clone();
            glib::idle_add_local_once(move || {
                if let Some(width) = this.pending_zoom_width.take() {
                    this.apply_zoom(width);
                }
            });
        }
        if let Some(source) = self.zoom_reflow_source.borrow_mut().take() {
            source.remove();
        }
        let this = self.clone();
        let source = glib::timeout_add_local(std::time::Duration::from_millis(150), move || {
            this.zoom_reflow_source.borrow_mut().take();
            if let Some(width) = this.pending_zoom_width.take() {
                this.apply_zoom(width);
            }
            glib::ControlFlow::Break
        });
        self.zoom_reflow_source.replace(Some(source));
    }

    /// Zoom is driven by width. Height scales by the same factor, preserving
    /// the custom width/height shape configured above. `persist` is false for
    /// the startup default so adopting it does not turn it into a preference.
    fn apply_zoom(&self, width: i32) {
        self.apply_tile_size(width, true);
    }

    fn apply_tile_size(&self, width: i32, persist: bool) {
        let old_width = self.tile_width.get().max(1);
        let old_height = self.tile_height.get().max(1);
        let width = width.clamp(MIN_TILE_WIDTH, MAX_TILE_WIDTH);
        if width == old_width {
            return;
        }

        // Capture the visible photo before the tile resize disturbs the layout.
        if self.group_mode.get() == GroupMode::Folder {
            self.zoom_anchor.set(
                self.photo_for_scroll_position(self.last_scroll_y.get())
                    .map(|photo| photo.id()),
            );
        } else {
            self.zoom_anchor.set(None);
        }

        let scale = width as f64 / old_width as f64;
        let height = ((old_height as f64) * scale).round().max(1.0) as i32;


        self.tile_width.set(width);
        self.tile_height.set(height);
        self.v2.set_tile_size(width, height);
        self.v2_folder.set_tile_size(width, height);
        if persist {
            (self.on_zoom_changed)(width);
        }


        let mut tiles = Vec::new();
        collect_tiles(self.root.upcast_ref(), &mut tiles);
        collect_tiles(self.folder_root.upcast_ref(), &mut tiles);

        for tile in tiles {
            tile.set_tile_size(width, height);
        }


        let root_width = if self.group_mode.get() == GroupMode::Folder {
            self.folder_root.width()
        } else {
            self.root.width()
        };
        if root_width > 100 {
            self.update_layout(root_width, true);
        } else {
            self.update_group_header_for_scroll(self.last_scroll_y.get());
        }
        self.zoom_anchor.set(None);

    }

    /// After Folder scrolling settles, load only cached thumbnails for tiles
    /// close to the actual viewport. GtkListView keeps a much larger recycled
    /// widget pool than the visible rows, so loading every bound tile causes
    /// thousands of unnecessary thumbnail operations during scrollbar jumps.


    /// Replace stale GridView visible requests with the thumbnails GTK is
    /// painting in the current frame. This is the non-Folder equivalent of the
    /// Folder motion pump and prevents fast scrollbar movement from filling the
    /// worker queue with viewports the user has already passed.
    pub fn queue_visible_grid_cached_tiles_async(&self, budget: usize) -> usize {
        if budget == 0 || self.group_mode.get() == GroupMode::Folder || self.root.height() <= 0 {
            return 0;
        }

        let viewport = self.root.height() as f32;
        let mut tiles = Vec::new();
        collect_tiles(self.root.upcast_ref(), &mut tiles);
        let mut candidates: Vec<(f32, SquareTile)> = Vec::new();

        for tile in tiles {
            if !tile.is_mapped() || tile.height() <= 0 || tile.imp().visual_loaded.get() {
                continue;
            }
            let Some(bounds) = tile.compute_bounds(&self.root) else {
                continue;
            };
            if bounds.y() + bounds.height() < 0.0 || bounds.y() > viewport {
                continue;
            }
            let center = bounds.y() + bounds.height() * 0.5;
            candidates.push(((center - viewport * 0.5).abs(), tile));
        }

        candidates.sort_by(|left, right| left.0.total_cmp(&right.0));
        let mut requests = Vec::new();
        for (_, tile) in candidates.into_iter().take(budget) {
            let Some(photo) = tile.imp().photo.borrow().as_ref().cloned() else {
                continue;
            };
            let Some(request) = photo_presentation_request(&photo, true) else {
                continue;
            };
            if let Some(paintable) = folder_thumbnail_cache_get(&request.key) {
                tile.apply_presentation_paintable(&request.key, &paintable);
            } else {
                requests.push(request);
            }
        }

        crate::thumbnail_display::replace_visible_requests(requests)
    }

    /// Apply already-decoded RAM paintables to the tiles visible in the
    /// GridView without touching the decode queue.
    ///
    /// During a direct scrollbar scrub the model-derived sampler owns the
    /// queue, so widget-derived requests must not replace it. Display must not
    /// be tied to that restriction: a recycled tile binds once, usually before
    /// its thumbnail decode completes, and the completion drain misses tiles
    /// that are recycled again before the decode arrives. Re-checking the
    /// visible tiles against the RAM cache every frame paints exactly those
    /// finished thumbnails while the scrub is still moving.
    pub fn apply_visible_grid_cached_paintables(&self) -> usize {
        if self.group_mode.get() == GroupMode::Folder || self.root.height() <= 0 {
            return 0;
        }

        let viewport = self.root.height() as f32;
        let mut tiles = Vec::new();
        collect_tiles(self.root.upcast_ref(), &mut tiles);
        let mut applied = 0usize;
        for tile in tiles {
            if !tile.is_mapped() || tile.height() <= 0 || tile.imp().visual_loaded.get() {
                continue;
            }
            let Some(bounds) = tile.compute_bounds(&self.root) else {
                continue;
            };
            if bounds.y() + bounds.height() < 0.0 || bounds.y() > viewport {
                continue;
            }
            let Some(photo) = tile.imp().photo.borrow().as_ref().cloned() else {
                continue;
            };
            let Some(key) = photo_presentation_key(&photo) else {
                continue;
            };
            if let Some(paintable) = folder_thumbnail_cache_get(&key) {
                if tile.apply_presentation_paintable(&key, &paintable) {
                    applied += 1;
                }
            }
        }
        applied
    }

    /// Queue the photo-model range that a large GridView scrollbar jump is
    /// moving toward, without waiting for GTK to realize/rebind those tiles.
    ///
    /// Scrollbar scrubbing can move by thousands of rows in one adjustment
    /// update. During that interval `collect_tiles()` still describes the old
    /// viewport for one or more frames, so widget-based visible detection is
    /// too late. This method derives the target rows directly from scroll
    /// position and queues only that viewport as visible-priority work.
    pub fn queue_grid_scroll_target_cached_tiles_async(
        &self,
        scroll_y: f64,
        viewport_height: f64,
        budget: usize,
    ) -> usize {
        if budget == 0 || self.group_mode.get() == GroupMode::Folder {
            return 0;
        }

        const ITEM_PADDING: f64 = 6.0;
        let columns = self.current_columns.get().max(1) as usize;
        let row_pitch = self.tile_height.get().max(1) as f64 + ITEM_PADDING * 2.0;
        let first = self.index_for_scroll_position(scroll_y);
        let visible_rows =
            ((viewport_height.max(row_pitch) / row_pitch).ceil() as usize).saturating_add(2);
        let wanted = visible_rows.saturating_mul(columns).min(budget);

        let photos = self.current_photos.borrow();
        if photos.is_empty() || first >= photos.len() {
            return 0;
        }
        let end = first.saturating_add(wanted).min(photos.len());
        let center = first + (end - first) / 2;
        let mut indexes = Vec::with_capacity(end - first);
        // Centre-out ordering makes the viewport useful as quickly as possible
        // after a teleport while still warming every tile on screen.
        for distance in 0..=(end - first) {
            let right = center.saturating_add(distance);
            if right < end {
                indexes.push(right);
            }
            if distance > 0 {
                let left = center.saturating_sub(distance);
                if left >= first && left < center {
                    indexes.push(left);
                }
            }
            if indexes.len() >= end - first {
                break;
            }
        }

        let mut requests = Vec::with_capacity(indexes.len());
        for index in indexes {
            let Some(photo) = photos.get(index) else {
                continue;
            };
            let Some(request) = photo_presentation_request(photo, true) else {
                continue;
            };
            if folder_thumbnail_cache_get(&request.key).is_none() {
                requests.push(request);
            }
        }
        drop(photos);

        crate::thumbnail_display::replace_visible_requests(requests)
    }

    fn visible_grid_photo_index_span(&self) -> Option<(usize, usize)> {
        if self.group_mode.get() == GroupMode::Folder || self.root.height() <= 0 {
            return None;
        }
        let viewport = self.root.height() as f32;
        let mut tiles = Vec::new();
        collect_tiles(self.root.upcast_ref(), &mut tiles);
        let mut first = usize::MAX;
        let mut last = 0usize;
        let mut found = false;
        for tile in tiles {
            if !tile.is_mapped() || tile.height() <= 0 {
                continue;
            }
            let Some(index) = tile.imp().photo_index.get() else {
                continue;
            };
            let Some(bounds) = tile.compute_bounds(&self.root) else {
                continue;
            };
            if bounds.y() + bounds.height() < 0.0 || bounds.y() > viewport {
                continue;
            }
            first = first.min(index);
            last = last.max(index);
            found = true;
        }
        found.then_some((first, last))
    }

    /// Reconcile the actually visible GridView cells after direct scrollbar
    /// scrubbing stops. During the scrub cells intentionally retain their last
    /// paintable to avoid a wall of empty templates; this pass either applies
    /// the correct RAM-cached thumbnail or restores the normal placeholder and
    /// queues the correct visible thumbnail.
    pub fn refresh_visible_grid_tiles(&self) -> usize {
        if self.group_mode.get() == GroupMode::Folder || self.root.height() <= 0 {
            return 0;
        }

        let viewport = self.root.height() as f32;
        let mut tiles = Vec::new();
        collect_tiles(self.root.upcast_ref(), &mut tiles);
        let mut refreshed = 0usize;
        for tile in tiles {
            if !tile.is_mapped() || tile.height() <= 0 {
                continue;
            }
            let Some(bounds) = tile.compute_bounds(&self.root) else {
                continue;
            };
            if bounds.y() + bounds.height() < 0.0 || bounds.y() > viewport {
                continue;
            }
            tile.unload_visual();
            tile.load_visual();
            refreshed += 1;
        }
        refreshed
    }

    /// Warm several screens of GridView thumbnails from the photo model so
    /// Library/Favourites/Albums/Search can scroll into already-decoded RAM
    /// paintables just like Folder mode.
    pub fn prefetch_grid_cached_tiles(&self, budget: usize, direction: f64) -> usize {
        if budget == 0 || self.group_mode.get() == GroupMode::Folder {
            return 0;
        }
        let Some((first_visible, last_visible)) = self.visible_grid_photo_index_span() else {
            return 0;
        };
        let photos = self.current_photos.borrow();
        if photos.is_empty() {
            return 0;
        }

        let visible_count = last_visible
            .saturating_sub(first_visible)
            .saturating_add(1)
            .max(self.current_columns.get() as usize);
        let ahead = visible_count.saturating_mul(5);
        let behind = visible_count;
        let mut indexes = Vec::with_capacity((ahead + behind).min(photos.len()));

        if direction < 0.0 {
            let ahead_start = first_visible.saturating_sub(ahead);
            indexes.extend((ahead_start..first_visible).rev());
            let behind_end = (last_visible + 1 + behind).min(photos.len());
            indexes.extend((last_visible + 1)..behind_end);
        } else {
            let ahead_end = (last_visible + 1 + ahead).min(photos.len());
            indexes.extend((last_visible + 1)..ahead_end);
            let behind_start = first_visible.saturating_sub(behind);
            indexes.extend((behind_start..first_visible).rev());
        }

        let mut queued = 0usize;
        for index in indexes {
            if queued >= budget {
                break;
            }
            let Some(photo) = photos.get(index) else {
                continue;
            };
            if queue_photo_presentation_async(photo, false) {
                queued += 1;
            }
        }
        queued
    }

    /// Queue cached thumbnails for the tiles that are actually visible now.
    ///
    /// This is the high-priority fast-scroll path. It may use the reserved
    /// visible-thumbnail worker capacity even when background prefetch already
    /// occupies its smaller quota, so a scrollbar jump cannot be blocked by
    /// thumbnails for rows the user has already passed.


    /// Folder counterpart of apply_visible_grid_cached_paintables: paint RAM
    /// thumbnails onto currently visible rows during a direct scrub without
    /// replacing the model-derived decode target.


    /// Queue the Folder destination directly from virtual-row geometry.
    ///
    /// Unlike `queue_visible_folder_cached_tiles_async`, this does not depend on
    /// GtkListView having already recycled and allocated the destination row
    /// widgets. That makes it suitable for Page Up/Down and direct scrollbar
    /// jumps, where the adjustment can move many rows before GTK has realized
    /// the new viewport.


    /// Warm a RAM thumbnail buffer around the Folder viewport.
    ///
    /// `direction` is the current scroll direction (negative = up, positive =
    /// down, 0 = unknown). Tiles ahead of the viewport in that direction are
    /// prioritised so they are already in RAM when they scroll into view, which
    /// is what stops the "blank then pop in" flicker during fast scrolling.
    ///
    /// Cache-file I/O and JPEG decode are queued on bounded worker threads;
    /// only texture creation/application returns to GTK. The hot ListView bind
    /// path stays strictly RAM-only.


    /// Warm decoded presentation thumbnails from the photo model, not just from
    /// GTK's realized row pool. That gives a scrollbar jump several screens of
    /// cache runway even before GtkListView has created/rebound those widgets.


    pub fn thumbnail_display_work_pending(&self) -> bool {
        crate::thumbnail_display::pending_count() > 0
    }

    /// Drain worker completions once per frame. Disk I/O and image transforms
    /// have already happened on background workers; GTK only creates textures,
    /// updates the RAM LRU, and repaints currently realized matching tiles.
    pub fn drain_thumbnail_display_completions(&self) -> usize {
        let completions = crate::thumbnail_display::take_completions();
        if completions.is_empty() {
            return 0;
        }

        let mut loaded = HashMap::<String, gtk::gdk::Paintable>::new();
        let mut missing = HashSet::<String>::new();
        for completion in completions {
            match completion.outcome {
                crate::thumbnail_display::DisplayOutcome::Loaded {
                    width,
                    height,
                    pixels,
                } => {
                    let bytes = glib::Bytes::from_owned(pixels);
                    let texture = gtk::gdk::MemoryTexture::new(
                        width,
                        height,
                        gtk::gdk::MemoryFormat::R8g8b8a8,
                        &bytes,
                        width as usize * 4,
                    );
                    let paintable: gtk::gdk::Paintable = texture.upcast();
                    folder_thumbnail_cache_insert(completion.key.clone(), paintable.clone());
                    loaded.insert(completion.key, paintable);
                }
                crate::thumbnail_display::DisplayOutcome::Missing
                | crate::thumbnail_display::DisplayOutcome::Failed => {
                    missing.insert(completion.key);
                }
            }
        }

        let mut tiles = Vec::new();
        collect_tiles(self.root.upcast_ref(), &mut tiles);
        collect_tiles(self.folder_root.upcast_ref(), &mut tiles);
        for tile in tiles {
            let Some(photo) = tile.imp().photo.borrow().as_ref().cloned() else {
                continue;
            };
            let Some(key) = photo_presentation_key(&photo) else {
                continue;
            };
            if let Some(paintable) = loaded.get(&key) {
                tile.apply_presentation_paintable(&key, paintable);
                if let Some(frame) = tile.first_child().and_downcast::<gtk::Overlay>() {
                    if let Some(picture) = frame.child().and_downcast::<gtk::Picture>() {
                        if picture.tooltip_text().is_none() && tile.is_mapped() {
                            picture.set_tooltip_text(Some(&photo.filename()));
                        }
                    }
                }
            } else if missing.contains(&key) {
                tile.mark_presentation_missing(&key);
            }
        }
        loaded.len() + missing.len()
    }

    pub fn refresh_thumbnails(&self) {
        let mut tiles = Vec::new();
        collect_tiles(self.root.upcast_ref(), &mut tiles);
        collect_tiles(self.folder_root.upcast_ref(), &mut tiles);
        for tile in tiles {
            tile.refresh_thumbnail();
        }

    }

    pub fn set_favorite_indicators_visible(&self, visible: bool) {
        let mut tiles = Vec::new();
        collect_tiles(self.root.upcast_ref(), &mut tiles);
        collect_tiles(self.folder_root.upcast_ref(), &mut tiles);
        for tile in tiles {
            tile.set_favorite_indicator_visible(visible);
        }
    }

    pub fn refresh_favorite_indicators(&self) {
        let mut tiles = Vec::new();
        collect_tiles(self.root.upcast_ref(), &mut tiles);
        collect_tiles(self.folder_root.upcast_ref(), &mut tiles);
        for tile in tiles {
            tile.refresh_favorite_indicator();
        }
    }

    pub fn update_favorites(&self, ids: &[i64], favorite: bool) {
        let ids = ids.iter().copied().collect::<HashSet<_>>();
        for photo in self.current_photos.borrow().iter() {
            if ids.contains(&photo.id()) {
                photo.set_favorite(favorite);
            }
        }
        self.refresh_favorite_indicators();
    }

    /// Remove visible photos from the current grid without rebuilding the model.
    /// This is used when a photo stops belonging to the active virtual view
    /// (Favourites or an Album). Preserve the viewport and move selection to
    /// the nearest remaining thumbnail instead of jumping back to item 0.
    pub fn remove_photos(&self, ids: &[i64]) {
        if ids.is_empty() {
            return;
        }

        let scroll_y = self.scroll_position();
        let ids = ids.iter().copied().collect::<HashSet<_>>();
        let positions = self
            .current_photos
            .borrow()
            .iter()
            .enumerate()
            .filter_map(|(position, photo)| ids.contains(&photo.id()).then_some(position))
            .collect::<Vec<_>>();
        if positions.is_empty() {
            return;
        }

        let next_position = positions.iter().copied().min().unwrap_or(0);

        self.current_photos
            .borrow_mut()
            .retain(|photo| !ids.contains(&photo.id()));
        for position in positions.into_iter().rev() {
            self.store.remove(position as u32);
        }

        if self.collage_selection_mode.get() {
            self.restore_collage_selection();
        } else {
            self.selection.unselect_all();
            let len = self.store.n_items() as usize;
            if len == 0 {
                (self.selected)(None);
            } else {
                self.selection
                    .select_item(next_position.min(len - 1) as u32, true);
            }
        }

        if self.group_mode.get() == GroupMode::Folder {
            let snapshot = self.current_photos.borrow().clone();
            self.v2_folder.replace_objects(&snapshot);
        } else if self.group_mode.get() != GroupMode::None {
            self.rebuild_group_ranges();
            self.update_group_header_for_scroll(scroll_y);
        }

        // Invalidate any pending progressive replacement before restoring the
        // old adjustment after the active list widget has processed removals.
        let generation = self.replace_generation.get().wrapping_add(1);
        self.replace_generation.set(generation);
        let folder_mode = self.group_mode.get() == GroupMode::Folder;
        let adjustment = if folder_mode {
            self.folder_root.vadjustment()
        } else {
            self.root.vadjustment()
        };
        schedule_scroll_restore(
            adjustment,
            scroll_y,
            self.replace_generation.clone(),
            generation,
        );
        if folder_mode {
            self.folder_root.grab_focus();
        } else {
            self.root.grab_focus();
        }
    }

    pub fn refresh_thumbnails_for_paths(&self, paths: &[std::path::PathBuf]) {
        if paths.is_empty() {
            return;
        }
        let paths = paths
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect::<HashSet<_>>();
        if std::env::var_os("PICASA_TRACE").is_some() { eprintln!("PIC_NAV thumbnail_refresh_paths count={}", paths.len()); }
        let mut tiles = Vec::new();
        collect_tiles(self.root.upcast_ref(), &mut tiles);
        collect_tiles(self.folder_root.upcast_ref(), &mut tiles);
        for tile in tiles {
            let matches = tile
                .imp()
                .photo
                .borrow()
                .as_ref()
                .is_some_and(|photo| paths.contains(&photo.path()));
            if matches {
                tile.refresh_thumbnail();
            }
        }
    }

    pub fn refresh_availability(&self) {
        // State-only repaint. Actual source probing belongs to
        // window::refresh_availability_ui, which performs it on a worker thread
        // and feeds the result back through apply_availability().
        let mut tiles = Vec::new();
        collect_tiles(self.root.upcast_ref(), &mut tiles);
        collect_tiles(self.folder_root.upcast_ref(), &mut tiles);
        for tile in tiles {
            tile.refresh_availability();
        }
    }

    pub fn apply_folder_availability(
        &self,
        updates: &[(i64, bool)],
        is_current: impl Fn() -> bool + 'static,
    ) {
        if updates.is_empty() {
            return;
        }
        // Folder state is the availability unit. Filter the loaded photo
        // objects once, then update only the objects belonging to folders
        // whose mounted state actually changed.
        let updates = updates
            .iter()
            .copied()
            .collect::<std::collections::HashMap<_, _>>();
        let mut photos = self
            .current_photos
            .borrow()
            .iter()
            .filter_map(|photo| {
                updates
                    .get(&photo.folder_id())
                    .is_some_and(|available| photo.original_available() != *available)
                    .then(|| photo.clone())
            })
            .collect::<Vec<_>>();
        if photos.is_empty() {
            return;
        }
        let generation = self.replace_generation.get();
        let current_generation = self.replace_generation.clone();
        let is_current = Rc::new(is_current);
        let root = self.root.downgrade();
        let folder_root = self.folder_root.downgrade();
        let mut offset = 0;
        glib::timeout_add_local(std::time::Duration::from_millis(16), move || {
            // Navigation/replacement must not paint an obsolete gallery.
            if current_generation.get() != generation || !is_current() {
                return glib::ControlFlow::Break;
            }
            let end = (offset + 128).min(photos.len());
            for photo in &photos[offset..end] {
                if let Some(available) = updates.get(&photo.folder_id()) {
                    if photo.original_available() != *available {
                        photo.set_original_available(*available);
                    }
                    photo.set_original_checked_at(Some(Instant::now()));
                }
            }

            offset = end;
            if offset < photos.len() {
                return glib::ControlFlow::Continue;
            }
            let mut tiles = Vec::new();
            if let Some(root) = root.upgrade() {
                collect_tiles(root.upcast_ref(), &mut tiles);
            }
            if let Some(root) = folder_root.upgrade() {
                collect_tiles(root.upcast_ref(), &mut tiles);
            }
            for tile in tiles {
                tile.refresh_availability();
            }
            glib::ControlFlow::Break
        });
    }

    pub fn replace(&self, photos: &[Photo]) {
        if self.group_mode.get() == GroupMode::Folder {
            let started = std::time::Instant::now();
            let objects = self.v2.objects_for(photos);
            self.v2_folder.replace_objects(&objects);

            // Keep the mature RC action/navigation model pointing at the same
            // persistent PhotoObjects without rebuilding another 22k GObjects.
            self.current_photos.replace(objects.clone());
            self.store.splice(0, self.store.n_items(), &objects);
            self.stream_building.set(false);

            if std::env::var_os("PICASA_TRACE").is_some() {
                eprintln!(
                    "PIC_V2_FOLDER gallery_replace photos={} elapsed_ms={}",
                    photos.len(),
                    started.elapsed().as_millis()
                );
            }
            return;
        }

        self.v2.replace(photos);
        if std::env::var_os("PICASA_TRACE").is_some() { eprintln!("PIC_NAV gallery_replace photos={}", photos.len()); }
        let generation = self.replace_generation.get().wrapping_add(1);
        self.replace_generation.set(generation);
        // Assume a build is in progress until each completion path clears it.
        // Callers such as folder navigation wait on this so they do not give
        // up while the virtualized Folder rows are still being constructed.
        self.stream_building.set(true);
        let unchanged = {
            let current = self.current_photos.borrow();
            current.len() == photos.len()
                && current
                    .iter()
                    .zip(photos)
                    .all(|(object, photo)| {
                        object.id() == photo.id
                            && object.history_caption() == photo.history_caption
                            && object.edited_at() == photo.edited_at
                            && object.edit_recipe() == photo.edit_recipe
                            && object.path() == photo.path
                            && object.mtime() == photo.mtime.unwrap_or_default()
                            && object.size_bytes() == photo.size_bytes.unwrap_or_default()
                    })
        };
        if unchanged {
            self.stream_building.set(false);
            return;
        }

        // Same photo set in a different order (for example Folder <-> All
        // Photos): reorder the existing PhotoObjects instead of reconstructing
        // tens of thousands of them. Measured 658-1363 ms to rebuild all 66k.
        let same_set = {
            let current = self.current_photos.borrow();
            current.len() == photos.len() && !current.is_empty() && {
                let ids = current
                    .iter()
                    .map(|object| object.id())
                    .collect::<std::collections::HashSet<_>>();
                photos.iter().all(|photo| ids.contains(&photo.id))
            }
        };
        if same_set {
            let current = self.current_photos.borrow().clone();
            let mut by_id = current
                .into_iter()
                .map(|object| (object.id(), object))
                .collect::<std::collections::HashMap<_, _>>();
            let reordered = photos
                .iter()
                .filter_map(|photo| {
                    let object = by_id.remove(&photo.id)?;
                    object.set_from_photo(photo);
                    Some(object)
                })
                .collect::<Vec<_>>();
            if !self.collage_selection_mode.get() {
                (self.selected)(None);
            }
            self.current_photos.replace(reordered.clone());
            self.store.splice(0, self.store.n_items(), &reordered);
            if self.collage_selection_mode.get() {
                self.restore_collage_selection();
            } else if reordered.is_empty() {
                self.selection.unselect_all();
            } else {
                self.selection.select_item(0, true);
            }
            if self.group_mode.get() != GroupMode::None {
                self.rebuild_group_ranges();
                self.update_group_header_for_scroll(self.last_scroll_y.get());
            }

            self.stream_building.set(false);
            return;
        }

        // Constructing tens of thousands of GObjects synchronously blocks
        // GTK for several seconds. Keep the existing model semantics for
        // normal refreshes, but let the main loop make progress between small
        // batches for library-sized replacements.
        const PROGRESSIVE_REPLACE_THRESHOLD: usize = 1_000;
        if photos.len() > PROGRESSIVE_REPLACE_THRESHOLD {

            self.replace_progressive(photos.to_vec(), generation);
            return;
        }

        if !self.collage_selection_mode.get() {
            (self.selected)(None);
        }
        let objects: Vec<PhotoObject> = photos.iter().map(PhotoObject::from_photo).collect();
        self.current_photos.replace(objects.clone());
        self.store.splice(0, self.store.n_items(), &objects);
        if self.collage_selection_mode.get() {
            self.restore_collage_selection();
        } else if objects.is_empty() {
            self.selection.unselect_all();
        } else {
            self.selection.select_item(0, true);
        }
        if self.group_mode.get() != GroupMode::None {
            self.rebuild_group_ranges();
            self.update_group_header_for_scroll(self.last_scroll_y.get());
        }

        self.stream_building.set(false);
    }

    fn replace_progressive(
        &self,
        photos: Vec<Photo>,
        generation: u64,
    ) {
        // Larger batches finish the model build in far fewer main-loop hops.
        // Each hop is scheduled at idle priority, so with 500-photo batches a
        // 66k stream needed 133 hops and could take >20 s of wall time even
        // though the actual construction work was under a second.
        const BATCH_SIZE: usize = 2_000;

        let photos = Rc::new(photos);
        let offset = Rc::new(Cell::new(0usize));
        let initialized = Rc::new(Cell::new(false));
        let store = self.store.clone();
        let selected = self.selected.clone();
        let current_photos = self.current_photos.clone();
        let selection = self.selection.clone();
        let collage_selection_mode = self.collage_selection_mode.clone();
        let collage_selected_ids = self.collage_selected_ids.clone();
        let group_mode = self.group_mode.clone();
        let group_date = self.group_date.clone();
        let group_ranges = self.group_ranges.clone();
        let group_header = self.group_header.clone();
        let group_title = self.group_title.clone();
        let group_count = self.group_count.clone();
        let last_scroll_y = self.last_scroll_y.clone();
        let current_columns = self.current_columns.clone();
        let tile_height = self.tile_height.clone();
        let replace_generation = self.replace_generation.clone();
        let stream_building = self.stream_building.clone();

        glib::idle_add_local(move || {
            if replace_generation.get() != generation {
                return glib::ControlFlow::Break;
            }

            let start = offset.get();
            let end = (start + BATCH_SIZE).min(photos.len());
            let objects: Vec<PhotoObject> = photos[start..end]
                .iter()
                .map(PhotoObject::from_photo)
                .collect();
            offset.set(end);

            if !initialized.replace(true) {
                if !collage_selection_mode.get() {
                    selected(None);
                }
                current_photos.replace(objects.clone());
                store.splice(0, store.n_items(), &objects);
            } else {
                current_photos.borrow_mut().extend(objects.iter().cloned());
                store.splice(store.n_items(), 0, &objects);
            }


            if end >= photos.len() {
                rebuild_group_ranges_for(&current_photos, &group_mode, &group_date, &group_ranges);
                if group_mode.get() == GroupMode::Folder {
                    group_header.set_visible(false);
                    group_title.set_text("");
                    group_count.set_text("");
                } else {
                    update_group_header_for_index_for(
                        &group_mode,
                        &group_ranges,
                        &group_header,
                        &group_title,
                        &group_count,
                        (((last_scroll_y.get() - 20.0).max(0.0)
                            / (tile_height.get().max(1) as f64 + 12.0))
                            .floor() as usize)
                            * current_columns.get().max(1) as usize,
                    );
                }
            }
            if end >= photos.len() && collage_selection_mode.get() {
                selection.unselect_all();
                let wanted = collage_selected_ids.borrow().clone();
                for position in 0..store.n_items() {
                    if store
                        .item(position)
                        .and_downcast::<PhotoObject>()
                        .is_some_and(|photo| wanted.contains(&photo.id()))
                    {
                        selection.select_item(position, false);
                    }
                }
            } else if end >= photos.len() && !objects.is_empty() {
                selection.select_item(0, true);
            }
            if end < photos.len() {
                glib::ControlFlow::Continue
            } else {

                // Folder rows (and therefore folder navigation targets) only
                // exist once every batch has been applied.
                stream_building.set(false);
                glib::ControlFlow::Break
            }
        });
    }

    /// Cold-start loader for Folder mode. The startup idle loop arrives in
    /// small batches so GTK can render quickly, but Folder section membership
    /// must not be rebuilt for every partial batch. Accumulate the shared
    /// PhotoObjects here and build Folder V2 exactly once on the final batch.
    pub fn load_startup_batch(&self, photos: &[Photo], first: bool, final_batch: bool) {
        if self.group_mode.get() != GroupMode::Folder {
            if first {
                self.replace(photos);
            } else {
                self.append_photos(photos);
            }
            return;
        }

        if photos.is_empty() {
            if final_batch {
                let snapshot = self.current_photos.borrow().clone();
                self.v2_folder.replace_objects(&snapshot);
                self.stream_building.set(false);
            }
            return;
        }

        self.stream_building.set(!final_batch);
        let objects = self.v2.objects_for(photos);

        if first {
            self.current_photos.replace(objects.clone());
            self.store.splice(0, self.store.n_items(), &objects);
        } else {
            self.current_photos.borrow_mut().extend(objects.iter().cloned());
            self.store.splice(self.store.n_items(), 0, &objects);
        }

        if final_batch {
            let snapshot = self.current_photos.borrow().clone();
            let started = std::time::Instant::now();
            self.v2_folder.replace_objects(&snapshot);
            self.stream_building.set(false);
            if std::env::var_os("PICASA_TRACE").is_some() {
                eprintln!(
                    "PIC_V2_FOLDER startup_finalize photos={} sections={} elapsed_ms={}",
                    snapshot.len(),
                    self.v2_folder.section_count(),
                    started.elapsed().as_millis()
                );
            }
        } else if std::env::var_os("PICASA_TRACE").is_some() {
            eprintln!(
                "PIC_V2_FOLDER startup_accumulate added={} total={}",
                photos.len(),
                self.current_photos.borrow().len()
            );
        }
    }

    pub fn append_photos(&self, photos: &[Photo]) {
        if photos.is_empty() {
            return;
        }
        let objects: Vec<PhotoObject> = photos.iter().map(PhotoObject::from_photo).collect();
        self.current_photos
            .borrow_mut()
            .extend(objects.iter().cloned());
        self.store.splice(self.store.n_items(), 0, &objects);
        if self.group_mode.get() == GroupMode::Folder {
            let snapshot = self.current_photos.borrow().clone();
            self.v2_folder.replace_objects(&snapshot);
        } else if self.group_mode.get() != GroupMode::None {
            self.rebuild_group_ranges();
            self.update_group_header_for_scroll(self.last_scroll_y.get());
        }

    }

    /// Stop a progressive replacement that is no longer the active view.
    ///
    /// Search replaces the model shortly afterward. Cancelling the old idle
    /// batches keeps a large library refresh from continuing to mutate the
    /// GTK model while the user is typing.
    pub fn cancel_progressive_build(&self) {
        self.replace_generation
            .set(self.replace_generation.get().wrapping_add(1));
        self.stream_building.set(false);
    }

}

