impl Gallery {
    /// Returns the currently selected thumbnail position when the grid has a
    /// single active selection. Keyboard navigation uses this to decide when
    /// Up/Down should cross into the adjacent folder.
    pub fn selected_position(&self) -> Option<usize> {
        let selected = self.selection.selection();
        gtk::BitsetIter::init_first(&selected).map(|(_, position)| position as usize)
    }

    pub fn scroll_position(&self) -> f64 {
        let adjustment = if self.group_mode.get() == GroupMode::Folder {
            self.folder_root.vadjustment()
        } else {
            self.root.vadjustment()
        };
        adjustment
            .map(|adjustment| adjustment.value())
            .unwrap_or_else(|| self.last_scroll_y.get())
    }

    /// Return keyboard focus to the realized `SquareTile` for `photo_id` in the
    /// folder stream. A `GtkListView` row may hold several photos, so focusing
    /// the list alone leaves arrow keys without a thumbnail to start from.
    /// Falls back to the `ListView` when the tile is not realized yet.
    fn focus_folder_tile(&self, photo_id: i64) {
        let root = self.folder_root.clone();
        let attempts = Rc::new(Cell::new(0u32));
        let attempts_for_timer = attempts.clone();
        // GtkListView may need a few frames to realize a far target row after
        // scroll_to(). Wait for the exact tile to exist, but never issue another
        // scroll here: repeated scroll requests are what used to fight the
        // smooth Folder scroller.
        glib::timeout_add_local(std::time::Duration::from_millis(16), move || {
            let mut tiles = Vec::new();
            collect_tiles(root.upcast_ref(), &mut tiles);
            if let Some(tile) = tiles.into_iter().find(|tile| {
                tile.imp()
                    .photo
                    .borrow()
                    .as_ref()
                    .is_some_and(|photo| photo.id() == photo_id)
            }) {
                tile.grab_focus();
                return glib::ControlFlow::Break;
            }

            let attempt = attempts_for_timer.get() + 1;
            attempts_for_timer.set(attempt);
            if attempt >= 30 {
                root.grab_focus();
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
    }

    pub fn restore_view(&self, photo_id: i64, scroll_y: f64) {
        let Some(position) = self
            .current_photos
            .borrow()
            .iter()
            .position(|photo| photo.id() == photo_id)
        else {
            return;
        };
        let folder_mode = self.group_mode.get() == GroupMode::Folder;
        let folder_row = folder_mode
            .then(|| self.folder_row_index_for_photo(photo_id))
            .flatten();
        let root = self.root.clone();
        let folder_root = self.folder_root.clone();
        let selection = self.selection.clone();
        glib::idle_add_local_once(move || {
            selection.select_item(position as u32, true);
            let scroll = gtk::ScrollInfo::new();
            scroll.set_enable_horizontal(false);
            scroll.set_enable_vertical(false);
            if folder_mode {
                if let Some(row) = folder_row {
                    folder_root.scroll_to(row, gtk::ListScrollFlags::FOCUS, Some(scroll));
                }
                if let Some(adjustment) = folder_root.vadjustment() {
                    let upper =
                        (adjustment.upper() - adjustment.page_size()).max(adjustment.lower());
                    adjustment.set_value(scroll_y.clamp(adjustment.lower(), upper));
                }
            } else {
                root.scroll_to(
                    position as u32,
                    gtk::ListScrollFlags::SELECT | gtk::ListScrollFlags::FOCUS,
                    Some(scroll),
                );
                if let Some(adjustment) = root.vadjustment() {
                    let upper =
                        (adjustment.upper() - adjustment.page_size()).max(adjustment.lower());
                    adjustment.set_value(scroll_y.clamp(adjustment.lower(), upper));
                }
            }
        });
        if folder_mode {
            self.focus_folder_tile(photo_id);
        }
    }

    pub fn restore_context_view(&self, photo_id: i64, scroll_y: f64) {
        let Some(position) = self
            .current_photos
            .borrow()
            .iter()
            .position(|photo| photo.id() == photo_id)
        else {
            return;
        };
        let folder_mode = self.group_mode.get() == GroupMode::Folder;
        let folder_row = folder_mode
            .then(|| self.folder_row_index_for_photo(photo_id))
            .flatten();
        let root = self.root.clone();
        let folder_root = self.folder_root.clone();
        glib::idle_add_local_once(move || {
            let scroll = gtk::ScrollInfo::new();
            scroll.set_enable_horizontal(false);
            scroll.set_enable_vertical(false);
            if folder_mode {
                if let Some(row) = folder_row {
                    folder_root.scroll_to(row, gtk::ListScrollFlags::FOCUS, Some(scroll));
                }
                if let Some(adjustment) = folder_root.vadjustment() {
                    let upper =
                        (adjustment.upper() - adjustment.page_size()).max(adjustment.lower());
                    adjustment.set_value(scroll_y.clamp(adjustment.lower(), upper));
                }
            } else {
                root.scroll_to(position as u32, gtk::ListScrollFlags::FOCUS, Some(scroll));
                if let Some(adjustment) = root.vadjustment() {
                    let upper =
                        (adjustment.upper() - adjustment.page_size()).max(adjustment.lower());
                    adjustment.set_value(scroll_y.clamp(adjustment.lower(), upper));
                }
            }
        });
        if folder_mode {
            self.focus_folder_tile(photo_id);
        }
    }

    pub fn is_at_vertical_boundary(&self, direction: i32) -> bool {
        let Some(position) = self.selected_position() else {
            return false;
        };
        let item_count = self.store.n_items() as usize;
        let columns = self.current_columns.get().max(1) as usize;
        if item_count == 0 {
            return false;
        }

        if direction < 0 {
            position < columns
        } else {
            position.saturating_add(columns) >= item_count
        }
    }

    pub fn update_edit_recipe(&self, id: i64, recipe: &str) {
        if let Some(photo) = self
            .current_photos
            .borrow()
            .iter()
            .find(|photo| photo.id() == id)
        {
            photo.set_edit_recipe(recipe.to_string());
        }
        let mut tiles = Vec::new();
        collect_tiles(self.root.upcast_ref(), &mut tiles);
        collect_tiles(self.folder_root.upcast_ref(), &mut tiles);
        for tile in tiles {
            let matches = tile
                .imp()
                .photo
                .borrow()
                .as_ref()
                .is_some_and(|photo| photo.id() == id);
            if matches {
                tile.refresh_thumbnail();
            }
        }
    }

    pub fn update_dimensions(&self, id: i64, width: Option<i64>, height: Option<i64>) {
        if let Some(photo) = self
            .current_photos
            .borrow()
            .iter()
            .find(|photo| photo.id() == id)
        {
            photo.set_width(width.unwrap_or_default());
            photo.set_height(height.unwrap_or_default());
        }
    }

    pub fn selected_photo_ids(&self, fallback_id: Option<i64>) -> Vec<i64> {
        if self.collage_selection_mode.get() {
            return self.collage_selected_ids.borrow().iter().copied().collect();
        }
        let selected = self.selection.selection();
        let mut ids = Vec::new();
        if let Some((mut iter, first)) = gtk::BitsetIter::init_first(&selected) {
            ids.extend(
                std::iter::once(first)
                    .chain(&mut iter)
                    .filter_map(|position| {
                        self.current_photos.borrow().get(position as usize).cloned()
                    })
                    .map(|photo| photo.id()),
            );
        }
        match fallback_id {
            Some(fallback) if !ids.contains(&fallback) => vec![fallback],
            _ => ids,
        }
    }

    pub fn set_collage_selection_mode(&self, active: bool) {
        self.collage_selection_mode.set(active);
        if !active {
            self.collage_selected_ids.borrow_mut().clear();
        }
    }

    pub fn set_selected_photo_ids(&self, ids: &[i64]) {
        self.collage_selected_ids
            .borrow_mut()
            .extend(ids.iter().copied());
        self.collage_selected_ids
            .borrow_mut()
            .retain(|id| ids.contains(id));
        self.selection.unselect_all();
        let wanted = ids.iter().copied().collect::<HashSet<_>>();
        for position in 0..self.store.n_items() {
            let Some(photo) = self.store.item(position).and_downcast::<PhotoObject>() else {
                continue;
            };
            if wanted.contains(&photo.id()) {
                self.selection.select_item(position, false);
            }
        }
    }

    fn restore_collage_selection(&self) {
        self.selection.unselect_all();
        let wanted = self.collage_selected_ids.borrow().clone();
        for position in 0..self.store.n_items() {
            if self
                .store
                .item(position)
                .and_downcast::<PhotoObject>()
                .is_some_and(|photo| wanted.contains(&photo.id()))
            {
                self.selection.select_item(position, false);
            }
        }
    }

    fn folder_row_index_for_photo(&self, photo_id: i64) -> Option<u32> {
        for position in 0..self.folder_store.n_items() {
            let Some(row) = self
                .folder_store
                .item(position)
                .and_downcast::<FolderRowObject>()
            else {
                continue;
            };
            if row.contains_photo(photo_id) {
                return Some(position);
            }
        }
        None
    }

    fn folder_header_row_for_target(&self, folder_id: i64, folder_path: &str) -> Option<u32> {
        let target_path = std::path::Path::new(folder_path);
        let mut descendant = None;
        for position in 0..self.folder_store.n_items() {
            let Some(row) = self
                .folder_store
                .item(position)
                .and_downcast::<FolderRowObject>()
            else {
                continue;
            };
            let data = row.data();
            if data.kind != FolderRowKind::Header {
                continue;
            }
            if data.folder_id == folder_id {
                return Some(position);
            }
            if descendant.is_none()
                && !data.folder_path.is_empty()
                && std::path::Path::new(&data.folder_path).starts_with(target_path)
            {
                descendant = Some(position);
            }
        }
        descendant
    }

    pub fn grab_focus(&self) {
        if self.group_mode.get() == GroupMode::Folder {
            self.folder_root.grab_focus();
        } else {
            self.root.grab_focus();
        }
    }

    /// True while the gallery is still constructing a progressive replacement.
    /// Folder navigation uses this to keep retrying folder/photo reveal until
    /// the virtualized Folder rows exist.
    pub fn stream_building(&self) -> bool {
        self.stream_building.get()
    }

    pub fn select_photo(&self, photo_id: i64) -> bool {
        let Some(model) = self.selection.model() else {
            return false;
        };
        for position in 0..model.n_items() {
            let matches = model
                .item(position)
                .and_downcast::<PhotoObject>()
                .is_some_and(|photo| photo.id() == photo_id);
            if !matches {
                continue;
            }
            if self.group_mode.get() == GroupMode::Folder {
                // The backing store is filled progressively, but the Folder
                // ListView rows are only built once the whole stream is ready.
                // Selecting the photo is safe early, but report success only
                // when the row exists so callers keep retrying instead of
                // leaving the user parked at the folder header/wrong row.
                self.selection.select_item(position, true);
                let Some(row) = self.folder_row_index_for_photo(photo_id) else {
                    return false;
                };
                self.folder_root
                    .scroll_to(row, gtk::ListScrollFlags::FOCUS, None);
                self.focus_folder_tile(photo_id);
                return true;
            }
            self.selection.select_item(position, true);
            self.root.scroll_to(
                position,
                gtk::ListScrollFlags::SELECT | gtk::ListScrollFlags::FOCUS,
                None,
            );
            return true;
        }
        false
    }

    /// Scroll the continuous Folder stream to the folder's real header row
    /// without rebuilding the gallery. Imported-root rows may not own photos
    /// directly, so the first descendant folder header is a valid target.
    pub fn scroll_to_folder(&self, folder_id: i64, folder_path: &str) -> bool {
        let target_path = std::path::Path::new(folder_path);
        let Some(photo_position) = self.current_photos.borrow().iter().position(|photo| {
            if photo.folder_id() == folder_id {
                return true;
            }
            photo
                .folder_path()
                .as_deref()
                .is_some_and(|path| std::path::Path::new(path).starts_with(target_path))
        }) else {
            return false;
        };

        self.selection.select_item(photo_position as u32, true);
        if self.group_mode.get() == GroupMode::Folder {
            let Some(row) = self.folder_header_row_for_target(folder_id, folder_path) else {
                return false;
            };
            self.folder_root
                .scroll_to(row, gtk::ListScrollFlags::FOCUS, None);
            self.folder_root.grab_focus();
        } else {
            self.root.scroll_to(
                photo_position as u32,
                gtk::ListScrollFlags::SELECT | gtk::ListScrollFlags::FOCUS,
                None,
            );
        }
        true
    }

    pub fn select_last_photo(&self) {
        let count = self.store.n_items();
        if count == 0 {
            return;
        }
        let position = count - 1;
        self.selection.select_item(position, true);
        if self.group_mode.get() == GroupMode::Folder {
            if let Some(row) = self.folder_row_index_for_photo(
                self.store
                    .item(position)
                    .and_downcast::<PhotoObject>()
                    .map(|photo| photo.id())
                    .unwrap_or_default(),
            ) {
                self.folder_root
                    .scroll_to(row, gtk::ListScrollFlags::FOCUS, None);
            }
        } else {
            self.root.scroll_to(
                position,
                gtk::ListScrollFlags::SELECT | gtk::ListScrollFlags::FOCUS,
                None,
            );
        }
    }

    pub fn photo_objects(&self) -> Vec<PhotoObject> {
        self.current_photos.borrow().clone()
    }
}
fn selection_position_for_id(selection: &gtk::MultiSelection, photo_id: i64) -> Option<u32> {
    (0..selection.n_items()).find(|position| {
        selection
            .item(*position)
            .and_downcast::<PhotoObject>()
            .is_some_and(|photo| photo.id() == photo_id)
    })
}

fn find_named_label(root: &gtk::Widget, name: &str) -> Option<gtk::Label> {
    if root.widget_name().as_str() == name {
        if let Ok(label) = root.clone().downcast::<gtk::Label>() {
            return Some(label);
        }
    }
    let mut child = root.first_child();
    while let Some(current) = child {
        if let Some(found) = find_named_label(&current, name) {
            return Some(found);
        }
        child = current.next_sibling();
    }
    None
}

fn selected_photo_id_set(selection: &gtk::MultiSelection) -> HashSet<i64> {
    let selected = selection.selection();
    let mut ids = HashSet::new();
    if let Some((mut iter, first)) = gtk::BitsetIter::init_first(&selected) {
        for position in std::iter::once(first).chain(&mut iter) {
            if let Some(photo) = selection.item(position).and_downcast::<PhotoObject>() {
                ids.insert(photo.id());
            }
        }
    }
    ids
}

fn selected_positions(selection: &gtk::MultiSelection) -> Vec<u32> {
    let selected = selection.selection();
    let mut positions = Vec::new();
    if let Some((mut iter, first)) = gtk::BitsetIter::init_first(&selected) {
        for position in std::iter::once(first).chain(&mut iter) {
            positions.push(position);
        }
    }
    positions
}

/// Resolve the folder-click outcomes into a sorted, deduplicated set of model
/// positions. Plain click replaces the selection, Control toggles one position,
/// and Shift selects the inclusive range between the stored anchor and the
/// clicked position.
fn folder_selection_after_click(
    item_count: u32,
    selected: &[u32],
    anchor: Option<u32>,
    position: u32,
    control: bool,
    shift: bool,
) -> Vec<u32> {
    if item_count == 0 {
        return Vec::new();
    }
    let last = item_count - 1;
    let position = position.min(last);
    let mut result = Vec::new();
    if control {
        result.extend_from_slice(selected);
        if let Some(index) = result.iter().position(|item| *item == position) {
            result.remove(index);
        } else {
            result.push(position);
        }
    } else if shift {
        let anchor = anchor.unwrap_or(position).min(last);
        let start = anchor.min(position);
        let end = anchor.max(position);
        result.extend(start..=end);
    } else {
        result.push(position);
    }
    result.sort_unstable();
    result.dedup();
    result
}

fn apply_folder_click_selection(
    selection: &gtk::MultiSelection,
    anchor: &Cell<Option<u32>>,
    position: u32,
    modifiers: gtk::gdk::ModifierType,
) {
    let control = modifiers.contains(gtk::gdk::ModifierType::CONTROL_MASK);
    let shift = modifiers.contains(gtk::gdk::ModifierType::SHIFT_MASK);
    let selected = selected_positions(selection);
    let desired = folder_selection_after_click(
        selection.n_items(),
        &selected,
        anchor.get(),
        position,
        control,
        shift,
    );
    if !shift || anchor.get().is_none() {
        anchor.set(Some(position));
    }
    selection.unselect_all();
    for item in desired {
        selection.select_item(item, false);
    }
}

/// Axis-aligned bounds for one realized folder tile, expressed in the folder
/// `ListView` coordinate space so drag hit testing is deterministic.
#[derive(Clone, Copy, Debug)]
struct FolderTileBounds {
    position: u32,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

fn folder_dragged_positions(
    tiles: &[FolderTileBounds],
    start: (f64, f64),
    end: (f64, f64),
) -> Vec<u32> {
    let left = start.0.min(end.0);
    let right = start.0.max(end.0);
    let top = start.1.min(end.1);
    let bottom = start.1.max(end.1);
    let mut positions: Vec<u32> = tiles
        .iter()
        .filter(|tile| {
            tile.width > 0.0
                && tile.height > 0.0
                && tile.x <= right
                && tile.x + tile.width >= left
                && tile.y <= bottom
                && tile.y + tile.height >= top
        })
        .map(|tile| tile.position)
        .collect();
    positions.sort_unstable();
    positions.dedup();
    positions
}

fn folder_drag_rectangle(start: (f64, f64), end: (f64, f64)) -> (f64, f64, f64, f64) {
    (
        start.0.min(end.0),
        start.1.min(end.1),
        (start.0 - end.0).abs(),
        (start.1 - end.1).abs(),
    )
}

#[derive(Default)]
struct FolderDragState {
    start: Option<(f64, f64)>,
    base_ids: HashSet<i64>,
    id_positions: HashMap<i64, u32>,
    positions_ready: bool,
    control: bool,
    active: bool,
    last: Vec<u32>,
}

impl FolderDragState {
    fn clear(&mut self) {
        self.start = None;
        self.base_ids.clear();
        self.id_positions.clear();
        self.positions_ready = false;
        self.control = false;
        self.active = false;
        self.last.clear();
    }
}

/// True when the picked widget sits on the unavailable-source badge button,
/// which owns its own click handler and must be allowed to receive the event.
fn picked_offline_badge(picked: &gtk::Widget) -> bool {
    let mut current = Some(picked.clone());
    while let Some(widget) = current {
        if let Some(button) = widget.downcast_ref::<gtk::Button>() {
            if button.has_css_class("offline-badge") {
                return true;
            }
        }
        current = widget.parent();
    }
    false
}

#[allow(clippy::too_many_arguments)]
fn install_folder_root_input(
    folder_root: &gtk::ListView,
    selection: &gtk::MultiSelection,
    current_photos: &Rc<RefCell<Vec<PhotoObject>>>,
    activate: &Rc<dyn Fn(Vec<PhotoObject>, usize)>,
    context_menu: &Rc<dyn Fn(PhotoObject, gtk::Widget, f64, f64)>,
    collage_mode: &Rc<Cell<bool>>,
    collage_ids: &Rc<RefCell<HashSet<i64>>>,
) -> gtk::DrawingArea {
    let anchor: Rc<Cell<Option<u32>>> = Rc::new(Cell::new(None));
    let rubberband = gtk::DrawingArea::new();
    rubberband.set_hexpand(true);
    rubberband.set_vexpand(true);
    rubberband.set_can_target(false);
    rubberband.add_css_class("folder-rubberband");
    let rubberband_rect: Rc<Cell<Option<(f64, f64, f64, f64)>>> = Rc::new(Cell::new(None));
    let rubberband_rect_for_draw = rubberband_rect.clone();
    rubberband.set_draw_func(move |_, context, width, height| {
        let Some((x, y, rect_width, rect_height)) = rubberband_rect_for_draw.get() else {
            return;
        };
        let x = x.clamp(0.0, width as f64);
        let y = y.clamp(0.0, height as f64);
        let right = (x + rect_width).clamp(0.0, width as f64);
        let bottom = (y + rect_height).clamp(0.0, height as f64);
        let width = (right - x).max(0.0);
        let height = (bottom - y).max(0.0);
        context.set_source_rgba(0.30, 0.62, 0.86, 0.18);
        context.rectangle(x, y, width, height);
        let _ = context.fill_preserve();
        context.set_source_rgba(0.47, 0.73, 0.91, 0.95);
        context.set_line_width(1.0);
        let _ = context.stroke();
    });
    rubberband.set_visible(false);

    let left_click = gtk::GestureClick::new();
    left_click.set_button(1);
    left_click.set_propagation_phase(gtk::PropagationPhase::Capture);
    let root_for_left = folder_root.clone();
    let selection_for_left = selection.clone();
    let current_photos_for_left = current_photos.clone();
    let activate_for_left = activate.clone();
    let collage_mode_for_left = collage_mode.clone();
    let collage_ids_for_left = collage_ids.clone();
    let anchor_for_left = anchor.clone();
    left_click.connect_pressed(move |gesture, n_press, x, y| {
        let Some(tile) = resolve_folder_tile(&root_for_left, x, y) else {
            return;
        };
        let Some(photo) = tile.imp().photo.borrow().as_ref().cloned() else {
            return;
        };
        let Some(position) = selection_position_for_id(&selection_for_left, photo.id()) else {
            return;
        };

        // Do not claim the sequence here: claiming on press prevents the
        // grouped drag gesture from ever emitting drag-update, which is what
        // broke rubberband selection.
        tile.grab_focus();

        if collage_mode_for_left.get() {
            let mut selected_ids = collage_ids_for_left.borrow_mut();
            if selected_ids.remove(&photo.id()) {
                selection_for_left.unselect_item(position);
            } else {
                selected_ids.insert(photo.id());
                selection_for_left.select_item(position, false);
            }
            return;
        }

        if n_press >= 2 {
            selection_for_left.select_item(position, true);
            let photos = current_photos_for_left.borrow().clone();
            if let Some(index) = photos.iter().position(|item| item.id() == photo.id()) {
                crate::source::net_trace(format!(
                    "tile_activated photo_id={} index={index}",
                    photo.id()
                ));
                (activate_for_left)(photos, index);
            }
            anchor_for_left.set(Some(position));
            return;
        }

        let modifiers = gesture.current_event_state();
        apply_folder_click_selection(&selection_for_left, &anchor_for_left, position, modifiers);
    });
    folder_root.add_controller(left_click.clone());

    let right_click = gtk::GestureClick::new();
    right_click.set_button(3);
    right_click.set_propagation_phase(gtk::PropagationPhase::Capture);
    let root_for_context = folder_root.clone();
    let selection_for_context = selection.clone();
    let context_menu_for_root = context_menu.clone();
    right_click.connect_pressed(move |gesture, _, x, y| {
        let Some(tile) = resolve_folder_tile(&root_for_context, x, y) else {
            return;
        };
        let Some(photo) = tile.imp().photo.borrow().as_ref().cloned() else {
            return;
        };
        let Some(position) = selection_position_for_id(&selection_for_context, photo.id()) else {
            return;
        };
        let Some(frame) = tile.first_child().and_downcast::<gtk::Overlay>() else {
            return;
        };
        let frame_widget = frame.clone().upcast::<gtk::Widget>();
        let local = root_for_context
            .compute_point(
                &frame_widget,
                &gtk::graphene::Point::new(x as f32, y as f32),
            )
            .unwrap_or_else(|| gtk::graphene::Point::new(0.0, 0.0));

        gesture.set_state(gtk::EventSequenceState::Claimed);
        if !selection_for_context.is_selected(position) {
            selection_for_context.select_item(position, true);
        }
        tile.grab_focus();
        (context_menu_for_root)(photo, frame_widget, local.x() as f64, local.y() as f64);
    });
    folder_root.add_controller(right_click);

    let drag = gtk::GestureDrag::new();
    drag.set_button(1);
    drag.set_propagation_phase(gtk::PropagationPhase::Capture);
    let drag_state = Rc::new(RefCell::new(FolderDragState::default()));

    let root_for_begin = folder_root.clone();
    let selection_for_begin = selection.clone();
    let collage_mode_for_begin = collage_mode.clone();
    let rubberband_for_begin = rubberband.clone();
    let rubberband_rect_for_begin = rubberband_rect.clone();
    let state_for_begin = drag_state.clone();
    drag.connect_drag_begin(move |gesture, x, y| {
        state_for_begin.borrow_mut().clear();
        rubberband_rect_for_begin.set(None);
        rubberband_for_begin.set_visible(false);
        rubberband_for_begin.queue_draw();
        if collage_mode_for_begin.get() {
            gesture.set_state(gtk::EventSequenceState::Denied);
            return;
        }
        // Rubber-band from anywhere in the folder stream, exactly like the
        // Library GridView: a press on a tile focuses it, while a press in the
        // row padding or the empty tail of a row starts an empty
        // rectangle instead of being rejected.
        if let Some(tile) = resolve_folder_tile(&root_for_begin, x, y) {
            tile.grab_focus();
        }
        let modifiers = gesture.current_event_state();
        let mut state = state_for_begin.borrow_mut();
        state.start = Some((x, y));
        state.active = true;
        state.control = modifiers.contains(gtk::gdk::ModifierType::CONTROL_MASK);
        state.base_ids = if state.control {
            selected_photo_id_set(&selection_for_begin)
        } else {
            HashSet::new()
        };
        state.last.clear();
        drop(state);
        root_for_begin.set_cursor_from_name(Some("crosshair"));
    });

    let root_for_update = folder_root.clone();
    let selection_for_update = selection.clone();
    let current_photos_for_update = current_photos.clone();
    let state_for_update = drag_state.clone();
    let rubberband_for_update = rubberband.clone();
    let rubberband_rect_for_update = rubberband_rect.clone();
    drag.connect_drag_update(move |gesture, offset_x, offset_y| {
        // A stationary press still emits tiny drag updates. Do not claim those,
        // otherwise the click gesture is cancelled and loses its double-click
        // counter, which broke double-click-to-open. Only take over once the
        // pointer has actually travelled.
        if offset_x.hypot(offset_y) < DRAG_CLAIM_THRESHOLD {
            return;
        }
        gesture.set_state(gtk::EventSequenceState::Claimed);
        {
            // Build the id -> model-position map only once a real drag starts,
            // so ordinary clicks never pay for a 66k-entry hash map.
            let mut state = state_for_update.borrow_mut();
            if state.active && !state.positions_ready {
                state.id_positions = current_photos_for_update
                    .borrow()
                    .iter()
                    .enumerate()
                    .map(|(index, photo)| (photo.id(), index as u32))
                    .collect();
                state.positions_ready = true;
            }
        }
        let hit = {
            let state = state_for_update.borrow();
            if !state.active {
                return;
            }
            let Some(start) = state.start else {
                return;
            };
            let end = (start.0 + offset_x, start.1 + offset_y);
            rubberband_rect_for_update.set(Some(folder_drag_rectangle(start, end)));
            rubberband_for_update.set_visible(true);
            rubberband_for_update.queue_draw();
            let mut bounds = Vec::new();
            let mut tiles = Vec::new();
            collect_tiles(root_for_update.upcast_ref(), &mut tiles);
            for tile in tiles.iter() {
                if !tile.is_mapped() || tile.height() <= 0 {
                    continue;
                }
                let Some(photo) = tile.imp().photo.borrow().as_ref().cloned() else {
                    continue;
                };
                let Some(position) = state.id_positions.get(&photo.id()).copied() else {
                    continue;
                };
                let Some(rect) = tile.compute_bounds(&root_for_update) else {
                    continue;
                };
                bounds.push(FolderTileBounds {
                    position,
                    x: rect.x() as f64,
                    y: rect.y() as f64,
                    width: rect.width() as f64,
                    height: rect.height() as f64,
                });
            }
            let mut positions = folder_dragged_positions(&bounds, start, end);
            if state.control {
                for id in &state.base_ids {
                    if let Some(position) = state.id_positions.get(id) {
                        positions.push(*position);
                    }
                }
                positions.sort_unstable();
                positions.dedup();
            }
            positions
        };
        {
            let mut state = state_for_update.borrow_mut();
            if state.last == hit {
                return;
            }
            state.last = hit.clone();
        }
        selection_for_update.unselect_all();
        for position in &hit {
            selection_for_update.select_item(*position, false);
        }
    });

    let state_for_end = drag_state.clone();
    let root_for_end = folder_root.clone();
    let rubberband_for_end = rubberband.clone();
    let rubberband_rect_for_end = rubberband_rect.clone();
    drag.connect_drag_end(move |_, _, _| {
        root_for_end.set_cursor_from_name(None);
        rubberband_rect_for_end.set(None);
        rubberband_for_end.set_visible(false);
        rubberband_for_end.queue_draw();
        state_for_end.borrow_mut().clear();
    });

    // Group the click and drag gestures so a click that turns into a drag can
    // hand the sequence over instead of the click denying the drag (the same
    // coordination GTK's own list widgets rely on for rubberband selection).
    // Both gestures must already belong to the same widget before grouping.
    // Grouping first triggered gtk_gesture_group assertions at startup.
    folder_root.add_controller(drag.clone());
    left_click.group_with(&drag);
    rubberband
}

fn refresh_folder_selection_styles(root: &gtk::ListView, selection: &gtk::MultiSelection) {
    let selected_ids = selected_photo_id_set(selection);
    let mut tiles = Vec::new();
    collect_tiles(root.upcast_ref(), &mut tiles);
    for tile in tiles {
        let selected = tile
            .imp()
            .photo
            .borrow()
            .as_ref()
            .is_some_and(|photo| selected_ids.contains(&photo.id()));
        tile.set_manual_selected(selected);
    }
}

fn folder_id_from_named_ancestor(widget: &gtk::Widget) -> Option<i64> {
    let mut current = Some(widget.clone());
    while let Some(candidate) = current {
        let name = candidate.widget_name();
        if let Some(value) = name.as_str().strip_prefix("picasa-folder-row-") {
            if let Ok(folder_id) = value.parse::<i64>() {
                return Some(folder_id);
            }
        }
        current = candidate.parent();
    }
    None
}

/// Nearest `SquareTile` ancestor of a picked widget, if any.
fn tile_ancestor(widget: &gtk::Widget) -> Option<SquareTile> {
    let mut current = Some(widget.clone());
    while let Some(candidate) = current {
        if let Ok(tile) = candidate.clone().downcast::<SquareTile>() {
            return Some(tile);
        }
        current = candidate.parent();
    }
    None
}

/// Resolve a point inside the Folder ListView to the bound SquareTile under it.
/// Folder rows now contain direct tile children, so GTK's normal `pick` result
/// is sufficient; there is no nested FlowBox child to resolve manually.
fn resolve_folder_tile(root: &gtk::ListView, x: f64, y: f64) -> Option<SquareTile> {
    let picked = root.pick(x, y, gtk::PickFlags::DEFAULT)?;
    if picked_offline_badge(&picked) {
        return None;
    }
    let tile = tile_ancestor(&picked)?;
    if tile.imp().photo.borrow().is_some() {
        Some(tile)
    } else {
        None
    }
}
