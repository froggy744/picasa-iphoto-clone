use std::cell::{Cell, RefCell};
use std::collections::{HashSet, VecDeque};
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

const RAW_THUMBNAIL_CACHE_CAPACITY: usize = 128;

thread_local! {
    static RAW_THUMBNAIL_CACHE: RefCell<VecDeque<(String, i32, gtk::gdk::Paintable)>> =
        const { RefCell::new(VecDeque::new()) };
    static SELECTION_POSITION_CALLS: Cell<u64> = const { Cell::new(0) };
    static SCROLL_PROBE_PICK_CALLS: Cell<u64> = const { Cell::new(0) };
    static SCROLL_PROBE_PICK_NS: Cell<u128> = const { Cell::new(0) };
    static SCROLL_PROBE_SCAN_NS: Cell<u128> = const { Cell::new(0) };
    static THUMB_LOAD_COUNT: Cell<u64> = const { Cell::new(0) };
    static THUMB_LOAD_RELOADS: Cell<u64> = const { Cell::new(0) };
    static THUMB_LOAD_MAX_MS: Cell<u128> = const { Cell::new(0) };
    static THUMB_LOAD_FS_MS: Cell<u128> = const { Cell::new(0) };
    static THUMB_LOAD_APPLY_MS: Cell<u128> = const { Cell::new(0) };
    static THUMB_LOAD_SEEN: RefCell<HashSet<i64>> = RefCell::new(HashSet::new());
}

pub(crate) fn take_scroll_probe_stats() -> (u64, u128, u128) {
    SCROLL_PROBE_PICK_CALLS.with(|calls| {
        SCROLL_PROBE_PICK_NS.with(|pick| {
            SCROLL_PROBE_SCAN_NS.with(|scan| {
                let stats = (calls.get(), pick.get(), scan.get());
                calls.set(0);
                pick.set(0);
                scan.set(0);
                stats
            })
        })
    })
}

fn record_thumb_load(id: i64, total_ms: u128, fs_ms: u128, apply_ms: u128) {
    THUMB_LOAD_COUNT.with(|count| count.set(count.get().wrapping_add(1)));
    THUMB_LOAD_MAX_MS.with(|max| max.set(max.get().max(total_ms)));
    THUMB_LOAD_FS_MS.with(|sum| sum.set(sum.get().wrapping_add(fs_ms)));
    THUMB_LOAD_APPLY_MS.with(|sum| sum.set(sum.get().wrapping_add(apply_ms)));
    THUMB_LOAD_SEEN.with(|seen| {
        if !seen.borrow_mut().insert(id) {
            THUMB_LOAD_RELOADS.with(|count| count.set(count.get().wrapping_add(1)));
        }
    });
}

pub(crate) fn take_thumb_load_stats() -> (u64, u64, u128, u128, u128) {
    let loads = THUMB_LOAD_COUNT.with(|cell| cell.replace(0));
    let reloads = THUMB_LOAD_RELOADS.with(|cell| cell.replace(0));
    let max_ms = THUMB_LOAD_MAX_MS.with(|cell| cell.replace(0));
    let fs_ms = THUMB_LOAD_FS_MS.with(|cell| cell.replace(0));
    let apply_ms = THUMB_LOAD_APPLY_MS.with(|cell| cell.replace(0));
    THUMB_LOAD_SEEN.with(|seen| seen.borrow_mut().clear());
    (loads, reloads, max_ms, fs_ms, apply_ms)
}

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
        pub favorite_indicators_visible: Cell<bool>,
        pub photo: RefCell<Option<PhotoObject>>,
        pub visual_loaded: Cell<bool>,
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
        if let Some(badge) = overlay_image(&frame, "favorite-badge") {
            badge.set_visible(self.imp().favorite_indicators_visible.get() && photo.favorite());
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

    fn load_visual(&self) {
        if self.imp().visual_loaded.get() {
            return;
        }
        let trace = std::env::var_os("PICASA_TRACE").is_some();
        let load_started = trace.then(Instant::now);
        let Some(photo) = self.imp().photo.borrow().as_ref().cloned() else {
            return;
        };
        let fs_started = trace.then(Instant::now);
        // The original may live on a spun-down or disconnected drive. Stat it
        // off the GTK thread; the offline badge updates when the async probe
        // completes. Measured: a synchronous stat here froze a scroll frame for
        // 4484 ms (load_visual fs_ms=4484, scroll-baseline3.log).
        schedule_availability_probe(self, &photo);
        let mut cache_hit = false;
        let mut request_priority = false;
        if let Some(path) = photo.cached_thumbnail_path() {
            let available = std::path::Path::new(&path).is_file();
            photo.set_thumbnail_available(available);
            cache_hit = available;
            if !available {
                request_priority = true;
                if trace {
                    eprintln!(
                        "THUMB PRIORITY visible_missing thread=main id={} path={} cache={}",
                        photo.id(),
                        photo.path(),
                        path
                    );
                }
            }
        }
        let fs_ms = fs_started.map(|started| started.elapsed().as_millis()).unwrap_or(0);
        let priority_started = trace.then(Instant::now);
        if request_priority {
            crate::thumbnail::request_priority(
                photo.path(),
                Some(photo.mtime()),
                Some(photo.size_bytes()),
            );
        }
        let priority_ms = priority_started
            .map(|started| started.elapsed().as_millis())
            .unwrap_or(0);
        crate::diagnostics::visible_thumbnail(photo.thumbnail_available());
        let apply_started = trace.then(Instant::now);
        self.refresh_thumbnail_with_probe(false);
        let apply_ms = apply_started
            .map(|started| started.elapsed().as_millis())
            .unwrap_or(0);
        self.imp().visual_loaded.set(true);
        if let Some(started) = load_started {
            let total_ms = started.elapsed().as_millis();
            record_thumb_load(photo.id(), total_ms, fs_ms, apply_ms);
            eprintln!(
                "UI PERF load_visual id={} fs_ms={} priority_ms={} apply_ms={} total_ms={} cache={}",
                photo.id(),
                fs_ms,
                priority_ms,
                apply_ms,
                total_ms,
                if cache_hit { "hit" } else { "miss" }
            );
        }
    }

    fn unload_visual(&self) {
        let trace = std::env::var_os("PICASA_TRACE").is_some();
        let started = trace.then(Instant::now);
        self.imp().visual_loaded.set(false);
        if let Some(frame) = self.first_child().and_downcast::<gtk::Overlay>() {
            if let Some(picture) = frame.child().and_downcast::<gtk::Picture>() {
                picture.set_paintable(gtk::gdk::Paintable::NONE);
            }
        }
        if let Some(started) = started {
            let id = self
                .imp()
                .photo
                .borrow()
                .as_ref()
                .map(|photo| photo.id())
                .unwrap_or_default();
            eprintln!(
                "UI PERF unload_visual id={} total_ms={}",
                id,
                started.elapsed().as_millis()
            );
        }
    }

    fn bind_photo(&self, photo: &PhotoObject) {
        let trace = std::env::var_os("PICASA_TRACE").is_some();
        let started = trace.then(Instant::now);
        self.set_photo_deferred(photo);
        self.load_visual();
        if let Some(started) = started {
            let elapsed_ms = started.elapsed().as_millis();
            if elapsed_ms >= 12 {
                eprintln!(
                    "UI PERF grid_bind_slow id={} elapsed_ms={}",
                    photo.id(),
                    elapsed_ms
                );
            }
        }
    }

    fn clear_photo(&self) {
        self.unload_visual();
        self.imp().photo.take();
        if let Some(frame) = self.first_child().and_downcast::<gtk::Overlay>() {
            if let Some(picture) = frame.child().and_downcast::<gtk::Picture>() {
                picture.set_paintable(gtk::gdk::Paintable::NONE);
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
        if selected {
            frame.add_css_class("folder-photo-selected");
        } else {
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
        let favorite_badge = overlay_image(&frame, "favorite-badge");
        let edited_badge = overlay_image(&frame, "edited-badge");
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
            // RAW correction requires decoding and cropping pixels. Do not do
            // that synchronously while GtkGridView binds a tile; the cached
            // file is already a usable thumbnail and keeps startup/scrolling
            // responsive.
            if crate::image_format::uses(&photo.path(), crate::image_format::DecoderKind::Raw)
                && photo.rotation().rem_euclid(360) == 0
                && crate::edit::EditRecipe::decode(&photo.edit_recipe()).is_default()
            {
                picture.set_filename(Some(path));
            } else if let Some(cropped) = raw_cached_thumbnail(&photo, path) {
                picture.set_paintable(Some(&cropped));
            } else if let Some(rotated) =
                crate::photo_texture::edited_thumbnail(path, photo.rotation(), &photo.edit_recipe())
            {
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
        if let Some(badge) = favorite_badge {
            badge.set_visible(self.imp().favorite_indicators_visible.get() && photo.favorite());
        }
        if let Some(badge) = edited_badge {
            let edited = !crate::edit::EditRecipe::decode(&photo.edit_recipe()).is_default();
            badge.set_visible(edited);
            badge.set_tooltip_text(if edited { Some("Edited") } else { None });
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

/// Probe an original file's availability without blocking the GTK main thread.
///
/// `gio` runs the stat on a worker thread and delivers the callback back to the
/// thread-default main context. The result is stored on the shared
/// `PhotoObject`, so other tiles bound to the same photo see it immediately.
fn schedule_availability_probe(tile: &SquareTile, photo: &PhotoObject) {
    // Re-probe on rebind once the previous result is older than the TTL, so a
    // drive/file that goes offline without a mount event still updates its
    // badge. The probe itself stays off the GTK thread.
    const AVAILABILITY_REPROBE_TTL: std::time::Duration = std::time::Duration::from_secs(10);
    let now = Instant::now();
    if let Some(checked_at) = photo.original_checked_at() {
        if now.duration_since(checked_at) < AVAILABILITY_REPROBE_TTL {
            return;
        }
    }
    photo.set_original_checked_at(Some(now));
    let file = crate::source::file(&photo.path());
    let photo = photo.clone();
    let tile = tile.clone();
    file.query_info_async(
        gio::FILE_ATTRIBUTE_STANDARD_TYPE,
        gio::FileQueryInfoFlags::NONE,
        glib::Priority::DEFAULT_IDLE,
        gio::Cancellable::NONE,
        move |result| {
            photo.set_original_available(result.is_ok());
            tile.refresh_availability();
        },
    );
}

fn overlay_image(frame: &gtk::Overlay, css_class: &str) -> Option<gtk::Image> {
    let mut child = frame.first_child();
    while let Some(current) = child {
        if let Some(image) = current.downcast_ref::<gtk::Image>() {
            if image.has_css_class(css_class) {
                return Some(image.clone());
            }
        }
        child = current.next_sibling();
    }
    None
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
    let recipe = crate::edit::EditRecipe::decode(&photo.edit_recipe());
    if recipe.is_default() {
        if let Some(paintable) = raw_thumbnail_cache_get(&source_path, rotation) {
            return Some(paintable);
        }
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
    if recipe.is_default() {
        raw_thumbnail_cache_insert(source_path, rotation, paintable.clone());
    }
    Some(paintable)
}

fn raw_thumbnail_cache_get(path: &str, rotation: i32) -> Option<gtk::gdk::Paintable> {
    RAW_THUMBNAIL_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        let index = cache.iter().position(|(cached_path, cached_rotation, _)| {
            cached_path == path && *cached_rotation == rotation
        })?;
        let entry = cache.remove(index)?;
        let paintable = entry.2.clone();
        cache.push_back(entry);
        Some(paintable)
    })
}

fn raw_thumbnail_cache_insert(path: String, rotation: i32, paintable: gtk::gdk::Paintable) {
    RAW_THUMBNAIL_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        cache.retain(|(cached_path, cached_rotation, _)| {
            cached_path != &path || *cached_rotation != rotation
        });
        cache.push_back((path, rotation, paintable));
        while cache.len() > RAW_THUMBNAIL_CACHE_CAPACITY {
            cache.pop_front();
        }
    });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupMode {
    None,
    Day,
    Month,
    /// Internal grouping used by Folder mode. This is not a user-selectable
    /// Library grouping preference; it divides the continuous Picasa-style
    /// Folder stream into real, non-sticky scrolling sections.
    Folder,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum FolderRowKind {
    #[default]
    Header,
    Photos,
}

const FOLDER_PHOTO_CHUNK_SIZE: usize = 8;

/// Chunk size for a given column count: a multiple of `columns` close to
/// `FOLDER_PHOTO_CHUNK_SIZE`. Keeping chunks a whole number of rows means no
/// chunk ends in a partial line, so the Folder grid has no ragged/padded rows.
fn folder_chunk_size(columns: u32) -> usize {
    let columns = columns.max(1) as usize;
    let lines = (FOLDER_PHOTO_CHUNK_SIZE as f64 / columns as f64)
        .round()
        .max(1.0) as usize;
    columns * lines
}

#[derive(Clone, Default)]
pub(crate) struct FolderRowData {
    kind: FolderRowKind,
    folder_id: i64,
    folder_path: String,
    label: String,
    count: usize,
    photos: Vec<PhotoObject>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FolderVirtualRow {
    kind: FolderRowKind,
    start: usize,
    end: usize,
}

/// Folder mode uses a virtualized model: one lightweight header row plus photo
/// chunks sized to a whole number of rows for the current column count. A
/// column change therefore reshapes the rows (rebuilt cheaply), which keeps
/// every chunk aligned to full rows.
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
}

fn make_folder_tile(
    tile_width: i32,
    tile_height: i32,
    unavailable: &Rc<dyn Fn(PhotoObject, gtk::Widget)>,
    selection: &gtk::MultiSelection,
    current_photos: &Rc<RefCell<Vec<PhotoObject>>>,
    activate: &Rc<dyn Fn(Vec<PhotoObject>, usize)>,
    context_menu: &Rc<dyn Fn(PhotoObject, gtk::Widget, f64, f64)>,
    collage_mode: &Rc<Cell<bool>>,
    collage_ids: &Rc<RefCell<HashSet<i64>>>,
    mapped_tiles: &Rc<RefCell<Vec<SquareTile>>>,
    folder_root_holder: &Rc<RefCell<Option<gtk::ListView>>>,
    folder_rebuild_active: &Rc<Cell<bool>>,
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

    let checkmark = gtk::Image::from_icon_name("object-select-symbolic");
    checkmark.set_pixel_size(18);
    checkmark.set_halign(gtk::Align::End);
    checkmark.set_valign(gtk::Align::Start);
    checkmark.set_margin_top(8);
    checkmark.set_margin_end(8);
    checkmark.add_css_class("selection-badge");
    frame.add_overlay(&checkmark);

    let favorite_badge = gtk::Image::from_icon_name("emote-love-symbolic");
    favorite_badge.set_pixel_size(18);
    favorite_badge.set_halign(gtk::Align::End);
    favorite_badge.set_valign(gtk::Align::End);
    favorite_badge.set_margin_bottom(8);
    favorite_badge.set_margin_end(8);
    favorite_badge.add_css_class("favorite-badge");
    favorite_badge.set_visible(false);
    frame.add_overlay(&favorite_badge);

    let edited_badge = gtk::Image::from_icon_name("document-edit-symbolic");
    edited_badge.set_pixel_size(18);
    edited_badge.set_halign(gtk::Align::Start);
    edited_badge.set_valign(gtk::Align::End);
    edited_badge.set_margin_bottom(8);
    edited_badge.set_margin_start(8);
    edited_badge.add_css_class("edited-badge");
    edited_badge.set_tooltip_text(Some("Edited"));
    edited_badge.set_visible(false);
    frame.add_overlay(&edited_badge);

    let unavailable_badge = gtk::Button::with_label("!");
    unavailable_badge.set_halign(gtk::Align::Start);
    unavailable_badge.set_valign(gtk::Align::Start);
    unavailable_badge.set_margin_top(8);
    unavailable_badge.set_margin_start(8);
    unavailable_badge.add_css_class("offline-badge");
    unavailable_badge.set_tooltip_text(Some("Original photo unavailable"));
    unavailable_badge.set_visible(false);
    frame.add_overlay(&unavailable_badge);

    let tile = SquareTile::new(tile_width, tile_height, &frame);
    tile.set_hexpand(false);
    tile.set_vexpand(false);
    tile.set_valign(gtk::Align::Start);
    tile.set_focusable(true);
    tile.set_margin_top(6);
    tile.set_margin_bottom(6);
    tile.set_margin_start(6);
    tile.set_margin_end(6);

    let tile_for_unavailable = tile.clone();
    let unavailable_for_click = unavailable.clone();
    let badge_for_click = unavailable_badge.clone();
    unavailable_badge.connect_clicked(move |_| {
        let Some(photo) = tile_for_unavailable.imp().photo.borrow().as_ref().cloned() else {
            return;
        };
        (unavailable_for_click)(photo, badge_for_click.clone().upcast());
    });

    let tile_for_left = tile.clone();
    let selection_for_left = selection.clone();
    let current_photos_for_left = current_photos.clone();
    let activate_for_left = activate.clone();
    let collage_mode_for_left = collage_mode.clone();
    let collage_ids_for_left = collage_ids.clone();
    let left_click = gtk::GestureClick::new();
    left_click.set_button(1);
    left_click.set_propagation_phase(gtk::PropagationPhase::Capture);
    left_click.connect_pressed(move |gesture, n_press, _, _| {
        let Some(photo) = tile_for_left.imp().photo.borrow().as_ref().cloned() else {
            return;
        };
        let Some(position) = selection_position_for_id(&selection_for_left, photo.id()) else {
            return;
        };

        gesture.set_state(gtk::EventSequenceState::Claimed);
        tile_for_left.grab_focus();
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
                (activate_for_left)(photos, index);
            }
            return;
        }

        let modifiers = gesture.current_event_state();
        if modifiers.contains(gtk::gdk::ModifierType::CONTROL_MASK) {
            if selection_for_left.is_selected(position) {
                selection_for_left.unselect_item(position);
            } else {
                selection_for_left.select_item(position, false);
            }
        } else if modifiers.contains(gtk::gdk::ModifierType::SHIFT_MASK) {
            let selected = selection_for_left.selection();
            let anchor = gtk::BitsetIter::init_first(&selected)
                .map(|(_, position)| position)
                .unwrap_or(position);
            let start = anchor.min(position);
            let end = anchor.max(position);
            selection_for_left.unselect_all();
            for item in start..=end {
                selection_for_left.select_item(item, false);
            }
        } else {
            selection_for_left.select_item(position, true);
        }
    });
    tile.add_controller(left_click);

    let tile_for_context = tile.clone();
    let selection_for_context = selection.clone();
    let context_menu_for_tile = context_menu.clone();
    let right_click = gtk::GestureClick::new();
    right_click.set_button(3);
    right_click.set_propagation_phase(gtk::PropagationPhase::Capture);
    right_click.connect_pressed(move |gesture, _, x, y| {
        let Some(photo) = tile_for_context.imp().photo.borrow().as_ref().cloned() else {
            return;
        };
        let Some(position) = selection_position_for_id(&selection_for_context, photo.id()) else {
            return;
        };
        let Some(frame) = tile_for_context.first_child().and_downcast::<gtk::Overlay>() else {
            return;
        };
        let frame_widget = frame.clone().upcast::<gtk::Widget>();
        let local = tile_for_context
            .compute_point(
                &frame_widget,
                &gtk::graphene::Point::new(x as f32, y as f32),
            )
            .unwrap_or_else(|| gtk::graphene::Point::new(0.0, 0.0));

        gesture.set_state(gtk::EventSequenceState::Claimed);
        if !selection_for_context.is_selected(position) {
            selection_for_context.select_item(position, true);
        }
        tile_for_context.grab_focus();
        (context_menu_for_tile)(photo, frame_widget, local.x() as f64, local.y() as f64);
    });
    tile.add_controller(right_click);

    let tile_for_map = tile.clone();
    let mapped_for_map = mapped_tiles.clone();
    let root_for_map = folder_root_holder.clone();
    let rebuild_active_for_map = folder_rebuild_active.clone();
    tile.connect_map(move |_| {
        {
            let mut mapped = mapped_for_map.borrow_mut();
            if !mapped.iter().any(|candidate| candidate == &tile_for_map) {
                mapped.push(tile_for_map.clone());
            }
        }
        // During a folder-store swap GTK maps its whole offscreen pool; the
        // deferred viewport pass loads the visible tiles instead.
        if rebuild_active_for_map.get() {
            return;
        }
        if let Some(root) = root_for_map.borrow().as_ref() {
            if tile_near_folder_viewport(&tile_for_map, root) {
                tile_for_map.load_visual();
            } else {
                let tile_for_idle = tile_for_map.clone();
                let root_for_idle = root.clone();
                glib::idle_add_local_once(move || {
                    if tile_near_folder_viewport(&tile_for_idle, &root_for_idle) {
                        tile_for_idle.load_visual();
                    }
                });
            }
        }
    });
    let tile_for_unmap = tile.clone();
    let mapped_for_unmap = mapped_tiles.clone();
    tile.connect_unmap(move |_| {
        tile_for_unmap.unload_visual();
        mapped_for_unmap
            .borrow_mut()
            .retain(|candidate| candidate != &tile_for_unmap);
    });

    tile
}

pub struct Gallery {
    // GtkGridView must remain the direct GtkScrolledWindow child. GTK's list
    // widgets are GtkScrollable and rely on that relationship for correct
    // visible-item allocation and virtualization. Do not wrap this GridView
    // in a Box/Viewport to implement grouping.
    pub root: gtk::GridView,
    pub folder_root: gtk::ListView,
    pub group_header: gtk::Box,
    group_title: gtk::Label,
    group_count: gtk::Label,
    folder_store: gio::ListStore,
    folder_selection: gtk::NoSelection,
    mapped_folder_tiles: Rc<RefCell<Vec<SquareTile>>>,
    // True while rebuild_folder_rows_for is swapping the folder model. The bind
    // path then skips eager thumbnail decodes (GTK binds its whole offscreen
    // pool on a model change) and a deferred viewport pass loads the visible
    // tiles. During normal scrolling this is false so tiles load immediately.
    folder_rebuild_active: Rc<Cell<bool>>,
    folder_view_changed: Rc<RefCell<Option<Rc<dyn Fn(bool)>>>>,
    selected: Rc<dyn Fn(Option<PhotoObject>)>,
    store: gio::ListStore,
    selection: gtk::MultiSelection,
    collage_selection_mode: Rc<Cell<bool>>,
    collage_selected_ids: Rc<RefCell<HashSet<i64>>>,
    current_columns: Rc<Cell<u32>>,
    last_layout_width: Rc<Cell<i32>>,
    tile_width: Rc<Cell<i32>>,
    tile_height: Rc<Cell<i32>>,
    current_photos: Rc<RefCell<Vec<PhotoObject>>>,
    replace_generation: Rc<Cell<u64>>,
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
        context_menu: impl Fn(PhotoObject, gtk::Widget, f64, f64) + 'static,
        unavailable: impl Fn(PhotoObject, gtk::Widget) + 'static,
        on_zoom_changed: impl Fn(i32) + 'static,
    ) -> Self {
        let selected: Rc<dyn Fn(Option<PhotoObject>)> = Rc::new(selected);
        let activate: Rc<dyn Fn(Vec<PhotoObject>, usize)> = Rc::new(activate);
        let context_menu: Rc<dyn Fn(PhotoObject, gtk::Widget, f64, f64)> = Rc::new(context_menu);
        let unavailable: Rc<dyn Fn(PhotoObject, gtk::Widget)> = Rc::new(unavailable);
        let on_zoom_changed: Rc<dyn Fn(i32)> = Rc::new(on_zoom_changed);
        let store = gio::ListStore::new::<PhotoObject>();
        let selection = gtk::MultiSelection::new(Some(store.clone()));
        let collage_selection_mode = Rc::new(Cell::new(false));
        let collage_selected_ids = Rc::new(RefCell::new(HashSet::new()));
        let current_columns = Rc::new(Cell::new(5u32));
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

        let current_photos = Rc::new(RefCell::new(Vec::<PhotoObject>::new()));

        let factory = gtk::SignalListItemFactory::new();
        let tile_width_for_setup = tile_width.clone();
        let tile_height_for_setup = tile_height.clone();
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

            let favorite_badge = gtk::Image::from_icon_name("emote-love-symbolic");
            favorite_badge.set_pixel_size(18);
            favorite_badge.set_halign(gtk::Align::End);
            favorite_badge.set_valign(gtk::Align::End);
            favorite_badge.set_margin_bottom(8);
            favorite_badge.set_margin_end(8);
            favorite_badge.add_css_class("favorite-badge");
            favorite_badge.set_visible(false);
            frame.add_overlay(&favorite_badge);

            let edited_badge = gtk::Image::from_icon_name("document-edit-symbolic");
            edited_badge.set_pixel_size(18);
            edited_badge.set_halign(gtk::Align::Start);
            edited_badge.set_valign(gtk::Align::End);
            edited_badge.set_margin_bottom(8);
            edited_badge.set_margin_start(8);
            edited_badge.add_css_class("edited-badge");
            edited_badge.set_tooltip_text(Some("Edited"));
            edited_badge.set_visible(false);
            frame.add_overlay(&edited_badge);

            let unavailable_badge = gtk::Button::with_label("!");
            unavailable_badge.set_halign(gtk::Align::Start);
            unavailable_badge.set_valign(gtk::Align::Start);
            unavailable_badge.set_margin_top(8);
            unavailable_badge.set_margin_start(8);
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
        root.add_css_class("section-grid");

        // GridView children are recycled and the pointer may land on any
        // descendant of a tile. Use one stable controller on GridView, pick
        // the tile under the pointer, and only claim the event after a bound
        // photo has been identified. Select an unselected clicked photo so
        // context-menu actions and focus restoration target that thumbnail.
        // Right-clicking within an existing multi-selection preserves it.
        let right_click = gtk::GestureClick::new();
        right_click.set_button(3);
        right_click.set_propagation_phase(gtk::PropagationPhase::Capture);
        let context_menu_for_grid = context_menu.clone();
        let root_for_context = root.clone();
        let selection_for_context = selection.clone();
        right_click.connect_pressed(move |gesture, _, x, y| {
            let Some(picked) = root_for_context.pick(x, y, gtk::PickFlags::DEFAULT) else {
                return;
            };
            let Some(tile) = picked
                .ancestor(SquareTile::static_type())
                .and_downcast::<SquareTile>()
            else {
                return;
            };
            let Some(photo) = tile.imp().photo.borrow().as_ref().cloned() else {
                return;
            };
            let Some(position) = (0..selection_for_context.n_items()).find(|position| {
                selection_for_context
                    .item(*position)
                    .and_downcast::<PhotoObject>()
                    .is_some_and(|item| item.id() == photo.id())
            }) else {
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
            root_for_context.grab_focus();
            (context_menu_for_grid)(
                photo,
                frame_widget,
                local.x() as f64,
                local.y() as f64,
            );
        });
        root.add_controller(right_click);

        // Handle Add Photos clicks on the GridView itself, before the
        // built-in GridView selection controller sees them. This makes the
        // mode behave like a checklist: each click toggles one item and does
        // not collapse the other selected photos.
        let collage_click = gtk::GestureClick::new();
        collage_click.set_button(1);
        collage_click.set_propagation_phase(gtk::PropagationPhase::Capture);
        let mode = collage_selection_mode.clone();
        let selection_for_click = selection.clone();
        let collage_selected_ids_for_click = collage_selected_ids.clone();
        let root_for_click = root.clone();
        collage_click.connect_pressed(move |gesture, _, x, y| {
            if !mode.get() {
                return;
            }
            let Some(picked) = root_for_click.pick(x, y, gtk::PickFlags::DEFAULT) else {
                return;
            };
            let Some(tile) = picked
                .ancestor(SquareTile::static_type())
                .and_downcast::<SquareTile>()
            else {
                return;
            };
            let Some(photo_id) = tile.imp().photo.borrow().as_ref().map(|photo| photo.id()) else {
                return;
            };
            let Some(position) = (0..selection_for_click.n_items()).find(|position| {
                selection_for_click
                    .item(*position)
                    .and_downcast::<PhotoObject>()
                    .is_some_and(|photo| photo.id() == photo_id)
            }) else {
                return;
            };
            gesture.set_state(gtk::EventSequenceState::Claimed);
            let mut selected_ids = collage_selected_ids_for_click.borrow_mut();
            if selected_ids.remove(&photo_id) {
                selection_for_click.unselect_item(position);
            } else {
                selected_ids.insert(photo_id);
                selection_for_click.select_item(position, false);
            }
        });
        root.add_controller(collage_click);

        let selected_for_signal = selected.clone();
        selection.connect_selection_changed(move |selection, _, _| {
            let selected = selection.selection();
            let photo = gtk::BitsetIter::init_first(&selected)
                .and_then(|(_, position)| selection.item(position))
                .and_downcast::<PhotoObject>();
            (selected_for_signal)(photo);
        });

        let current_photos_for_activate = current_photos.clone();
        let selection_for_activate = selection.clone();
        let activate_for_grid = activate.clone();
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
            (activate_for_grid)(photos, index);
        });

        // Folder mode is a single virtualized ListView. Its model shape is
        // stable across zoom levels: one header row per folder plus photo
        // chunks of at most 8 items (the maximum supported column count).
        // GTK therefore realizes only viewport-near chunks instead of one
        // giant FlowBox containing every photo in a folder.
        let folder_store = gio::ListStore::new::<FolderRowObject>();
        let folder_selection = gtk::NoSelection::new(Some(folder_store.clone()));
        let folder_factory = gtk::SignalListItemFactory::new();
        let mapped_folder_tiles: Rc<RefCell<Vec<SquareTile>>> = Rc::new(RefCell::new(Vec::new()));
        let folder_rebuild_active: Rc<Cell<bool>> = Rc::new(Cell::new(false));
        let folder_root_holder: Rc<RefCell<Option<gtk::ListView>>> = Rc::new(RefCell::new(None));
        // Cached selected photo ids. The folder bind path used to call
        // selection_position_for_id (an O(n_items) scan of the 66k-photo model)
        // eight times per bound chunk, which made each row bind ~29 ms and
        // saturated the main thread during scroll.
        let folder_selected_ids: Rc<RefCell<HashSet<i64>>> = Rc::new(RefCell::new(HashSet::new()));

        let setup_columns = current_columns.clone();
        let setup_tile_height = tile_height.clone();
        folder_factory.connect_setup(move |_, object| {
            let Some(list_item) = object.downcast_ref::<gtk::ListItem>() else {
                return;
            };

            let row_root = gtk::Box::new(gtk::Orientation::Vertical, 0);
            row_root.set_hexpand(true);
            row_root.set_vexpand(false);
            row_root.add_css_class("folder-stream-row");
            // Give GTK a realistic height before bind. Without it the row
            // measures ~0 at setup, so a large scrollbar jump made the
            // ListView realize hundreds of rows (mapped=1407, 3081 ms frame).
            // connect_bind overrides this for headers and short chunks.
            row_root.set_height_request(folder_chunk_height(
                folder_chunk_size(setup_columns.get()),
                setup_columns.get().max(1),
                setup_tile_height.get(),
            ));

            let header_outer = gtk::Box::new(gtk::Orientation::Vertical, 0);
            header_outer.set_widget_name("picasa-folder-section-header");
            header_outer.set_hexpand(true);
            header_outer.set_margin_top(26);
            header_outer.set_margin_bottom(8);
            header_outer.set_margin_start(6);
            header_outer.set_margin_end(6);
            header_outer.add_css_class("folder-section-header");

            let header_line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            header_line.set_hexpand(true);
            let folder_icon = gtk::Image::from_icon_name("folder-symbolic");
            folder_icon.set_pixel_size(16);
            folder_icon.add_css_class("folder-section-icon");
            header_line.append(&folder_icon);

            let title = gtk::Label::new(None);
            title.set_widget_name("picasa-folder-section-title");
            title.set_xalign(0.0);
            title.set_ellipsize(gtk::pango::EllipsizeMode::End);
            title.add_css_class("folder-section-title");
            header_line.append(&title);

            let count = gtk::Label::new(None);
            count.set_widget_name("picasa-folder-section-count");
            count.set_xalign(0.0);
            count.add_css_class("dim-label");
            count.add_css_class("folder-section-count");
            header_line.append(&count);

            let header_spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            header_spacer.set_hexpand(true);
            header_line.append(&header_spacer);

            let actions = gtk::Box::new(gtk::Orientation::Horizontal, 4);
            actions.set_widget_name("picasa-folder-section-actions");
            actions.add_css_class("folder-section-actions");
            header_line.append(&actions);

            header_outer.append(&header_line);
            let separator = gtk::Separator::new(gtk::Orientation::Horizontal);
            separator.add_css_class("folder-section-separator");
            header_outer.append(&separator);
            row_root.append(&header_outer);

            let flow = gtk::FlowBox::new();
            flow.set_widget_name("picasa-folder-chunk-flow");
            flow.set_hexpand(true);
            flow.set_vexpand(false);
            flow.set_homogeneous(true);
            flow.set_selection_mode(gtk::SelectionMode::None);
            flow.set_row_spacing(0);
            flow.set_column_spacing(0);
            flow.set_min_children_per_line(1);
            flow.set_max_children_per_line(FOLDER_PHOTO_CHUNK_SIZE as u32);
            flow.add_css_class("folder-photo-flow");
            row_root.append(&flow);

            list_item.set_child(Some(&row_root));
        });

        let tile_width_for_folder_bind = tile_width.clone();
        let tile_height_for_folder_bind = tile_height.clone();
        let current_columns_for_folder_bind = current_columns.clone();
        let unavailable_for_folder_bind = unavailable.clone();
        let selection_for_folder_bind = selection.clone();
        let current_photos_for_folder_bind = current_photos.clone();
        let activate_for_folder_bind = activate.clone();
        let context_menu_for_folder_bind = context_menu.clone();
        let collage_mode_for_folder_bind = collage_selection_mode.clone();
        let collage_ids_for_folder_bind = collage_selected_ids.clone();
        let mapped_tiles_for_folder_bind = mapped_folder_tiles.clone();
        let root_holder_for_folder_bind = folder_root_holder.clone();
        let selected_ids_for_folder_bind = folder_selected_ids.clone();
        let rebuild_active_for_folder_bind = folder_rebuild_active.clone();

        folder_factory.connect_bind(move |_, object| {
            let trace = std::env::var_os("PICASA_TRACE").is_some();
            let bind_started = trace.then(Instant::now);
            let Some(list_item) = object.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let Some(row) = list_item.item().and_downcast::<FolderRowObject>() else {
                return;
            };
            let Some(row_root) = list_item.child().and_downcast::<gtk::Box>() else {
                return;
            };
            let data = row.data();

            row_root.set_widget_name(&format!("picasa-folder-row-{}", data.folder_id));
            let Some(header) = row_root.first_child().and_downcast::<gtk::Box>() else {
                return;
            };
            let Some(flow) = row_root.last_child().and_downcast::<gtk::FlowBox>() else {
                return;
            };

            match data.kind {
                FolderRowKind::Header => {
                    row_root.set_height_request(58);
                    header.set_visible(true);
                    flow.set_visible(false);
                    if let Some(title) = find_named_label(header.upcast_ref(), "picasa-folder-section-title") {
                        title.set_text(&data.label);
                        title.set_tooltip_text(Some(&data.folder_path));
                    }
                    if let Some(count) = find_named_label(header.upcast_ref(), "picasa-folder-section-count") {
                        count.set_text(&format!(
                            "{} {}",
                            format_count(data.count),
                            if data.count == 1 { "photo" } else { "photos" }
                        ));
                    }
                    if list_item.position() == 0 {
                        header.add_css_class("first-folder-section-header");
                        header.set_margin_top(10);
                    } else {
                        header.remove_css_class("first-folder-section-header");
                        header.set_margin_top(26);
                    }
                }
                FolderRowKind::Photos => {
                    header.set_visible(false);
                    flow.set_visible(true);
                    flow.set_max_children_per_line(current_columns_for_folder_bind.get().max(1));

                    // A recycled ListItem keeps a fixed pool of at most eight
                    // tile widgets. Rebinding changes only the PhotoObject; it
                    // does not rebuild overlays, gestures, or badges. Create
                    // only as many as this chunk needs so small folders (many
                    // of the 497) do not build seven unused tiles each.
                    let mut tiles = flow_box_tiles(&flow);
                    while tiles.len() < data.photos.len() {
                        let tile = make_folder_tile(
                            tile_width_for_folder_bind.get(),
                            tile_height_for_folder_bind.get(),
                            &unavailable_for_folder_bind,
                            &selection_for_folder_bind,
                            &current_photos_for_folder_bind,
                            &activate_for_folder_bind,
                            &context_menu_for_folder_bind,
                            &collage_mode_for_folder_bind,
                            &collage_ids_for_folder_bind,
                            &mapped_tiles_for_folder_bind,
                            &root_holder_for_folder_bind,
                            &rebuild_active_for_folder_bind,
                        );
                        flow.insert(&tile, -1);
                        tiles.push(tile);
                    }

                    for (slot, tile) in tiles.iter().enumerate() {
                        if let Some(photo) = data.photos.get(slot) {
                            tile.set_visible(true);
                            tile.set_tile_size(
                                tile_width_for_folder_bind.get(),
                                tile_height_for_folder_bind.get(),
                            );
                            tile.set_photo_deferred(photo);
                            let selected = selected_ids_for_folder_bind
                                .borrow()
                                .contains(&photo.id());
                            tile.set_manual_selected(selected);
                            if !rebuild_active_for_folder_bind.get() && tile.is_mapped() {
                                if let Some(root) = root_holder_for_folder_bind.borrow().as_ref() {
                                    if tile_near_folder_viewport(tile, root) {
                                        tile.load_visual();
                                    }
                                }
                            }
                        } else {
                            tile.clear_photo();
                            tile.set_visible(false);
                        }
                    }
                    row_root.set_height_request(folder_chunk_height(
                        data.photos.len(),
                        current_columns_for_folder_bind.get().max(1),
                        tile_height_for_folder_bind.get(),
                    ));
                }
            }

            if trace {
                let elapsed_ms = bind_started
                    .map(|started| started.elapsed().as_millis())
                    .unwrap_or(0);
                if elapsed_ms >= 12 {
                    eprintln!(
                        "UI PERF folder_virtual_bind_slow row={} kind={:?} folder_id={} photos={} elapsed_ms={}",
                        list_item.position(),
                        data.kind,
                        data.folder_id,
                        data.photos.len(),
                        elapsed_ms
                    );
                }
            }
        });

        folder_factory.connect_unbind(move |_, object| {
            let Some(list_item) = object.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let Some(row_root) = list_item.child().and_downcast::<gtk::Box>() else {
                return;
            };
            let Some(flow) = row_root.last_child().and_downcast::<gtk::FlowBox>() else {
                return;
            };
            for tile in flow_box_tiles(&flow) {
                tile.clear_photo();
                tile.set_visible(false);
            }
        });

        let folder_root = gtk::ListView::new(Some(folder_selection.clone()), Some(folder_factory));
        folder_root.set_single_click_activate(false);
        folder_root.set_show_separators(false);
        folder_root.set_hexpand(true);
        folder_root.set_vexpand(true);
        folder_root.set_halign(gtk::Align::Fill);
        folder_root.set_valign(gtk::Align::Fill);
        folder_root.add_css_class("folder-stream");
        folder_root.add_css_class("photo-grid");
        folder_root_holder.replace(Some(folder_root.clone()));

        let folder_root_for_selection = folder_root.clone();
        let selection_for_folder_style = selection.clone();
        let selected_ids_for_selection = folder_selected_ids.clone();
        let styles_refresh_scheduled = Rc::new(Cell::new(false));
        selection.connect_selection_changed(move |selection, _, _| {
            // Keep the bind-path cache current. GtkMultiSelection emits
            // selection-changed both when the model gains items and when
            // select_item runs, so one logical change can fire this twice.
            selected_ids_for_selection.replace(selected_photo_id_set(selection));
            // Coalesce the burst into a single idle refresh.
            if styles_refresh_scheduled.replace(true) {
                return;
            }
            let root = folder_root_for_selection.clone();
            let selection = selection_for_folder_style.clone();
            let scheduled = styles_refresh_scheduled.clone();
            glib::idle_add_local_once(move || {
                scheduled.set(false);
                refresh_folder_selection_styles(&root, &selection);
            });
        });

        let gallery = Self {
            root,
            folder_root,
            group_header,
            group_title,
            group_count,
            folder_store,
            folder_selection,
            mapped_folder_tiles,
            folder_rebuild_active,
            folder_view_changed: Rc::new(RefCell::new(None)),
            selected,
            store,
            selection,
            collage_selection_mode,
            collage_selected_ids,
            current_columns,
            last_layout_width: Rc::new(Cell::new(0)),
            tile_width,
            tile_height,
            current_photos,
            replace_generation: Rc::new(Cell::new(0)),
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
        let trace = std::env::var_os("PICASA_TRACE").is_some();
        let started = trace.then(Instant::now);
        let old_columns = self.current_columns.get();
        let old_width = self.last_layout_width.get();
        let available = (width - 48).max(200);
        let columns = ((available as f64) / (self.tile_width.get() as f64 + 30.0))
            .floor()
            .clamp(1.0, 8.0) as u32;
        if width == old_width && columns == old_columns {
            return;
        }
        if trace {
            eprintln!(
                "UI PERF update_width_begin mode={:?} width={} old_layout_width={} tile={}x{} old_columns={} new_columns={} photos={} folder_ranges={} folder_rows={}",
                self.group_mode.get(),
                width,
                old_width,
                self.tile_width.get(),
                self.tile_height.get(),
                old_columns,
                columns,
                self.current_photos.borrow().len(),
                self.group_ranges.borrow().len(),
                self.folder_store.n_items()
            );
        }
        self.last_layout_width.set(width);
        let folder_mode = self.group_mode.get() == GroupMode::Folder;
        if columns == old_columns {
            // Zooming within the same column count only changes tile geometry.
            // Replacing the Folder ListStore here used to invalidate every
            // realized row and cost ~0.8-1.1s for a 4.5k-photo library.
            if folder_mode {
                let mut flows = Vec::new();
                collect_folder_flows(self.folder_root.upcast_ref(), &mut flows);
                for flow in flows {
                    update_folder_flow_layout(&flow, columns.max(1), self.tile_height.get());
                }
                self.folder_root.queue_resize();
                self.refresh_folder_viewport_tiles();
            } else {
                self.root.queue_resize();
                self.update_group_header_for_scroll(self.last_scroll_y.get());
            }
            if trace {
                eprintln!(
                    "UI PERF update_width_skip_reflow mode={:?} width={} columns={} reason=columns_unchanged",
                    self.group_mode.get(),
                    width,
                    columns
                );
            }
            return;
        }

        self.current_columns.set(columns);
        self.root.set_min_columns(columns);
        self.root.set_max_columns(columns);
        self.root.queue_resize();
        if folder_mode {
            // The chunk size is a multiple of the column count, so a column
            // change reshapes the rows. Rebuild once (fast now) so every chunk
            // is a whole number of full rows, keeping the photo at the top.
            let anchor = self
                .photo_for_scroll_position(self.last_scroll_y.get())
                .map(|photo| photo.id());
            let before = self
                .folder_root
                .vadjustment()
                .map(|adjustment| adjustment.value())
                .unwrap_or(0.0);
            self.rebuild_folder_rows();
            if let Some(anchor) = anchor {
                self.scroll_folder_to_photo(anchor);
            }
            if trace {
                eprintln!(
                    "UI PERF folder_zoom_anchor id={:?} before={:.0}",
                    anchor, before
                );
            }
            self.refresh_folder_viewport_tiles();
        } else {
            self.update_group_header_for_scroll(self.last_scroll_y.get());
        }
        if trace {
            eprintln!(
                "UI PERF update_width_end mode={:?} width={} columns={} rebuild_ms=0 total_ms={} folder_rows={} strategy={}",
                self.group_mode.get(),
                width,
                columns,
                started.map(|value| value.elapsed().as_millis()).unwrap_or(0),
                self.folder_store.n_items(),
                if folder_mode { "virtual_chunks_rebuilt" } else { "grid_columns" }
            );
        }
    }

    /// Scroll the Folder ListView so the row holding `photo_id` sits at the
    /// top edge, without changing the selection. Runs on idle so it applies
    /// after the model swap's layout, and scrolls past the target first so the
    /// follow-up aligns to the leading edge rather than the nearest edge.
    fn scroll_folder_to_photo(&self, photo_id: i64) {
        let Some(row) = self.folder_row_index_for_photo(photo_id) else {
            return;
        };
        let root = self.folder_root.clone();
        let tile_height = self.tile_height.get();
        let total = self.folder_store.n_items();
        let trace = std::env::var_os("PICASA_TRACE").is_some();
        glib::idle_add_local_once(move || {
            let page = root
                .vadjustment()
                .map(|adjustment| adjustment.page_size())
                .unwrap_or(0.0);
            let row_pitch = (tile_height.max(1) + 12) as f64;
            let overshoot_rows = ((page / row_pitch).ceil() as u32).saturating_add(2);
            let overshoot = row
                .saturating_add(overshoot_rows)
                .min(total.saturating_sub(1));
            if overshoot > row {
                root.scroll_to(overshoot, gtk::ListScrollFlags::NONE, None);
            }
            root.scroll_to(row, gtk::ListScrollFlags::NONE, None);
            if trace {
                let after = root
                    .vadjustment()
                    .map(|adjustment| adjustment.value())
                    .unwrap_or(0.0);
                eprintln!("UI PERF folder_zoom_anchor row={row} after={after:.0}");
            }
        });
    }

    pub fn set_grouping(&self, mode: GroupMode, date: GroupDate) {
        let old_mode = self.group_mode.replace(mode);
        let old_date = self.group_date.replace(date);
        let changed = old_mode != mode || old_date != date;
        if changed || mode != GroupMode::None {
            self.rebuild_group_ranges();
        }

        if mode == GroupMode::Folder {
            // Folder headers live inside the scrolling ListView. The old
            // external heading would be sticky, which is deliberately not
            // Picasa-style.
            self.group_header.set_visible(false);
            self.group_title.set_text("");
            self.group_count.set_text("");
            if old_mode != GroupMode::Folder {
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

        if let Some(handler) = self.folder_view_changed.borrow().as_ref() {
            handler(mode == GroupMode::Folder);
        }
    }

    pub fn set_folder_view_changed_handler(&self, handler: impl Fn(bool) + 'static) {
        let handler: Rc<dyn Fn(bool)> = Rc::new(handler);
        handler(self.group_mode.get() == GroupMode::Folder);
        self.folder_view_changed.replace(Some(handler));
    }

    pub fn update_group_header_for_scroll(&self, scroll_y: f64) {
        self.last_scroll_y.set(scroll_y.max(0.0));
        let mode = self.group_mode.get();
        if mode == GroupMode::Folder {
            self.refresh_folder_viewport_tiles();
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
        if self.group_mode.get() == GroupMode::Folder {
            return self.photo_for_visible_folder_row();
        }
        self.current_photos
            .borrow()
            .get(self.index_for_scroll_position(scroll_y))
            .cloned()
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
    }

    fn rebuild_folder_rows(&self) {
        rebuild_folder_rows_for(
            &self.current_photos,
            &self.group_ranges,
            &self.current_columns,
            &self.folder_store,
            &self.folder_selection,
            &self.folder_root,
            &self.mapped_folder_tiles,
            &self.folder_rebuild_active,
        );
    }

    /// Reorder the Folder stream to match a new sidebar tree mode without
    /// re-querying the database or rebuilding every PhotoObject. Photos keep
    /// their within-folder order; only whole folder blocks move.
    pub fn reorder_folder_stream(&self, folder_order: &[i64]) {
        // Splicing the whole store clears GtkMultiSelection, so remember which
        // photos were selected and re-select them at their new positions.
        let selected_ids = selected_photo_id_set(&self.selection);
        let photos = self.current_photos.borrow().clone();
        let mut buckets: std::collections::HashMap<i64, Vec<PhotoObject>> =
            std::collections::HashMap::new();
        let mut seen: Vec<i64> = Vec::new();
        for photo in photos.iter() {
            let folder_id = photo.folder_id();
            if !buckets.contains_key(&folder_id) {
                seen.push(folder_id);
                buckets.insert(folder_id, Vec::new());
            }
            if let Some(bucket) = buckets.get_mut(&folder_id) {
                bucket.push(photo.clone());
            }
        }

        let mut reordered = Vec::with_capacity(photos.len());
        for folder_id in folder_order {
            if let Some(mut bucket) = buckets.remove(folder_id) {
                reordered.append(&mut bucket);
            }
        }
        // Folders missing from the computed order (legacy/corrupt links) keep
        // their previous relative order at the end.
        for folder_id in seen {
            if let Some(mut bucket) = buckets.remove(&folder_id) {
                reordered.append(&mut bucket);
            }
        }

        self.current_photos.replace(reordered.clone());
        self.store.splice(0, self.store.n_items(), &reordered);
        if !selected_ids.is_empty() {
            let positions = self
                .current_photos
                .borrow()
                .iter()
                .enumerate()
                .filter(|(_, photo)| selected_ids.contains(&photo.id()))
                .map(|(position, _)| position as u32)
                .collect::<Vec<_>>();
            for position in positions {
                self.selection.select_item(position, false);
            }
        }
        self.rebuild_group_ranges();
        if self.group_mode.get() == GroupMode::Folder {
            self.rebuild_folder_rows();
        }
    }

    fn refresh_folder_viewport_tiles(&self) {
        refresh_folder_viewport_tiles_for(&self.folder_root, &self.mapped_folder_tiles);
    }

    fn photo_for_visible_folder_row(&self) -> Option<PhotoObject> {
        let trace = std::env::var_os("PICASA_TRACE").is_some();
        let width = self.folder_root.width().max(1) as f64;
        let mut folder_fallback: Option<PhotoObject> = None;
        for y in [6.0_f64, 20.0, 40.0, 64.0, 92.0, 120.0] {
            let pick_started = trace.then(Instant::now);
            let picked = self
                .folder_root
                .pick(width * 0.5, y, gtk::PickFlags::DEFAULT);
            if let Some(started) = pick_started {
                SCROLL_PROBE_PICK_CALLS.with(|calls| calls.set(calls.get().wrapping_add(1)));
                SCROLL_PROBE_PICK_NS
                    .with(|ns| ns.set(ns.get().wrapping_add(started.elapsed().as_nanos())));
            }
            let Some(picked) = picked else {
                continue;
            };
            // Prefer the photo tile actually under the probe point. Callers
            // that need the visible photo (zoom anchoring) must not jump to the
            // folder's first photo.
            if let Some(tile) = tile_ancestor(&picked) {
                if let Some(photo) = tile.imp().photo.borrow().clone() {
                    return Some(photo);
                }
            }
            if folder_fallback.is_none() {
                if let Some(folder_id) = folder_id_from_named_ancestor(&picked) {
                    let scan_started = trace.then(Instant::now);
                    folder_fallback = {
                        let ranges = self.group_ranges.borrow();
                        let photos = self.current_photos.borrow();
                        ranges
                            .iter()
                            .find(|range| range.folder_id == folder_id)
                            .and_then(|range| photos.get(range.start))
                            .cloned()
                    };
                    if let Some(started) = scan_started {
                        SCROLL_PROBE_SCAN_NS
                            .with(|ns| ns.set(ns.get().wrapping_add(started.elapsed().as_nanos())));
                    }
                }
            }
        }
        // During a resize/rebind there may briefly be no realized row at the
        // probe point. Keep the existing sidebar location instead of falsely
        // jumping it back to the first folder.
        folder_fallback
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
        let trace = std::env::var_os("PICASA_TRACE").is_some();
        let zoom_started = trace.then(Instant::now);
        let old_width = self.tile_width.get().max(1);
        let old_height = self.tile_height.get().max(1);
        let width = width.clamp(MIN_TILE_WIDTH, MAX_TILE_WIDTH);
        if width == old_width {
            return;
        }

        let scale = width as f64 / old_width as f64;
        let height = ((old_height as f64) * scale).round().max(1.0) as i32;
        if trace {
            eprintln!(
                "UI PERF folder_zoom_begin mode={:?} old_tile={}x{} new_tile={}x{} photos={} folders={} columns={} folder_rows={}",
                self.group_mode.get(),
                old_width,
                old_height,
                width,
                height,
                self.current_photos.borrow().len(),
                self.group_ranges.borrow().len(),
                self.current_columns.get(),
                self.folder_store.n_items()
            );
        }

        self.tile_width.set(width);
        self.tile_height.set(height);
        let callback_started = trace.then(Instant::now);
        (self.on_zoom_changed)(width);
        if trace {
            eprintln!(
                "UI PERF folder_zoom_setting_callback ms={}",
                callback_started.map(|value| value.elapsed().as_millis()).unwrap_or(0)
            );
        }

        let collect_started = trace.then(Instant::now);
        let mut tiles = Vec::new();
        collect_tiles(self.root.upcast_ref(), &mut tiles);
        let grid_tiles = tiles.len();
        if self.group_mode.get() == GroupMode::Folder {
            let mut mapped = self.mapped_folder_tiles.borrow_mut();
            mapped.retain(|tile| tile.is_mapped());
            tiles.extend(mapped.iter().filter(|tile| tile_near_folder_viewport(tile, &self.folder_root)).cloned());
        } else {
            collect_tiles(self.folder_root.upcast_ref(), &mut tiles);
        }
        let total_tiles = tiles.len();
        if trace {
            eprintln!(
                "UI PERF folder_zoom_collect_tiles grid={} folder={} total={} ms={}",
                grid_tiles,
                total_tiles.saturating_sub(grid_tiles),
                total_tiles,
                collect_started.map(|value| value.elapsed().as_millis()).unwrap_or(0)
            );
        }
        let resize_started = trace.then(Instant::now);
        for tile in tiles {
            tile.set_tile_size(width, height);
        }
        if trace {
            eprintln!(
                "UI PERF folder_zoom_resize_tiles count={} ms={}",
                total_tiles,
                resize_started.map(|value| value.elapsed().as_millis()).unwrap_or(0)
            );
        }

        let root_width = if self.group_mode.get() == GroupMode::Folder {
            self.folder_root.width()
        } else {
            self.root.width()
        };
        self.last_layout_width.set(0);
        let layout_started = trace.then(Instant::now);
        if root_width > 100 {
            self.update_width(root_width);
        } else {
            self.update_group_header_for_scroll(self.last_scroll_y.get());
        }
        if trace {
            eprintln!(
                "UI PERF folder_zoom_end root_width={} columns={} folder_rows={} layout_ms={} total_ms={}",
                root_width,
                self.current_columns.get(),
                self.folder_store.n_items(),
                layout_started.map(|value| value.elapsed().as_millis()).unwrap_or(0),
                zoom_started.map(|value| value.elapsed().as_millis()).unwrap_or(0)
            );
        }
    }

    pub fn refresh_thumbnails(&self) {
        let trace = std::env::var_os("PICASA_TRACE").is_some();
        let started = trace.then(Instant::now);
        let mut tiles = Vec::new();
        collect_tiles(self.root.upcast_ref(), &mut tiles);
        collect_tiles(self.folder_root.upcast_ref(), &mut tiles);
        let count = tiles.len();
        for tile in tiles {
            tile.refresh_thumbnail();
        }
        if let Some(started) = started {
            eprintln!("UI PERF refresh_thumbnails tiles={} ms={}", count, started.elapsed().as_millis());
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
        let updates = self
            .current_photos
            .borrow()
            .iter()
            .map(|photo| {
                (
                    photo.id(),
                    crate::source::cached_file_available(&photo.path()),
                )
            })
            .collect::<Vec<_>>();
        self.apply_availability(&updates);
    }

    pub fn availability_snapshot(&self) -> Vec<(i64, String)> {
        self.current_photos
            .borrow()
            .iter()
            .map(|photo| (photo.id(), photo.path()))
            .collect()
    }

    pub fn apply_availability(&self, updates: &[(i64, bool)]) {
        for (id, available) in updates {
            if let Some(photo) = self
                .current_photos
                .borrow()
                .iter()
                .find(|photo| photo.id() == *id)
            {
                photo.set_original_available(*available);
                photo.set_original_checked_at(Some(Instant::now()));
            }
        }

        let mut tiles = Vec::new();
        collect_tiles(self.root.upcast_ref(), &mut tiles);
        collect_tiles(self.folder_root.upcast_ref(), &mut tiles);
        for tile in tiles {
            tile.refresh_availability();
        }
    }

    pub fn replace(&self, photos: &[Photo]) {
        let trace = std::env::var_os("PICASA_TRACE").is_some();
        let replace_started = trace.then(Instant::now);
        let profile_started = crate::diagnostics::refresh_started(photos.len());
        let generation = self.replace_generation.get().wrapping_add(1);
        self.replace_generation.set(generation);
        let unchanged = {
            let current = self.current_photos.borrow();
            current.len() == photos.len()
                && current
                    .iter()
                    .zip(photos)
                    .all(|(object, photo)| object.id() == photo.id)
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
            if let Some(started) = replace_started {
                eprintln!("UI PERF gallery_replace photos={} unchanged=true ms={}", photos.len(), started.elapsed().as_millis());
            }
            return;
        }

        // Same photo set in a different order (for example Folder <-> All
        // Photos): reorder the existing PhotoObjects instead of reconstructing
        // tens of thousands of them. Measured 658-1363 ms to rebuild all 66k.
        let same_set = {
            let current = self.current_photos.borrow();
            current.len() == photos.len()
                && !current.is_empty()
                && {
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
                .filter_map(|photo| by_id.remove(&photo.id))
                .collect::<Vec<_>>();
            if !self.collage_selection_mode.get() {
                (self.selected)(None);
            }
            crate::diagnostics::refresh_first_batch(profile_started, reordered.len());
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
            if let Some(started) = replace_started {
                eprintln!(
                    "UI PERF gallery_replace photos={} same_set_reorder ms={}",
                    reordered.len(),
                    started.elapsed().as_millis()
                );
            }
            crate::diagnostics::refresh_finished(profile_started, reordered.len());
            return;
        }

        // Constructing tens of thousands of GObjects synchronously blocks
        // GTK for several seconds. Keep the existing model semantics for
        // normal refreshes, but let the main loop make progress between small
        // batches for library-sized replacements.
        const PROGRESSIVE_REPLACE_THRESHOLD: usize = 1_000;
        if photos.len() > PROGRESSIVE_REPLACE_THRESHOLD {
            if let Some(started) = replace_started {
                eprintln!("UI PERF gallery_replace photos={} progressive=true ms={}", photos.len(), started.elapsed().as_millis());
            }
            self.replace_progressive(photos.to_vec(), generation, profile_started);
            return;
        }

        if !self.collage_selection_mode.get() {
            (self.selected)(None);
        }
        let objects: Vec<PhotoObject> = photos.iter().map(PhotoObject::from_photo).collect();
        crate::diagnostics::refresh_first_batch(profile_started, objects.len());
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
        if let Some(started) = replace_started {
            eprintln!("UI PERF gallery_replace photos={} unchanged=false ms={}", objects.len(), started.elapsed().as_millis());
        }
        crate::diagnostics::refresh_finished(profile_started, objects.len());
    }

    fn replace_progressive(
        &self,
        photos: Vec<Photo>,
        generation: u64,
        profile_started: Option<Instant>,
    ) {
        const BATCH_SIZE: usize = 500;

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
        let folder_selection = self.folder_selection.clone();
        let folder_root = self.folder_root.clone();
        let mapped_folder_tiles = self.mapped_folder_tiles.clone();
        let folder_rebuild_active = self.folder_rebuild_active.clone();
        let replace_generation = self.replace_generation.clone();

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
                crate::diagnostics::refresh_first_batch(profile_started, objects.len());
            } else {
                current_photos.borrow_mut().extend(objects.iter().cloned());
                store.splice(store.n_items(), 0, &objects);
            }

            if end >= photos.len() {
                rebuild_group_ranges_for(&current_photos, &group_mode, &group_date, &group_ranges);
                if group_mode.get() == GroupMode::Folder {
                    rebuild_folder_rows_for(
                        &current_photos,
                        &group_ranges,
                        &current_columns,
                        &folder_store,
                        &folder_selection,
                        &folder_root,
                        &mapped_folder_tiles,
                        &folder_rebuild_active,
                    );
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
                crate::diagnostics::refresh_finished(
                    profile_started,
                    current_photos.borrow().len(),
                );
                glib::ControlFlow::Break
            }
        });
    }

    pub fn append_photos(&self, photos: &[Photo]) {
        if photos.is_empty() {
            return;
        }
        let trace = std::env::var_os("PICASA_TRACE").is_some();
        let append_started = trace.then(Instant::now);
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
        if let Some(started) = append_started {
            eprintln!("UI PERF gallery_append photos={} total={} ms={}", photos.len(), self.current_photos.borrow().len(), started.elapsed().as_millis());
        }
    }

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
                    let upper = (adjustment.upper() - adjustment.page_size())
                        .max(adjustment.lower());
                    adjustment.set_value(scroll_y.clamp(adjustment.lower(), upper));
                }
            } else {
                root.scroll_to(
                    position as u32,
                    gtk::ListScrollFlags::SELECT | gtk::ListScrollFlags::FOCUS,
                    Some(scroll),
                );
                if let Some(adjustment) = root.vadjustment() {
                    let upper = (adjustment.upper() - adjustment.page_size())
                        .max(adjustment.lower());
                    adjustment.set_value(scroll_y.clamp(adjustment.lower(), upper));
                }
            }
        });
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
                    let upper = (adjustment.upper() - adjustment.page_size())
                        .max(adjustment.lower());
                    adjustment.set_value(scroll_y.clamp(adjustment.lower(), upper));
                }
            } else {
                root.scroll_to(position as u32, gtk::ListScrollFlags::FOCUS, Some(scroll));
                if let Some(adjustment) = root.vadjustment() {
                    let upper = (adjustment.upper() - adjustment.page_size())
                        .max(adjustment.lower());
                    adjustment.set_value(scroll_y.clamp(adjustment.lower(), upper));
                }
            }
        });
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
            let data = row.data();
            if data.photos.iter().any(|photo| photo.id() == photo_id) {
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
            self.selection.select_item(position, true);
            if self.group_mode.get() == GroupMode::Folder {
                if let Some(row) = self.folder_row_index_for_photo(photo_id) {
                    self.folder_root
                        .scroll_to(row, gtk::ListScrollFlags::FOCUS, None);
                    self.folder_root.grab_focus();
                }
            } else {
                self.root.scroll_to(
                    position,
                    gtk::ListScrollFlags::SELECT | gtk::ListScrollFlags::FOCUS,
                    None,
                );
            }
            return true;
        }
        false
    }

    /// Scroll the continuous Folder stream to the folder's real header row
    /// without rebuilding the gallery. Imported-root rows may not own photos
    /// directly, so the first descendant folder header is a valid target.
    pub fn scroll_to_folder(&self, folder_id: i64, folder_path: &str) -> bool {
        let target_path = std::path::Path::new(folder_path);
        let Some(photo_position) = self
            .current_photos
            .borrow()
            .iter()
            .position(|photo| {
                if photo.folder_id() == folder_id {
                    return true;
                }
                photo
                    .folder_path()
                    .as_deref()
                    .is_some_and(|path| std::path::Path::new(path).starts_with(target_path))
            })
        else {
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

fn build_folder_virtual_objects(
    ranges: &[GroupRange],
    photos: &[PhotoObject],
    chunk_size: usize,
) -> Vec<FolderRowObject> {
    let plans = folder_virtual_rows(ranges, chunk_size);
    let mut rows = Vec::with_capacity(plans.len());
    let mut range_index = 0usize;

    for plan in plans {
        while range_index + 1 < ranges.len() && plan.start >= ranges[range_index].end {
            range_index += 1;
        }
        let Some(range) = ranges.get(range_index) else {
            break;
        };
        let folder_path = photos
            .get(range.start)
            .and_then(|photo| photo.folder_path())
            .unwrap_or_default();
        let row_photos = if plan.kind == FolderRowKind::Photos {
            photos[plan.start..plan.end].to_vec()
        } else {
            Vec::new()
        };
        rows.push(FolderRowObject::new(FolderRowData {
            kind: plan.kind,
            folder_id: range.folder_id,
            folder_path,
            label: range.label.clone(),
            count: range.end.saturating_sub(range.start),
            photos: row_photos,
        }));
    }
    rows
}

fn folder_virtual_row_matches(old: &FolderRowData, new: &FolderRowData) -> bool {
    old.kind == new.kind
        && old.folder_id == new.folder_id
        && old.folder_path == new.folder_path
        && old.label == new.label
        && old.count == new.count
        && old.photos.len() == new.photos.len()
        && old
            .photos
            .iter()
            .zip(&new.photos)
            .all(|(left, right)| left.id() == right.id())
}

fn rebuild_folder_rows_for(
    current_photos: &Rc<RefCell<Vec<PhotoObject>>>,
    group_ranges: &Rc<RefCell<Vec<GroupRange>>>,
    current_columns: &Rc<Cell<u32>>,
    folder_store: &gio::ListStore,
    folder_selection: &gtk::NoSelection,
    folder_root: &gtk::ListView,
    mapped_folder_tiles: &Rc<RefCell<Vec<SquareTile>>>,
    folder_rebuild_active: &Rc<Cell<bool>>,
) {
    let trace = std::env::var_os("PICASA_TRACE").is_some();
    let started = trace.then(Instant::now);
    let ranges = group_ranges.borrow();
    let photos = current_photos.borrow();
    let old_rows = folder_store.n_items();
    let chunk_size = folder_chunk_size(current_columns.get());
    let new_rows = build_folder_virtual_objects(&ranges, &photos, chunk_size);

    if trace {
        eprintln!(
            "UI PERF folder_virtual_plan photos={} folders={} chunk_size={} columns={} model_rows={} old_store_rows={} ms={}",
            photos.len(),
            ranges.len(),
            chunk_size,
            current_columns.get().max(1),
            new_rows.len(),
            old_rows,
            started.map(|value| value.elapsed().as_millis()).unwrap_or(0)
        );
    }

    // Find the unchanged head and tail so a mostly-unchanged stream does not
    // force the live ListView to rebind every visible tile. This subsumes the
    // old all-or-nothing "unchanged" check, which never fired in any measured
    // run (scroll-baseline*.log) despite old_rows == new_rows.
    let old_len = old_rows as usize;
    let new_len = new_rows.len();
    let mut prefix = 0usize;
    while prefix < old_len
        && prefix < new_len
        && folder_store
            .item(prefix as u32)
            .and_downcast::<FolderRowObject>()
            .is_some_and(|old_row| {
                folder_virtual_row_matches(&old_row.data(), &new_rows[prefix].data())
            })
    {
        prefix += 1;
    }
    let mut suffix = 0usize;
    while suffix < old_len - prefix
        && suffix < new_len - prefix
        && folder_store
            .item((old_len - 1 - suffix) as u32)
            .and_downcast::<FolderRowObject>()
            .is_some_and(|old_row| {
                folder_virtual_row_matches(
                    &old_row.data(),
                    &new_rows[new_len - 1 - suffix].data(),
                )
            })
    {
        suffix += 1;
    }

    if trace && prefix < old_len && prefix < new_len {
        // First differing row: pinpoints why the stream is considered changed
        // when old_rows == new_rows (photos, label, folder, count...).
        if let Some(old_row) = folder_store
            .item(prefix as u32)
            .and_downcast::<FolderRowObject>()
        {
            let old_data = old_row.data();
            let new_data = new_rows[prefix].data();
            eprintln!(
                "UI PERF folder_store_first_row index={} old_kind={:?} new_kind={:?} old_folder_id={} new_folder_id={} old_label={:?} new_label={:?} old_count={} new_count={} old_photos={} new_photos={}",
                prefix,
                old_data.kind,
                new_data.kind,
                old_data.folder_id,
                new_data.folder_id,
                old_data.label,
                new_data.label,
                old_data.count,
                new_data.count,
                old_data.photos.len(),
                new_data.photos.len(),
            );
        }
    }

    if prefix == old_len && prefix == new_len {
        if trace {
            eprintln!(
                "UI PERF folder_store_update strategy=unchanged old_rows={} new_rows={} total_ms={}",
                old_rows,
                new_len,
                started.map(|value| value.elapsed().as_millis()).unwrap_or(0)
            );
        }
        return;
    }

    let update_started = trace.then(Instant::now);
    let removed = (old_len - prefix - suffix) as u32;
    let inserted = &new_rows[prefix..new_len - suffix];
    // Suppress eager bind decodes while the model is swapped: GTK binds its
    // whole offscreen pool here and would otherwise decode ~1400 cached JPEGs.
    folder_rebuild_active.set(true);
    // The old "attached splice is slower" measurement (6207 ms,
    // scroll-baseline4.log) predates the O(n) selection fix. With per-row binds
    // now ~1 ms, splicing while attached lets the ListView reuse its realized
    // pool instead of the detach rebuilding it. Only a from-scratch build
    // detaches.
    let strategy = if old_len == 0 {
        folder_selection.set_model(Option::<&gio::ListStore>::None);
        folder_store.splice(0, 0, inserted);
        folder_selection.set_model(Some(folder_store));
        "virtual_chunks_detached"
    } else {
        folder_store.splice(prefix as u32, removed, inserted);
        "attached_splice"
    };
    if trace {
        eprintln!(
            "UI PERF folder_store_update strategy={} old_rows={} new_rows={} prefix={} suffix={} model_ms={} total_ms={}",
            strategy,
            old_rows,
            folder_store.n_items(),
            prefix,
            suffix,
            update_started.map(|value| value.elapsed().as_millis()).unwrap_or(0),
            started.map(|value| value.elapsed().as_millis()).unwrap_or(0)
        );
    }
    // The bind path skipped decodes while the model swapped, so load the tiles
    // GTK actually allocates for the new model on the next idle, then re-enable
    // eager loading.
    let root = folder_root.clone();
    let mapped = mapped_folder_tiles.clone();
    let rebuild_active = folder_rebuild_active.clone();
    glib::idle_add_local_once(move || {
        refresh_folder_viewport_tiles_for(&root, &mapped);
        rebuild_active.set(false);
    });
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
    if mode == GroupMode::Folder {
        let folder_path = photo.folder_path().unwrap_or_default();
        if folder_path.is_empty() {
            return "Unknown Folder".to_string();
        }
        return std::path::Path::new(&folder_path)
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or(folder_path.as_str())
            .to_string();
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

fn selection_position_for_id(selection: &gtk::MultiSelection, photo_id: i64) -> Option<u32> {
    SELECTION_POSITION_CALLS.with(|count| count.set(count.get().wrapping_add(1)));
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

fn refresh_folder_selection_styles(root: &gtk::ListView, selection: &gtk::MultiSelection) {
    let trace = std::env::var_os("PICASA_TRACE").is_some();
    let started = trace.then(Instant::now);
    let calls_before = SELECTION_POSITION_CALLS.with(Cell::get);
    let selected_ids = selected_photo_id_set(selection);
    let mut tiles = Vec::new();
    collect_tiles(root.upcast_ref(), &mut tiles);
    let count = tiles.len();
    for tile in tiles {
        let selected = tile
            .imp()
            .photo
            .borrow()
            .as_ref()
            .is_some_and(|photo| selected_ids.contains(&photo.id()));
        tile.set_manual_selected(selected);
    }
    if let Some(started) = started {
        eprintln!("UI PERF folder_selection_styles tiles={} items={} spfid_calls={} ms={}", count, selection.n_items(), SELECTION_POSITION_CALLS.with(Cell::get).wrapping_sub(calls_before), started.elapsed().as_millis());
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

fn folder_chunk_height(photo_count: usize, columns: u32, tile_height: i32) -> i32 {
    let columns = columns.max(1) as usize;
    let lines = photo_count.max(1).div_ceil(columns);
    (lines as i32) * (tile_height.max(1) + 12)
}

fn update_folder_flow_layout(flow: &gtk::FlowBox, columns: u32, tile_height: i32) {
    flow.set_max_children_per_line(columns.max(1));
    let visible_photos = flow_box_tiles(flow)
        .into_iter()
        .filter(|tile| tile.is_visible() && tile.imp().photo.borrow().is_some())
        .count();
    if let Some(row_root) = flow.parent().and_downcast::<gtk::Box>() {
        row_root.set_height_request(folder_chunk_height(visible_photos, columns, tile_height));
    }
}

fn refresh_folder_viewport_tiles_for(
    root: &gtk::ListView,
    mapped_tiles: &Rc<RefCell<Vec<SquareTile>>>,
) {
    let trace = std::env::var_os("PICASA_TRACE").is_some();
    let started = trace.then(Instant::now);
    let mut mapped = mapped_tiles.borrow_mut();
    mapped.retain(|tile| tile.is_mapped());
    let mapped_count = mapped.len();
    let mut near = 0usize;
    let mut loaded = 0usize;
    let mut unloaded = 0usize;
    for tile in mapped.iter() {
        // A recycled pool tile that GTK has not allocated reports height 0 and
        // bounds at the origin, so it looked "near" and made us decode
        // thumbnails for hundreds of offscreen rows. Leave it untouched.
        if tile.height() <= 0 {
            continue;
        }
        if tile_near_folder_viewport(tile, root) {
            near += 1;
            if !tile.imp().visual_loaded.get() {
                loaded += 1;
            }
            tile.load_visual();
        } else {
            if tile.imp().visual_loaded.get() {
                unloaded += 1;
            }
            tile.unload_visual();
        }
    }
    let _ = near;
    if let Some(started) = started {
        let page = root
            .vadjustment()
            .map(|adjustment| adjustment.page_size())
            .unwrap_or_default();
        eprintln!(
            "UI PERF viewport_tiles mapped={} near={} loaded={} unloaded={} ms={} root_h={} page={}",
            mapped_count,
            near,
            loaded,
            unloaded,
            started.elapsed().as_millis(),
            root.height(),
            page
        );
    }
}

fn tile_near_folder_viewport(tile: &SquareTile, root: &gtk::ListView) -> bool {
    if !tile.is_mapped() || root.height() <= 0 {
        return false;
    }
    let Some(bounds) = tile.compute_bounds(root) else {
        return false;
    };
    let viewport = root.height() as f32;
    // Keep one viewport of prefetch on either side. This preserves smooth
    // scrolling while avoiding thumbnail work for ListView overscan rows.
    bounds.y() + bounds.height() >= -viewport && bounds.y() <= viewport * 2.0
}

fn flow_box_tiles(flow: &gtk::FlowBox) -> Vec<SquareTile> {
    let mut tiles = Vec::new();
    let mut child = flow.first_child();
    while let Some(current) = child {
        if let Some(tile) = current
            .first_child()
            .and_then(|widget| widget.downcast::<SquareTile>().ok())
        {
            tiles.push(tile);
        } else if let Ok(tile) = current.clone().downcast::<SquareTile>() {
            tiles.push(tile);
        }
        child = current.next_sibling();
    }
    tiles
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

fn collect_folder_flows(widget: &gtk::Widget, flows: &mut Vec<gtk::FlowBox>) {
    if let Some(flow) = widget.downcast_ref::<gtk::FlowBox>() {
        flows.push(flow.clone());
    }
    let mut child = widget.first_child();
    while let Some(current) = child {
        collect_folder_flows(&current, flows);
        child = current.next_sibling();
    }
}

#[cfg(test)]
mod folder_stream_tests {
    use super::{
        folder_chunk_height, folder_chunk_size, folder_virtual_row_matches, folder_virtual_rows,
        FolderRowData, FolderRowKind, GroupRange, FOLDER_PHOTO_CHUNK_SIZE,
    };

    fn sample_ranges() -> Vec<GroupRange> {
        vec![
            GroupRange {
                start: 0,
                end: 5,
                label: "Pictures".to_string(),
                folder_id: 10,
            },
            GroupRange {
                start: 5,
                end: 18,
                label: "Drone".to_string(),
                folder_id: 11,
            },
        ]
    }

    #[test]
    fn folder_virtual_stream_uses_header_plus_fixed_eight_photo_chunks() {
        let rows = folder_virtual_rows(&sample_ranges(), FOLDER_PHOTO_CHUNK_SIZE);
        assert_eq!(FOLDER_PHOTO_CHUNK_SIZE, 8);
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[0].kind, FolderRowKind::Header);
        assert_eq!((rows[1].start, rows[1].end), (0, 5));
        assert_eq!(rows[2].kind, FolderRowKind::Header);
        assert_eq!((rows[3].start, rows[3].end), (5, 13));
        assert_eq!((rows[4].start, rows[4].end), (13, 18));
    }

    #[test]
    fn folder_chunk_size_is_a_whole_number_of_rows() {
        // Every chunk must be a whole number of rows for the column count so
        // no chunk ends in a partial line.
        for columns in 1..=8u32 {
            let chunk = folder_chunk_size(columns);
            assert_eq!(
                chunk % columns as usize,
                0,
                "chunk {chunk} not aligned to {columns} columns"
            );
        }
        assert_eq!(folder_chunk_size(3), 9);
        assert_eq!(folder_chunk_size(5), 10);
        assert_eq!(folder_chunk_size(8), 8);
    }

    #[test]
    fn folder_chunk_height_tracks_wrapped_lines_without_changing_model_shape() {
        assert_eq!(folder_chunk_height(8, 8, 100), 112);
        assert_eq!(folder_chunk_height(8, 4, 100), 224);
        assert_eq!(folder_chunk_height(5, 3, 100), 224);
    }

    #[test]
    fn folder_virtual_photo_chunks_never_exceed_chunk_size() {
        let rows = folder_virtual_rows(&sample_ranges(), FOLDER_PHOTO_CHUNK_SIZE);
        for row in rows.into_iter().filter(|row| row.kind == FolderRowKind::Photos) {
            assert!(row.end - row.start <= FOLDER_PHOTO_CHUNK_SIZE);
        }
    }

    #[test]
    fn folder_virtual_row_match_compares_identity_fields() {
        let base = FolderRowData {
            kind: FolderRowKind::Header,
            folder_id: 7,
            folder_path: "/photos".to_string(),
            label: "photos".to_string(),
            count: 3,
            photos: Vec::new(),
        };
        assert!(folder_virtual_row_matches(&base, &base.clone()));

        let mut different_folder = base.clone();
        different_folder.folder_id = 8;
        assert!(!folder_virtual_row_matches(&base, &different_folder));

        let mut different_count = base.clone();
        different_count.count = 4;
        assert!(!folder_virtual_row_matches(&base, &different_count));
    }
}
