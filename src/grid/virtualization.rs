/// Lightweight folder metadata used only to keep Folder sections in the same
/// hierarchy order as the sidebar. Parent/container folders that do not own
/// photos directly are navigation nodes only; they never become blank rows in
/// the continuous photo stream.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FolderCatalogEntry {
    folder_id: i64,
    parent_id: Option<i64>,
    photo_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FolderSectionPlan {
    folder_id: i64,
    range_index: Option<usize>,
}

/// A fully built Folder stream kept alive between view switches.
///
/// The Folder virtual rows in `folder_store` reference photos by index and the
/// shared selection model reads them through `store`, so keeping the exact
/// PhotoObjects plus their group ranges lets re-entering Folder mode reuse an
/// already populated model instead of rebuilding ~12k rows with GTK.
#[derive(Clone)]
struct FolderStreamCache {
    photos: Vec<PhotoObject>,
    ranges: Vec<GroupRange>,
    columns: u32,
    order: Vec<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum FolderRowKind {
    #[default]
    Header,
    Photos,
}

/// Minimum height needed by the Folder section header's contents.
const FOLDER_HEADER_HEIGHT: i32 = 70;

/// Exact height of one Folder model row. Headers stay compact while photo
/// lines continue to follow the current thumbnail zoom. Every scroll/anchor
/// calculation uses this helper so GtkListView allocation and our own geometry
/// stay in sync even though the two row kinds have different heights.
fn folder_model_row_height(kind: FolderRowKind, tile_height: i32) -> i32 {
    match kind {
        FolderRowKind::Header => FOLDER_HEADER_HEIGHT,
        FolderRowKind::Photos => folder_line_height(tile_height),
    }
}

fn folder_chunk_size(columns: u32) -> usize {
    columns.max(1) as usize
}

/// Exact vertical offset for a virtual Folder row. Folder mode deliberately
/// gives every model row a fixed height, so we do not need GtkListView's
/// estimated far-row position when restoring an anchor after a column change.
fn folder_row_offset(rows: &[FolderVirtualRow], target_row: usize, tile_height: i32) -> f64 {
    rows.iter()
        .take(target_row)
        .map(|row| f64::from(folder_model_row_height(row.kind, tile_height)))
        .sum()
}

#[derive(Clone, Default)]
pub(crate) struct FolderRowData {
    kind: FolderRowKind,
    folder_id: i64,
    folder_path: String,
    label: String,
    count: usize,
    start: usize,
    end: usize,
    // Lightweight identity for the photos represented by this visual line.
    // Range geometry alone is not enough: a refresh can replace photo ids in
    // place while leaving folder/count/start/end unchanged.
    photo_ids: Vec<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FolderVirtualRow {
    kind: FolderRowKind,
    start: usize,
    end: usize,
}

/// Folder mode uses a flat virtualized model: one lightweight header row plus
/// fixed photo lines containing at most `columns` photos. GtkListView therefore
/// virtualizes one stable-height visual line at a time.
fn folder_virtual_rows(ranges: &[GroupRange], chunk_size: usize) -> Vec<FolderVirtualRow> {
    let chunk_size = chunk_size.max(1);
    let mut rows = Vec::new();
    for range in ranges {
        rows.push(FolderVirtualRow {
            kind: FolderRowKind::Header,
            start: range.start,
            end: range.start,
        });
        let mut start = range.start;
        while start < range.end {
            let end = (start + chunk_size).min(range.end);
            rows.push(FolderVirtualRow {
                kind: FolderRowKind::Photos,
                start,
                end,
            });
            start = end;
        }
    }
    rows
}

mod folder_row_object {
    use std::cell::RefCell;

    use glib::subclass::prelude::*;

    use super::FolderRowData;

    #[derive(Default)]
    pub(crate) struct FolderRowObject {
        pub(crate) data: RefCell<FolderRowData>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for FolderRowObject {
        const NAME: &'static str = "PicasaFolderRowObject";
        type Type = super::FolderRowObject;
    }

    impl ObjectImpl for FolderRowObject {}
}

glib::wrapper! {
    pub(crate) struct FolderRowObject(ObjectSubclass<folder_row_object::FolderRowObject>);
}

impl FolderRowObject {
    fn new(data: FolderRowData) -> Self {
        let object: Self = glib::Object::new();
        object.imp().data.replace(data);
        object
    }

    fn data(&self) -> FolderRowData {
        self.imp().data.borrow().clone()
    }

    /// Cheap membership test that does not clone the row data. This runs over
    /// tens of thousands of rows when a zoom/column change re-anchors the
    /// viewport, so it must not allocate.
    fn contains_photo(&self, photo_id: i64) -> bool {
        let data = self.imp().data.borrow();
        data.kind == FolderRowKind::Photos && data.photo_ids.contains(&photo_id)
    }
}

fn make_folder_tile(
    tile_width: i32,
    tile_height: i32,
    unavailable: &Rc<dyn Fn(PhotoObject, gtk::Widget)>,
) -> SquareTile {
    let frame = gtk::Overlay::new();
    frame.set_overflow(gtk::Overflow::Hidden);
    frame.add_css_class("photo-frame");
    frame.add_css_class("photo-tile");

    let picture = gtk::Picture::new();
    picture.set_content_fit(gtk::ContentFit::Cover);
    picture.set_can_shrink(true);
    picture.set_size_request(1, 1);
    picture.set_hexpand(true);
    picture.set_vexpand(true);
    picture.set_halign(gtk::Align::Fill);
    picture.set_valign(gtk::Align::Fill);
    picture.add_css_class("thumbnail");
    frame.set_child(Some(&picture));

    let placeholder = gtk::Image::from_icon_name("image-x-generic-symbolic");
    placeholder.set_pixel_size(32);
    placeholder.add_css_class("dim-label");
    placeholder.set_visible(false);
    frame.add_overlay(&placeholder);

    // Badges (selection / favourite / edited / offline) are deliberately NOT
    // built here. GtkListView destroys and re-creates every realized Folder row
    // on a column change, so four extra widgets per tile dominated the rebuild
    // (~250-520 ms) and made the sidebar animation jerk. Each badge is created
    // lazily by ensure_*_badge() only when a photo actually needs it.
    let tile = SquareTile::new(tile_width, tile_height, &frame);
    tile.set_unavailable_handler(unavailable.clone());
    // The old FlowBoxChild supplied 6 px padding around each thumbnail. Keep
    // the same geometry directly on the tile now that the nested FlowBox is
    // gone, and let the horizontal line distribute spare width evenly.
    tile.set_hexpand(true);
    tile.set_vexpand(false);
    tile.set_valign(gtk::Align::Start);
    tile.set_margin_start(6);
    tile.set_margin_end(6);
    tile.set_margin_top(6);
    tile.set_margin_bottom(6);
    tile.set_focusable(true);

    tile
}

#[derive(Clone)]
struct ZoomPointerAnchor {
    scrolled: glib::WeakRef<gtk::ScrolledWindow>,
    photo_id: i64,
    position: u32,
    viewport_x: f32,
    viewport_y: f32,
    relative_x: f32,
    relative_y: f32,
    layer: Option<gtk::Fixed>,
    ghost: Option<gtk::Picture>,
    layer_cursor_x: f64,
    layer_cursor_y: f64,
}

impl Gallery {
    pub fn set_zoom_anchor_layer(&self, layer: &gtk::Fixed) {
        self.zoom_anchor_layer.replace(Some(layer.downgrade()));
    }

    fn capture_zoom_pointer_anchor(
        &self,
        scrolled: &gtk::ScrolledWindow,
        x: f64,
        y: f64,
    ) -> Option<ZoomPointerAnchor> {
        let mut tiles = Vec::new();
        collect_tiles(self.root.upcast_ref(), &mut tiles);

        let x = x as f32;
        let y = y as f32;
        tiles.into_iter().find_map(|tile| {
            if !tile.is_mapped() || !tile.is_visible() {
                return None;
            }
            let photo_id = tile.imp().photo.borrow().as_ref()?.id();
            let position = tile.imp().photo_index.get()? as u32;
            let bounds = tile.compute_bounds(scrolled)?;
            let inside = x >= bounds.x()
                && x <= bounds.x() + bounds.width()
                && y >= bounds.y()
                && y <= bounds.y() + bounds.height();
            if !inside
                || bounds.width() <= f32::EPSILON
                || bounds.height() <= f32::EPSILON
            {
                return None;
            }

            let relative_x = ((x - bounds.x()) / bounds.width()).clamp(0.0, 1.0);
            let relative_y = ((y - bounds.y()) / bounds.height()).clamp(0.0, 1.0);

            let mut layer = None;
            let mut ghost = None;
            let mut layer_cursor_x = f64::from(x);
            let mut layer_cursor_y = f64::from(y);

            if let Some(anchor_layer) = self
                .zoom_anchor_layer
                .borrow()
                .as_ref()
                .and_then(|weak| weak.upgrade())
            {
                if let (Some(point), Some(paintable)) = (
                    scrolled.compute_point(
                        &anchor_layer,
                        &gtk::graphene::Point::new(x, y),
                    ),
                    tile.transition_paintable(),
                ) {
                    let picture = gtk::Picture::for_paintable(&paintable);
                    picture.set_content_fit(if self.fit_whole_photo.get() {
                        gtk::ContentFit::Contain
                    } else {
                        gtk::ContentFit::Cover
                    });
                    picture.set_can_shrink(true);
                    picture.set_can_target(false);
                    picture.set_size_request(
                        bounds.width().round().max(1.0) as i32,
                        bounds.height().round().max(1.0) as i32,
                    );
                    picture.add_css_class("thumbnail");

                    layer_cursor_x = f64::from(point.x());
                    layer_cursor_y = f64::from(point.y());
                    let ghost_x =
                        layer_cursor_x - f64::from(relative_x * bounds.width());
                    let ghost_y =
                        layer_cursor_y - f64::from(relative_y * bounds.height());
                    anchor_layer.put(&picture, ghost_x, ghost_y);

                    // The real GridView cell may move to a completely different
                    // column. Hide it while the floating copy carries the visual
                    // identity under the pointer.
                    tile.set_opacity(0.0);
                    layer = Some(anchor_layer);
                    ghost = Some(picture);
                }
            }

            let anchor = ZoomPointerAnchor {
                scrolled: scrolled.downgrade(),
                photo_id,
                position,
                viewport_x: x,
                viewport_y: y,
                relative_x,
                relative_y,
                layer,
                ghost,
                layer_cursor_x,
                layer_cursor_y,
            };
            if std::env::var_os("PICASA_TRACE").is_some() {
                eprintln!(
                    "PIC_ZOOM_ANCHOR capture id={} pos={} cursor=({:.1},{:.1}) rel=({:.3},{:.3}) bounds=({:.1},{:.1},{:.1},{:.1}) floating={}",
                    anchor.photo_id,
                    anchor.position,
                    anchor.viewport_x,
                    anchor.viewport_y,
                    anchor.relative_x,
                    anchor.relative_y,
                    bounds.x(),
                    bounds.y(),
                    bounds.width(),
                    bounds.height(),
                    anchor.ghost.is_some()
                );
            }
            Some(anchor)
        })
    }

    fn clear_zoom_pointer_anchor(&self) {
        let anchor = self.pending_zoom_pointer_anchor.borrow_mut().take();
        let Some(anchor) = anchor else {
            return;
        };

        if let (Some(layer), Some(ghost)) = (anchor.layer.as_ref(), anchor.ghost.as_ref()) {
            if ghost.parent().is_some() {
                layer.remove(ghost);
            }
        }

        let mut tiles = Vec::new();
        collect_tiles(self.root.upcast_ref(), &mut tiles);
        if let Some(tile) = tiles.into_iter().find(|tile| {
            tile.imp()
                .photo
                .borrow()
                .as_ref()
                .is_some_and(|photo| photo.id() == anchor.photo_id)
        }) {
            tile.set_presentation_offset(0.0, 0.0);
            tile.set_opacity(1.0);
        }
    }

    fn restore_zoom_pointer_anchor(&self, anchor: &ZoomPointerAnchor) -> bool {
        let Some(scrolled) = anchor.scrolled.upgrade() else {
            return false;
        };

        let mut tiles = Vec::new();
        collect_tiles(self.root.upcast_ref(), &mut tiles);
        let Some(tile) = tiles.into_iter().find(|tile| {
            tile.is_mapped()
                && tile
                    .imp()
                    .photo
                    .borrow()
                    .as_ref()
                    .is_some_and(|photo| photo.id() == anchor.photo_id)
        }) else {
            if std::env::var_os("PICASA_TRACE").is_some() {
                eprintln!(
                    "PIC_ZOOM_ANCHOR missing id={} pos={}",
                    anchor.photo_id,
                    anchor.position
                );
            }
            return false;
        };
        let Some(bounds) = tile.compute_bounds(&scrolled) else {
            return false;
        };

        // Keep the real grid vertically aligned with the cursor so the final
        // handoff does not jump when the floating copy is removed.
        let anchored_y = bounds.y() + bounds.height() * anchor.relative_y;
        let delta_y = f64::from(anchored_y - anchor.viewport_y);
        if delta_y.abs() > 0.25 {
            let adjustment = scrolled.vadjustment();
            let lower = adjustment.lower();
            let upper = (adjustment.upper() - adjustment.page_size()).max(lower);
            adjustment.set_value((adjustment.value() + delta_y).clamp(lower, upper));
        }

        tile.set_presentation_offset(0.0, 0.0);
        if let (Some(layer), Some(ghost)) = (anchor.layer.as_ref(), anchor.ghost.as_ref()) {
            tile.set_opacity(0.0);
            let width = bounds.width().max(1.0);
            let height = bounds.height().max(1.0);
            ghost.set_size_request(width.round() as i32, height.round() as i32);
            let ghost_x = anchor.layer_cursor_x - f64::from(anchor.relative_x * width);
            let ghost_y = anchor.layer_cursor_y - f64::from(anchor.relative_y * height);
            layer.move_(ghost, ghost_x, ghost_y);
        } else {
            tile.set_opacity(1.0);
        }

        if std::env::var_os("PICASA_TRACE").is_some() {
            let anchored_x = bounds.x() + bounds.width() * anchor.relative_x;
            let delta_x = anchor.viewport_x - anchored_x;
            eprintln!(
                "PIC_ZOOM_ANCHOR restore id={} pos={} delta_x={:.1} delta_y={:.1} bounds=({:.1},{:.1},{:.1},{:.1}) floating={}",
                anchor.photo_id,
                anchor.position,
                delta_x,
                delta_y,
                bounds.x(),
                bounds.y(),
                bounds.width(),
                bounds.height(),
                anchor.ghost.is_some()
            );
        }
        true
    }

    fn release_zoom_pointer_anchor(self: &Rc<Self>, anchor: ZoomPointerAnchor, generation: u64) {
        const RELEASE_MS: f64 = 140.0;

        self.pending_zoom_pointer_anchor.replace(None);

        let mut tiles = Vec::new();
        collect_tiles(self.root.upcast_ref(), &mut tiles);
        let tile = tiles.into_iter().find(|tile| {
            tile.is_mapped()
                && tile
                    .imp()
                    .photo
                    .borrow()
                    .as_ref()
                    .is_some_and(|photo| photo.id() == anchor.photo_id)
        });

        let (Some(layer), Some(ghost), Some(tile)) =
            (anchor.layer.clone(), anchor.ghost.clone(), tile)
        else {
            if let (Some(layer), Some(ghost)) = (anchor.layer.as_ref(), anchor.ghost.as_ref()) {
                if ghost.parent().is_some() {
                    layer.remove(ghost);
                }
            }
            return;
        };

        let Some(target) = tile.compute_bounds(&layer) else {
            if ghost.parent().is_some() {
                layer.remove(&ghost);
            }
            tile.set_opacity(1.0);
            return;
        };

        let start_width = ghost.width().max(1) as f64;
        let start_height = ghost.height().max(1) as f64;
        let start_x = anchor.layer_cursor_x - f64::from(anchor.relative_x) * start_width;
        let start_y = anchor.layer_cursor_y - f64::from(anchor.relative_y) * start_height;
        let target_x = f64::from(target.x());
        let target_y = f64::from(target.y());
        let target_width = f64::from(target.width().max(1.0));
        let target_height = f64::from(target.height().max(1.0));

        let this = self.clone();
        let started = Instant::now();
        ghost.add_tick_callback(move |ghost, _| {
            if this.zoom_animation_generation.get() != generation {
                let same_photo_still_owned = this
                    .pending_zoom_pointer_anchor
                    .borrow()
                    .as_ref()
                    .is_some_and(|current| current.photo_id == anchor.photo_id);
                if ghost.parent().is_some() {
                    layer.remove(ghost);
                }
                if !same_photo_still_owned {
                    tile.set_opacity(1.0);
                }
                return glib::ControlFlow::Break;
            }

            let linear = (started.elapsed().as_secs_f64() * 1000.0 / RELEASE_MS)
                .clamp(0.0, 1.0);
            let eased = 1.0 - (1.0 - linear).powi(3);
            let x = start_x + (target_x - start_x) * eased;
            let y = start_y + (target_y - start_y) * eased;
            let width = start_width + (target_width - start_width) * eased;
            let height = start_height + (target_height - start_height) * eased;
            ghost.set_size_request(
                width.round().max(1.0) as i32,
                height.round().max(1.0) as i32,
            );
            layer.move_(ghost, x, y);

            if linear >= 1.0 {
                if ghost.parent().is_some() {
                    layer.remove(ghost);
                }
                tile.set_opacity(1.0);
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
    }

    fn settle_zoom_pointer_anchor(
        self: &Rc<Self>,
        anchor: ZoomPointerAnchor,
        generation: u64,
    ) {
        let attempts = Rc::new(Cell::new(0_u8));
        let realization_requested = Rc::new(Cell::new(false));
        let this = self.clone();

        self.root.add_tick_callback(move |_, _| {
            if this.zoom_animation_generation.get() != generation {
                return glib::ControlFlow::Break;
            }

            let attempt = attempts.get().saturating_add(1);
            attempts.set(attempt);

            if this.restore_zoom_pointer_anchor(&anchor) {
                if this.zoom_reflow_source.borrow().is_none()
                    && this.pending_zoom_width.get().is_none()
                {
                    this.release_zoom_pointer_anchor(anchor.clone(), generation);
                }
                return glib::ControlFlow::Break;
            }

            // GtkGridView may recycle the anchor tile for a frame or two while
            // crossing a column boundary. Give normal realization two frames
            // first. Only then ask it to realize the exact model position.
            if attempt >= 3 && !realization_requested.replace(true) {
                // Realize the anchor without permitting scroll_to() to change
                // either axis. The floating copy stays under the pointer while
                // the real cell is being re-created.
                let scroll = gtk::ScrollInfo::new();
                scroll.set_enable_horizontal(false);
                scroll.set_enable_vertical(false);
                if std::env::var_os("PICASA_TRACE").is_some() {
                    eprintln!(
                        "PIC_ZOOM_ANCHOR realize_without_scroll id={} pos={} attempt={}",
                        anchor.photo_id,
                        anchor.position,
                        attempt
                    );
                }
                this.root.scroll_to(
                    anchor.position,
                    gtk::ListScrollFlags::empty(),
                    Some(scroll),
                );
            }

            if attempt >= 8 {
                if this.zoom_reflow_source.borrow().is_none()
                    && this.pending_zoom_width.get().is_none()
                {
                    this.clear_zoom_pointer_anchor();
                }
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
    }

    pub fn zoom_in_at(self: &Rc<Self>, scrolled: &gtk::ScrolledWindow, x: f64, y: f64) {
        let base = self
            .pending_zoom_width
            .get()
            .unwrap_or_else(|| self.tile_width.get());
        let target = next_zoom_level(base);
        if target == base {
            if self.zoom_reflow_source.borrow().is_none() {
                self.clear_zoom_pointer_anchor();
            }
            return;
        }
        if self.pending_zoom_pointer_anchor.borrow().is_none() {
            self.pending_zoom_pointer_anchor
                .replace(self.capture_zoom_pointer_anchor(scrolled, x, y));
        }
        self.request_zoom_internal(target);
    }

    pub fn zoom_out_at(self: &Rc<Self>, scrolled: &gtk::ScrolledWindow, x: f64, y: f64) {
        let base = self
            .pending_zoom_width
            .get()
            .unwrap_or_else(|| self.tile_width.get());
        let target = prev_zoom_level(base);
        if target == base {
            if self.zoom_reflow_source.borrow().is_none() {
                self.clear_zoom_pointer_anchor();
            }
            return;
        }
        if self.pending_zoom_pointer_anchor.borrow().is_none() {
            self.pending_zoom_pointer_anchor
                .replace(self.capture_zoom_pointer_anchor(scrolled, x, y));
        }
        self.request_zoom_internal(target);
    }

    pub fn zoom_in(self: &Rc<Self>) {
        let base = self
            .pending_zoom_width
            .get()
            .unwrap_or_else(|| self.tile_width.get());
        self.clear_zoom_pointer_anchor();
        self.request_zoom_internal(next_zoom_level(base));
    }

    pub fn zoom_out(self: &Rc<Self>) {
        let base = self
            .pending_zoom_width
            .get()
            .unwrap_or_else(|| self.tile_width.get());
        self.clear_zoom_pointer_anchor();
        self.request_zoom_internal(prev_zoom_level(base));
    }

    /// Reset to the default view level: ~4 thumbnails per row for the current
    /// surface, matching the startup default for users who never picked a
    /// size. Falls back to the ladder level nearest the legacy fixed default
    /// before the first real layout is known.
    pub fn reset_zoom(self: &Rc<Self>) {
        let folder_list_mode = self.group_mode.get() == GroupMode::Folder
            && !crate::grid::folder_gridview_experiment_enabled();
        let width = if folder_list_mode {
            self.folder_root.width()
        } else {
            self.root.width()
        };
        let target = if width > 0 {
            zoom_level_for_four_columns(width)
        } else {
            nearest_zoom_level(DEFAULT_TILE_WIDTH)
        };
        self.clear_zoom_pointer_anchor();
        self.request_zoom_internal(target);
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

    /// Toggle filenames in the fixed-height tile caption area. This only walks
    /// GTK's realized tile pool; future GridView and Folder tiles read the
    /// shared setting during setup/bind.
    pub fn set_show_file_names(self: &Rc<Self>, show: bool) {
        if self.show_file_names.get() == show {
            return;
        }
        self.show_file_names.set(show);
        let mut tiles = Vec::new();
        collect_tiles(self.root.upcast_ref(), &mut tiles);
        collect_tiles(self.folder_root.upcast_ref(), &mut tiles);
        for tile in tiles {
            tile.set_filename_visible(show);
        }
    }

    /// Record a zoom request. Isolated clicks apply immediately; a rapid
    /// Ctrl+wheel spin coalesces its extra notches into one trailing reflow so
    /// crossing several column boundaries does not rebuild the Folder rows per
    /// notch.
    fn request_zoom_internal(self: &Rc<Self>, width: i32) {
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
            } else if this.zoom_animation_layout_width.get().is_none() {
                if let Some(anchor) = this.pending_zoom_pointer_anchor.borrow().clone() {
                    let generation = this.zoom_animation_generation.get();
                    this.release_zoom_pointer_anchor(anchor, generation);
                }
            }
            glib::ControlFlow::Break
        });
        self.zoom_reflow_source.replace(Some(source));
    }

    /// Zoom is driven by width. Height scales by the same factor, preserving
    /// the custom width/height shape configured above. `persist` is false for
    /// the startup default so adopting it does not turn it into a preference.
    fn apply_zoom(self: &Rc<Self>, width: i32) {
        const ZOOM_ANIMATION_MS: f64 = 180.0;

        let target_width = width.clamp(MIN_TILE_WIDTH, MAX_TILE_WIDTH);
        let pointer_anchor = self.pending_zoom_pointer_anchor.borrow().clone();
        let start_width = self.tile_width.get().max(1);
        if target_width == start_width {
            return;
        }

        // The legacy Folder ListView still has model rows whose membership is
        // tied to the column count. Keep that fallback on the old immediate
        // path; Gallery v2's direct photo GridView is the animation target.
        if self.group_mode.get() == GroupMode::Folder
            && !crate::grid::folder_gridview_experiment_enabled()
        {
            self.apply_tile_size(target_width, true);
            return;
        }

        let start_height = self.tile_height.get().max(1);
        let target_height = ((start_height as f64)
            * target_width as f64
            / start_width as f64)
            .round()
            .max(1.0) as i32;

        let generation = self.zoom_animation_generation.get().wrapping_add(1);
        self.zoom_animation_generation.set(generation);
        // Use the last width supplied by the outer gallery surface, not
        // GridView::width(). The GridView's own width changes as its column
        // requisition changes and was the source of the 40px feedback loop.
        let stable_layout_width = if self.last_layout_width.get() > 100 {
            self.last_layout_width.get()
        } else {
            self.root.width().max(1)
        };
        self.zoom_animation_layout_width.set(Some(stable_layout_width));
        set_grid_zoom_animation_active(true);
        let started = Instant::now();
        let this = self.clone();

        // Drive presentation geometry from GTK's frame clock. GridView keeps
        // the same PhotoObject model while realized tiles grow/shrink and GTK
        // continuously repositions them toward the destination layout.
        self.root.add_tick_callback(move |_, _| {
            if this.zoom_animation_generation.get() != generation {
                // A newer zoom animation owns the shared motion flag.
                return glib::ControlFlow::Break;
            }

            let linear = (started.elapsed().as_secs_f64() * 1000.0 / ZOOM_ANIMATION_MS)
                .clamp(0.0, 1.0);
            // Cubic ease-out: quick response to input, gentle arrival.
            let eased = 1.0 - (1.0 - linear).powi(3);
            let frame_width = (start_width as f64
                + (target_width - start_width) as f64 * eased)
                .round() as i32;
            let frame_height = (start_height as f64
                + (target_height - start_height) as f64 * eased)
                .round() as i32;

            // Measure the allocation produced by the previous frame before
            // changing geometry again. Measuring immediately after queue_resize()
            // reads stale bounds and was the main reason the focal photo drifted.
            if let Some(anchor) = pointer_anchor.as_ref() {
                this.restore_zoom_pointer_anchor(anchor);
            }
            this.apply_tile_geometry(frame_width, frame_height, false);

            if linear >= 1.0 {
                // Land exactly on the canonical zoom level and persist only
                // once. Intermediate animation frames never touch settings.
                this.apply_tile_geometry(target_width, target_height, true);
                this.zoom_animation_layout_width.set(None);
                set_grid_zoom_animation_active(false);
                if let Some(anchor) = pointer_anchor.clone() {
                    this.settle_zoom_pointer_anchor(anchor, generation);
                }
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
    }

    fn apply_tile_size(&self, width: i32, persist: bool) {
        let old_width = self.tile_width.get().max(1);
        let old_height = self.tile_height.get().max(1);
        let width = width.clamp(MIN_TILE_WIDTH, MAX_TILE_WIDTH);
        if width == old_width {
            return;
        }
        let scale = width as f64 / old_width as f64;
        let height = ((old_height as f64) * scale).round().max(1.0) as i32;
        self.apply_tile_geometry(width, height, persist);
    }

    fn apply_tile_geometry(&self, width: i32, height: i32, persist: bool) {
        // Trace only completed/persisted zooms. Animation frames run at display
        // cadence and must not flood stderr or distort the animation timing.
        let trace_zoom = persist && std::env::var_os("PICASA_TRACE").is_some();
        let zoom_started = trace_zoom.then(Instant::now);
        let old_width = self.tile_width.get().max(1);
        let old_height = self.tile_height.get().max(1);
        let width = width.clamp(MIN_TILE_WIDTH, MAX_TILE_WIDTH);
        let height = height.max(1);
        if width == old_width && height == old_height {
            if persist {
                (self.on_zoom_changed)(width);
            }
            return;
        }
        // Capture the visible photo before the tile resize disturbs the layout.
        let folder_mode = self.group_mode.get() == GroupMode::Folder;
        let folder_list_mode = folder_mode && !crate::grid::folder_gridview_experiment_enabled();
        let anchor_started = trace_zoom.then(Instant::now);
        if folder_mode {
            self.zoom_anchor.set(
                self.photo_for_scroll_position(self.last_scroll_y.get())
                    .map(|photo| photo.id()),
            );
        } else {
            self.zoom_anchor.set(None);
        }
        let anchor_us = anchor_started.map_or(0, |started| started.elapsed().as_micros());

        self.tile_width.set(width);
        self.tile_height.set(height);
        if persist {
            (self.on_zoom_changed)(width);
        }


        let mut tiles = Vec::new();
        if folder_list_mode {
            collect_tiles(self.folder_root.upcast_ref(), &mut tiles);
        } else {
            collect_tiles(self.root.upcast_ref(), &mut tiles);
        }
        let realized_tile_count = tiles.len();

        let resize_started = trace_zoom.then(Instant::now);
        for tile in tiles {
            tile.set_tile_size(width, height);
        }
        let resize_us = resize_started.map_or(0, |started| started.elapsed().as_micros());


        let root_width = if folder_list_mode {
            self.folder_root.width()
        } else {
            self.zoom_animation_layout_width
                .get()
                .unwrap_or_else(|| self.root.width())
        };
        let layout_started = trace_zoom.then(Instant::now);
        if root_width > 100 {
            self.update_layout(root_width, true);
        } else {
            self.update_group_header_for_scroll(self.last_scroll_y.get());
        }
        let layout_us = layout_started.map_or(0, |started| started.elapsed().as_micros());
        self.zoom_anchor.set(None);

        if let Some(started) = zoom_started {
            eprintln!(
                "PIC_ZOOM apply old_width={} width={} height={} mode={} view={} realized_tiles={} anchor_us={} tile_resize_us={} layout_us={} total_us={}",
                old_width,
                width,
                height,
                if folder_mode { "folder" } else { "grid" },
                if folder_list_mode {
                    "folder_list"
                } else {
                    "photo_grid"
                },
                realized_tile_count,
                anchor_us,
                resize_us,
                layout_us,
                started.elapsed().as_micros()
            );
        }

    }

    /// After Folder scrolling settles, load only cached thumbnails for tiles
    /// close to the actual viewport. GtkListView keeps a much larger recycled
    /// widget pool than the visible rows, so loading every bound tile causes
    /// thousands of unnecessary thumbnail operations during scrollbar jumps.
    pub fn refresh_visible_folder_tiles(&self) -> usize {
        if self.group_mode.get() != GroupMode::Folder
            || crate::grid::folder_gridview_experiment_enabled()
            || self.folder_root.height() <= 0
        {
            return 0;
        }

        let viewport = self.folder_root.height() as f32;
        let mut tiles = Vec::new();
        collect_tiles(self.folder_root.upcast_ref(), &mut tiles);
        let mut loaded = 0usize;

        for tile in tiles {
            if !tile.is_mapped() || tile.height() <= 0 {
                tile.unload_visual();
                continue;
            }
            let is_near = tile
                .compute_bounds(&self.folder_root)
                .is_some_and(|bounds| {
                    bounds.y() + bounds.height() >= -viewport * 0.25
                        && bounds.y() <= viewport * 1.25
                });
            if is_near {
                if !tile.imp().visual_loaded.get() {
                    loaded += 1;
                }
                tile.load_folder_cached_visual();
            } else {
                tile.unload_visual();
            }
        }


        loaded
    }

    /// Replace stale GridView visible requests with the thumbnails GTK is
    /// painting in the current frame. This is the non-Folder equivalent of the
    /// Folder motion pump and prevents fast scrollbar movement from filling the
    /// worker queue with viewports the user has already passed.
    pub fn queue_visible_grid_cached_tiles_async(&self, budget: usize) -> usize {
        if budget == 0
            || (self.group_mode.get() == GroupMode::Folder
                && !crate::grid::folder_gridview_experiment_enabled())
            || self.root.height() <= 0
        {
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
        if (self.group_mode.get() == GroupMode::Folder
            && !crate::grid::folder_gridview_experiment_enabled())
            || self.root.height() <= 0
        {
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
        if budget == 0
            || (self.group_mode.get() == GroupMode::Folder
                && !crate::grid::folder_gridview_experiment_enabled())
        {
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
        if (self.group_mode.get() == GroupMode::Folder
            && !crate::grid::folder_gridview_experiment_enabled())
            || self.root.height() <= 0
        {
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
        if (self.group_mode.get() == GroupMode::Folder
            && !crate::grid::folder_gridview_experiment_enabled())
            || self.root.height() <= 0
        {
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
        if budget == 0
            || (self.group_mode.get() == GroupMode::Folder
                && !crate::grid::folder_gridview_experiment_enabled())
        {
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
    pub fn queue_visible_folder_cached_tiles_async(&self, budget: usize) -> usize {
        if budget == 0
            || self.group_mode.get() != GroupMode::Folder
            || self.folder_root.height() <= 0
        {
            return 0;
        }

        let viewport = self.folder_root.height() as f32;
        let mut tiles = Vec::new();
        collect_tiles(self.folder_root.upcast_ref(), &mut tiles);
        let mut candidates: Vec<(f32, SquareTile)> = Vec::new();

        for tile in tiles {
            if tile.height() <= 0 || tile.imp().visual_loaded.get() {
                continue;
            }
            let Some(bounds) = tile.compute_bounds(&self.folder_root) else {
                continue;
            };
            let center = bounds.y() + bounds.height() * 0.5;
            if bounds.y() + bounds.height() < 0.0 || bounds.y() > viewport {
                continue;
            }
            // Start near the viewport centre, then fan out. This makes a large
            // scrollbar jump paint the part the user is looking at first.
            candidates.push(((center - viewport * 0.5).abs(), tile));
        }

        candidates.sort_by(|left, right| left.0.total_cmp(&right.0));
        let mut queued = 0usize;
        for (_, tile) in candidates.into_iter().take(budget) {
            if tile.queue_folder_cached_visual_async(true) {
                queued += 1;
            }
        }
        queued
    }

    /// Folder counterpart of apply_visible_grid_cached_paintables: paint RAM
    /// thumbnails onto currently visible rows during a direct scrub without
    /// replacing the model-derived decode target.
    pub fn apply_visible_folder_cached_paintables(&self) -> usize {
        if self.group_mode.get() != GroupMode::Folder || self.folder_root.height() <= 0 {
            return 0;
        }

        let viewport = self.folder_root.height() as f32;
        let mut tiles = Vec::new();
        collect_tiles(self.folder_root.upcast_ref(), &mut tiles);
        let mut applied = 0usize;
        for tile in tiles {
            if tile.height() <= 0 || tile.imp().visual_loaded.get() {
                continue;
            }
            let Some(bounds) = tile.compute_bounds(&self.folder_root) else {
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

    /// Queue the Folder destination directly from virtual-row geometry.
    ///
    /// Unlike `queue_visible_folder_cached_tiles_async`, this does not depend on
    /// GtkListView having already recycled and allocated the destination row
    /// widgets. That makes it suitable for Page Up/Down and direct scrollbar
    /// jumps, where the adjustment can move many rows before GTK has realized
    /// the new viewport.
    pub fn queue_folder_scroll_target_cached_tiles_async(
        &self,
        scroll_y: f64,
        viewport_height: f64,
        budget: usize,
    ) -> usize {
        if budget == 0 || self.group_mode.get() != GroupMode::Folder {
            return 0;
        }

        let photos = self.current_photos.borrow();
        let ranges = self.group_ranges.borrow();
        if photos.is_empty() || ranges.is_empty() {
            return 0;
        }

        let columns = self.current_columns.get().max(1) as usize;
        let tile_height = self.tile_height.get().max(1);
        let header_height = f64::from(folder_model_row_height(FolderRowKind::Header, tile_height));
        let photo_row_height =
            f64::from(folder_model_row_height(FolderRowKind::Photos, tile_height));
        let smallest_row_height = header_height.min(photo_row_height).max(1.0);
        let view_start = scroll_y.max(0.0);
        let view_end = view_start + viewport_height.max(smallest_row_height);
        // Include one photo line on either side so a page jump paints the edge
        // rows too, without wasting decode work on several speculative screens.
        let target_start = (view_start - photo_row_height).max(0.0);
        let target_end = view_end + photo_row_height;

        let mut y = 0.0_f64;
        let mut indexes = Vec::<usize>::new();

        for range in ranges.iter() {
            let photo_count = range.end.saturating_sub(range.start);
            let photo_rows = photo_count.div_ceil(columns);
            let section_height = header_height + photo_rows as f64 * photo_row_height;
            let section_end = y + section_height;

            if section_end < target_start {
                y = section_end;
                continue;
            }
            if y > target_end {
                break;
            }

            let photo_rows_y = y + header_height;
            for row in 0..photo_rows {
                let row_top = photo_rows_y + row as f64 * photo_row_height;
                let row_bottom = row_top + photo_row_height;
                if row_bottom < target_start {
                    continue;
                }
                if row_top > target_end {
                    break;
                }

                let start = range.start + row * columns;
                let end = (start + columns).min(range.end).min(photos.len());
                indexes.extend(start..end);
                if indexes.len() >= budget {
                    break;
                }
            }
            if indexes.len() >= budget {
                break;
            }
            y = section_end;
        }

        if indexes.is_empty() {
            return 0;
        }

        // Queue centre-out so the middle of the viewport becomes useful first.
        let midpoint = indexes.len() / 2;
        let mut ordered = Vec::with_capacity(indexes.len());
        for distance in 0..=indexes.len() {
            if midpoint >= distance {
                ordered.push(indexes[midpoint - distance]);
            }
            if distance != 0 && midpoint + distance < indexes.len() {
                ordered.push(indexes[midpoint + distance]);
            }
            if ordered.len() >= indexes.len() {
                break;
            }
        }

        let mut requests = Vec::with_capacity(ordered.len().min(budget));
        for index in ordered.into_iter().take(budget) {
            let Some(photo) = photos.get(index) else {
                continue;
            };
            let Some(request) = photo_presentation_request(photo, true) else {
                continue;
            };
            if folder_thumbnail_cache_get(&request.key).is_some() {
                continue;
            }
            requests.push(request);
        }
        drop(ranges);
        drop(photos);

        let queued = requests.len();
        crate::thumbnail_display::replace_visible_requests(requests);
        queued
    }

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
    fn visible_folder_photo_index_span(&self) -> Option<(usize, usize)> {
        if self.group_mode.get() != GroupMode::Folder || self.folder_root.height() <= 0 {
            return None;
        }
        let viewport = self.folder_root.height() as f32;
        let mut tiles = Vec::new();
        collect_tiles(self.folder_root.upcast_ref(), &mut tiles);
        let mut first = usize::MAX;
        let mut last = 0usize;
        let mut found = false;
        for tile in tiles {
            if tile.height() <= 0 {
                continue;
            }
            let Some(index) = tile.imp().photo_index.get() else {
                continue;
            };
            let Some(bounds) = tile.compute_bounds(&self.folder_root) else {
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

    /// Warm decoded presentation thumbnails from the photo model, not just from
    /// GTK's realized row pool. That gives a scrollbar jump several screens of
    /// cache runway even before GtkListView has created/rebound those widgets.
    pub fn prefetch_folder_cached_tiles(&self, budget: usize, direction: f64) -> usize {
        if budget == 0 || self.group_mode.get() != GroupMode::Folder {
            return 0;
        }
        let Some((first_visible, last_visible)) = self.visible_folder_photo_index_span() else {
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
        let ahead = visible_count.saturating_mul(6);
        let behind = visible_count.saturating_mul(2);
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

        if self.group_mode.get() != GroupMode::None {
            self.rebuild_group_ranges();
            if self.group_mode.get() == GroupMode::Folder {
                self.rebuild_folder_rows();
            } else {
                self.update_group_header_for_scroll(scroll_y);
            }
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
        // Folder mode keeps its own stream so re-entering it does not rebuild
        // tens of thousands of objects. Keep that inactive stream in sync as
        // well; otherwise it can retain badges from before a drive remount.
        // These are model updates only: availability was already determined
        // from registered folders above, with no original-file access.
        if self.group_mode.get() != GroupMode::Folder {
            if let Some(cache) = self.folder_cache.borrow().as_ref() {
                photos.extend(cache.photos.iter().filter_map(|photo| {
                    updates
                        .get(&photo.folder_id())
                        .is_some_and(|available| photo.original_available() != *available)
                        .then(|| photo.clone())
                }));
            }
        }
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
            // Entering Folder mode can intentionally clear the transient
            // Folder ListView while the correctly ordered stream is prepared.
            // If the DB result happens to have the same id order (for example
            // a library containing only one folder), rebuild those rows rather
            // than leaving the Folder view blank.
            if self.group_mode.get() == GroupMode::Folder && self.folder_store.n_items() == 0 {
                self.rebuild_group_ranges();
                self.rebuild_folder_rows();
            }

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
                if self.group_mode.get() == GroupMode::Folder {
                    self.rebuild_folder_rows();
                } else {
                    self.update_group_header_for_scroll(self.last_scroll_y.get());
                }
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
            if self.group_mode.get() == GroupMode::Folder {
                self.rebuild_folder_rows();
            } else {
                self.update_group_header_for_scroll(self.last_scroll_y.get());
            }
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
        let folder_store = self.folder_store.clone();
        let folder_order = self.folder_order.clone();
        let folder_catalog = self.folder_catalog.clone();
        let folder_cache = self.folder_cache.clone();
        let _folder_root = self.folder_root.clone();
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
                    if !crate::grid::folder_gridview_experiment_enabled() {
                        rebuild_folder_rows_for(
                            &current_photos,
                            &group_ranges,
                            &current_columns,
                            &folder_order,
                            &folder_catalog,
                            &folder_store,
                        );
                    }
                    if group_mode.get() == GroupMode::Folder {
                        save_folder_cache_for(
                            &folder_cache,
                            &current_photos,
                            &group_ranges,
                            &current_columns,
                            &folder_order,
                        );
                    }

                    if crate::grid::folder_gridview_experiment_enabled() {
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
                    } else {
                        group_header.set_visible(false);
                        group_title.set_text("");
                        group_count.set_text("");
                    }
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

    pub fn append_photos(&self, photos: &[Photo]) {
        if photos.is_empty() {
            return;
        }
        let objects: Vec<PhotoObject> = photos.iter().map(PhotoObject::from_photo).collect();
        self.current_photos
            .borrow_mut()
            .extend(objects.iter().cloned());
        self.store.splice(self.store.n_items(), 0, &objects);
        if self.group_mode.get() != GroupMode::None {
            self.rebuild_group_ranges();
            if self.group_mode.get() == GroupMode::Folder {
                self.rebuild_folder_rows();
            } else {
                self.update_group_header_for_scroll(self.last_scroll_y.get());
            }
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
fn folder_section_plan(
    ranges: &[GroupRange],
    catalog: &[FolderCatalogEntry],
    folder_order: &[i64],
) -> Vec<FolderSectionPlan> {
    let mut plan = Vec::new();
    let mut emitted_ranges = HashSet::new();
    let mut seen_folder_ids = HashSet::new();

    // Prefer the sidebar/tree order when available. Parent/container folders
    // can appear in that order, but only ids backed by a real photo range are
    // emitted below.
    let ordered_ids: Vec<i64> = if folder_order.is_empty() {
        catalog
            .iter()
            .filter(|folder| folder.photo_count > 0)
            .map(|folder| folder.folder_id)
            .collect()
    } else {
        folder_order.to_vec()
    };

    for folder_id in ordered_ids {
        if !seen_folder_ids.insert(folder_id) {
            continue;
        }
        for (range_index, range) in ranges.iter().enumerate() {
            if range.folder_id == folder_id {
                plan.push(FolderSectionPlan {
                    folder_id,
                    range_index: Some(range_index),
                });
                emitted_ranges.insert(range_index);
            }
        }
        // No direct range means this is only a parent/container navigation
        // node. Do not emit a blank header row for it; scroll_to_folder() will
        // resolve such a target to the first photo-bearing descendant header.
    }

    // Never hide a real photo range just because the folder catalog/order was
    // stale or incomplete. Append any unplanned ranges in their source order.
    for (range_index, range) in ranges.iter().enumerate() {
        if emitted_ranges.insert(range_index) {
            plan.push(FolderSectionPlan {
                folder_id: range.folder_id,
                range_index: Some(range_index),
            });
        }
    }

    plan
}

fn build_folder_virtual_objects(
    ranges: &[GroupRange],
    photos: &[PhotoObject],
    line_size: usize,
    catalog: &[FolderCatalogEntry],
    folder_order: &[i64],
) -> Vec<FolderRowObject> {
    let line_size = line_size.max(1);
    let plan = folder_section_plan(ranges, catalog, folder_order);
    let estimated_rows = ranges.iter().fold(plan.len(), |total, range| {
        let photos_in_range = range.end.saturating_sub(range.start);
        total + photos_in_range.div_ceil(line_size)
    });
    let mut rows = Vec::with_capacity(estimated_rows);

    for section in plan {
        if let Some(range_index) = section.range_index {
            let range = &ranges[range_index];
            rows.push(FolderRowObject::new(FolderRowData {
                kind: FolderRowKind::Header,
                folder_id: range.folder_id,
                folder_path: photos
                    .get(range.start)
                    .and_then(|photo| photo.folder_path())
                    .unwrap_or_default(),
                label: range.label.clone(),
                count: range.end.saturating_sub(range.start),
                start: range.start,
                end: range.start,
                photo_ids: Vec::new(),
            }));

            let mut start = range.start;
            while start < range.end {
                let end = (start + line_size).min(range.end);
                let photo_ids = photos
                    .get(start..end)
                    .map(|slice| slice.iter().map(|photo| photo.id()).collect())
                    .unwrap_or_default();
                rows.push(FolderRowObject::new(FolderRowData {
                    kind: FolderRowKind::Photos,
                    folder_id: range.folder_id,
                    // Photo lines need only folder/range/photo identity. Avoid
                    // cloning heading strings into thousands of rows.
                    folder_path: String::new(),
                    label: String::new(),
                    count: 0,
                    start,
                    end,
                    photo_ids,
                }));
                start = end;
            }
        }
    }

    rows
}

fn folder_virtual_row_matches(old: &FolderRowData, new: &FolderRowData) -> bool {
    old.kind == new.kind
        && old.folder_id == new.folder_id
        && old.folder_path == new.folder_path
        && old.label == new.label
        && old.count == new.count
        && old.start == new.start
        && old.end == new.end
        && old.photo_ids == new.photo_ids
}

fn save_folder_cache_for(
    cache: &Rc<RefCell<Option<FolderStreamCache>>>,
    current_photos: &Rc<RefCell<Vec<PhotoObject>>>,
    group_ranges: &Rc<RefCell<Vec<GroupRange>>>,
    current_columns: &Rc<Cell<u32>>,
    folder_order: &Rc<RefCell<Vec<i64>>>,
) {
    cache.replace(Some(FolderStreamCache {
        photos: current_photos.borrow().clone(),
        ranges: group_ranges.borrow().clone(),
        columns: current_columns.get(),
        order: folder_order.borrow().clone(),
    }));
}

fn rebuild_folder_rows_for(
    current_photos: &Rc<RefCell<Vec<PhotoObject>>>,
    group_ranges: &Rc<RefCell<Vec<GroupRange>>>,
    current_columns: &Rc<Cell<u32>>,
    folder_order: &Rc<RefCell<Vec<i64>>>,
    folder_catalog: &Rc<RefCell<Vec<FolderCatalogEntry>>>,
    folder_store: &gio::ListStore,
) {
    let ranges = group_ranges.borrow();
    let photos = current_photos.borrow();
    let old_rows = folder_store.n_items();
    let line_size = folder_chunk_size(current_columns.get());
    let catalog = folder_catalog.borrow();
    let order = folder_order.borrow();
    let new_rows = build_folder_virtual_objects(&ranges, &photos, line_size, &catalog, &order);


    let old_len = old_rows as usize;
    let new_len = new_rows.len();
    let mut prefix = 0usize;
    while prefix < old_len && prefix < new_len {
        let Some(old_row) = folder_store
            .item(prefix as u32)
            .and_downcast::<FolderRowObject>()
        else {
            break;
        };
        let old = old_row.data();
        let new = new_rows[prefix].data();
        if folder_virtual_row_matches(&old, &new) {
            prefix += 1;
            continue;
        }
        // Progressive appends change the last folder header's count. Replace
        // that one row and keep scanning so the unchanged photo rows retain
        // their GTK objects and the final splice only appends the new tail.
        if old.kind == FolderRowKind::Header
            && new.kind == FolderRowKind::Header
            && old.folder_id == new.folder_id
            && old.folder_path == new.folder_path
            && old.start == new.start
            && old.end == new.end
        {
            folder_store.splice(prefix as u32, 1, &new_rows[prefix..prefix + 1]);
            prefix += 1;
            continue;
        }
        break;
    }
    let mut suffix = 0usize;
    while suffix < old_len - prefix
        && suffix < new_len - prefix
        && folder_store
            .item((old_len - 1 - suffix) as u32)
            .and_downcast::<FolderRowObject>()
            .is_some_and(|old_row| {
                folder_virtual_row_matches(&old_row.data(), &new_rows[new_len - 1 - suffix].data())
            })
    {
        suffix += 1;
    }


    if prefix == old_len && prefix == new_len {

        return;
    }

    let removed = (old_len - prefix - suffix) as u32;
    let inserted = &new_rows[prefix..new_len - suffix];
    // Keep the ListView attached. Fixed-height photo lines give GTK a stable
    // geometry estimate, so normal ListStore splicing can reuse the realized
    // row pool without a detach/re-attach storm.
    //
    // Note: this splice is the dominant cost of opening a folder (~400 ms for
    // the ~11.7k-row 66k stream). Measurement showed the ListStore splice itself
    // is ~0-1 ms; GTK's GtkListView spends the time incorporating the new rows.
    // Detaching the model before the splice and re-attaching with set_model
    // moved the cost to the re-attach (413 ms), so it is the ListView
    // population, not the store, that is expensive. Making folder open instant
    // therefore requires reusing an already-populated folder model instead of
    // rebuilding it.
    folder_store.splice(prefix as u32, removed, inserted);

}
