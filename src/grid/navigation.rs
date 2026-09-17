impl Gallery {
    /// Photo that must stay at the top across a reshape. While a previous
    /// reframe is still settling, keep its photo: the viewport is mid-flight
    /// and no longer describes the user's position. Otherwise capture the
    /// current photo (or a zoom's saved anchor) and mark the reframe pending.
    fn take_reframe_anchor(&self) -> Option<i64> {
        if self.folder_pending_reframe.get() {
            return self.folder_reframe_photo.get();
        }
        let captured = self.zoom_anchor.take().or_else(|| {
            self.photo_for_scroll_position(self.last_scroll_y.get())
                .map(|photo| photo.id())
        });
        if let Some(photo_id) = captured {
            self.folder_reframe_photo.set(Some(photo_id));
            self.folder_pending_reframe.set(true);
        }
        captured
    }

    /// Keep the Folder viewport on `photo_id` after the rows were reshaped by a
    /// zoom/column change. Realizes the target row, then aligns its tile to the
    /// top edge using the tile's computed bounds (independent of the variable
    /// row heights). Waits for allocation and bounds the settling corrections.
    fn scroll_folder_to_photo(&self, photo_id: i64) {
        self.folder_anchor_photo.set(Some(photo_id));
        let Some(row) = self.folder_row_index_for_photo(photo_id) else {
            return;
        };
        let tile_height = self.tile_height.get();
        let exact_offset = (0..row)
            .filter_map(|position| {
                self.folder_store
                    .item(position)
                    .and_downcast::<FolderRowObject>()
            })
            .map(|row| f64::from(folder_model_row_height(row.data().kind, tile_height)))
            .sum::<f64>();
        let root = self.folder_root.clone();
        let generation = self.folder_scroll_generation.clone();
        let request = generation.get().wrapping_add(1);
        generation.set(request);
        let model_generation = self.replace_generation.clone();
        let model_request = model_generation.get();
        let mode = self.group_mode.clone();
        let pending = self.folder_pending_reframe.clone();
        let reframe_photo = self.folder_reframe_photo.clone();
        let trace = std::env::var_os("PICASA_TRACE").is_some();
        glib::idle_add_local_once(move || {
            if generation.get() != request
                || model_generation.get() != model_request
                || mode.get() != GroupMode::Folder
            {
                return;
            }
            // Position the viewport from the exact model geometry instead of
            // asking GtkListView to estimate a far, unrealized row. Header rows
            // are compact while photo rows follow zoom, so summing the model
            // row kinds above keeps the anchor deterministic.
            if let Some(adjustment) = root.vadjustment() {
                let upper = (adjustment.upper() - adjustment.page_size()).max(adjustment.lower());
                adjustment.set_value(exact_offset.clamp(adjustment.lower(), upper));
                if trace {
                    eprintln!(
                        "UI PERF folder_zoom_anchor_exact row={row} target={:.0} after={:.0}",
                        exact_offset,
                        adjustment.value()
                    );
                }

                // Exact virtual-row positioning succeeded. Do not run the old
                // bounds-based settle loop afterwards: GTK can report transient
                // tile bounds for a few frames after a rebuild, which causes the
                // small visible "rebound" even though the exact anchor is already
                // correct.
                pending.set(false);
                reframe_photo.set(None);
                return;
            }
            // Exact positioning was unavailable. Fall back to the older
            // allocation/bounds correction path below.
            // An idle after another idle does NOT guarantee a GTK allocation.
            // Wait for a frame to lay out the rebuilt/rebound rows before using
            // their bounds; otherwise the old coordinates move us to another row.
            let frames = Cell::new(0);
            let stable_frames = Cell::new(0);
            root.add_tick_callback(move |root_for_align, _| {
                if generation.get() != request
                    || model_generation.get() != model_request
                    || mode.get() != GroupMode::Folder
                {
                    return glib::ControlFlow::Break;
                }
                frames.set(frames.get() + 1);
                if frames.get() < 2 {
                    return glib::ControlFlow::Continue;
                }
                let mut tiles = Vec::new();
                collect_tiles(root_for_align.upcast_ref(), &mut tiles);
                let tile = tiles.into_iter().find(|tile| {
                    tile.imp()
                        .photo
                        .borrow()
                        .as_ref()
                        .is_some_and(|photo| photo.id() == photo_id)
                });
                let Some(tile) = tile else {
                    pending.set(false);
                    return glib::ControlFlow::Break;
                };
                let Some(bounds) = tile.compute_bounds(root_for_align) else {
                    pending.set(false);
                    return glib::ControlFlow::Break;
                };
                let Some(adjustment) = root_for_align.vadjustment() else {
                    pending.set(false);
                    return glib::ControlFlow::Break;
                };
                // Align the row, not the tile's six-pixel top margin. GTK may
                // refine its estimated row heights for another frame after a
                // splice; allow a bounded correction until the anchor settles.
                let target = adjustment.value() + bounds.y() as f64 - tile.margin_top() as f64;
                let upper = (adjustment.upper() - adjustment.page_size()).max(adjustment.lower());
                let target = target.clamp(adjustment.lower(), upper);
                if (target - adjustment.value()).abs() <= 1.0 {
                    stable_frames.set(stable_frames.get() + 1);
                } else {
                    stable_frames.set(0);
                    adjustment.set_value(target);
                }
                if trace {
                    eprintln!(
                        "UI PERF folder_zoom_anchor row={row} bounds_y={:.0} after={:.0}",
                        bounds.y(),
                        adjustment.value()
                    );
                }
                if stable_frames.get() >= 2 || frames.get() >= 8 {
                    // Selecting a fresh anchor is allowed again once this
                    // reshape has been positioned.
                    pending.set(false);
                    reframe_photo.set(None);
                    glib::ControlFlow::Break
                } else {
                    glib::ControlFlow::Continue
                }
            });
        });
    }
}
fn schedule_scroll_restore(
    adjustment: Option<gtk::Adjustment>,
    scroll_y: f64,
    replace_generation: Rc<Cell<u64>>,
    generation: u64,
) {
    glib::idle_add_local_once(move || {
        if replace_generation.get() != generation {
            return;
        }
        if let Some(adjustment) = adjustment {
            let upper = (adjustment.upper() - adjustment.page_size()).max(adjustment.lower());
            adjustment.set_value(scroll_y.clamp(adjustment.lower(), upper));
        }
    });
}

fn folder_line_height(tile_height: i32) -> i32 {
    tile_height.max(1) + 12
}

fn box_tiles(line: &gtk::Box) -> Vec<SquareTile> {
    let mut tiles = Vec::new();
    let mut child = line.first_child();
    while let Some(current) = child {
        if let Ok(tile) = current.clone().downcast::<SquareTile>() {
            tiles.push(tile);
        }
        child = current.next_sibling();
    }
    tiles
}

fn update_folder_realized_rows(widget: &gtk::Widget, tile_width: i32, tile_height: i32) {
    if widget.widget_name().as_str() == "picasa-folder-photo-line" {
        if let Some(line) = widget.downcast_ref::<gtk::Box>() {
            for tile in box_tiles(line) {
                tile.set_tile_size(tile_width, tile_height);
            }
            if line.is_visible() {
                if let Some(row_root) = line.parent().and_downcast::<gtk::Box>() {
                    row_root.set_height_request(folder_model_row_height(
                        FolderRowKind::Photos,
                        tile_height,
                    ));
                }
            }
        }
    }
    let mut child = widget.first_child();
    while let Some(current) = child {
        update_folder_realized_rows(&current, tile_width, tile_height);
        child = current.next_sibling();
    }
}

fn collect_tiles(widget: &gtk::Widget, tiles: &mut Vec<SquareTile>) {
    if let Some(tile) = widget.downcast_ref::<SquareTile>() {
        tiles.push(tile.clone());
    }
    let mut child = widget.first_child();
    while let Some(current) = child {
        collect_tiles(&current, tiles);
        child = current.next_sibling();
    }
}
