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

#[derive(Clone)]
struct Section {
    folder_id: i64,
    folder_path: String,
    label: String,
    start: usize,
    model: gio::ListStore,
    selection: gtk::MultiSelection,
}

struct Completion {
    key: String,
    outcome: DisplayOutcome,
}

fn trace_enabled() -> bool {
    std::env::var_os("PICASA_TRACE").is_some()
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
    format!("gallery-v2-folder-{photo_id}-{:016x}", hasher.finish())
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

#[allow(clippy::too_many_arguments)]
fn make_photo_factory(
    tile_width: Rc<Cell<i32>>,
    tile_height: Rc<Cell<i32>>,
    fit_whole_photo: Rc<Cell<bool>>,
    live_tiles: Rc<RefCell<Vec<glib::WeakRef<gtk::Overlay>>>>,
    live_pictures: Rc<RefCell<Vec<glib::WeakRef<gtk::Picture>>>>,
    cache: Rc<RefCell<HashMap<String, gtk::gdk::Paintable>>>,
    pending: Rc<RefCell<HashMap<String, Vec<(glib::WeakRef<gtk::Picture>, String)>>>>,
    inflight: Rc<RefCell<HashSet<String>>>,
    jobs: mpsc::Sender<DisplayRequest>,
    selected: Rc<dyn Fn(Option<PhotoObject>)>,
    context_menu: Rc<dyn Fn(PhotoObject, gtk::Widget, f64, f64)>,
    unavailable: Rc<dyn Fn(PhotoObject, gtk::Widget)>,
) -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();

    {
        let tile_width = tile_width.clone();
        let tile_height = tile_height.clone();
        let live_tiles = live_tiles.clone();
        let live_pictures = live_pictures.clone();
        let fit = fit_whole_photo.clone();
        let unavailable = unavailable.clone();
        let context_menu = context_menu.clone();
        let selected = selected.clone();

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
            let selected_for_menu = selected.clone();
            let context_for_menu = context_menu.clone();
            let frame_for_menu = frame.clone();
            right_click.connect_pressed(move |gesture, _, x, y| {
                let Some(photo) = item_for_menu.item().and_downcast::<PhotoObject>() else {
                    return;
                };
                selected_for_menu(Some(photo.clone()));
                gesture.set_state(gtk::EventSequenceState::Claimed);
                context_for_menu(photo, frame_for_menu.clone().upcast(), x, y);
            });
            frame.add_controller(right_click);

            live_tiles.borrow_mut().push(frame.downgrade());
            live_pictures.borrow_mut().push(picture.downgrade());
            list_item.set_child(Some(&frame));
        });
    }

    {
        let cache = cache.clone();
        let pending = pending.clone();
        let inflight = inflight.clone();
        let jobs = jobs.clone();

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

            let Some(key) = visual_key(&photo) else {
                return;
            };
            let marker = binding_marker(photo.id(), &key);
            picture.set_widget_name(&marker);

            if let Some(paintable) = cache.borrow().get(&key).cloned() {
                picture.set_paintable(Some(&paintable));
                if let Some(placeholder) = picture.next_sibling().and_downcast::<gtk::Image>() {
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

    factory
}

pub(crate) struct GalleryV2Folder {
    pub root: gtk::ListView,
    groups: gio::ListStore,
    current_photos: Rc<RefCell<Vec<PhotoObject>>>,
    folder_positions: Rc<RefCell<HashMap<i64, u32>>>,
    tile_width: Rc<Cell<i32>>,
    tile_height: Rc<Cell<i32>>,
    current_columns: Rc<Cell<u32>>,
    last_width: Cell<i32>,
    live_tiles: Rc<RefCell<Vec<glib::WeakRef<gtk::Overlay>>>>,
    live_pictures: Rc<RefCell<Vec<glib::WeakRef<gtk::Picture>>>>,
    live_grids: Rc<RefCell<Vec<(glib::WeakRef<gtk::GridView>, gio::ListStore)>>>,
    fit_whole_photo: Rc<Cell<bool>>,
    layout_signature: RefCell<Vec<(i64, i64)>>,
    selected: Rc<dyn Fn(Option<PhotoObject>)>,
}

impl GalleryV2Folder {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        tile_width: i32,
        tile_height: i32,
        selected: Rc<dyn Fn(Option<PhotoObject>)>,
        activate: Rc<dyn Fn(Vec<PhotoObject>, usize)>,
        context_menu: Rc<dyn Fn(PhotoObject, gtk::Widget, f64, f64)>,
        unavailable: Rc<dyn Fn(PhotoObject, gtk::Widget)>,
    ) -> Self {
        let groups = gio::ListStore::new::<glib::BoxedAnyObject>();
        let group_selection = gtk::NoSelection::new(Some(groups.clone()));
        let current_photos = Rc::new(RefCell::new(Vec::<PhotoObject>::new()));
        let folder_positions = Rc::new(RefCell::new(HashMap::<i64, u32>::new()));
        let tile_width = Rc::new(Cell::new(tile_width.max(1)));
        let tile_height = Rc::new(Cell::new(tile_height.max(1)));
        let current_columns = Rc::new(Cell::new(5_u32));
        let live_tiles = Rc::new(RefCell::new(Vec::<glib::WeakRef<gtk::Overlay>>::new()));
        let live_pictures = Rc::new(RefCell::new(Vec::<glib::WeakRef<gtk::Picture>>::new()));
        let live_grids = Rc::new(RefCell::new(Vec::<(
            glib::WeakRef<gtk::GridView>,
            gio::ListStore,
        )>::new()));
        let fit_whole_photo = Rc::new(Cell::new(false));

        let cache = Rc::new(RefCell::new(HashMap::<String, gtk::gdk::Paintable>::new()));
        let pending = Rc::new(RefCell::new(HashMap::<
            String,
            Vec<(glib::WeakRef<gtk::Picture>, String)>,
        >::new()));
        let inflight = Rc::new(RefCell::new(HashSet::<String>::new()));

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
            let cache = cache.clone();
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
                            if let Some(placeholder) =
                                picture.next_sibling().and_downcast::<gtk::Image>()
                            {
                                placeholder.set_visible(false);
                            }
                        }
                    }
                }
                glib::ControlFlow::Continue
            });
        }

        let folder_factory = gtk::SignalListItemFactory::new();

        {
            let tile_width_for_setup = tile_width.clone();
            let tile_height_for_setup = tile_height.clone();
            let fit_for_setup = fit_whole_photo.clone();
            let live_tiles_for_setup = live_tiles.clone();
            let live_pictures_for_setup = live_pictures.clone();
            let cache_for_setup = cache.clone();
            let pending_for_setup = pending.clone();
            let inflight_for_setup = inflight.clone();
            let jobs_for_setup = job_tx.clone();
            let selected_for_setup = selected.clone();
            let context_for_setup = context_menu.clone();
            let unavailable_for_setup = unavailable.clone();
            let activate_for_setup = activate.clone();
            let all_for_setup = current_photos.clone();
            let groups_for_setup = groups.clone();
            let positions_for_setup = folder_positions.clone();

            folder_factory.connect_setup(move |_, object| {
                let Some(list_item) = object.downcast_ref::<gtk::ListItem>() else {
                    return;
                };

                let section_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
                section_box.add_css_class("folder-section");
                section_box.set_hexpand(true);
                section_box.set_vexpand(false);
                section_box.set_valign(gtk::Align::Start);

                let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                header.add_css_class("folder-section-header");
                header.set_hexpand(true);

                let icon = gtk::Image::from_icon_name("folder-symbolic");
                icon.add_css_class("folder-section-icon");

                let title = gtk::Label::new(None);
                title.add_css_class("folder-section-title");
                title.set_xalign(0.0);
                title.set_hexpand(true);
                title.set_ellipsize(gtk::pango::EllipsizeMode::End);

                let count = gtk::Label::new(None);
                count.add_css_class("folder-section-count");
                count.set_xalign(0.0);

                header.append(&icon);
                header.append(&title);
                header.append(&count);

                let separator = gtk::Separator::new(gtk::Orientation::Horizontal);
                separator.add_css_class("folder-section-separator");

                let photo_factory = make_photo_factory(
                    tile_width_for_setup.clone(),
                    tile_height_for_setup.clone(),
                    fit_for_setup.clone(),
                    live_tiles_for_setup.clone(),
                    live_pictures_for_setup.clone(),
                    cache_for_setup.clone(),
                    pending_for_setup.clone(),
                    inflight_for_setup.clone(),
                    jobs_for_setup.clone(),
                    selected_for_setup.clone(),
                    context_for_setup.clone(),
                    unavailable_for_setup.clone(),
                );

                let grid = gtk::GridView::new(None::<gtk::MultiSelection>, Some(photo_factory));
                grid.add_css_class("folder-grid");
                grid.add_css_class("gallery-v2-folder-grid");
                // Match the same presentation class used by Library/Photos.
                grid.add_css_class("section-grid");
                grid.set_min_columns(1);
                grid.set_max_columns(12);
                grid.set_single_click_activate(false);
                grid.set_enable_rubberband(true);
                grid.set_hexpand(true);
                grid.set_vexpand(false);
                grid.set_halign(gtk::Align::Fill);
                grid.set_valign(gtk::Align::Start);

                // Do not attach every folder model while the outer ListView is
                // measuring its rows. Attaching here caused all 22k photos to
                // bind at once, defeating virtualization and leaving visible
                // holes while thousands of thumbnail jobs drained.
                {
                    let groups = groups_for_setup.clone();
                    let positions = positions_for_setup.clone();
                    grid.connect_map(move |grid| {
                        let Ok(folder_id) = grid.widget_name().parse::<i64>() else {
                            return;
                        };
                        let Some(position) = positions.borrow().get(&folder_id).copied() else {
                            return;
                        };
                        let Some(boxed) = groups
                            .item(position)
                            .and_downcast::<glib::BoxedAnyObject>()
                        else {
                            return;
                        };
                        let section = boxed.borrow::<Section>();
                        grid.set_model(Some(&section.selection));
                        if trace_enabled() {
                            eprintln!(
                                "PIC_V2_FOLDER map folder_id={} photos={}",
                                section.folder_id,
                                section.model.n_items()
                            );
                        }
                    });
                }
                grid.connect_unmap(|grid| {
                    grid.set_model(None::<&gtk::SelectionModel>);
                });

                let activate = activate_for_setup.clone();
                let all = all_for_setup.clone();
                grid.connect_activate(move |grid, position| {
                    let Some(photo) = grid
                        .model()
                        .and_then(|model| model.item(position))
                        .and_downcast::<PhotoObject>()
                    else {
                        return;
                    };
                    let photos = all.borrow().clone();
                    let index = photos
                        .iter()
                        .position(|candidate| candidate.id() == photo.id())
                        .unwrap_or(position as usize);
                    activate(photos, index);
                });

                section_box.append(&header);
                section_box.append(&separator);
                section_box.append(&grid);
                list_item.set_child(Some(&section_box));
            });
        }

        {
            let columns = current_columns.clone();
            let tile_height_for_bind = tile_height.clone();
            let live_grids_for_bind = live_grids.clone();

            folder_factory.connect_bind(move |_, object| {
                let Some(list_item) = object.downcast_ref::<gtk::ListItem>() else {
                    return;
                };
                let Some(boxed) = list_item.item().and_downcast::<glib::BoxedAnyObject>() else {
                    return;
                };
                let section = boxed.borrow::<Section>();

                let Some(section_box) = list_item.child().and_downcast::<gtk::Box>() else {
                    return;
                };
                let Some(header) = section_box.first_child().and_downcast::<gtk::Box>() else {
                    return;
                };
                let Some(icon) = header.first_child().and_downcast::<gtk::Image>() else {
                    return;
                };
                let Some(title) = icon.next_sibling().and_downcast::<gtk::Label>() else {
                    return;
                };
                let Some(count) = title.next_sibling().and_downcast::<gtk::Label>() else {
                    return;
                };
                let Some(separator) = header.next_sibling().and_downcast::<gtk::Separator>() else {
                    return;
                };
                let Some(grid) = separator.next_sibling().and_downcast::<gtk::GridView>() else {
                    return;
                };

                title.set_label(&section.label);
                title.set_tooltip_text(Some(&section.folder_path));
                count.set_label(&format!("{} photos", section.model.n_items()));

                let current_columns = columns.get().max(1);
                grid.set_widget_name(&section.folder_id.to_string());
                grid.set_min_columns(current_columns);
                grid.set_max_columns(current_columns);

                let rows = (section.model.n_items() + current_columns - 1) / current_columns;
                let row_height = tile_height_for_bind.get() + 12;
                grid.set_height_request((rows as i32 * row_height).max(row_height));

                live_grids_for_bind
                    .borrow_mut()
                    .push((grid.downgrade(), section.model.clone()));

                if trace_enabled() {
                    eprintln!(
                        "PIC_V2_FOLDER bind folder_id={} start={} photos={}",
                        section.folder_id,
                        section.start,
                        section.model.n_items()
                    );
                }
            });
        }

        folder_factory.connect_unbind(|_, object| {
            let Some(list_item) = object.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let Some(section_box) = list_item.child().and_downcast::<gtk::Box>() else {
                return;
            };
            let Some(header) = section_box.first_child().and_downcast::<gtk::Box>() else {
                return;
            };
            let Some(separator) = header.next_sibling().and_downcast::<gtk::Separator>() else {
                return;
            };
            let Some(grid) = separator.next_sibling().and_downcast::<gtk::GridView>() else {
                return;
            };
            grid.set_model(None::<&gtk::SelectionModel>);
            grid.set_widget_name("");
        });

        let root = gtk::ListView::new(Some(group_selection), Some(folder_factory));
        root.add_css_class("folder-list");
        root.add_css_class("gallery-v2-folder-list");
        root.set_single_click_activate(false);
        root.set_hexpand(true);
        root.set_vexpand(true);

        Self {
            root,
            groups,
            current_photos,
            folder_positions,
            tile_width,
            tile_height,
            current_columns,
            last_width: Cell::new(0),
            live_tiles,
            live_pictures,
            live_grids,
            fit_whole_photo,
            layout_signature: RefCell::new(Vec::new()),
            selected,
        }
    }

    pub(crate) fn replace(&self, photos: &[Photo]) {
        let objects = photos.iter().map(PhotoObject::from_photo).collect::<Vec<_>>();
        self.replace_objects(&objects);
    }

    pub(crate) fn replace_objects(&self, objects: &[PhotoObject]) {
        let started = std::time::Instant::now();

        // Folder layout depends only on stable photo order + folder membership.
        // When returning to Folder view with the same library, keep the
        // existing section objects, per-folder ListStores and GridViews alive.
        // Metadata on the shared PhotoObjects has already been refreshed by
        // GalleryV2::objects_for(), so no section/model rebuild is required.
        let signature = objects
            .iter()
            .map(|photo| (photo.id(), photo.folder_id()))
            .collect::<Vec<_>>();
        if self.groups.n_items() > 0 && *self.layout_signature.borrow() == signature {
            self.current_photos.replace(objects.to_vec());
            if trace_enabled() {
                eprintln!(
                    "PIC_V2_FOLDER replace photos={} sections={} mode=reuse elapsed_ms={}",
                    objects.len(),
                    self.groups.n_items(),
                    started.elapsed().as_millis()
                );
            }
            return;
        }

        self.groups.remove_all();
        self.folder_positions.borrow_mut().clear();
        self.current_photos.replace(objects.to_vec());

        let mut sections = Vec::<glib::BoxedAnyObject>::new();
        let mut start = 0usize;
        let mut section_index = 0u32;
        while start < objects.len() {
            let folder_id = objects[start].folder_id();
            let folder_path = objects[start].folder_path().unwrap_or_default();
            let mut end = start + 1;
            while end < objects.len()
                && objects[end].folder_id() == folder_id
                && objects[end].folder_path().unwrap_or_default() == folder_path
            {
                end += 1;
            }

            let model = gio::ListStore::new::<PhotoObject>();
            model.splice(0, 0, &objects[start..end]);
            let selection = gtk::MultiSelection::new(Some(model.clone()));

            // Install this once per persistent section. Doing it from the
            // ListItem bind callback would accumulate handlers on rebind.
            let selected = self.selected.clone();
            selection.connect_selection_changed(move |selection, _, _| {
                let selected_set = selection.selection();
                let first = gtk::BitsetIter::init_first(&selected_set)
                    .and_then(|(_, position)| selection.item(position))
                    .and_downcast::<PhotoObject>();
                selected(first);
            });

            let label = if folder_path.is_empty() {
                "Photos".to_string()
            } else {
                crate::source::filename(&folder_path)
            };

            self.folder_positions
                .borrow_mut()
                .insert(folder_id, section_index);
            sections.push(glib::BoxedAnyObject::new(Section {
                folder_id,
                folder_path,
                label,
                start,
                model,
                selection,
            }));

            section_index += 1;
            start = end;
        }

        if !sections.is_empty() {
            self.groups.splice(0, 0, &sections);
        }
        self.layout_signature.replace(signature);

        if trace_enabled() {
            eprintln!(
                "PIC_V2_FOLDER replace photos={} sections={} mode=rebuild elapsed_ms={}",
                objects.len(),
                self.groups.n_items(),
                started.elapsed().as_millis()
            );
        }
    }

    pub(crate) fn has_sections(&self) -> bool {
        self.groups.n_items() > 0
    }

    pub(crate) fn scroll_to_folder(&self, folder_id: i64) -> bool {
        let Some(position) = self.folder_positions.borrow().get(&folder_id).copied() else {
            return false;
        };
        self.root
            .scroll_to(position, gtk::ListScrollFlags::FOCUS, None::<gtk::ScrollInfo>);
        self.root.grab_focus();
        if trace_enabled() {
            eprintln!("PIC_V2_FOLDER scroll folder_id={} section={}", folder_id, position);
        }
        true
    }

    pub(crate) fn select_photo(&self, photo_id: i64) -> bool {
        for position in 0..self.groups.n_items() {
            let Some(boxed) = self.groups.item(position).and_downcast::<glib::BoxedAnyObject>() else {
                continue;
            };
            let section = boxed.borrow::<Section>();
            for item in 0..section.model.n_items() {
                let Some(photo) = section.model.item(item).and_downcast::<PhotoObject>() else {
                    continue;
                };
                if photo.id() == photo_id {
                    section.selection.select_item(item, true);
                    self.root.scroll_to(
                        position,
                        gtk::ListScrollFlags::FOCUS,
                        None::<gtk::ScrollInfo>,
                    );
                    return true;
                }
            }
        }
        false
    }

    pub(crate) fn selected_photo_ids(&self) -> Vec<i64> {
        let mut ids = Vec::new();
        for position in 0..self.groups.n_items() {
            let Some(boxed) = self.groups.item(position).and_downcast::<glib::BoxedAnyObject>() else {
                continue;
            };
            let section = boxed.borrow::<Section>();
            let selected = section.selection.selection();
            if let Some((iter, first)) = gtk::BitsetIter::init_first(&selected) {
                if let Some(photo) = section.selection.item(first).and_downcast::<PhotoObject>() {
                    ids.push(photo.id());
                }
                for item in iter {
                    if let Some(photo) = section.selection.item(item).and_downcast::<PhotoObject>() {
                        ids.push(photo.id());
                    }
                }
            }
        }
        ids
    }

    pub(crate) fn set_selected_ids(&self, ids: &[i64]) {
        let wanted = ids.iter().copied().collect::<HashSet<_>>();
        for position in 0..self.groups.n_items() {
            let Some(boxed) = self.groups.item(position).and_downcast::<glib::BoxedAnyObject>() else {
                continue;
            };
            let section = boxed.borrow::<Section>();
            section.selection.unselect_all();
            if wanted.is_empty() {
                continue;
            }
            for item in 0..section.model.n_items() {
                if section
                    .model
                    .item(item)
                    .and_downcast::<PhotoObject>()
                    .is_some_and(|photo| wanted.contains(&photo.id()))
                {
                    section.selection.select_item(item, false);
                }
            }
        }
    }

    pub(crate) fn set_tile_size(&self, width: i32, height: i32) {
        let width = width.max(1);
        let height = height.max(1);
        self.tile_width.set(width);
        self.tile_height.set(height);

        let tiles = {
            let mut weak = self.live_tiles.borrow_mut();
            let mut realized = Vec::with_capacity(weak.len());
            weak.retain(|item| {
                let Some(tile) = item.upgrade() else {
                    return false;
                };
                realized.push(tile);
                true
            });
            realized
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
        self.current_columns.set(columns);

        let grids = {
            let mut weak = self.live_grids.borrow_mut();
            let mut realized = Vec::with_capacity(weak.len());
            weak.retain(|(grid, model)| {
                let Some(grid) = grid.upgrade() else {
                    return false;
                };
                realized.push((grid, model.clone()));
                true
            });
            realized
        };

        for (grid, model) in grids {
            grid.set_min_columns(columns);
            grid.set_max_columns(columns);
            let rows = (model.n_items() + columns - 1) / columns;
            let row_height = self.tile_height.get() + 12;
            grid.set_height_request((rows as i32 * row_height).max(row_height));
            grid.queue_resize();
        }
        self.root.queue_resize();
    }

    pub(crate) fn set_fit_whole_photo(&self, fit: bool) {
        self.fit_whole_photo.set(fit);
        let pictures = {
            let mut weak = self.live_pictures.borrow_mut();
            let mut realized = Vec::with_capacity(weak.len());
            weak.retain(|item| {
                let Some(picture) = item.upgrade() else {
                    return false;
                };
                realized.push(picture);
                true
            });
            realized
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
