fn folder_navigation_scope(
    relationships: impl IntoIterator<Item = (i64, Option<i64>)>,
    target_id: i64,
) -> HashSet<i64> {
    let mut children = HashMap::<i64, Vec<i64>>::new();
    for (folder_id, parent_id) in relationships {
        if let Some(parent_id) = parent_id {
            children.entry(parent_id).or_default().push(folder_id);
        }
    }

    let mut scope = HashSet::from([target_id]);
    let mut pending = vec![target_id];
    while let Some(parent_id) = pending.pop() {
        if let Some(folder_ids) = children.get(&parent_id) {
            for folder_id in folder_ids {
                if scope.insert(*folder_id) {
                    pending.push(*folder_id);
                }
            }
        }
    }
    scope
}

impl Gallery {
    /// Returns the currently selected thumbnail position when the grid has a
    /// single active selection. Keyboard navigation uses this to decide when
    /// Up/Down should cross into the adjacent folder.
    pub fn selected_position(&self) -> Option<usize> {
        let selected = self.selection.selection();
        gtk::BitsetIter::init_first(&selected).map(|(_, position)| position as usize)
    }
    pub fn scroll_position(&self) -> f64 {
        if self.group_mode.get() == GroupMode::Folder {
            self.v2_folder.scroll_position()
        } else {
            self.root
                .vadjustment()
                .map(|adjustment| adjustment.value())
                .unwrap_or_else(|| self.last_scroll_y.get())
        }
    }


    /// Return the realized photo nearest the visual centre of the active
    /// viewport. Session restore uses this as its focus anchor: the scrollbar
    /// value restores the exact view, while this id restores keyboard focus to
    /// the middle of what the user was looking at.
    pub fn viewport_center_photo(&self) -> Option<PhotoObject> {
        if self.group_mode.get() == GroupMode::Folder {
            return self.v2_folder.visible_photo();
        }
        let root: gtk::Widget = self.root.clone().upcast();
        let center_x = root.width().max(1) as f32 * 0.5;
        let center_y = root.height().max(1) as f32 * 0.5;
        let mut tiles = Vec::new();
        collect_tiles(&root, &mut tiles);
        tiles
            .into_iter()
            .filter_map(|tile| {
                if !tile.is_mapped() || !tile.is_visible() {
                    return None;
                }
                let photo = tile.imp().photo.borrow().clone()?;
                let bounds = tile.compute_bounds(&root)?;
                if bounds.x() + bounds.width() <= 0.0
                    || bounds.x() >= root.width() as f32
                    || bounds.y() + bounds.height() <= 0.0
                    || bounds.y() >= root.height() as f32
                {
                    return None;
                }
                let dx = bounds.x() + bounds.width() * 0.5 - center_x;
                let dy = bounds.y() + bounds.height() * 0.5 - center_y;
                Some((dx * dx + dy * dy, photo))
            })
            .min_by(|left, right| left.0.total_cmp(&right.0))
            .map(|(_, photo)| photo)
            .or_else(|| self.photo_for_scroll_position(self.scroll_position()))
    }


    /// Restore the most recently activated photo as the visual and keyboard
    /// focus of the grid. Returning `false` lets startup fall back to the
    /// separately saved viewport when that photo is not in the current view.
    pub fn restore_activated_photo(&self, photo_id: i64) -> bool {
        if self.group_mode.get() == GroupMode::Folder {
            let selected = self.v2_folder.select_photo(photo_id);
            if selected {
                if let Some(position) = self
                    .current_photos
                    .borrow()
                    .iter()
                    .position(|photo| photo.id() == photo_id)
                {
                    self.selection.select_item(position as u32, true);
                }
            }
            return selected;
        }

        let Some(position) = self
            .current_photos
            .borrow()
            .iter()
            .position(|photo| photo.id() == photo_id)
        else {
            return false;
        };
        self.selection.select_item(position as u32, true);
        self.root.scroll_to(
            position as u32,
            gtk::ListScrollFlags::SELECT | gtk::ListScrollFlags::FOCUS,
            None,
        );
        true
    }


    /// Return keyboard focus to the realized `SquareTile` for `photo_id` in the
    /// folder stream. A `GtkListView` row may hold several photos, so focusing
    /// the list alone leaves arrow keys without a thumbnail to start from.
    /// Falls back to the `ListView` when the tile is not realized yet.
    pub fn restore_view(&self, photo_id: i64, scroll_y: f64) {
        if self.group_mode.get() == GroupMode::Folder {
            if self.v2_folder.select_photo(photo_id) {
                if let Some(adjustment) = self.v2_folder.root.vadjustment() {
                    let upper =
                        (adjustment.upper() - adjustment.page_size()).max(adjustment.lower());
                    adjustment.set_value(scroll_y.clamp(adjustment.lower(), upper));
                    self.v2_folder
                        .update_visible_sections(adjustment.value(), adjustment.page_size());
                }
            }
            return;
        }
        let Some(position) = self
            .current_photos
            .borrow()
            .iter()
            .position(|photo| photo.id() == photo_id)
        else {
            return;
        };
        self.selection.select_item(position as u32, true);
        let scroll = gtk::ScrollInfo::new();
        scroll.set_enable_horizontal(false);
        scroll.set_enable_vertical(false);
        self.root.scroll_to(
            position as u32,
            gtk::ListScrollFlags::SELECT | gtk::ListScrollFlags::FOCUS,
            Some(scroll),
        );
        if let Some(adjustment) = self.root.vadjustment() {
            let upper = (adjustment.upper() - adjustment.page_size()).max(adjustment.lower());
            adjustment.set_value(scroll_y.clamp(adjustment.lower(), upper));
        }
    }
    pub fn restore_context_view(&self, photo_id: i64, scroll_y: f64) {
        if self.group_mode.get() == GroupMode::Folder {
            if self.v2_folder.select_photo(photo_id) {
                if let Some(adjustment) = self.v2_folder.root.vadjustment() {
                    let upper =
                        (adjustment.upper() - adjustment.page_size()).max(adjustment.lower());
                    adjustment.set_value(scroll_y.clamp(adjustment.lower(), upper));
                    self.v2_folder
                        .update_visible_sections(adjustment.value(), adjustment.page_size());
                }
            }
            return;
        }
        let Some(position) = self
            .current_photos
            .borrow()
            .iter()
            .position(|photo| photo.id() == photo_id)
        else {
            return;
        };
        let scroll = gtk::ScrollInfo::new();
        scroll.set_enable_horizontal(false);
        scroll.set_enable_vertical(false);
        self.root.scroll_to(position as u32, gtk::ListScrollFlags::FOCUS, Some(scroll));
        if let Some(adjustment) = self.root.vadjustment() {
            let upper = (adjustment.upper() - adjustment.page_size()).max(adjustment.lower());
            adjustment.set_value(scroll_y.clamp(adjustment.lower(), upper));
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
        self.update_edit_recipes_batch(&[(id, recipe.to_string())]);
    }

    pub fn update_edit_recipes_batch(&self, updates: &[(i64, String)]) {
        if updates.is_empty() {
            return;
        }
        let wanted: std::collections::HashMap<i64, &str> = updates
            .iter()
            .map(|(id, recipe)| (*id, recipe.as_str()))
            .collect();
        for photo in self.current_photos.borrow().iter() {
            if let Some(recipe) = wanted.get(&photo.id()) {
                photo.set_edit_recipe(recipe.to_string());
            }
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
                .map(|photo| photo.id())
                .is_some_and(|id| wanted.contains_key(&id));
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
        if self.group_mode.get() == GroupMode::Folder {
            let ids = self.v2_folder.selected_photo_ids();
            return match fallback_id {
                Some(fallback) if !ids.contains(&fallback) => vec![fallback],
                _ => ids,
            };
        }
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
        if self.group_mode.get() == GroupMode::Folder {
            self.v2_folder.set_selected_ids(ids);
        }
        self.collage_selected_ids.borrow_mut().clear();
        self.collage_selected_ids
            .borrow_mut()
            .extend(ids.iter().copied());
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
    pub fn grab_focus(&self) {
        if self.group_mode.get() == GroupMode::Folder {
            self.v2_folder.root.grab_focus();
        } else {
            self.root.grab_focus();
        }
    }


    /// The list widget that currently owns gallery keyboard input. Folder mode
    /// shows `folder_root` and hides `gallery.root`, so focus helpers must not
    /// hardcode the GridView.
    pub fn visible_root(&self) -> gtk::Widget {
        if self.group_mode.get() == GroupMode::Folder {
            self.v2_folder.root.clone().upcast()
        } else {
            self.root.clone().upcast()
        }
    }


    /// Drive Folder-stream keyboard navigation from the photo model. GtkListView
    /// is backed by NoSelection over virtual rows, so GTK's built-in Ctrl+A and
    /// move-cursor bindings never touch the photo MultiSelection.


    /// True while the gallery is still constructing a progressive replacement.
    /// Folder navigation uses this to keep retrying folder/photo reveal until
    /// the virtualized Folder rows exist.
    pub fn stream_building(&self) -> bool {
        self.stream_building.get()
    }
    pub fn select_photo(&self, photo_id: i64) -> bool {
        if self.group_mode.get() == GroupMode::Folder {
            let selected = self.v2_folder.select_photo(photo_id);
            if selected {
                if let Some(position) = self
                    .current_photos
                    .borrow()
                    .iter()
                    .position(|photo| photo.id() == photo_id)
                {
                    self.selection.select_item(position as u32, true);
                }
            }
            return selected;
        }

        let Some(model) = self.selection.model() else {
            return false;
        };
        for position in 0..model.n_items() {
            if model
                .item(position)
                .and_downcast::<PhotoObject>()
                .is_some_and(|photo| photo.id() == photo_id)
            {
                self.selection.select_item(position, true);
                self.root.scroll_to(
                    position,
                    gtk::ListScrollFlags::SELECT | gtk::ListScrollFlags::FOCUS,
                    None,
                );
                return true;
            }
        }
        false
    }


    /// Scroll the continuous Folder stream to the folder's real header row
    /// without rebuilding the gallery. Imported-root rows may not own photos
    /// directly, so the first descendant folder header is a valid target.
    pub fn scroll_to_folder(&self, folder_id: i64, _folder_path: &str) -> bool {
        let scope = folder_navigation_scope(
            self.folder_catalog
                .borrow()
                .iter()
                .map(|folder| (folder.folder_id, folder.parent_id)),
            folder_id,
        );
        let Some(photo_position) = self
            .current_photos
            .borrow()
            .iter()
            .position(|photo| scope.contains(&photo.folder_id()))
        else {
            return false;
        };
        self.selection.select_item(photo_position as u32, true);
        if self.group_mode.get() == GroupMode::Folder {
            let actual_folder_id = self.current_photos.borrow()[photo_position].folder_id();
            return self.v2_folder.scroll_to_folder(actual_folder_id);
        }
        self.root.scroll_to(
            photo_position as u32,
            gtk::ListScrollFlags::SELECT | gtk::ListScrollFlags::FOCUS,
            None,
        );
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
            if let Some(photo) = self.store.item(position).and_downcast::<PhotoObject>() {
                self.v2_folder.select_photo(photo.id());
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


/// Move Folder keyboard focus one photo in `(dx, dy)` and sync the photo
/// MultiSelection. Prefers a realized tile in that direction (so Up/Down match
/// the visual grid, including header gaps); falls back to model order at the
/// viewport edge so the stream can scroll.


fn position_for_photo(
    selection: &gtk::MultiSelection,
    current_photos: &Rc<RefCell<Vec<PhotoObject>>>,
    photo_id: i64,
) -> Option<u32> {
    if let Some(position) = current_photos
        .borrow()
        .iter()
        .position(|photo| photo.id() == photo_id)
    {
        return Some(position as u32);
    }
    let model = selection.model()?;
    (0..model.n_items()).find(|&position| {
        model
            .item(position)
            .and_downcast::<PhotoObject>()
            .is_some_and(|photo| photo.id() == photo_id)
    })
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


