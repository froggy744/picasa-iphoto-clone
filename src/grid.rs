use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Instant;

use chrono::{Local, TimeZone};

use gio::prelude::*;
use glib::subclass::prelude::*;
use gtk::prelude::*;
use gtk4 as gtk;

use crate::db::Photo;
use crate::photo_object::PhotoObject;

// EDIT THESE TWO VALUES to set your default thumbnail width and height.
// They are independent. Example: 180 x 120, 200 x 140, 220 x 160.
const DEFAULT_TILE_WIDTH: i32 = 180;
const DEFAULT_TILE_HEIGHT: i32 = 120;

// Existing +/- zoom remains enabled. Width changes by this step and height
// scales by the same factor, preserving the custom shape above.
const MIN_TILE_WIDTH: i32 = 100;
const MAX_TILE_WIDTH: i32 = 300;
const ZOOM_STEP_WIDTH: i32 = 24;

mod square_tile {
    use std::cell::{Cell, RefCell};

    use glib::subclass::prelude::*;
    use gtk::prelude::*;
    use gtk::subclass::prelude::*;
    use gtk4 as gtk;

    use crate::photo_object::PhotoObject;

    #[derive(Default)]
    pub struct SquareTile {
        pub width: Cell<i32>,
        pub height: Cell<i32>,
        pub photo: RefCell<Option<PhotoObject>>,
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
    fn new(width: i32, height: i32, child: &impl IsA<gtk::Widget>) -> Self {
        let tile: Self = glib::Object::new();
        tile.imp().width.set(width.max(1));
        tile.imp().height.set(height.max(1));
        child.as_ref().set_parent(&tile);
        tile
    }

    fn set_tile_size(&self, width: i32, height: i32) {
        self.imp().width.set(width.max(1));
        self.imp().height.set(height.max(1));
        self.queue_resize();
    }

    fn bind_photo(&self, photo: &PhotoObject) {
        self.imp().photo.replace(Some(photo.clone()));
        self.refresh_thumbnail_with_probe(false);
    }

    fn refresh_thumbnail(&self) {
        self.refresh_thumbnail_with_probe(true);
    }

    fn refresh_availability(&self) {
        let Some(photo) = self.imp().photo.borrow().clone() else {
            return;
        };
        let available = photo
            .folder_path()
            .as_deref()
            .map(crate::source::cached_source_available)
            .unwrap_or(true)
            && crate::source::cached_file_available(&photo.path());
        photo.set_original_available(available);

        if let Some(frame) = self.first_child().and_downcast::<gtk::Overlay>() {
            if let Some(badge) = frame.last_child().and_downcast::<gtk::Button>() {
                badge.set_visible(!available);
            }
        }
    }

    fn refresh_thumbnail_with_probe(&self, probe_thumbnail: bool) {
        let Some(photo) = self.imp().photo.borrow().clone() else {
            return;
        };
        let Some(frame) = self.first_child().and_downcast::<gtk::Overlay>() else {
            return;
        };
        let Some(picture) = frame.child().and_downcast::<gtk::Picture>() else {
            return;
        };
        let placeholder = picture.next_sibling().and_downcast::<gtk::Image>();
        let unavailable_badge = frame.last_child().and_downcast::<gtk::Button>();
        let cached = photo.cached_thumbnail_path();
        let thumbnail_available = if probe_thumbnail {
            // This is an explicit refresh (for example after thumbnail
            // generation or clearing the cache), not a per-bind probe.
            let available = cached
                .as_deref()
                .map(|path| std::path::Path::new(path).is_file())
                .unwrap_or(false);
            photo.set_thumbnail_available(available);
            available
        } else {
            photo.thumbnail_available()
        };
        let existing = cached.as_deref().filter(|_| thumbnail_available);

        if let Some(path) = existing {
            if let Some(rotated) = crate::photo_texture::rotated_thumbnail(path, photo.rotation()) {
                picture.set_paintable(Some(&rotated));
            } else {
                picture.set_filename(Some(path));
            }
        } else {
            picture.set_paintable(gtk::gdk::Paintable::NONE);
        }
        picture.set_tooltip_text(Some(&photo.filename()));
        if let Some(badge) = unavailable_badge {
            let unavailable = !photo.original_available();
            badge.set_visible(unavailable);
            badge.set_tooltip_text(if unavailable {
                Some("Original photo unavailable")
            } else {
                None
            });
        }
        if let Some(placeholder) = placeholder {
            placeholder.set_visible(existing.is_none());
        }
        if existing.is_some() {
            picture.remove_css_class("missing-thumbnail");
        } else {
            picture.add_css_class("missing-thumbnail");
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupMode {
    None,
    Day,
    Month,
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
}

pub struct Gallery {
    // GtkGridView must remain the direct GtkScrolledWindow child. GTK's list
    // widgets are GtkScrollable and rely on that relationship for correct
    // visible-item allocation and virtualization. Do not wrap this GridView
    // in a Box/Viewport to implement grouping.
    pub root: gtk::GridView,
    pub group_header: gtk::Box,
    group_title: gtk::Label,
    group_count: gtk::Label,
    selected: Rc<dyn Fn(Option<PhotoObject>)>,
    store: gio::ListStore,
    selection: gtk::MultiSelection,
    current_columns: Rc<Cell<u32>>,
    last_layout_width: Rc<Cell<i32>>,
    tile_width: Rc<Cell<i32>>,
    tile_height: Rc<Cell<i32>>,
    current_photos: Rc<RefCell<Vec<PhotoObject>>>,
    group_mode: Rc<Cell<GroupMode>>,
    group_date: Rc<Cell<GroupDate>>,
    group_ranges: Rc<RefCell<Vec<GroupRange>>>,
    last_scroll_y: Rc<Cell<f64>>,
    on_zoom_changed: Rc<dyn Fn(i32)>,
}

impl Gallery {
    pub fn new(
        photos: &[Photo],
        initial_tile_width: i32,
        selected: impl Fn(Option<PhotoObject>) + 'static,
        activate: impl Fn(Vec<PhotoObject>, usize) + 'static,
        context_menu: impl Fn(PhotoObject, gtk::Widget) + 'static,
        unavailable: impl Fn(PhotoObject, gtk::Widget) + 'static,
        on_zoom_changed: impl Fn(i32) + 'static,
    ) -> Self {
        let selected: Rc<dyn Fn(Option<PhotoObject>)> = Rc::new(selected);
        let activate: Rc<dyn Fn(Vec<PhotoObject>, usize)> = Rc::new(activate);
        let context_menu: Rc<dyn Fn(PhotoObject, gtk::Widget)> = Rc::new(context_menu);
        let unavailable: Rc<dyn Fn(PhotoObject, gtk::Widget)> = Rc::new(unavailable);
        let on_zoom_changed: Rc<dyn Fn(i32)> = Rc::new(on_zoom_changed);
        let store = gio::ListStore::new::<PhotoObject>();
        let selection = gtk::MultiSelection::new(Some(store.clone()));
        // Independent thumbnail width/height. Change DEFAULT_TILE_WIDTH and
        // DEFAULT_TILE_HEIGHT above to choose your preferred starting size.
        let tile_width = Rc::new(Cell::new(
            initial_tile_width.clamp(MIN_TILE_WIDTH, MAX_TILE_WIDTH),
        ));
        let tile_height = Rc::new(Cell::new(
            ((DEFAULT_TILE_HEIGHT as f64) * tile_width.get() as f64 / DEFAULT_TILE_WIDTH as f64)
                .round()
                .max(1.0) as i32,
        ));

        // Grouping is presented as a sticky heading outside the scrolled
        // GridView. This deliberately avoids nesting multiple GtkGridViews in
        // a GtkViewport, which broke row allocation and virtualization.
        let group_header = gtk::Box::new(gtk::Orientation::Horizontal, 7);
        group_header.set_hexpand(true);
        group_header.set_visible(false);
        group_header.set_margin_start(20);
        group_header.set_margin_end(20);
        group_header.set_margin_top(10);
        group_header.set_margin_bottom(4);
        group_header.add_css_class("group-heading-bar");

        let group_title = gtk::Label::new(None);
        group_title.set_xalign(0.0);
        group_title.add_css_class("section-heading");
        group_header.append(&group_title);

        let group_count = gtk::Label::new(None);
        group_count.set_xalign(0.0);
        group_count.add_css_class("dim-label");
        group_count.add_css_class("section-count");
        group_header.append(&group_count);

        let factory = gtk::SignalListItemFactory::new();
        let tile_width_for_setup = tile_width.clone();
        let tile_height_for_setup = tile_height.clone();
        let context_menu_for_setup = context_menu.clone();
        let unavailable_for_setup = unavailable.clone();
        factory.connect_setup(move |_, object| {
            let Some(list_item) = object.downcast_ref::<gtk::ListItem>() else {
                return;
            };

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

            let checkmark = gtk::Image::from_icon_name("object-select-symbolic");
            checkmark.set_pixel_size(18);
            checkmark.set_halign(gtk::Align::End);
            checkmark.set_valign(gtk::Align::Start);
            checkmark.set_margin_top(8);
            checkmark.set_margin_end(8);
            checkmark.add_css_class("selection-badge");
            frame.add_overlay(&checkmark);

            let unavailable_badge = gtk::Button::with_label("!");
            unavailable_badge.set_halign(gtk::Align::End);
            unavailable_badge.set_valign(gtk::Align::Start);
            unavailable_badge.set_margin_top(8);
            unavailable_badge.set_margin_end(8);
            unavailable_badge.add_css_class("offline-badge");
            unavailable_badge.set_tooltip_text(Some("Original photo unavailable"));
            unavailable_badge.set_visible(false);
            frame.add_overlay(&unavailable_badge);

            let list_item_for_unavailable = list_item.clone();
            let unavailable_for_click = unavailable_for_setup.clone();
            let badge_for_click = unavailable_badge.clone();
            unavailable_badge.connect_clicked(move |_| {
                if let Some(photo) = list_item_for_unavailable
                    .item()
                    .and_downcast::<PhotoObject>()
                {
                    (unavailable_for_click)(photo, badge_for_click.clone().upcast());
                }
            });

            let tile = SquareTile::new(
                tile_width_for_setup.get(),
                tile_height_for_setup.get(),
                &frame,
            );
            tile.set_hexpand(true);
            tile.set_vexpand(false);
            tile.set_valign(gtk::Align::Start);
            list_item.set_child(Some(&tile));

            let right_click = gtk::GestureClick::new();
            right_click.set_button(3);
            right_click.set_propagation_phase(gtk::PropagationPhase::Capture);
            let list_item_for_context = list_item.clone();
            let context_menu = context_menu_for_setup.clone();
            let frame_for_context = frame.clone();
            right_click.connect_pressed(move |gesture, _, _, _| {
                if let Some(photo) = list_item_for_context.item().and_downcast::<PhotoObject>() {
                    (context_menu)(photo, frame_for_context.clone().upcast());
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                }
            });
            frame.add_controller(right_click);
        });

        factory.connect_bind(|_, object| {
            let Some(list_item) = object.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let Some(photo) = list_item.item().and_downcast::<PhotoObject>() else {
                return;
            };
            let Some(tile) = list_item.child().and_downcast::<SquareTile>() else {
                return;
            };
            tile.bind_photo(&photo);
        });

        let root = gtk::GridView::new(Some(selection.clone()), Some(factory));
        root.set_min_columns(5);
        root.set_max_columns(5);
        root.set_single_click_activate(false);
        root.set_enable_rubberband(true);
        root.set_hexpand(true);
        root.set_vexpand(true);
        root.set_halign(gtk::Align::Fill);
        root.set_valign(gtk::Align::Fill);
        root.set_margin_start(20);
        root.set_margin_end(20);
        root.set_margin_top(20);
        root.set_margin_bottom(24);
        root.add_css_class("section-grid");

        let selected_for_signal = selected.clone();
        selection.connect_selection_changed(move |selection, _, _| {
            let selected = selection.selection();
            let photo = gtk::BitsetIter::init_first(&selected)
                .and_then(|(_, position)| selection.item(position))
                .and_downcast::<PhotoObject>();
            (selected_for_signal)(photo);
        });

        let current_photos = Rc::new(RefCell::new(Vec::<PhotoObject>::new()));
        let current_photos_for_activate = current_photos.clone();
        let selection_for_activate = selection.clone();
        root.connect_activate(move |_, position| {
            let Some(activated) = selection_for_activate
                .model()
                .and_then(|model| model.item(position))
                .and_downcast::<PhotoObject>()
            else {
                return;
            };
            let photos = current_photos_for_activate.borrow().clone();
            let index = photos
                .iter()
                .position(|photo| photo.id() == activated.id())
                .unwrap_or(position as usize);
            (activate)(photos, index);
        });

        let gallery = Self {
            root,
            group_header,
            group_title,
            group_count,
            selected,
            store,
            selection,
            current_columns: Rc::new(Cell::new(5)),
            last_layout_width: Rc::new(Cell::new(0)),
            tile_width,
            tile_height,
            current_photos,
            group_mode: Rc::new(Cell::new(GroupMode::None)),
            group_date: Rc::new(Cell::new(GroupDate::Taken)),
            group_ranges: Rc::new(RefCell::new(Vec::new())),
            last_scroll_y: Rc::new(Cell::new(0.0)),
            on_zoom_changed,
        };
        gallery.replace(photos);
        gallery
    }

    pub fn update_width(&self, width: i32) {
        let available = (width - 48).max(200);
        let columns = ((available as f64) / (self.tile_width.get() as f64 + 30.0))
            .floor()
            .clamp(1.0, 8.0) as u32;
        if width == self.last_layout_width.get() && columns == self.current_columns.get() {
            return;
        }
        self.last_layout_width.set(width);
        self.current_columns.set(columns);
        self.root.set_min_columns(columns);
        self.root.set_max_columns(columns);
        self.root.queue_resize();
        self.update_group_header_for_scroll(self.last_scroll_y.get());
    }

    pub fn set_grouping(&self, mode: GroupMode, date: GroupDate) {
        let old_mode = self.group_mode.replace(mode);
        let old_date = self.group_date.replace(date);
        let changed = old_mode != mode || old_date != date;
        if changed || mode != GroupMode::None {
            self.rebuild_group_ranges();
        }

        let visible = mode != GroupMode::None && !self.group_ranges.borrow().is_empty();
        self.group_header.set_visible(visible);
        if visible {
            self.update_group_header_for_scroll(self.last_scroll_y.get());
        } else {
            self.group_title.set_text("");
            self.group_count.set_text("");
        }
    }

    pub fn update_group_header_for_scroll(&self, scroll_y: f64) {
        self.last_scroll_y.set(scroll_y.max(0.0));
        if self.group_mode.get() == GroupMode::None {
            return;
        }

        // Grid item padding is 6px on each edge in window.rs. The top grid
        // margin is 20px. Using the same constants keeps the sticky heading in
        // sync with the visible row without changing the GridView's geometry.
        const ITEM_PADDING: f64 = 6.0;
        const GRID_TOP_MARGIN: f64 = 20.0;
        let tile_height = self.tile_height.get().max(1) as f64;
        let row_pitch = tile_height + ITEM_PADDING * 2.0;
        let row = ((scroll_y - GRID_TOP_MARGIN).max(0.0) / row_pitch).floor() as usize;
        let index = row.saturating_mul(self.current_columns.get().max(1) as usize);
        self.update_group_header_for_index(index);
    }

    fn rebuild_group_ranges(&self) {
        let mode = self.group_mode.get();
        let date = self.group_date.get();
        let photos = self.current_photos.borrow();

        let mut ranges = Vec::<GroupRange>::new();
        if mode != GroupMode::None {
            for (index, photo) in photos.iter().enumerate() {
                let label = group_label(photo, mode, date);
                match ranges.last_mut() {
                    Some(last) if last.label == label => last.end = index + 1,
                    _ => ranges.push(GroupRange {
                        start: index,
                        end: index + 1,
                        label,
                    }),
                }
            }
        }
        self.group_ranges.replace(ranges);
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

    pub fn zoom_in(&self) {
        self.set_zoom(self.tile_width.get() + ZOOM_STEP_WIDTH);
    }

    pub fn zoom_out(&self) {
        self.set_zoom(self.tile_width.get() - ZOOM_STEP_WIDTH);
    }

    /// Zoom is driven by width. Height scales by the same factor, preserving
    /// the custom width/height shape configured above.
    pub fn set_zoom(&self, width: i32) {
        let old_width = self.tile_width.get().max(1);
        let old_height = self.tile_height.get().max(1);
        let width = width.clamp(MIN_TILE_WIDTH, MAX_TILE_WIDTH);
        if width == old_width {
            return;
        }

        let scale = width as f64 / old_width as f64;
        let height = ((old_height as f64) * scale).round().max(1.0) as i32;

        self.tile_width.set(width);
        self.tile_height.set(height);
        (self.on_zoom_changed)(width);

        let mut tiles = Vec::new();
        collect_tiles(self.root.upcast_ref(), &mut tiles);
        for tile in tiles {
            tile.set_tile_size(width, height);
        }

        let root_width = self.root.width();
        self.last_layout_width.set(0);
        if root_width > 100 {
            self.update_width(root_width);
        } else {
            self.update_group_header_for_scroll(self.last_scroll_y.get());
        }
    }

    pub fn refresh_thumbnails(&self) {
        let mut tiles = Vec::new();
        collect_tiles(self.root.upcast_ref(), &mut tiles);
        for tile in tiles {
            tile.refresh_thumbnail();
        }
    }

    pub fn refresh_availability(&self) {
        for photo in self.current_photos.borrow().iter() {
            let available = photo
                .folder_path()
                .as_deref()
                .map(crate::source::cached_source_available)
                .unwrap_or(true)
                && crate::source::cached_file_available(&photo.path());
            photo.set_original_available(available);
        }

        let mut tiles = Vec::new();
        collect_tiles(self.root.upcast_ref(), &mut tiles);
        for tile in tiles {
            tile.refresh_availability();
        }
    }

    pub fn replace(&self, photos: &[Photo]) {
        let started = Instant::now();
        eprintln!("GRID PERF replace_start rows={}", photos.len());
        let unchanged = {
            let current = self.current_photos.borrow();
            current.len() == photos.len()
                && current
                    .iter()
                    .zip(photos)
                    .all(|(object, photo)| object.id() == photo.id)
        };
        if unchanged {
            eprintln!(
                "GRID PERF replace_unchanged rows={} elapsed_ms={}",
                photos.len(),
                started.elapsed().as_millis()
            );
            return;
        }

        (self.selected)(None);
        let object_started = Instant::now();
        let objects: Vec<PhotoObject> = photos.iter().map(PhotoObject::from_photo).collect();
        eprintln!(
            "GRID PERF photo_objects_done rows={} elapsed_ms={}",
            objects.len(),
            object_started.elapsed().as_millis()
        );
        self.current_photos.replace(objects.clone());
        let splice_started = Instant::now();
        self.store.splice(0, self.store.n_items(), &objects);
        eprintln!(
            "GRID PERF model_splice_done rows={} elapsed_ms={} total_ms={}",
            objects.len(),
            splice_started.elapsed().as_millis(),
            started.elapsed().as_millis()
        );
        if objects.is_empty() {
            self.selection.unselect_all();
        } else {
            self.selection.select_item(0, true);
        }
        if self.group_mode.get() != GroupMode::None {
            self.rebuild_group_ranges();
            self.update_group_header_for_scroll(self.last_scroll_y.get());
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
        if self.group_mode.get() != GroupMode::None {
            self.rebuild_group_ranges();
            self.update_group_header_for_scroll(self.last_scroll_y.get());
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
}

fn group_label(photo: &PhotoObject, mode: GroupMode, date: GroupDate) -> String {
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
        GroupMode::Month => value.format("%B %Y").to_string(),
        GroupMode::Day => {
            let date = value.date_naive();
            let today = Local::now().date_naive();
            if date == today {
                "Today".to_string()
            } else if date == today.pred_opt().unwrap_or(today) {
                "Yesterday".to_string()
            } else {
                value.format("%-d %B %Y").to_string()
            }
        }
    }
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
