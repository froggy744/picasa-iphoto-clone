const SECTIONED_HEADER_HEIGHT: f64 = 70.0;
const SECTIONED_SIDE_MARGIN: f64 = 20.0;
const SECTIONED_OVERSCAN_PX: f64 = 320.0;
const SECTIONED_TILE_POOL_CAP: usize = 180;
const SECTIONED_HEADER_POOL_CAP: usize = 12;

#[derive(Clone, Copy, Debug)]
struct SectionedFolderGeometry {
    header_y: f64,
    first_photo_y: f64,
    end_y: f64,
}

#[derive(Clone)]
struct SectionedFolderTile {
    tile: SquareTile,
    index: Rc<Cell<Option<u32>>>,
}

struct SectionedFolderView {
    root: gtk::Fixed,
    spacer: gtk::Box,
    current_photos: Rc<RefCell<Vec<PhotoObject>>>,
    group_ranges: Rc<RefCell<Vec<GroupRange>>>,
    selection: gtk::MultiSelection,
    current_columns: Rc<Cell<u32>>,
    tile_width: Rc<Cell<i32>>,
    tile_height: Rc<Cell<i32>>,
    fit_whole_photo: Rc<Cell<bool>>,
    show_file_names: Rc<Cell<bool>>,
    activate: Rc<dyn Fn(Vec<PhotoObject>, usize, Option<(gtk::Widget, gtk::gdk::Paintable)>)>,
    context_menu: Rc<dyn Fn(PhotoObject, gtk::Widget, f64, f64)>,
    unavailable: Rc<dyn Fn(PhotoObject, gtk::Widget)>,
    collage_mode: Rc<Cell<bool>>,
    collage_ids: Rc<RefCell<HashSet<i64>>>,
    rubberband: gtk::DrawingArea,
    scroll: RefCell<Option<gtk::ScrolledWindow>>,
    geometry: RefCell<Vec<SectionedFolderGeometry>>,
    geometry_width: Cell<i32>,
    geometry_columns: Cell<u32>,
    geometry_tile_height: Cell<i32>,
    total_height: Cell<f64>,
    live_tiles: RefCell<HashMap<u32, SectionedFolderTile>>,
    tile_pool: RefCell<VecDeque<SectionedFolderTile>>,
    live_headers: RefCell<HashMap<usize, gtk::Label>>,
    header_pool: RefCell<VecDeque<gtk::Label>>,
    selection_anchor: Rc<Cell<Option<u32>>>,
}

impl SectionedFolderView {
    fn new(
        current_photos: Rc<RefCell<Vec<PhotoObject>>>,
        group_ranges: Rc<RefCell<Vec<GroupRange>>>,
        selection: gtk::MultiSelection,
        current_columns: Rc<Cell<u32>>,
        tile_width: Rc<Cell<i32>>,
        tile_height: Rc<Cell<i32>>,
        fit_whole_photo: Rc<Cell<bool>>,
        show_file_names: Rc<Cell<bool>>,
        activate: Rc<dyn Fn(Vec<PhotoObject>, usize, Option<(gtk::Widget, gtk::gdk::Paintable)>)>,
        context_menu: Rc<dyn Fn(PhotoObject, gtk::Widget, f64, f64)>,
        unavailable: Rc<dyn Fn(PhotoObject, gtk::Widget)>,
        collage_mode: Rc<Cell<bool>>,
        collage_ids: Rc<RefCell<HashSet<i64>>>,
    ) -> Rc<Self> {
        let root = gtk::Fixed::new();
        root.set_hexpand(true);
        root.set_vexpand(false);
        root.set_focusable(true);
        root.add_css_class("sectioned-folder-view");

        let spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
        spacer.set_can_target(false);
        root.put(&spacer, 0.0, 0.0);

        let rubberband = gtk::DrawingArea::new();
        rubberband.set_can_target(false);
        rubberband.set_visible(false);
        rubberband.set_draw_func(|_, context, width, height| {
            context.set_source_rgba(0.30, 0.62, 0.86, 0.18);
            context.rectangle(0.0, 0.0, width as f64, height as f64);
            let _ = context.fill_preserve();
            context.set_source_rgba(0.47, 0.73, 0.91, 0.95);
            context.set_line_width(1.0);
            let _ = context.stroke();
        });
        root.put(&rubberband, 0.0, 0.0);

        let view = Rc::new(Self {
            root,
            spacer,
            current_photos,
            group_ranges,
            selection,
            current_columns,
            tile_width,
            tile_height,
            fit_whole_photo,
            show_file_names,
            activate,
            context_menu,
            unavailable,
            collage_mode,
            collage_ids,
            rubberband,
            scroll: RefCell::new(None),
            geometry: RefCell::new(Vec::new()),
            geometry_width: Cell::new(0),
            geometry_columns: Cell::new(0),
            geometry_tile_height: Cell::new(0),
            total_height: Cell::new(1.0),
            live_tiles: RefCell::new(HashMap::new()),
            tile_pool: RefCell::new(VecDeque::new()),
            live_headers: RefCell::new(HashMap::new()),
            header_pool: RefCell::new(VecDeque::new()),
            selection_anchor: Rc::new(Cell::new(None)),
        });

        let keyboard = gtk::EventControllerKey::new();
        keyboard.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(&view);
        keyboard.connect_key_pressed(move |_, key, _, modifiers| {
            let Some(view) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            let control = modifiers.contains(gtk::gdk::ModifierType::CONTROL_MASK);
            if control && matches!(key, gtk::gdk::Key::a | gtk::gdk::Key::A) {
                view.selection.select_all();
                return glib::Propagation::Stop;
            }
            if control || modifiers.contains(gtk::gdk::ModifierType::ALT_MASK) {
                return glib::Propagation::Proceed;
            }

            let selected = selected_positions(&view.selection);
            let current = selected.first().copied().unwrap_or(0);
            if matches!(key, gtk::gdk::Key::Return | gtk::gdk::Key::KP_Enter) {
                let photos = view.current_photos.borrow().clone();
                if (current as usize) < photos.len() {
                    (view.activate)(photos, current as usize, None);
                    return glib::Propagation::Stop;
                }
                return glib::Propagation::Proceed;
            }

            let count = view.selection.n_items();
            if count == 0 {
                return glib::Propagation::Proceed;
            }
            let columns = view.current_columns.get().max(1);
            let next = match key {
                gtk::gdk::Key::Left => current.checked_sub(1),
                gtk::gdk::Key::Right => {
                    let candidate = current.saturating_add(1);
                    (candidate < count).then_some(candidate)
                }
                gtk::gdk::Key::Up => current.checked_sub(columns),
                gtk::gdk::Key::Down => {
                    let candidate = current.saturating_add(columns);
                    if candidate < count {
                        Some(candidate)
                    } else if current + 1 < count {
                        Some(count - 1)
                    } else {
                        None
                    }
                }
                _ => return glib::Propagation::Proceed,
            };
            let Some(next) = next else {
                return glib::Propagation::Stop;
            };
            view.selection.select_item(next, true);
            view.selection_anchor.set(Some(next));
            view.scroll_to_index(next, false);
            if let Some(photo) = view.current_photos.borrow().get(next as usize) {
                view.focus_photo(photo.id());
            }
            glib::Propagation::Stop
        });
        view.root.add_controller(keyboard);

        let drag = gtk::GestureDrag::new();
        drag.set_button(1);
        drag.set_propagation_phase(gtk::PropagationPhase::Capture);
        let drag_start = Rc::new(Cell::new(None::<(f64, f64)>));
        let drag_base = Rc::new(RefCell::new(HashSet::<u32>::new()));

        {
            let weak = Rc::downgrade(&view);
            let drag_start = drag_start.clone();
            let drag_base = drag_base.clone();
            drag.connect_drag_begin(move |gesture, x, y| {
                let Some(view) = weak.upgrade() else {
                    return;
                };
                if view.collage_mode.get() {
                    gesture.set_state(gtk::EventSequenceState::Denied);
                    return;
                }
                drag_start.set(Some((x, y)));
                // Reinsert the band so it snapshots above recycled tiles/headers.
                view.root.remove(&view.rubberband);
                view.root.put(&view.rubberband, x, y);
                let control = gesture
                    .current_event_state()
                    .contains(gtk::gdk::ModifierType::CONTROL_MASK);
                let mut base = drag_base.borrow_mut();
                base.clear();
                if control {
                    base.extend(selected_positions(&view.selection));
                }
                view.rubberband.set_visible(false);
                view.root.set_cursor_from_name(Some("crosshair"));
            });
        }

        {
            let weak = Rc::downgrade(&view);
            let drag_start = drag_start.clone();
            let drag_base = drag_base.clone();
            drag.connect_drag_update(move |gesture, dx, dy| {
                if dx.hypot(dy) < DRAG_CLAIM_THRESHOLD {
                    return;
                }
                let Some(view) = weak.upgrade() else {
                    return;
                };
                let Some((sx, sy)) = drag_start.get() else {
                    return;
                };
                gesture.set_state(gtk::EventSequenceState::Claimed);
                let ex = sx + dx;
                let ey = sy + dy;
                let left = sx.min(ex);
                let top = sy.min(ey);
                let right = sx.max(ex);
                let bottom = sy.max(ey);

                view.rubberband
                    .set_size_request((right - left).ceil() as i32, (bottom - top).ceil() as i32);
                view.root.move_(&view.rubberband, left, top);
                view.rubberband.set_visible(true);
                view.rubberband.queue_draw();

                let mut selected = drag_base.borrow().clone();
                for (index, entry) in view.live_tiles.borrow().iter() {
                    let Some(bounds) = entry.tile.compute_bounds(&view.root) else {
                        continue;
                    };
                    let tile_left = f64::from(bounds.x());
                    let tile_top = f64::from(bounds.y());
                    let tile_right = tile_left + f64::from(bounds.width());
                    let tile_bottom = tile_top + f64::from(bounds.height());
                    if tile_right >= left
                        && tile_left <= right
                        && tile_bottom >= top
                        && tile_top <= bottom
                    {
                        selected.insert(*index);
                    }
                }

                view.selection.unselect_all();
                let mut selected = selected.into_iter().collect::<Vec<_>>();
                selected.sort_unstable();
                for position in selected {
                    view.selection.select_item(position, false);
                }
            });
        }

        {
            let weak = Rc::downgrade(&view);
            let drag_start = drag_start.clone();
            drag.connect_drag_end(move |_, _, _| {
                drag_start.set(None);
                if let Some(view) = weak.upgrade() {
                    view.rubberband.set_visible(false);
                    view.root.set_cursor(None::<&gtk::gdk::Cursor>);
                }
            });
        }
        view.root.add_controller(drag);
        view
    }

    fn root(&self) -> &gtk::Fixed {
        &self.root
    }

    fn attach_scroll(self: &Rc<Self>, scrolled: &gtk::ScrolledWindow) {
        self.scroll.replace(Some(scrolled.clone()));

        let this = self.clone();
        scrolled
            .vadjustment()
            .connect_value_changed(move |_| this.refresh());

        let this = self.clone();
        let scrolled_for_tick = scrolled.clone();
        let last_width = Rc::new(Cell::new(0_i32));
        let last_width_for_tick = last_width.clone();
        scrolled.add_tick_callback(move |_, _| {
            let width = scrolled_for_tick.width();
            if width > 0 && width != last_width_for_tick.get() {
                last_width_for_tick.set(width);
                // Width-only motion with the same column count does not change
                // vertical section geometry. Refresh only to stretch headers;
                // update_layout invalidates geometry when columns actually change.
                this.refresh();
            }
            glib::ControlFlow::Continue
        });

        self.refresh();
    }

    fn invalidate_geometry(&self) {
        self.geometry_width.set(0);
        self.geometry_columns.set(0);
        self.geometry_tile_height.set(0);
    }

    fn geometry_for_current_layout(&self, width: i32) {
        let columns = self.current_columns.get().max(1);
        let tile_height = self.tile_height.get();
        let range_count = self.group_ranges.borrow().len();

        // Width by itself does not affect section Y positions. Only the number
        // of columns, tile height, or section membership changes vertical
        // geometry. Remember the latest width for diagnostics/header sizing,
        // but avoid rebuilding every frame while the sidebar/window animates.
        if self.geometry_columns.get() == columns
            && self.geometry_tile_height.get() == tile_height
            && self.geometry.borrow().len() == range_count
        {
            self.geometry_width.set(width);
            return;
        }

        let row_height = f64::from(folder_line_height(tile_height));
        let ranges = self.group_ranges.borrow();
        let mut y = 0.0;
        let mut geometry = Vec::with_capacity(ranges.len());

        for range in ranges.iter() {
            let count = range.end.saturating_sub(range.start);
            let rows = count.div_ceil(columns as usize);
            let header_y = y;
            let first_photo_y = header_y + SECTIONED_HEADER_HEIGHT;
            let end_y = first_photo_y + rows as f64 * row_height;
            geometry.push(SectionedFolderGeometry {
                header_y,
                first_photo_y,
                end_y,
            });
            y = end_y;
        }

        self.geometry.replace(geometry);
        self.geometry_width.set(width);
        self.geometry_columns.set(columns);
        self.geometry_tile_height.set(tile_height);
        self.total_height.set(y.max(1.0));
    }

    fn make_tile(self: &Rc<Self>) -> SectionedFolderTile {
        let tile = make_folder_tile(
            self.tile_width.get(),
            self.tile_height.get(),
            &self.unavailable,
        );
        tile.set_filename_visible(self.show_file_names.get());
        tile.set_content_fit(if self.fit_whole_photo.get() {
            gtk::ContentFit::Contain
        } else {
            gtk::ContentFit::Cover
        });

        let index = Rc::new(Cell::new(None::<u32>));

        let click = gtk::GestureClick::new();
        click.set_button(1);
        let index_for_click = index.clone();
        let selection = self.selection.clone();
        let selection_anchor = self.selection_anchor.clone();
        let photos = self.current_photos.clone();
        let activate = self.activate.clone();
        let tile_for_click = tile.clone();
        let self_for_click_collage_mode = self.collage_mode.clone();
        let self_for_click_collage_ids = self.collage_ids.clone();
        click.connect_pressed(move |gesture, presses, _, _| {
            let Some(position) = index_for_click.get() else {
                return;
            };
            if self_for_click_collage_mode.get() {
                let Some(photo) = photos.borrow().get(position as usize).cloned() else {
                    return;
                };
                let mut ids = self_for_click_collage_ids.borrow_mut();
                if ids.remove(&photo.id()) {
                    selection.unselect_item(position);
                } else {
                    ids.insert(photo.id());
                    selection.select_item(position, false);
                }
                selection_anchor.set(Some(position));
                return;
            }
            let state = gesture.current_event_state();
            let control = state.contains(gtk::gdk::ModifierType::CONTROL_MASK);
            let shift = state.contains(gtk::gdk::ModifierType::SHIFT_MASK);

            if shift {
                let selected = selected_positions(&selection);
                let next = folder_selection_after_click(
                    selection.n_items(),
                    &selected,
                    selection_anchor.get(),
                    position,
                    false,
                    true,
                );
                selection.unselect_all();
                for item in next {
                    selection.select_item(item, false);
                }
            } else if control {
                if selection.is_selected(position) {
                    selection.unselect_item(position);
                } else {
                    selection.select_item(position, false);
                }
                selection_anchor.set(Some(position));
            } else {
                selection.select_item(position, true);
                selection_anchor.set(Some(position));
            }

            if presses == 2 {
                let photos = photos.borrow().clone();
                let index = position as usize;
                let source = tile_for_click
                    .transition_paintable()
                    .map(|paintable| (tile_for_click.clone().upcast::<gtk::Widget>(), paintable));
                activate(photos, index, source);
            }
        });
        tile.add_controller(click);

        let right_click = gtk::GestureClick::new();
        right_click.set_button(3);
        right_click.set_propagation_phase(gtk::PropagationPhase::Capture);
        let index_for_context = index.clone();
        let selection_for_context = self.selection.clone();
        let photos_for_context = self.current_photos.clone();
        let context_menu = self.context_menu.clone();
        let tile_for_context = tile.clone();
        right_click.connect_pressed(move |gesture, _, x, y| {
            let Some(position) = index_for_context.get() else {
                return;
            };
            let Some(photo) = photos_for_context.borrow().get(position as usize).cloned() else {
                return;
            };
            if !selection_for_context.is_selected(position) {
                selection_for_context.select_item(position, true);
            }
            gesture.set_state(gtk::EventSequenceState::Claimed);
            context_menu(
                photo,
                tile_for_context.clone().upcast::<gtk::Widget>(),
                x,
                y,
            );
        });
        tile.add_controller(right_click);

        SectionedFolderTile { tile, index }
    }

    fn refresh(self: &Rc<Self>) {
        let Some(scrolled) = self.scroll.borrow().as_ref().cloned() else {
            return;
        };
        let width = scrolled.width().max(1);
        self.geometry_for_current_layout(width);
        self.spacer
            .set_size_request(width, self.total_height.get().ceil() as i32);

        let adjustment = scrolled.vadjustment();
        let top = (adjustment.value() - SECTIONED_OVERSCAN_PX).max(0.0);
        let bottom = adjustment.value() + adjustment.page_size() + SECTIONED_OVERSCAN_PX;
        let columns = self.current_columns.get().max(1);
        let row_height = f64::from(folder_line_height(self.tile_height.get()));
        let ranges = self.group_ranges.borrow().clone();
        let geometry = self.geometry.borrow().clone();

        let mut wanted_headers = Vec::<usize>::new();
        let mut wanted_tiles = Vec::<(u32, usize, u32, u32)>::new();

        for (section_index, (range, geom)) in ranges.iter().zip(geometry.iter()).enumerate() {
            if geom.end_y < top || geom.header_y > bottom {
                continue;
            }
            wanted_headers.push(section_index);
            let count = range.end.saturating_sub(range.start) as u32;
            if count == 0 {
                continue;
            }
            let start_row = if top <= geom.first_photo_y {
                0
            } else {
                ((top - geom.first_photo_y) / row_height).floor().max(0.0) as u32
            };
            let end_row = (((bottom - geom.first_photo_y) / row_height)
                .ceil()
                .max(0.0) as u32)
                .min(count.div_ceil(columns));

            for row in start_row..end_row {
                let row_start = range.start as u32 + row * columns;
                for col in 0..columns {
                    let index = row_start + col;
                    if index >= range.end as u32 {
                        break;
                    }
                    wanted_tiles.push((index, section_index, row, col));
                }
            }
        }

        let wanted_ids = wanted_tiles
            .iter()
            .map(|item| item.0)
            .collect::<HashSet<_>>();
        let stale = self
            .live_tiles
            .borrow()
            .keys()
            .copied()
            .filter(|index| !wanted_ids.contains(index))
            .collect::<Vec<_>>();
        for index in stale {
            if let Some(tile) = self.live_tiles.borrow_mut().remove(&index) {
                self.root.remove(&tile.tile);
                tile.index.set(None);
                let mut pool = self.tile_pool.borrow_mut();
                if pool.len() < SECTIONED_TILE_POOL_CAP {
                    pool.push_back(tile);
                }
            }
        }

        let photos = self.current_photos.borrow();
        for (index, section_index, row, col) in wanted_tiles {
            let existing = {
                let live = self.live_tiles.borrow();
                live.get(&index).cloned()
            };
            let tile = if let Some(tile) = existing {
                tile
            } else {
                let tile = self
                    .tile_pool
                    .borrow_mut()
                    .pop_front()
                    .unwrap_or_else(|| self.make_tile());
                let Some(photo) = photos.get(index as usize) else {
                    continue;
                };
                tile.index.set(Some(index));
                tile.tile
                    .set_tile_size(self.tile_width.get(), self.tile_height.get());
                tile.tile.set_filename_visible(self.show_file_names.get());
                tile.tile.set_content_fit(if self.fit_whole_photo.get() {
                    gtk::ContentFit::Contain
                } else {
                    gtk::ContentFit::Cover
                });
                tile.tile.bind_photo_folder_fast(photo, index as usize);
                tile.tile.set_manual_selected(self.selection.is_selected(index));
                self.root.put(&tile.tile, 0.0, 0.0);
                self.live_tiles.borrow_mut().insert(index, tile.clone());
                tile
            };

            tile.tile
                .set_tile_size(self.tile_width.get(), self.tile_height.get());
            tile.tile.set_manual_selected(self.selection.is_selected(index));

            let x = SECTIONED_SIDE_MARGIN
                + f64::from(col) * (f64::from(self.tile_width.get()) + 30.0);
            let y = geometry[section_index].first_photo_y + f64::from(row) * row_height;
            self.root.move_(&tile.tile, x, y);
        }
        drop(photos);

        let wanted_header_ids = wanted_headers.iter().copied().collect::<HashSet<_>>();
        let stale_headers = self
            .live_headers
            .borrow()
            .keys()
            .copied()
            .filter(|index| !wanted_header_ids.contains(index))
            .collect::<Vec<_>>();
        for index in stale_headers {
            if let Some(label) = self.live_headers.borrow_mut().remove(&index) {
                self.root.remove(&label);
                let mut pool = self.header_pool.borrow_mut();
                if pool.len() < SECTIONED_HEADER_POOL_CAP {
                    pool.push_back(label);
                }
            }
        }

        for section_index in wanted_headers {
            let existing = {
                let live = self.live_headers.borrow();
                live.get(&section_index).cloned()
            };
            let label = if let Some(label) = existing {
                label
            } else {
                let label = self
                    .header_pool
                    .borrow_mut()
                    .pop_front()
                    .unwrap_or_else(|| {
                        let label = gtk::Label::new(None);
                        label.set_xalign(0.0);
                        label.set_yalign(0.5);
                        label.add_css_class("section-heading");
                        label.add_css_class("folder-section-heading");
                        label
                    });
                let range = &ranges[section_index];
                label.set_text(&format!(
                    "{}   ·   {} photos",
                    range.label,
                    range.end.saturating_sub(range.start)
                ));
                self.root.put(&label, SECTIONED_SIDE_MARGIN, 0.0);
                self.live_headers
                    .borrow_mut()
                    .insert(section_index, label.clone());
                label
            };
            label.set_size_request(
                (width - (SECTIONED_SIDE_MARGIN * 2.0) as i32).max(1),
                SECTIONED_HEADER_HEIGHT as i32,
            );
            self.root
                .move_(&label, SECTIONED_SIDE_MARGIN, geometry[section_index].header_y);
        }
    }

    fn sync_selection(&self) {
        for (index, tile) in self.live_tiles.borrow().iter() {
            tile.tile.set_manual_selected(self.selection.is_selected(*index));
        }
    }

    fn refresh_model(self: &Rc<Self>) {
        // A replacement can keep the same numeric positions while changing
        // PhotoObject metadata. Recycle the bounded realized set so every
        // visible tile is rebound exactly once to the current model.
        let live = std::mem::take(&mut *self.live_tiles.borrow_mut());
        for (_, tile) in live {
            self.root.remove(&tile.tile);
            tile.index.set(None);
            let mut pool = self.tile_pool.borrow_mut();
            if pool.len() < SECTIONED_TILE_POOL_CAP {
                pool.push_back(tile);
            }
        }
        let headers = std::mem::take(&mut *self.live_headers.borrow_mut());
        for (_, label) in headers {
            self.root.remove(&label);
            let mut pool = self.header_pool.borrow_mut();
            if pool.len() < SECTIONED_HEADER_POOL_CAP {
                pool.push_back(label);
            }
        }
        self.invalidate_geometry();
        self.refresh();
    }

    fn capture_center_anchor(&self) -> Option<(i64, f64)> {
        let scrolled = self.scroll.borrow().as_ref()?.clone();
        let adjustment = scrolled.vadjustment();
        let target = adjustment.value() + adjustment.page_size() * 0.5;
        let columns = self.current_columns.get().max(1);
        let row_height = f64::from(folder_line_height(self.tile_height.get()));
        let ranges = self.group_ranges.borrow();
        let geometry = self.geometry.borrow();

        for (range, geom) in ranges.iter().zip(geometry.iter()) {
            if target >= geom.end_y || range.start == range.end {
                continue;
            }
            let row = if target <= geom.first_photo_y {
                0
            } else {
                ((target - geom.first_photo_y) / row_height)
                    .floor()
                    .max(0.0) as u32
            };
            let local = (row * columns).min(range.end.saturating_sub(range.start) as u32 - 1);
            let index = range.start as u32 + local;
            let photo = self.current_photos.borrow().get(index as usize)?.clone();
            let y = geom.first_photo_y + f64::from(row) * row_height;
            return Some((photo.id(), y - adjustment.value()));
        }
        None
    }

    fn restore_anchor(self: &Rc<Self>, photo_id: i64, offset: f64) -> bool {
        let Some(index) = self
            .current_photos
            .borrow()
            .iter()
            .position(|photo| photo.id() == photo_id)
        else {
            return false;
        };
        self.refresh();
        let Some(y) = self.y_for_index(index as u32) else {
            return false;
        };
        let Some(scrolled) = self.scroll.borrow().as_ref().cloned() else {
            return false;
        };
        let adjustment = scrolled.vadjustment();
        let upper = (adjustment.upper() - adjustment.page_size()).max(adjustment.lower());
        adjustment.set_value((y - offset).clamp(adjustment.lower(), upper));
        self.refresh();
        true
    }

    fn y_for_index(&self, index: u32) -> Option<f64> {
        let columns = self.current_columns.get().max(1);
        let row_height = f64::from(folder_line_height(self.tile_height.get()));
        let ranges = self.group_ranges.borrow();
        let geometry = self.geometry.borrow();
        for (range, geom) in ranges.iter().zip(geometry.iter()) {
            if index >= range.start as u32 && index < range.end as u32 {
                let local = index - range.start as u32;
                return Some(geom.first_photo_y + f64::from(local / columns) * row_height);
            }
        }
        None
    }

    fn scroll_to_index(self: &Rc<Self>, index: u32, header: bool) -> bool {
        self.refresh();
        let ranges = self.group_ranges.borrow();
        let geometry = self.geometry.borrow();
        let target = ranges
            .iter()
            .zip(geometry.iter())
            .find_map(|(range, geom)| {
                (index >= range.start as u32 && index < range.end as u32).then(|| {
                    if header {
                        geom.header_y
                    } else {
                        let local = index - range.start as u32;
                        let row = local / self.current_columns.get().max(1);
                        geom.first_photo_y
                            + f64::from(row) * f64::from(folder_line_height(self.tile_height.get()))
                    }
                })
            });
        drop(geometry);
        drop(ranges);
        let Some(target) = target else {
            return false;
        };
        let Some(scrolled) = self.scroll.borrow().as_ref().cloned() else {
            return false;
        };
        let adjustment = scrolled.vadjustment();
        let upper = (adjustment.upper() - adjustment.page_size()).max(adjustment.lower());
        adjustment.set_value(target.clamp(adjustment.lower(), upper));
        self.refresh();
        true
    }

    fn scroll_position(&self) -> f64 {
        self.scroll
            .borrow()
            .as_ref()
            .map(|scroll| scroll.vadjustment().value())
            .unwrap_or(0.0)
    }

    fn photo_for_scroll_position(&self, scroll_y: f64) -> Option<PhotoObject> {
        let columns = self.current_columns.get().max(1);
        let row_height = f64::from(folder_line_height(self.tile_height.get()));
        let ranges = self.group_ranges.borrow();
        let geometry = self.geometry.borrow();
        for (range, geom) in ranges.iter().zip(geometry.iter()) {
            if scroll_y >= geom.end_y || range.start == range.end {
                continue;
            }
            let row = if scroll_y <= geom.first_photo_y {
                0
            } else {
                ((scroll_y - geom.first_photo_y) / row_height)
                    .floor()
                    .max(0.0) as u32
            };
            let local = (row * columns).min(range.end.saturating_sub(range.start) as u32 - 1);
            return self
                .current_photos
                .borrow()
                .get(range.start + local as usize)
                .cloned();
        }
        None
    }

    fn viewport_center_photo(&self) -> Option<PhotoObject> {
        let scrolled = self.scroll.borrow().as_ref()?.clone();
        let adjustment = scrolled.vadjustment();
        self.photo_for_scroll_position(adjustment.value() + adjustment.page_size() * 0.5)
    }


    fn set_scroll_y(self: &Rc<Self>, scroll_y: f64) {
        let Some(scrolled) = self.scroll.borrow().as_ref().cloned() else {
            return;
        };
        self.refresh();
        let adjustment = scrolled.vadjustment();
        let upper = (adjustment.upper() - adjustment.page_size()).max(adjustment.lower());
        adjustment.set_value(scroll_y.clamp(adjustment.lower(), upper));
        self.refresh();
    }
    fn focus_photo(&self, photo_id: i64) {
        if let Some(tile) = self.live_tiles.borrow().values().find(|entry| {
            entry
                .tile
                .photo()
                .as_ref()
                .is_some_and(|photo| photo.id() == photo_id)
        }) {
            tile.tile.grab_focus();
        }
    }
}


impl Gallery {
    pub fn attach_sectioned_folder_scroll(self: &Rc<Self>, scrolled: &gtk::ScrolledWindow) {
        self.sectioned_folder.attach_scroll(scrolled);
    }

    pub fn refresh_sectioned_folder(self: &Rc<Self>) {
        self.sectioned_folder.refresh_model();
    }

    pub fn using_sectioned_folder_view(&self) -> bool {
        self.group_mode.get() == GroupMode::Folder && crate::grid::sectioned_folder_view_enabled()
    }

    fn sectioned_capture_anchor(&self) -> Option<(i64, f64)> {
        self.sectioned_folder.capture_center_anchor()
    }

    fn sectioned_restore_anchor(self: &Rc<Self>, anchor: Option<(i64, f64)>) {
        self.sectioned_folder.invalidate_geometry();
        self.sectioned_folder.refresh();
        if let Some((photo_id, offset)) = anchor {
            self.sectioned_folder.restore_anchor(photo_id, offset);
        }
    }

    fn sectioned_sync_selection(&self) {
        self.sectioned_folder.sync_selection();
    }
}
