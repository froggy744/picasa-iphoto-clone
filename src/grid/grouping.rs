#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupMode {
    None,
    Day,
    Month,
    /// Internal grouping used by Folder mode. This is not a user-selectable
    /// Library grouping preference; it divides the continuous Picasa-style
    /// Folder stream into real, non-sticky scrolling sections.
    Folder,
    /// Internal sticky-date grouping for the History stream. Buckets are
    /// Today / Yesterday / Earlier This Week / Earlier This Month / month-year
    /// from `edited_at`, not EXIF or import dates. Not a user preference.
    History,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupDate {
    Taken,
    Added,
}

#[derive(Debug, Clone)]
struct GroupRange {
    start: usize,
    end: usize,
    label: String,
    // Folder groups also carry their id so equal basenames do not merge.
    folder_id: i64,
}

impl Gallery {
    /// Apply additions from an authoritative, sorted Folder stream while
    /// retaining existing PhotoObjects and list rows. Return false when an
    /// existing photo changed or disappeared and a full replacement is needed.
    pub fn apply_folder_stream_additions(&self, photos: &[crate::db::Photo]) -> bool {
        if self.group_mode.get() != GroupMode::Folder {
            return false;
        }
        if self.stream_building.get() {
            return false;
        }

        let current = self.current_photos.borrow();
        if photos.len() < current.len() {
            return false;
        }
        let current_ids = current.iter().map(|photo| photo.id()).collect::<HashSet<_>>();
        let mut current_index = 0usize;
        let mut updated = Vec::with_capacity(photos.len());
        let mut insertions = Vec::<(usize, Vec<PhotoObject>)>::new();

        for (position, photo) in photos.iter().enumerate() {
            if current_index < current.len() && current[current_index].id() == photo.id {
                let object = &current[current_index];
                if object.path() != photo.path
                    || object.history_caption() != photo.history_caption
                    || object.edited_at() != photo.edited_at
                    || object.edit_recipe() != photo.edit_recipe
                    || object.mtime() != photo.mtime.unwrap_or_default()
                    || object.size_bytes() != photo.size_bytes.unwrap_or_default()
                    || object.taken_at() != photo.taken_at
                    || object.width() != photo.width.unwrap_or_default()
                    || object.height() != photo.height.unwrap_or_default()
                    || object.folder_id() != photo.folder_id.unwrap_or_default()
                    || object.folder_path() != photo.folder_path
                    || object.camera() != photo.camera
                    || object.rotation() != photo.rotation
                    || object.favorite() != photo.favorite
                {
                    return false;
                }
                updated.push(object.clone());
                current_index += 1;
            } else {
                if current_ids.contains(&photo.id) {
                    return false;
                }
                let object = PhotoObject::from_photo(photo);
                match insertions.last_mut() {
                    Some((start, objects)) if *start + objects.len() == position => {
                        objects.push(object.clone());
                    }
                    _ => insertions.push((position, vec![object.clone()])),
                }
                updated.push(object);
            }
        }
        if current_index != current.len() {
            return false;
        }
        drop(current);

        for (start, objects) in insertions.into_iter().rev() {
            self.store.splice(start as u32, 0, &objects);
        }
        self.current_photos.replace(updated);
        self.rebuild_group_ranges();
        self.rebuild_folder_rows();
        true
    }

    pub fn set_grouping(&self, mode: GroupMode, date: GroupDate) {
        let old_mode = self.group_mode.replace(mode);
        let old_date = self.group_date.replace(date);
        let changed = old_mode != mode || old_date != date;
        if changed || mode != GroupMode::None {
            self.rebuild_group_ranges();
        }

        if mode == GroupMode::Folder {
            // The experiment uses the existing external heading as a sticky
            // folder indicator because GtkGridView has no full-width header
            // factory in the GTK version available to PIC.
            if crate::grid::folder_gridview_experiment_enabled() {
                self.update_group_header_for_scroll(self.last_scroll_y.get());
            } else {
                self.group_header.set_visible(false);
                self.group_title.set_text("");
                self.group_count.set_text("");
            }
            if old_mode != GroupMode::Folder && !self.restore_folder_cache() {
                // The current model may still contain All Photos/Favourites in
                // a global date order. Do not briefly render that as hundreds
                // of false folder sections while the correctly ordered Folder
                // stream is loading. The subsequent photo replacement builds
                // the Folder rows once; reapplying Folder grouping must not
                // build the same store a second time.
                self.folder_store.remove_all();
            }
        } else {
            let visible = mode != GroupMode::None && !self.group_ranges.borrow().is_empty();
            self.group_header.set_visible(visible);
            if visible {
                self.update_group_header_for_scroll(self.last_scroll_y.get());
            } else {
                self.group_title.set_text("");
                self.group_count.set_text("");
            }
        }

        if mode == GroupMode::Folder && crate::grid::folder_gridview_experiment_enabled() {
            self.update_group_header_for_scroll(self.last_scroll_y.get());
        }

        if let Some(handler) = self.folder_view_changed.borrow().as_ref() {
            handler(mode == GroupMode::Folder);
        }
    }

    pub fn set_folder_view_changed_handler(&self, handler: impl Fn(bool) + 'static) {
        let handler: Rc<dyn Fn(bool)> = Rc::new(handler);
        handler(self.group_mode.get() == GroupMode::Folder);
        self.folder_view_changed.replace(Some(handler));
    }

    /// True only while the gallery is showing the continuous grouped Folder
    /// stream. Active global search temporarily switches Folder mode to
    /// `GroupMode::None`, so delayed search work can use this to detect that a
    /// Folder navigation has already won and must not be overwritten.
    pub(crate) fn folder_grouping_active(&self) -> bool {
        self.group_mode.get() == GroupMode::Folder
    }

    pub fn update_group_header_for_scroll(&self, scroll_y: f64) {
        self.last_scroll_y.set(scroll_y.max(0.0));
        let mode = self.group_mode.get();
        if mode == GroupMode::Folder && !crate::grid::folder_gridview_experiment_enabled() {
            return;
        }
        if mode == GroupMode::None {
            return;
        }

        self.update_group_header_for_index(self.index_for_scroll_position(scroll_y));
    }

    /// Return the photo represented by the leading visible content. Folder
    /// mode reads the virtualized ListView row at the viewport edge; other
    /// modes keep the existing GridView geometry.
    pub fn photo_for_scroll_position(&self, scroll_y: f64) -> Option<PhotoObject> {
        if self.group_mode.get() == GroupMode::Folder
            && !crate::grid::folder_gridview_experiment_enabled()
        {
            return self.photo_for_visible_folder_row();
        }
        self.current_photos
            .borrow()
            .get(self.index_for_scroll_position(scroll_y))
            .cloned()
    }

    /// Month and year for the photo nearest a GridView scroll position. This
    /// intentionally bypasses grouping, so a scrub indicator can be useful
    /// even when the regular sticky date heading is disabled.
    pub fn month_label_for_scroll_position(&self, scroll_y: f64, date: GroupDate) -> String {
        self.current_photos
            .borrow()
            .get(self.index_for_scroll_position(scroll_y))
            .map(|photo| group_label(photo, GroupMode::Month, date))
            .unwrap_or_else(|| "Unknown Date".to_string())
    }

    fn index_for_scroll_position(&self, scroll_y: f64) -> usize {
        // Grid item padding is 6px on each edge in window.rs. The top grid
        // margin is 20px. Keep this calculation shared with the sticky heading.
        const ITEM_PADDING: f64 = 6.0;
        const GRID_TOP_MARGIN: f64 = 20.0;
        let tile_height = self.tile_height.get().max(1) as f64;
        let row_pitch = tile_height + ITEM_PADDING * 2.0;
        let row = ((scroll_y - GRID_TOP_MARGIN).max(0.0) / row_pitch).floor() as usize;
        row.saturating_mul(self.current_columns.get().max(1) as usize)
    }

    fn rebuild_group_ranges(&self) {
        let mode = self.group_mode.get();
        let date = self.group_date.get();
        let photos = self.current_photos.borrow();

        let mut ranges = Vec::<GroupRange>::new();
        if mode != GroupMode::None {
            for (index, photo) in photos.iter().enumerate() {
                let label = group_label(photo, mode, date);
                let folder_id = if mode == GroupMode::Folder {
                    photo.folder_id()
                } else {
                    0
                };
                match ranges.last_mut() {
                    Some(last) if last.label == label && last.folder_id == folder_id => {
                        last.end = index + 1
                    }
                    _ => ranges.push(GroupRange {
                        start: index,
                        end: index + 1,
                        label,
                        folder_id,
                    }),
                }
            }
        }
        self.group_ranges.replace(ranges);
        if mode == GroupMode::Folder && crate::grid::folder_gridview_experiment_enabled() {
            self.update_group_header_for_scroll(self.last_scroll_y.get());
        }
    }

    fn rebuild_folder_rows(&self) {
        if crate::grid::folder_gridview_experiment_enabled() {
            // The experimental Folder view binds the already ordered photo
            // model directly to GtkGridView; virtual row GObjects are unused.
            self.save_folder_cache();
            return;
        }
        rebuild_folder_rows_for(
            &self.current_photos,
            &self.group_ranges,
            &self.current_columns,
            &self.folder_order,
            &self.folder_catalog,
            &self.folder_store,
        );
        self.save_folder_cache();
    }

    /// Remember the current Folder stream so re-entering Folder mode can reuse
    /// the already built PhotoObjects and virtual rows.
    fn save_folder_cache(&self) {
        if self.group_mode.get() != GroupMode::Folder {
            return;
        }
        save_folder_cache_for(
            &self.folder_cache,
            &self.current_photos,
            &self.group_ranges,
            &self.current_columns,
            &self.folder_order,
        );
    }

    /// Restore a previously built Folder stream when re-entering Folder mode.
    ///
    /// Returns true when the cached rows can be reused as-is (tile geometry and
    /// section order unchanged), in which case `folder_store` must not be
    /// cleared. The Folder rows reference the restored PhotoObjects by index and
    /// `selection` reads them through `store`, so both must be replaced.
    pub fn can_restore_folder_cache(&self) -> bool {
        let Some(cache) = self.folder_cache.borrow().as_ref().cloned() else {
            if crate::diagnostics::trace_enabled() { eprintln!("PIC_NAV folder_cache_restore result=reject reason=no_cached_rows"); }
            return false;
        };
        if cache.columns != self.current_columns.get()
            && !crate::grid::folder_gridview_experiment_enabled()
            && !crate::grid::folder_chunked_experiment_enabled()
        {
            if crate::diagnostics::trace_enabled() { eprintln!("PIC_NAV folder_cache_restore result=reject reason=current_columns_mismatch cached={} current={}", cache.columns, self.current_columns.get()); }
            return false;
        }
        if cache.order != *self.folder_order.borrow() {
            if crate::diagnostics::trace_enabled() { eprintln!("PIC_NAV folder_cache_restore result=reject reason=folder_order_mismatch"); }
            return false;
        }
        if crate::diagnostics::trace_enabled() { eprintln!("PIC_NAV folder_cache_restore result=hit"); }
        if std::env::var_os("PICASA_TRACE_BACKTRACE").is_some() {
            eprintln!("PIC_NAV can_restore_backtrace\n{}", std::backtrace::Backtrace::force_capture());
        }
        true
    }

    /// Atomic Folder navigation reuse decision: the caller may skip the model
    /// rebuild and scroll only when the CURRENT model is the continuous Folder
    /// stream and the target folder actually exists inside it.
    ///
    /// `can_restore_folder_cache` only reports that a cached stream exists with
    /// compatible geometry; it cannot see whether the model still shows that
    /// stream (library/album/search/history replaces it without clearing the
    /// cache). Combining the cache check with the actual `scroll_to_folder`
    /// lookup removes that race: a `true` result always means the scroll
    /// happened, and `false` always means a reload is required.
    pub fn try_restore_folder_navigation(&self, folder_id: i64, folder_path: &str) -> bool {
        if self.group_mode.get() != GroupMode::Folder {
            // The visible model is not the Folder stream (library, album,
            // search, history). Scrolling cannot reveal the destination.
            if crate::diagnostics::trace_enabled() { eprintln!("PIC_NAV folder_cache_restore result=reject reason=not_folder_mode"); }
            return false;
        }
        if !self.can_restore_folder_cache() {
            return false;
        }
        if self.scroll_to_folder(folder_id, folder_path) {
            true
        } else {
            // The cached stream is valid but predates the current model, or
            // the destination folder has no photos in the stream yet.
            if crate::diagnostics::trace_enabled() { eprintln!("PIC_NAV folder_cache_restore result=reject reason=target_folder_missing_from_stream"); }
            false
        }
    }

    pub fn set_pending_folder_target(&self, folder_id: i64, folder_path: String) {
        self.pending_folder_target
            .replace(Some((folder_id, folder_path)));
    }

    pub fn clear_pending_folder_target(&self) {
        self.pending_folder_target.replace(None);
    }

    pub fn has_pending_folder_target(&self) -> bool {
        self.pending_folder_target.borrow().is_some()
    }

    /// Focus the search-selected folder only after its real Folder rows exist.
    /// A cache restore makes those rows available immediately; a progressive
    /// build makes them available once ranges and rows have been rebuilt.
    pub fn try_focus_pending_folder(&self) -> bool {
        if self.stream_building.get() || self.group_mode.get() != GroupMode::Folder {
            return false;
        }
        let Some((folder_id, folder_path)) = self.pending_folder_target.borrow().clone() else {
            return false;
        };
        if !self.scroll_to_folder(folder_id, &folder_path) {
            return false;
        }
        self.pending_folder_target.replace(None);
        
        true
    }

    fn restore_folder_cache(&self) -> bool {
        let Some(cache) = self.folder_cache.borrow().clone() else {
            return false;
        };
        if (cache.columns != self.current_columns.get()
            && !crate::grid::folder_gridview_experiment_enabled()
            && !crate::grid::folder_chunked_experiment_enabled())
            || cache.order != *self.folder_order.borrow()
        {
            return false;
        }
        self.current_photos.replace(cache.photos.clone());
        self.group_ranges.replace(cache.ranges.clone());
        self.store.splice(0, self.store.n_items(), &cache.photos);
        
        true
    }

    /// Supply the folder hierarchy/order used by the sidebar. The catalog is
    /// ordering metadata only: folders without direct photos must not become
    /// empty gallery sections.
    pub fn set_folder_catalog(&self, folders: &[Folder], folder_order: &[i64]) {
        if crate::diagnostics::trace_enabled() && *self.folder_order.borrow() != folder_order { eprintln!("PIC_NAV folder_order_changed old_count={} new_count={}", self.folder_order.borrow().len(), folder_order.len()); }
        let catalog = folders
            .iter()
            .map(|folder| FolderCatalogEntry {
                folder_id: folder.id,
                parent_id: folder.parent_id,
                photo_count: usize::try_from(folder.photo_count.max(0)).unwrap_or(0),
            })
            .collect::<Vec<_>>();
        let catalog_changed = self.folder_catalog.borrow().as_slice() != catalog.as_slice();
        let order_changed = self.folder_order.borrow().as_slice() != folder_order;
        if !catalog_changed && !order_changed {
            return;
        }

        self.folder_catalog.replace(catalog);
        self.folder_order.replace(folder_order.to_vec());
        // Cached rows encode the previous catalog/order. Force the next Folder
        // entry to use the new anchors instead of reusing stale rows.
        self.folder_cache.replace(None);
        if self.group_mode.get() == GroupMode::Folder {
            self.rebuild_folder_rows();
        }
    }

    /// Folder id at the leading visible row. This is intentionally cheaper
    /// than `photo_for_visible_folder_row`: passive sidebar follow needs only
    /// the row's folder identity, not a PhotoObject or a group-range scan.
    pub fn visible_folder_id(&self) -> Option<i64> {
        if self.group_mode.get() != GroupMode::Folder {
            return None;
        }
        let width = self.folder_root.width().max(1) as f64;
        for y in [4.0_f64, 20.0, 40.0, 64.0] {
            let picked = self
                .folder_root
                .pick(width * 0.5, y, gtk::PickFlags::DEFAULT);
            if let Some(folder_id) = picked
                .as_ref()
                .and_then(folder_id_from_named_ancestor)
                .filter(|folder_id| *folder_id > 0)
            {
                return Some(folder_id);
            }
        }
        None
    }

    fn photo_for_visible_folder_row(&self) -> Option<PhotoObject> {
        // A hit-test at the viewport centre can land between columns (especially
        // with an even column count). Falling back to the folder's first photo
        // then jumps thousands of rows. Use actual visible tile bounds instead.
        let mut tiles = Vec::new();
        collect_tiles(self.folder_root.upcast_ref(), &mut tiles);
        let height = self.folder_root.height() as f32;
        let preferred = self.folder_anchor_photo.get();
        tiles
            .into_iter()
            .filter_map(|tile| {
                if !tile.is_mapped() || !tile.is_visible() {
                    return None;
                }
                let photo = tile.imp().photo.borrow().clone()?;
                let bounds = tile.compute_bounds(&self.folder_root)?;
                if bounds.y() + bounds.height() <= 0.0 || bounds.y() >= height {
                    return None;
                }
                Some((bounds.y(), bounds.x(), photo))
            })
            // Keep the same logical photo while it remains in the leading row.
            // Choosing the leftmost slot anew on every 6↔7-column transition
            // rounds backwards each time and gradually scrolls up the folder.
            .min_by(|a, b| {
                a.0.total_cmp(&b.0)
                    .then_with(|| (Some(b.2.id()) == preferred).cmp(&(Some(a.2.id()) == preferred)))
                    .then_with(|| a.1.total_cmp(&b.1))
            })
            .map(|(_, _, photo)| photo)
    }

    fn update_group_header_for_index(&self, index: usize) {
        let ranges = self.group_ranges.borrow();
        let Some(range) = ranges
            .iter()
            .find(|range| index >= range.start && index < range.end)
            .or_else(|| ranges.last())
        else {
            self.group_header.set_visible(false);
            return;
        };

        self.group_header.set_visible(true);
        self.group_title.set_text(&range.label);
        let count = range.end.saturating_sub(range.start);
        self.group_count.set_text(&format!(
            "•  {} {}",
            format_count(count),
            if count == 1 { "photo" } else { "photos" }
        ));
    }
}
fn rebuild_group_ranges_for(
    current_photos: &Rc<RefCell<Vec<PhotoObject>>>,
    group_mode: &Rc<Cell<GroupMode>>,
    group_date: &Rc<Cell<GroupDate>>,
    group_ranges: &Rc<RefCell<Vec<GroupRange>>>,
) {
    let mode = group_mode.get();
    let date = group_date.get();
    let photos = current_photos.borrow();
    let mut ranges: Vec<GroupRange> = Vec::new();
    if mode != GroupMode::None {
        for (index, photo) in photos.iter().enumerate() {
            let label = group_label(photo, mode, date);
            let folder_id = if mode == GroupMode::Folder {
                photo.folder_id()
            } else {
                0
            };
            match ranges.last_mut() {
                Some(last) if last.label == label && last.folder_id == folder_id => {
                    last.end = index + 1
                }
                _ => ranges.push(GroupRange {
                    start: index,
                    end: index + 1,
                    label,
                    folder_id,
                }),
            }
        }
    }
    group_ranges.replace(ranges);
}

fn update_group_header_for_index_for(
    group_mode: &Rc<Cell<GroupMode>>,
    group_ranges: &Rc<RefCell<Vec<GroupRange>>>,
    group_header: &gtk::Box,
    group_title: &gtk::Label,
    group_count: &gtk::Label,
    index: usize,
) {
    if group_mode.get() == GroupMode::None {
        group_header.set_visible(false);
        group_title.set_text("");
        group_count.set_text("");
        return;
    }

    let ranges = group_ranges.borrow();
    let Some(range) = ranges
        .iter()
        .find(|range| index >= range.start && index < range.end)
        .or_else(|| ranges.last())
    else {
        group_header.set_visible(false);
        return;
    };

    group_header.set_visible(true);
    group_title.set_text(&range.label);
    let count = range.end.saturating_sub(range.start);
    group_count.set_text(&format!(
        "•  {} {}",
        format_count(count),
        if count == 1 { "photo" } else { "photos" }
    ));
}

fn group_label(photo: &PhotoObject, mode: GroupMode, date: GroupDate) -> String {
    if mode == GroupMode::History {
        return history_group_label(photo.edited_at());
    }
    if mode == GroupMode::Folder {
        let folder_path = photo.folder_path().unwrap_or_default();
        if folder_path.is_empty() {
            return "Unknown Folder".to_string();
        }
        return crate::db::folder_name(&folder_path);
    }

    let value = match date {
        GroupDate::Taken => photo.taken_at().and_then(|value| parse_photo_date(&value)),
        GroupDate::Added => {
            let mtime = photo.mtime();
            (mtime > 0)
                .then(|| Local.timestamp_opt(mtime, 0).single())
                .flatten()
        }
    };

    let Some(value) = value else {
        return "Unknown Date".to_string();
    };

    match mode {
        GroupMode::None => String::new(),
        GroupMode::Folder => unreachable!("folder grouping returns before date grouping"),
        GroupMode::History => unreachable!("history grouping returns before date grouping"),
        GroupMode::Month => value.format("%b %Y").to_string(),
        GroupMode::Day => {
            let date = value.date_naive();
            let today = Local::now().date_naive();
            if date == today {
                "Today".to_string()
            } else if date == today.pred_opt().unwrap_or(today) {
                "Yesterday".to_string()
            } else {
                // Match the bottom bar's dd Mmm yyyy date style so headers
                // and the infobar agree across All Photos/Favourites views.
                value.format("%d %b %Y").to_string()
            }
        }
    }
}

/// Sticky History heading buckets from the last edit time, newest bucket
/// first. "Earlier This Week" is a rolling 7-day window (excluding today and
/// yesterday); "Earlier This Month" is the rest of the current calendar month.
fn history_group_label(edited_at: i64) -> String {
    if edited_at <= 0 {
        return "Unknown Date".to_string();
    }
    let Some(value) = chrono::DateTime::from_timestamp_millis(edited_at) else {
        return "Unknown Date".to_string();
    };
    let value = value.with_timezone(&Local);
    let date = value.date_naive();
    let today = Local::now().date_naive();
    if date == today {
        return "Today".to_string();
    }
    if date == today.pred_opt().unwrap_or(today) {
        return "Yesterday".to_string();
    }
    let age_days = (today - date).num_days();
    if age_days <= 7 {
        return "Earlier This Week".to_string();
    }
    if value.format("%Y-%m").to_string() == today.format("%Y-%m").to_string() {
        return "Earlier This Month".to_string();
    }
    value.format("%b %Y").to_string()
}

fn parse_photo_date(value: &str) -> Option<chrono::DateTime<Local>> {
    if let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(value) {
        return Some(parsed.with_timezone(&Local));
    }
    if let Ok(parsed) = chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S") {
        return Local.from_local_datetime(&parsed).single();
    }
    if let Ok(parsed) = chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H-%M-%S") {
        return Local.from_local_datetime(&parsed).single();
    }
    None
}

fn format_count(value: usize) -> String {
    let digits = value.to_string();
    let mut result = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, character) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            result.push(',');
        }
        result.push(character);
    }
    result
}
