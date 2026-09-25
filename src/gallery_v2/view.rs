use std::cell::{Cell, RefCell};
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use gio::prelude::*;
use gtk::prelude::*;
use gtk4 as gtk;

use crate::db::Photo;
use crate::photo_object::PhotoObject;
use crate::thumbnail_display::{DisplayOutcome, DisplayRequest};

const MEMORY_CACHE_CAPACITY: usize = 512;
const WORKERS: usize = 4;

struct Completion {
    key: String,
    outcome: DisplayOutcome,
}

fn visual_key(photo: &PhotoObject) -> Option<String> {
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

fn request(photo: &PhotoObject) -> Option<DisplayRequest> {
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
        true,
    ))
}

fn binding_marker(photo_id: i64, key: &str) -> String {
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    format!("gallery-v2-{photo_id}-{:016x}", hasher.finish())
}

fn make_texture(width: i32, height: i32, pixels: Vec<u8>) -> gtk::gdk::Paintable {
    let stride = width.max(1) as usize * 4;
    let bytes = glib::Bytes::from_owned(pixels);
    let texture = gtk::gdk::MemoryTexture::new(
        width,
        height,
        gtk::gdk::MemoryFormat::R8g8b8a8,
        &bytes,
        stride,
    );
    texture.upcast()
}

/// First production adapter for the proven persistent GridView architecture.
///
/// This view deliberately owns presentation state only. The database, Photo,
/// PhotoObject, edit recipes, favourites and thumbnail cache remain the RC
/// implementations. Zoom changes tile geometry/columns and never model
/// membership.
pub(crate) struct GalleryV2 {
    pub root: gtk::GridView,
    store: gio::ListStore,
    selection: gtk::MultiSelection,
    current_photos: Rc<RefCell<Vec<PhotoObject>>>,
    tile_width: Rc<Cell<i32>>,
    tile_height: Rc<Cell<i32>>,
    current_columns: Rc<Cell<u32>>,
    last_width: Cell<i32>,
    live_tiles: Rc<RefCell<Vec<glib::WeakRef<gtk::Overlay>>>>,
    live_pictures: Rc<RefCell<Vec<glib::WeakRef<gtk::Picture>>>>,
    fit_whole_photo: Rc<Cell<bool>>,
    object_cache: Rc<RefCell<HashMap<i64, PhotoObject>>>,
    replace_generation: Rc<Cell<u64>>,
}

impl GalleryV2 {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        tile_width: i32,
        tile_height: i32,
        selected: Rc<dyn Fn(Option<PhotoObject>)>,
        activate: Rc<dyn Fn(Vec<PhotoObject>, usize)>,
        context_menu: Rc<dyn Fn(PhotoObject, gtk::Widget, f64, f64)>,
        unavailable: Rc<dyn Fn(PhotoObject, gtk::Widget)>,
        selection_ids_changed: Rc<dyn Fn(Vec<i64>)>,
    ) -> Self {
        let store = gio::ListStore::new::<PhotoObject>();
        let selection = gtk::MultiSelection::new(Some(store.clone()));
        let current_photos = Rc::new(RefCell::new(Vec::<PhotoObject>::new()));
        let tile_width = Rc::new(Cell::new(tile_width.max(1)));
        let tile_height = Rc::new(Cell::new(tile_height.max(1)));
        let current_columns = Rc::new(Cell::new(5_u32));
        let live_tiles: Rc<RefCell<Vec<glib::WeakRef<gtk::Overlay>>>> =
            Rc::new(RefCell::new(Vec::new()));
        let live_pictures: Rc<RefCell<Vec<glib::WeakRef<gtk::Picture>>>> =
            Rc::new(RefCell::new(Vec::new()));
        let fit_whole_photo = Rc::new(Cell::new(false));

        let memory_cache: Rc<RefCell<HashMap<String, gtk::gdk::Paintable>>> =
            Rc::new(RefCell::new(HashMap::new()));
        let pending: Rc<
            RefCell<HashMap<String, Vec<(glib::WeakRef<gtk::Picture>, String)>>>,
        > = Rc::new(RefCell::new(HashMap::new()));
        let inflight: Rc<RefCell<HashSet<String>>> = Rc::new(RefCell::new(HashSet::new()));

        let (job_tx, job_rx) = mpsc::channel::<DisplayRequest>();
        let (completion_tx, completion_rx) = mpsc::channel::<Completion>();
        let shared_jobs = Arc::new(Mutex::new(job_rx));

        for _ in 0..WORKERS {
            let jobs = shared_jobs.clone();
            let results = completion_tx.clone();
            std::thread::spawn(move || loop {
                let job = {
                    let Ok(receiver) = jobs.lock() else {
                        return;
                    };
                    match receiver.recv() {
                        Ok(job) => job,
                        Err(_) => return,
                    }
                };
                let key = job.key.clone();
                let outcome = crate::thumbnail_display::load_display_thumbnail(&job);
                if results.send(Completion { key, outcome }).is_err() {
                    return;
                }
            });
        }

        {
            let cache = memory_cache.clone();
            let pending = pending.clone();
            let inflight = inflight.clone();
            glib::timeout_add_local(Duration::from_millis(16), move || {
                while let Ok(completion) = completion_rx.try_recv() {
                    inflight.borrow_mut().remove(&completion.key);
                    let waiters = pending
                        .borrow_mut()
                        .remove(&completion.key)
                        .unwrap_or_default();

                    let paintable = match completion.outcome {
                        DisplayOutcome::Loaded {
                            width,
                            height,
                            pixels,
                        } if width > 0 && height > 0 && !pixels.is_empty() => {
                            Some(make_texture(width, height, pixels))
                        }
                        DisplayOutcome::Loaded { .. }
                        | DisplayOutcome::Missing
                        | DisplayOutcome::Failed => None,
                    };

                    if let Some(paintable) = paintable {
                        {
                            let mut cache = cache.borrow_mut();
                            if cache.len() >= MEMORY_CACHE_CAPACITY {
                                cache.clear();
                            }
                            cache.insert(completion.key.clone(), paintable.clone());
                        }

                        for (weak, marker) in waiters {
                            let Some(picture) = weak.upgrade() else {
                                continue;
                            };
                            if picture.widget_name() != marker {
                                continue;
                            }
                            picture.set_paintable(Some(&paintable));
                            if let Some(placeholder) = picture
                                .next_sibling()
                                .and_downcast::<gtk::Image>()
                            {
                                placeholder.set_visible(false);
                            }
                        }
                    }
                }
                glib::ControlFlow::Continue
            });
        }

        let factory = gtk::SignalListItemFactory::new();
        {
            let tile_width = tile_width.clone();
            let tile_height = tile_height.clone();
            let live_tiles = live_tiles.clone();
            let live_pictures = live_pictures.clone();
            let fit = fit_whole_photo.clone();
            let unavailable = unavailable.clone();
            let context_menu = context_menu.clone();
            let selection = selection.clone();

            factory.connect_setup(move |_, object| {
                let Some(list_item) = object.downcast_ref::<gtk::ListItem>() else {
                    return;
                };

                let frame = gtk::Overlay::new();
                frame.set_overflow(gtk::Overflow::Hidden);
                frame.add_css_class("photo-frame");
                frame.add_css_class("photo-tile");
                frame.add_css_class("gallery-v2-tile");
                frame.set_size_request(tile_width.get(), tile_height.get());
                frame.set_hexpand(false);
                frame.set_vexpand(false);
                frame.set_halign(gtk::Align::Center);
                frame.set_valign(gtk::Align::Start);

                let picture = gtk::Picture::new();
                picture.set_content_fit(if fit.get() {
                    gtk::ContentFit::Contain
                } else {
                    gtk::ContentFit::Cover
                });
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
                placeholder.set_can_target(false);
                placeholder.set_visible(true);
                frame.add_overlay(&placeholder);

                let checkmark = gtk::Image::from_icon_name("object-select-symbolic");
                checkmark.set_pixel_size(18);
                checkmark.set_halign(gtk::Align::End);
                checkmark.set_valign(gtk::Align::Start);
                checkmark.set_margin_top(8);
                checkmark.set_margin_end(8);
                checkmark.add_css_class("selection-badge");
                checkmark.set_can_target(false);
                frame.add_overlay(&checkmark);
                let _ = list_item
                    .bind_property("selected", &checkmark, "visible")
                    .sync_create()
                    .build();

                let favorite = gtk::Image::from_icon_name("emote-love-symbolic");
                favorite.set_pixel_size(18);
                favorite.set_halign(gtk::Align::End);
                favorite.set_valign(gtk::Align::End);
                favorite.set_margin_bottom(8);
                favorite.set_margin_end(8);
                favorite.add_css_class("favorite-badge");
                favorite.set_can_target(false);
                favorite.set_visible(false);
                frame.add_overlay(&favorite);

                let edited = gtk::Image::from_icon_name("document-edit-symbolic");
                edited.set_pixel_size(18);
                edited.set_halign(gtk::Align::Start);
                edited.set_valign(gtk::Align::End);
                edited.set_margin_bottom(8);
                edited.set_margin_start(8);
                edited.add_css_class("edited-badge");
                edited.set_tooltip_text(Some("Edited"));
                edited.set_can_target(false);
                edited.set_visible(false);
                frame.add_overlay(&edited);

                let offline = gtk::Button::with_label("!");
                offline.set_halign(gtk::Align::Start);
                offline.set_valign(gtk::Align::Start);
                offline.set_margin_top(8);
                offline.set_margin_start(8);
                offline.add_css_class("offline-badge");
                offline.set_tooltip_text(Some("Original photo unavailable"));
                offline.set_visible(false);
                frame.add_overlay(&offline);

                let item_for_offline = list_item.clone();
                let unavailable_for_click = unavailable.clone();
                offline.connect_clicked(move |button| {
                    let Some(photo) = item_for_offline.item().and_downcast::<PhotoObject>() else {
                        return;
                    };
                    unavailable_for_click(photo, button.clone().upcast());
                });

                let right_click = gtk::GestureClick::new();
                right_click.set_button(3);
                right_click.set_propagation_phase(gtk::PropagationPhase::Capture);
                let item_for_menu = list_item.clone();
                let selection_for_menu = selection.clone();
                let context_for_menu = context_menu.clone();
                let frame_for_menu = frame.clone();
                right_click.connect_pressed(move |gesture, _, x, y| {
                    let Some(photo) = item_for_menu.item().and_downcast::<PhotoObject>() else {
                        return;
                    };
                    let position = item_for_menu.position();
                    if !selection_for_menu.is_selected(position) {
                        selection_for_menu.select_item(position, true);
                    }
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                    context_for_menu(
                        photo,
                        frame_for_menu.clone().upcast(),
                        x,
                        y,
                    );
                });
                frame.add_controller(right_click);

                live_tiles.borrow_mut().push(frame.downgrade());
                live_pictures.borrow_mut().push(picture.downgrade());
                list_item.set_child(Some(&frame));
            });
        }

        {
            let cache = memory_cache.clone();
            let pending = pending.clone();
            let inflight = inflight.clone();
            let jobs = job_tx.clone();

            factory.connect_bind(move |_, object| {
                let Some(list_item) = object.downcast_ref::<gtk::ListItem>() else {
                    return;
                };
                let Some(photo) = list_item.item().and_downcast::<PhotoObject>() else {
                    return;
                };
                let Some(frame) = list_item.child().and_downcast::<gtk::Overlay>() else {
                    return;
                };
                let Some(picture) = frame.child().and_downcast::<gtk::Picture>() else {
                    return;
                };

                picture.set_paintable(gtk::gdk::Paintable::NONE);
                picture.set_tooltip_text(Some(&photo.filename()));
                let Some(key) = visual_key(&photo) else {
                    return;
                };
                let marker = binding_marker(photo.id(), &key);
                picture.set_widget_name(&marker);

                let mut child = picture.next_sibling();
                while let Some(widget) = child {
                    if let Some(image) = widget.downcast_ref::<gtk::Image>() {
                        if image.has_css_class("favorite-badge") {
                            image.set_visible(photo.favorite());
                        } else if image.has_css_class("edited-badge") {
                            image.set_visible(
                                !crate::edit::EditRecipe::decode(&photo.edit_recipe()).is_default(),
                            );
                        } else if image.has_css_class("dim-label") {
                            image.set_visible(true);
                        }
                    }
                    if let Some(button) = widget.downcast_ref::<gtk::Button>() {
                        if button.has_css_class("offline-badge") {
                            button.set_visible(!photo.original_available());
                        }
                    }
                    child = widget.next_sibling();
                }

                if let Some(paintable) = cache.borrow().get(&key).cloned() {
                    picture.set_paintable(Some(&paintable));
                    if let Some(placeholder) =
                        picture.next_sibling().and_downcast::<gtk::Image>()
                    {
                        placeholder.set_visible(false);
                    }
                    return;
                }

                pending
                    .borrow_mut()
                    .entry(key.clone())
                    .or_default()
                    .push((picture.downgrade(), marker));

                if inflight.borrow_mut().insert(key) {
                    if let Some(request) = request(&photo) {
                        let _ = jobs.send(request);
                    }
                }
            });
        }

        factory.connect_unbind(|_, object| {
            let Some(list_item) = object.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let Some(frame) = list_item.child().and_downcast::<gtk::Overlay>() else {
                return;
            };
            let Some(picture) = frame.child().and_downcast::<gtk::Picture>() else {
                return;
            };
            picture.set_widget_name("");
            picture.set_paintable(gtk::gdk::Paintable::NONE);
            picture.set_tooltip_text(None);
        });

        let root = gtk::GridView::new(Some(selection.clone()), Some(factory));
        root.set_min_columns(current_columns.get());
        root.set_max_columns(current_columns.get());
        root.set_single_click_activate(false);
        root.set_enable_rubberband(true);
        root.set_hexpand(true);
        root.set_vexpand(true);
        root.set_halign(gtk::Align::Fill);
        root.set_valign(gtk::Align::Fill);
        root.add_css_class("section-grid");
        root.add_css_class("gallery-v2-grid");

        {
            let selected = selected.clone();
            let selection_ids_changed = selection_ids_changed.clone();
            selection.connect_selection_changed(move |selection, _, _| {
                let bitset = selection.selection();
                let first = gtk::BitsetIter::init_first(&bitset)
                    .and_then(|(_, position)| selection.item(position))
                    .and_downcast::<PhotoObject>();
                selected(first);

                let mut ids = Vec::new();
                if let Some((iter, first_position)) = gtk::BitsetIter::init_first(&bitset) {
                    if let Some(photo) = selection
                        .item(first_position)
                        .and_downcast::<PhotoObject>()
                    {
                        ids.push(photo.id());
                    }
                    for position in iter {
                        if let Some(photo) = selection.item(position).and_downcast::<PhotoObject>() {
                            ids.push(photo.id());
                        }
                    }
                }
                selection_ids_changed(ids);
            });
        }

        {
            let current_photos = current_photos.clone();
            let selection = selection.clone();
            root.connect_activate(move |_, position| {
                let Some(activated) = selection
                    .item(position)
                    .and_downcast::<PhotoObject>()
                else {
                    return;
                };
                let photos = current_photos.borrow().clone();
                let index = photos
                    .iter()
                    .position(|photo| photo.id() == activated.id())
                    .unwrap_or(position as usize);
                activate(photos, index);
            });
        }

        Self {
            root,
            store,
            selection,
            current_photos,
            tile_width,
            tile_height,
            current_columns,
            last_width: Cell::new(0),
            live_tiles,
            live_pictures,
            fit_whole_photo,
            object_cache: Rc::new(RefCell::new(HashMap::new())),
            replace_generation: Rc::new(Cell::new(0)),
        }
    }

    pub(crate) fn replace(&self, photos: &[Photo]) {
        const PROGRESSIVE_THRESHOLD: usize = 1_000;
        const BATCH_SIZE: usize = 1_000;

        let generation = self.replace_generation.get().wrapping_add(1);
        self.replace_generation.set(generation);
        let started = std::time::Instant::now();

        // Reuse persistent PhotoObjects by database id. Switching between
        // Folder/Album/Favourites and All Photos must not rebuild tens of
        // thousands of GObjects every time.
        let mut ready = Vec::with_capacity(photos.len());
        let mut missing = Vec::new();
        {
            let cache = self.object_cache.borrow();
            for (index, photo) in photos.iter().enumerate() {
                if let Some(object) = cache.get(&photo.id).cloned() {
                    object.set_from_photo(photo);
                    ready.push((index, object));
                } else {
                    missing.push((index, photo.clone()));
                }
            }
        }

        // If everything is already cached, this is just a cheap model reorder.
        if missing.is_empty() {
            ready.sort_by_key(|(index, _)| *index);
            let objects = ready.into_iter().map(|(_, object)| object).collect::<Vec<_>>();
            self.store.splice(0, self.store.n_items(), &objects);
            self.current_photos.replace(objects);
            if std::env::var_os("PICASA_TRACE").is_some() {
                eprintln!(
                    "PIC_V2 replace photos={} mode=reuse elapsed_ms={}",
                    photos.len(),
                    started.elapsed().as_millis()
                );
            }
            return;
        }

        // Small destinations are cheap enough to finish synchronously.
        if photos.len() <= PROGRESSIVE_THRESHOLD {
            let mut cache = self.object_cache.borrow_mut();
            let mut objects = Vec::with_capacity(photos.len());
            for photo in photos {
                let object = cache
                    .entry(photo.id)
                    .or_insert_with(|| PhotoObject::from_photo(photo))
                    .clone();
                object.set_from_photo(photo);
                objects.push(object);
            }
            drop(cache);
            self.store.splice(0, self.store.n_items(), &objects);
            self.current_photos.replace(objects);
            if std::env::var_os("PICASA_TRACE").is_some() {
                eprintln!(
                    "PIC_V2 replace photos={} mode=sync elapsed_ms={}",
                    photos.len(),
                    started.elapsed().as_millis()
                );
            }
            return;
        }

        // Large first loads are progressive: clear immediately, then create
        // only a bounded batch per GTK idle turn. The UI stays responsive while
        // the cache warms; later visits become the fast reuse path above.
        self.store.remove_all();
        self.current_photos.borrow_mut().clear();

        let photos = Rc::new(photos.to_vec());
        let offset = Rc::new(Cell::new(0usize));
        let store = self.store.clone();
        let current = self.current_photos.clone();
        let object_cache = self.object_cache.clone();
        let replace_generation = self.replace_generation.clone();

        glib::idle_add_local(move || {
            if replace_generation.get() != generation {
                return glib::ControlFlow::Break;
            }

            let start = offset.get();
            if start >= photos.len() {
                return glib::ControlFlow::Break;
            }
            let end = (start + BATCH_SIZE).min(photos.len());

            let mut batch = Vec::with_capacity(end - start);
            {
                let mut cache = object_cache.borrow_mut();
                for photo in &photos[start..end] {
                    let object = cache
                        .entry(photo.id)
                        .or_insert_with(|| PhotoObject::from_photo(photo))
                        .clone();
                    object.set_from_photo(photo);
                    batch.push(object);
                }
            }

            store.splice(store.n_items(), 0, &batch);
            current.borrow_mut().extend(batch);
            offset.set(end);

            if end >= photos.len() {
                if std::env::var_os("PICASA_TRACE").is_some() {
                    eprintln!(
                        "PIC_V2 replace_complete photos={} elapsed_ms={}",
                        photos.len(),
                        started.elapsed().as_millis()
                    );
                }
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });

        if std::env::var_os("PICASA_TRACE").is_some() {
            eprintln!(
                "PIC_V2 replace photos={} mode=progressive missing={}",
                photos.len(),
                missing.len()
            );
        }
    }

    pub(crate) fn photo_objects(&self) -> Vec<PhotoObject> {
        self.current_photos.borrow().clone()
    }

    pub(crate) fn selected_photo_ids(&self) -> Vec<i64> {
        let bitset = self.selection.selection();
        let mut ids = Vec::new();
        if let Some((iter, first)) = gtk::BitsetIter::init_first(&bitset) {
            if let Some(photo) = self.selection.item(first).and_downcast::<PhotoObject>() {
                ids.push(photo.id());
            }
            for position in iter {
                if let Some(photo) = self.selection.item(position).and_downcast::<PhotoObject>() {
                    ids.push(photo.id());
                }
            }
        }
        ids
    }

    pub(crate) fn set_selected_ids(&self, ids: &[i64]) {
        let wanted = ids.iter().copied().collect::<HashSet<_>>();
        self.selection.unselect_all();
        if wanted.is_empty() {
            return;
        }
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

    pub(crate) fn set_tile_size(&self, width: i32, height: i32) {
        let width = width.max(1);
        let height = height.max(1);
        self.tile_width.set(width);
        self.tile_height.set(height);

        let tiles = {
            let mut weak_tiles = self.live_tiles.borrow_mut();
            let mut tiles = Vec::with_capacity(weak_tiles.len());
            weak_tiles.retain(|weak| {
                let Some(tile) = weak.upgrade() else {
                    return false;
                };
                tiles.push(tile);
                true
            });
            tiles
        };

        for tile in tiles {
            tile.set_size_request(width, height);
        }
        self.update_width(self.last_width.get());
    }

    pub(crate) fn update_width(&self, width: i32) {
        if width <= 0 {
            return;
        }
        self.last_width.set(width);
        let available = (width - 48).max(200);
        let columns = ((available as f64) / (self.tile_width.get() as f64 + 30.0))
            .floor()
            .clamp(1.0, 12.0) as u32;
        if self.current_columns.replace(columns) != columns {
            self.root.set_min_columns(columns);
            self.root.set_max_columns(columns);
        }
        self.root.queue_resize();
    }

    pub(crate) fn set_fit_whole_photo(&self, fit: bool) {
        self.fit_whole_photo.set(fit);
        let pictures = {
            let mut weak = self.live_pictures.borrow_mut();
            let mut pictures = Vec::with_capacity(weak.len());
            weak.retain(|item| {
                let Some(picture) = item.upgrade() else {
                    return false;
                };
                pictures.push(picture);
                true
            });
            pictures
        };
        let content_fit = if fit {
            gtk::ContentFit::Contain
        } else {
            gtk::ContentFit::Cover
        };
        for picture in pictures {
            picture.set_content_fit(content_fit);
        }
    }
}
