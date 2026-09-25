use crate::photo::PhotoObject;
use adw::prelude::*;
use gio::prelude::*;
use gtk::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant, UNIX_EPOCH};
use walkdir::WalkDir;

const MIN_TILE: i32 = 84;
const MAX_TILE: i32 = 320;
const DEFAULT_TILE: i32 = 176;
const MAX_COLUMNS: u32 = 12;
const THUMB_EDGE: u32 = 384;
const MEMORY_THUMB_CACHE: usize = 512;

#[derive(Clone)]
struct FolderGroupData {
    folder: PathBuf,
    label: String,
    model: gio::ListStore,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ViewMode {
    Photos,
    Favorites,
}

struct FolderNavEntry {
    path: PathBuf,
    label: String,
    depth: u32,
    count: u32,
    target_index: usize,
}

struct ThumbResult {
    path: String,
    width: i32,
    height: i32,
    stride: usize,
    rgba: Vec<u8>,
}

fn trace_enabled() -> bool {
    std::env::var_os("PICASA_TRACE").is_some()
}

fn images_disabled() -> bool {
    std::env::var_os("PIC_NO_IMAGES").is_some()
}

fn app_cache_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_CACHE_HOME") {
        return PathBuf::from(dir).join("pic-library-prototype");
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".cache")
        .join("pic-library-prototype")
}

fn app_config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(dir).join("pic-library-prototype");
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config")
        .join("pic-library-prototype")
}

fn favorites_file() -> PathBuf {
    app_config_dir().join("favorites.txt")
}

fn load_favorites() -> HashSet<String> {
    let Ok(text) = fs::read_to_string(favorites_file()) else {
        return HashSet::new();
    };
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn save_favorites(favorites: &HashSet<String>) {
    let file = favorites_file();
    if let Some(parent) = file.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let mut paths = favorites.iter().cloned().collect::<Vec<_>>();
    paths.sort();
    let text = if paths.is_empty() {
        String::new()
    } else {
        format!("{}\n", paths.join("\n"))
    };
    let _ = fs::write(file, text);
}

fn thumbnail_cache_path(source: &str, cache_dir: &Path) -> PathBuf {
    let mut hasher = DefaultHasher::new();
    source.hash(&mut hasher);

    if let Ok(metadata) = fs::metadata(source) {
        metadata.len().hash(&mut hasher);
        if let Ok(modified) = metadata.modified() {
            if let Ok(age) = modified.duration_since(UNIX_EPOCH) {
                age.as_secs().hash(&mut hasher);
                age.subsec_nanos().hash(&mut hasher);
            }
        }
    }

    cache_dir.join(format!("{:016x}.png", hasher.finish()))
}

fn decode_thumb_cached(path: &str, cache_dir: &Path) -> ThumbResult {
    let cached = thumbnail_cache_path(path, cache_dir);

    let decoded = if cached.is_file() {
        image::ImageReader::open(&cached)
            .ok()
            .and_then(|reader| reader.with_guessed_format().ok())
            .and_then(|reader| reader.decode().ok())
    } else {
        None
    };

    let image = if let Some(image) = decoded {
        image
    } else {
        let source = image::ImageReader::open(path)
            .ok()
            .and_then(|reader| reader.with_guessed_format().ok())
            .and_then(|reader| reader.decode().ok());

        let Some(source) = source else {
            return ThumbResult {
                path: path.to_string(),
                width: 0,
                height: 0,
                stride: 0,
                rgba: Vec::new(),
            };
        };

        let thumb = source.thumbnail(THUMB_EDGE, THUMB_EDGE);
        if let Some(parent) = cached.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = thumb.save(&cached);
        thumb
    };

    let rgba = image.to_rgba8();
    let (width, height) = rgba.dimensions();
    ThumbResult {
        path: path.to_string(),
        width: width as i32,
        height: height as i32,
        stride: width as usize * 4,
        rgba: rgba.into_raw(),
    }
}

pub fn build_window(app: &adw::Application) -> adw::ApplicationWindow {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("PIC Library Prototype")
        .default_width(1380)
        .default_height(860)
        .build();

    let groups = gio::ListStore::new::<glib::BoxedAnyObject>();
    let group_selection = gtk::NoSelection::new(Some(groups.clone()));
    let master_groups: Rc<RefCell<Vec<FolderGroupData>>> = Rc::new(RefCell::new(Vec::new()));
    let roots: Rc<RefCell<Vec<PathBuf>>> = Rc::new(RefCell::new(Vec::new()));
    let mode = Rc::new(Cell::new(ViewMode::Photos));
    let search_text = Rc::new(RefCell::new(String::new()));
    let favorite_paths = Rc::new(RefCell::new(load_favorites()));

    let tile_size = Rc::new(Cell::new(DEFAULT_TILE));
    let live_tiles: Rc<RefCell<Vec<glib::WeakRef<gtk::Widget>>>> =
        Rc::new(RefCell::new(Vec::new()));
    let live_grids: Rc<RefCell<Vec<(glib::WeakRef<gtk::GridView>, gio::ListStore)>>> =
        Rc::new(RefCell::new(Vec::new()));
    let current_columns = Rc::new(Cell::new(6u32));
    let favorite_changed: Rc<RefCell<Option<Rc<dyn Fn()>>>> =
        Rc::new(RefCell::new(None));

    let thumb_cache: Rc<RefCell<HashMap<String, gtk::gdk::Texture>>> =
        Rc::new(RefCell::new(HashMap::new()));
    let thumb_pending: Rc<RefCell<HashMap<String, Vec<glib::WeakRef<gtk::Picture>>>>> =
        Rc::new(RefCell::new(HashMap::new()));
    let thumb_inflight: Rc<RefCell<HashSet<String>>> =
        Rc::new(RefCell::new(HashSet::new()));

    let (thumb_job_tx, thumb_job_rx) = mpsc::channel::<String>();
    let (thumb_result_tx, thumb_result_rx) = mpsc::channel::<ThumbResult>();
    let shared_jobs = Arc::new(Mutex::new(thumb_job_rx));
    let disk_cache_dir = app_cache_dir().join("thumbs");
    let _ = fs::create_dir_all(&disk_cache_dir);

    for _ in 0..3 {
        let jobs = shared_jobs.clone();
        let results = thumb_result_tx.clone();
        let cache_dir = disk_cache_dir.clone();
        std::thread::spawn(move || loop {
            let path = {
                let Ok(receiver) = jobs.lock() else {
                    return;
                };
                match receiver.recv() {
                    Ok(path) => path,
                    Err(_) => return,
                }
            };

            let decoded = decode_thumb_cached(&path, &cache_dir);
            if results.send(decoded).is_err() {
                return;
            }
        });
    }

    {
        let cache = thumb_cache.clone();
        let pending = thumb_pending.clone();
        let inflight = thumb_inflight.clone();
        glib::timeout_add_local(Duration::from_millis(16), move || {
            while let Ok(result) = thumb_result_rx.try_recv() {
                inflight.borrow_mut().remove(&result.path);
                let waiters = pending.borrow_mut().remove(&result.path).unwrap_or_default();

                if result.width <= 0 || result.height <= 0 || result.rgba.is_empty() {
                    continue;
                }

                let bytes = glib::Bytes::from_owned(result.rgba);
                let memory = gtk::gdk::MemoryTexture::new(
                    result.width,
                    result.height,
                    gtk::gdk::MemoryFormat::R8g8b8a8,
                    &bytes,
                    result.stride,
                );
                let texture: gtk::gdk::Texture = memory.upcast();

                {
                    let mut cache = cache.borrow_mut();
                    if cache.len() >= MEMORY_THUMB_CACHE {
                        cache.clear();
                    }
                    cache.insert(result.path.clone(), texture.clone());
                }

                for weak in waiters {
                    let Some(picture) = weak.upgrade() else {
                        continue;
                    };
                    if picture.tooltip_text().as_deref() == Some(result.path.as_str()) {
                        picture.set_paintable(Some(&texture));
                    }
                }

                if trace_enabled() {
                    eprintln!(
                        "PIC_PROTO thumb_ready path={} size={}x{}",
                        result.path, result.width, result.height
                    );
                }
            }
            glib::ControlFlow::Continue
        });
    }

    let sidebar = gtk::Box::new(gtk::Orientation::Vertical, 0);
    sidebar.add_css_class("library-sidebar");
    sidebar.set_size_request(250, -1);

    let library_heading = gtk::Label::new(Some("Library"));
    library_heading.add_css_class("sidebar-heading");
    library_heading.set_xalign(0.0);
    sidebar.append(&library_heading);

    let (photos_button, photos_count) =
        make_sidebar_button("image-x-generic-symbolic", "Photos", "0");
    let (favorites_button, favorites_count) =
        make_sidebar_button("emote-love-symbolic", "Favourites", "0");
    sidebar.append(&photos_button);
    sidebar.append(&favorites_button);

    let sidebar_separator = gtk::Separator::new(gtk::Orientation::Horizontal);
    sidebar_separator.set_margin_top(8);
    sidebar_separator.set_margin_bottom(6);
    sidebar.append(&sidebar_separator);

    let folders_heading = gtk::Label::new(Some("Folders"));
    folders_heading.add_css_class("sidebar-heading");
    folders_heading.set_xalign(0.0);
    sidebar.append(&folders_heading);

    let folder_box = gtk::Box::new(gtk::Orientation::Vertical, 1);
    let folder_scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .vexpand(true)
        .child(&folder_box)
        .build();
    sidebar.append(&folder_scroller);

    let count_label = gtk::Label::new(Some("0 photos · 0 folders"));
    count_label.add_css_class("dim-label");

    let content_title = gtk::Label::new(Some("Photos"));
    content_title.add_css_class("title");

    let folder_factory = gtk::SignalListItemFactory::new();

    {
        let tile_size = tile_size.clone();
        let live_tiles = live_tiles.clone();
        let current_columns = current_columns.clone();
        let cache = thumb_cache.clone();
        let pending = thumb_pending.clone();
        let inflight = thumb_inflight.clone();
        let jobs = thumb_job_tx.clone();
        let favorites = favorite_paths.clone();
        let changed = favorite_changed.clone();
        let window_weak = window.downgrade();

        folder_factory.connect_setup(move |_, object| {
            let Some(list_item) = object.downcast_ref::<gtk::ListItem>() else {
                return;
            };

            let section = gtk::Box::new(gtk::Orientation::Vertical, 4);
            section.add_css_class("folder-section");
            section.set_hexpand(true);
            section.set_vexpand(false);
            section.set_valign(gtk::Align::Start);

            let header_line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            header_line.add_css_class("folder-section-header");
            header_line.set_hexpand(true);

            let icon = gtk::Image::from_icon_name("folder-symbolic");
            icon.add_css_class("folder-section-icon");

            let title = gtk::Label::new(None);
            title.add_css_class("folder-section-title");
            title.set_xalign(0.0);
            title.set_ellipsize(gtk::pango::EllipsizeMode::End);

            let count = gtk::Label::new(None);
            count.add_css_class("folder-section-count");
            count.set_xalign(0.0);

            header_line.append(&icon);
            header_line.append(&title);
            header_line.append(&count);

            let separator = gtk::Separator::new(gtk::Orientation::Horizontal);
            separator.add_css_class("folder-section-separator");

            let photo_factory = make_photo_factory(
                tile_size.clone(),
                live_tiles.clone(),
                cache.clone(),
                pending.clone(),
                inflight.clone(),
                jobs.clone(),
                favorites.clone(),
                changed.clone(),
            );

            let grid = gtk::GridView::new(None::<gtk::SingleSelection>, Some(photo_factory));
            grid.add_css_class("folder-grid");
            grid.set_min_columns(current_columns.get());
            grid.set_max_columns(current_columns.get());
            grid.set_single_click_activate(false);
            grid.set_hexpand(true);
            grid.set_vexpand(false);
            grid.set_halign(gtk::Align::Fill);
            grid.set_valign(gtk::Align::Start);

            let parent = window_weak.clone();
            grid.connect_activate(move |grid, position| {
                let Some(photo) = grid
                    .model()
                    .and_then(|model| model.item(position))
                    .and_downcast::<PhotoObject>()
                else {
                    return;
                };
                let Some(parent) = parent.upgrade() else {
                    return;
                };
                open_lightbox(&parent, &photo);
            });

            section.append(&header_line);
            section.append(&separator);
            section.append(&grid);
            list_item.set_child(Some(&section));
        });
    }

    {
        let tile_size = tile_size.clone();
        let live_grids = live_grids.clone();
        let current_columns = current_columns.clone();

        folder_factory.connect_bind(move |_, object| {
            let Some(list_item) = object.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let Some(group_object) = list_item.item().and_downcast::<glib::BoxedAnyObject>() else {
                return;
            };
            let group = group_object.borrow::<FolderGroupData>();

            let Some(section) = list_item.child().and_downcast::<gtk::Box>() else {
                return;
            };
            let Some(header_line) = section.first_child().and_downcast::<gtk::Box>() else {
                return;
            };
            let Some(icon) = header_line.first_child().and_downcast::<gtk::Image>() else {
                return;
            };
            let Some(title) = icon.next_sibling().and_downcast::<gtk::Label>() else {
                return;
            };
            let Some(count) = title.next_sibling().and_downcast::<gtk::Label>() else {
                return;
            };
            let Some(separator) = header_line.next_sibling().and_downcast::<gtk::Separator>() else {
                return;
            };
            let Some(grid) = separator.next_sibling().and_downcast::<gtk::GridView>() else {
                return;
            };

            title.set_label(&group.label);
            count.set_label(&format!("{} photos", group.model.n_items()));

            let columns = current_columns.get().max(1);
            grid.set_min_columns(columns);
            grid.set_max_columns(columns);

            let selection = gtk::SingleSelection::new(Some(group.model.clone()));
            selection.set_autoselect(false);
            selection.set_can_unselect(true);
            grid.set_model(Some(&selection));

            let rows = (group.model.n_items() + columns - 1) / columns;
            let row_height = tile_size.get() + 12;
            grid.set_height_request((rows as i32 * row_height).max(row_height));

            live_grids
                .borrow_mut()
                .push((grid.downgrade(), group.model.clone()));

            if trace_enabled() {
                eprintln!(
                    "PIC_PROTO bind_group folder={} photos={}",
                    group.folder.display(),
                    group.model.n_items()
                );
            }
        });
    }

    folder_factory.connect_unbind(|_, object| {
        let Some(list_item) = object.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(section) = list_item.child().and_downcast::<gtk::Box>() else {
            return;
        };
        let Some(header_line) = section.first_child().and_downcast::<gtk::Box>() else {
            return;
        };
        let Some(separator) = header_line.next_sibling().and_downcast::<gtk::Separator>() else {
            return;
        };
        let Some(grid) = separator.next_sibling().and_downcast::<gtk::GridView>() else {
            return;
        };
        grid.set_model(None::<&gtk::SelectionModel>);
    });

    let list = gtk::ListView::new(Some(group_selection.clone()), Some(folder_factory));
    list.add_css_class("folder-list");
    list.set_single_click_activate(false);
    list.set_hexpand(true);
    list.set_vexpand(true);

    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .child(&list)
        .build();

    if trace_enabled() {
        let adjustment = scroller.vadjustment();
        adjustment.connect_value_changed(move |adj| {
            eprintln!(
                "PIC_PROTO scroll value={:.0} upper={:.0} page={:.0}",
                adj.value(),
                adj.upper(),
                adj.page_size()
            );
        });
    }

    let header = adw::HeaderBar::new();
    header.set_title_widget(Some(&content_title));
    header.pack_start(&count_label);

    let search = gtk::SearchEntry::new();
    search.set_placeholder_text(Some("Search photos or folders"));
    search.set_size_request(260, -1);
    search.set_tooltip_text(Some("Search filename, folder name, or path"));
    header.pack_end(&search);

    let zoom = gtk::Scale::with_range(
        gtk::Orientation::Horizontal,
        f64::from(MIN_TILE),
        f64::from(MAX_TILE),
        1.0,
    );
    zoom.set_value(f64::from(DEFAULT_TILE));
    zoom.set_draw_value(false);
    zoom.set_size_request(180, -1);
    zoom.set_tooltip_text(Some("Thumbnail size"));
    header.pack_end(&zoom);

    let plus = gtk::Button::from_icon_name("zoom-in-symbolic");
    plus.set_tooltip_text(Some("Larger thumbnails"));
    let minus = gtk::Button::from_icon_name("zoom-out-symbolic");
    minus.set_tooltip_text(Some("Smaller thumbnails"));
    header.pack_end(&plus);
    header.pack_end(&minus);

    let open = gtk::Button::with_label("Open Folder");
    header.pack_start(&open);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&header);
    content.append(&scroller);

    let paned = gtk::Paned::new(gtk::Orientation::Horizontal);
    paned.set_start_child(Some(&sidebar));
    paned.set_end_child(Some(&content));
    paned.set_position(250);
    paned.set_resize_start_child(false);
    paned.set_shrink_start_child(false);
    paned.set_resize_end_child(true);
    paned.set_shrink_end_child(false);
    window.set_content(Some(&paned));

    install_css();

    {
        let groups = groups.clone();
        let master = master_groups.clone();
        let mode = mode.clone();
        let count_label = count_label.clone();
        let content_title = content_title.clone();
        let photos_button = photos_button.clone();
        let favorites_button = favorites_button.clone();
        let favorites_count = favorites_count.clone();
        let search_text = search_text.clone();

        let refresh: Rc<dyn Fn()> = Rc::new(move || {
            let favorite_total = count_favorites(&master.borrow());
            favorites_count.set_label(&favorite_total.to_string());

            if mode.get() == ViewMode::Favorites {
                apply_view(
                    &groups,
                    &master.borrow(),
                    ViewMode::Favorites,
                    &count_label,
                    &content_title,
                    &photos_button,
                    &favorites_button,
                    &search_text.borrow(),
                );
            }
        });
        favorite_changed.replace(Some(refresh));
    }

    {
        let groups = groups.clone();
        let master = master_groups.clone();
        let mode = mode.clone();
        let count_label = count_label.clone();
        let title = content_title.clone();
        let photos_button_for_style = photos_button.clone();
        let favorites_button_for_style = favorites_button.clone();
        let search_text = search_text.clone();

        photos_button.connect_clicked(move |_| {
            mode.set(ViewMode::Photos);
            apply_view(
                &groups,
                &master.borrow(),
                ViewMode::Photos,
                &count_label,
                &title,
                &photos_button_for_style,
                &favorites_button_for_style,
                &search_text.borrow(),
            );
        });
    }

    {
        let groups = groups.clone();
        let master = master_groups.clone();
        let mode = mode.clone();
        let count_label = count_label.clone();
        let title = content_title.clone();
        let photos_button_for_style = photos_button.clone();
        let favorites_button_for_style = favorites_button.clone();

        let search_text = search_text.clone();

        favorites_button.connect_clicked(move |_| {
            mode.set(ViewMode::Favorites);
            apply_view(
                &groups,
                &master.borrow(),
                ViewMode::Favorites,
                &count_label,
                &title,
                &photos_button_for_style,
                &favorites_button_for_style,
                &search_text.borrow(),
            );
        });
    }

    {
        let groups = groups.clone();
        let master = master_groups.clone();
        let mode = mode.clone();
        let count_label = count_label.clone();
        let title = content_title.clone();
        let photos_button = photos_button.clone();
        let favorites_button = favorites_button.clone();
        let search_text = search_text.clone();

        search.connect_search_changed(move |entry| {
            search_text.replace(entry.text().trim().to_string());
            apply_view(
                &groups,
                &master.borrow(),
                mode.get(),
                &count_label,
                &title,
                &photos_button,
                &favorites_button,
                &search_text.borrow(),
            );
        });
    }

    let folder_activate: Rc<dyn Fn(usize)> = {
        let groups = groups.clone();
        let master = master_groups.clone();
        let mode = mode.clone();
        let count_label = count_label.clone();
        let title = content_title.clone();
        let list = list.clone();
        let photos_button = photos_button.clone();
        let favorites_button = favorites_button.clone();
        let search_text = search_text.clone();
        let search = search.clone();

        Rc::new(move |index| {
            if !search_text.borrow().is_empty() {
                search_text.borrow_mut().clear();
                search.set_text("");
            }
            if mode.get() != ViewMode::Photos {
                mode.set(ViewMode::Photos);
                apply_view(
                    &groups,
                    &master.borrow(),
                    ViewMode::Photos,
                    &count_label,
                    &title,
                    &photos_button,
                    &favorites_button,
                    "",
                );
            }

            let list = list.clone();
            glib::idle_add_local_once(move || {
                list.scroll_to(
                    index as u32,
                    gtk::ListScrollFlags::FOCUS,
                    None::<gtk::ScrollInfo>,
                );
            });
        })
    };

    {
        let live_tiles = live_tiles.clone();
        let live_grids = live_grids.clone();
        let tile_size = tile_size.clone();
        let current_columns = current_columns.clone();
        let list = list.clone();
        let scroller = scroller.clone();

        zoom.connect_value_changed(move |scale| {
            let size = scale.value().round() as i32;
            if size == tile_size.replace(size) {
                return;
            }

            let started = Instant::now();
            let mut touched = 0usize;
            let mut tiles = live_tiles.borrow_mut();
            tiles.retain(|weak| {
                let Some(widget) = weak.upgrade() else {
                    return false;
                };
                widget.set_size_request(size, size);
                touched += 1;
                true
            });

            update_grid_layout(
                scroller.width(),
                size,
                &current_columns,
                &live_grids,
                &list,
            );

            if trace_enabled() {
                eprintln!(
                    "PIC_PROTO zoom size={} realized_tiles={} elapsed_us={}",
                    size,
                    touched,
                    started.elapsed().as_micros()
                );
            }
        });
    }

    {
        let zoom = zoom.clone();
        plus.connect_clicked(move |_| {
            zoom.set_value((zoom.value() + 20.0).min(f64::from(MAX_TILE)));
        });
    }

    {
        let zoom = zoom.clone();
        minus.connect_clicked(move |_| {
            zoom.set_value((zoom.value() - 20.0).max(f64::from(MIN_TILE)));
        });
    }

    {
        let scroller = scroller.clone();
        let tile_size = tile_size.clone();
        let current_columns = current_columns.clone();
        let live_grids = live_grids.clone();
        let list = list.clone();
        let last_width = Rc::new(Cell::new(-1));
        let last_width_for_tick = last_width.clone();

        glib::timeout_add_local(Duration::from_millis(120), move || {
            let width = scroller.width();
            if width > 0 && width != last_width_for_tick.replace(width) {
                update_grid_layout(
                    width,
                    tile_size.get(),
                    &current_columns,
                    &live_grids,
                    &list,
                );
            }
            glib::ControlFlow::Continue
        });
    }

    {
        let window_weak = window.downgrade();
        let groups = groups.clone();
        let master = master_groups.clone();
        let roots_state = roots.clone();
        let favorite_paths = favorite_paths.clone();
        let count_label = count_label.clone();
        let title = content_title.clone();
        let photos_button = photos_button.clone();
        let favorites_button = favorites_button.clone();
        let photos_count = photos_count.clone();
        let favorites_count = favorites_count.clone();
        let folder_box = folder_box.clone();
        let folder_activate = folder_activate.clone();
        let mode = mode.clone();

        open.connect_clicked(move |_| {
            let Some(window) = window_weak.upgrade() else {
                return;
            };
            let dialog = gtk::FileDialog::builder()
                .title("Choose photo library folder")
                .modal(true)
                .build();

            let groups = groups.clone();
            let master = master.clone();
            let roots_state = roots_state.clone();
            let favorite_paths = favorite_paths.clone();
            let count_label = count_label.clone();
            let title = title.clone();
            let photos_button = photos_button.clone();
            let favorites_button = favorites_button.clone();
            let photos_count = photos_count.clone();
            let favorites_count = favorites_count.clone();
            let folder_box = folder_box.clone();
            let folder_activate = folder_activate.clone();
            let mode = mode.clone();

            dialog.select_folder(Some(&window), gio::Cancellable::NONE, move |result| {
                let Ok(folder) = result else {
                    return;
                };
                let Some(path) = folder.path() else {
                    return;
                };

                let new_roots = vec![path];
                let scanned = scan_directories(&new_roots, &favorite_paths.borrow());
                roots_state.replace(new_roots.clone());
                master.replace(scanned);
                mode.set(ViewMode::Photos);

                refresh_library_chrome(
                    &groups,
                    &master,
                    &new_roots,
                    &folder_box,
                    &folder_activate,
                    &count_label,
                    &title,
                    &photos_button,
                    &favorites_button,
                    &photos_count,
                    &favorites_count,
                );
            });
        });
    }

    let mut startup_paths = std::env::args_os()
        .skip(1)
        .map(PathBuf::from)
        .filter(|path| path.is_dir())
        .collect::<Vec<_>>();

    if startup_paths.is_empty() {
        if let Some(path) = std::env::var_os("PIC_LIBRARY_DIR")
            .map(PathBuf::from)
            .filter(|path| path.is_dir())
        {
            startup_paths.push(path);
        } else if let Some(path) = std::env::var_os("HOME")
            .map(|home| PathBuf::from(home).join("Pictures"))
            .filter(|path| path.is_dir())
        {
            startup_paths.push(path);
        }
    }

    if !startup_paths.is_empty() {
        let started = Instant::now();
        let scanned = scan_directories(&startup_paths, &favorite_paths.borrow());
        roots.replace(startup_paths.clone());
        master_groups.replace(scanned);

        refresh_library_chrome(
            &groups,
            &master_groups,
            &startup_paths,
            &folder_box,
            &folder_activate,
            &count_label,
            &content_title,
            &photos_button,
            &favorites_button,
            &photos_count,
            &favorites_count,
        );

        if trace_enabled() {
            eprintln!(
                "PIC_PROTO library_loaded photos={} folders={} roots={} elapsed_ms={}",
                count_photos(&master_groups.borrow()),
                master_groups.borrow().len(),
                startup_paths.len(),
                started.elapsed().as_millis()
            );
        }
    }

    window
}

fn make_sidebar_button(icon_name: &str, text: &str, count: &str) -> (gtk::Button, gtk::Label) {
    let button = gtk::Button::new();
    button.add_css_class("sidebar-row");

    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let icon = gtk::Image::from_icon_name(icon_name);
    icon.set_pixel_size(16);

    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.set_hexpand(true);

    let count_label = gtk::Label::new(Some(count));
    count_label.add_css_class("dim-label");
    count_label.add_css_class("sidebar-count");

    row.append(&icon);
    row.append(&label);
    row.append(&count_label);
    button.set_child(Some(&row));
    (button, count_label)
}

fn make_photo_factory(
    tile_size: Rc<Cell<i32>>,
    live_tiles: Rc<RefCell<Vec<glib::WeakRef<gtk::Widget>>>>,
    thumb_cache: Rc<RefCell<HashMap<String, gtk::gdk::Texture>>>,
    thumb_pending: Rc<RefCell<HashMap<String, Vec<glib::WeakRef<gtk::Picture>>>>>,
    thumb_inflight: Rc<RefCell<HashSet<String>>>,
    thumb_job_tx: mpsc::Sender<String>,
    favorite_paths: Rc<RefCell<HashSet<String>>>,
    favorite_changed: Rc<RefCell<Option<Rc<dyn Fn()>>>>,
) -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();

    {
        let favorite_paths = favorite_paths.clone();
        let favorite_changed = favorite_changed.clone();

        factory.connect_setup(move |_, object| {
            let Some(list_item) = object.downcast_ref::<gtk::ListItem>() else {
                return;
            };

            let overlay = gtk::Overlay::new();
            overlay.add_css_class("prototype-photo-tile");
            overlay.set_overflow(gtk::Overflow::Hidden);
            overlay.set_size_request(tile_size.get(), tile_size.get());
            overlay.set_hexpand(false);
            overlay.set_vexpand(false);

            let picture = gtk::Picture::new();
            picture.set_size_request(1, 1);
            picture.set_hexpand(true);
            picture.set_vexpand(true);
            picture.set_halign(gtk::Align::Fill);
            picture.set_valign(gtk::Align::Fill);
            picture.set_can_shrink(true);
            picture.set_content_fit(gtk::ContentFit::Cover);
            overlay.set_child(Some(&picture));

            let favorite_icon = gtk::Image::from_icon_name("non-starred-symbolic");
            favorite_icon.set_pixel_size(16);
            let favorite_button = gtk::Button::new();
            favorite_button.add_css_class("favorite-tile-button");
            favorite_button.set_child(Some(&favorite_icon));
            favorite_button.set_halign(gtk::Align::End);
            favorite_button.set_valign(gtk::Align::Start);
            favorite_button.set_margin_top(6);
            favorite_button.set_margin_end(6);
            overlay.add_overlay(&favorite_button);

            let list_item_for_favorite = list_item.clone();
            let favorite_paths = favorite_paths.clone();
            let favorite_changed = favorite_changed.clone();
            favorite_button.connect_clicked(move |button| {
                let Some(photo) = list_item_for_favorite
                    .item()
                    .and_downcast::<PhotoObject>()
                else {
                    return;
                };

                let new_value = !photo.favorite();
                photo.set_favorite(new_value);
                let path = photo.path();

                {
                    let mut favorites = favorite_paths.borrow_mut();
                    if new_value {
                        favorites.insert(path.clone());
                    } else {
                        favorites.remove(&path);
                    }
                    save_favorites(&favorites);
                }

                if let Some(image) = button.child().and_downcast::<gtk::Image>() {
                    image.set_icon_name(Some(if new_value {
                        "starred-symbolic"
                    } else {
                        "non-starred-symbolic"
                    }));
                }
                button.set_tooltip_text(Some(if new_value {
                    "Remove from Favourites"
                } else {
                    "Add to Favourites"
                }));

                if let Some(callback) = favorite_changed.borrow().as_ref().cloned() {
                    glib::idle_add_local_once(move || callback());
                }
            });

            let widget: gtk::Widget = overlay.clone().upcast();
            live_tiles.borrow_mut().push(widget.downgrade());
            list_item.set_child(Some(&overlay));
        });
    }

    {
        let cache = thumb_cache.clone();
        let pending = thumb_pending.clone();
        let inflight = thumb_inflight.clone();
        let jobs = thumb_job_tx.clone();

        factory.connect_bind(move |_, object| {
            let Some(list_item) = object.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let Some(photo) = list_item.item().and_downcast::<PhotoObject>() else {
                return;
            };
            let Some(overlay) = list_item.child().and_downcast::<gtk::Overlay>() else {
                return;
            };
            let Some(picture) = overlay.child().and_downcast::<gtk::Picture>() else {
                return;
            };
            let Some(favorite_button) = overlay.last_child().and_downcast::<gtk::Button>() else {
                return;
            };

            let path = photo.path();
            picture.set_tooltip_text(Some(&path));
            picture.set_paintable(None::<&gtk::gdk::Paintable>);

            if let Some(image) = favorite_button.child().and_downcast::<gtk::Image>() {
                image.set_icon_name(Some(if photo.favorite() {
                    "starred-symbolic"
                } else {
                    "non-starred-symbolic"
                }));
            }
            favorite_button.set_tooltip_text(Some(if photo.favorite() {
                "Remove from Favourites"
            } else {
                "Add to Favourites"
            }));

            if images_disabled() {
                return;
            }

            if let Some(texture) = cache.borrow().get(&path).cloned() {
                picture.set_paintable(Some(&texture));
                return;
            }

            pending
                .borrow_mut()
                .entry(path.clone())
                .or_default()
                .push(picture.downgrade());

            if inflight.borrow_mut().insert(path.clone()) {
                let _ = jobs.send(path.clone());
                if trace_enabled() {
                    eprintln!(
                        "PIC_PROTO thumb_queue position={} path={}",
                        list_item.position(),
                        path
                    );
                }
            }
        });
    }

    factory.connect_unbind(|_, object| {
        let Some(list_item) = object.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(overlay) = list_item.child().and_downcast::<gtk::Overlay>() else {
            return;
        };
        let Some(picture) = overlay.child().and_downcast::<gtk::Picture>() else {
            return;
        };
        picture.set_paintable(None::<&gtk::gdk::Paintable>);
        picture.set_tooltip_text(None);
    });

    factory
}

fn open_lightbox(parent: &adw::ApplicationWindow, photo: &PhotoObject) {
    let path = photo.path();
    let name = Path::new(&path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("Photo");

    let lightbox = gtk::Window::builder()
        .transient_for(parent)
        .modal(true)
        .title(name)
        .default_width(1100)
        .default_height(760)
        .build();

    let header = gtk::HeaderBar::new();
    let close = gtk::Button::from_icon_name("window-close-symbolic");
    close.set_tooltip_text(Some("Close"));
    header.pack_end(&close);

    let picture = gtk::Picture::new();
    picture.set_hexpand(true);
    picture.set_vexpand(true);
    picture.set_can_shrink(true);
    picture.set_content_fit(gtk::ContentFit::Contain);
    picture.set_file(Some(&gio::File::for_path(&path)));

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.append(&header);
    root.append(&picture);
    lightbox.set_child(Some(&root));

    let lightbox_for_close = lightbox.clone();
    close.connect_clicked(move |_| lightbox_for_close.close());

    let key = gtk::EventControllerKey::new();
    let lightbox_for_key = lightbox.clone();
    key.connect_key_pressed(move |_, key, _, _| {
        if key == gtk::gdk::Key::Escape {
            lightbox_for_key.close();
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    lightbox.add_controller(key);
    lightbox.present();
}

fn update_grid_layout(
    width: i32,
    size: i32,
    current_columns: &Rc<Cell<u32>>,
    live_grids: &Rc<RefCell<Vec<(glib::WeakRef<gtk::GridView>, gio::ListStore)>>>,
    list: &gtk::ListView,
) {
    let available = (width - 48).max(220);
    let columns = ((available as f64) / (size as f64 + 18.0))
        .floor()
        .clamp(1.0, MAX_COLUMNS as f64) as u32;
    current_columns.set(columns);

    let mut grids = live_grids.borrow_mut();
    grids.retain(|(weak, model)| {
        let Some(grid) = weak.upgrade() else {
            return false;
        };
        grid.set_min_columns(columns);
        grid.set_max_columns(columns);
        let rows = (model.n_items() + columns - 1) / columns;
        grid.set_height_request((rows as i32 * (size + 12)).max(size + 12));
        grid.queue_resize();
        true
    });

    list.queue_resize();
}

fn scan_directories(roots: &[PathBuf], favorites: &HashSet<String>) -> Vec<FolderGroupData> {
    let mut by_folder = BTreeMap::<PathBuf, Vec<PathBuf>>::new();

    for root in roots {
        for entry in WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file())
        {
            let path = entry.into_path();
            if !is_displayable_photo(&path) {
                continue;
            }
            let folder = path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| root.to_path_buf());
            by_folder.entry(folder).or_default().push(path);
        }
    }

    let mut groups = Vec::with_capacity(by_folder.len());

    for (folder, mut paths) in by_folder {
        paths.sort_unstable();
        let model = gio::ListStore::new::<PhotoObject>();

        for path in paths {
            let path = path.to_string_lossy().into_owned();
            model.append(&PhotoObject::new(path.clone(), favorites.contains(&path)));
        }

        let label = folder
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or("Photos")
            .to_string();

        groups.push(FolderGroupData {
            folder,
            label,
            model,
        });
    }

    groups
}

fn apply_view(
    groups: &gio::ListStore,
    master: &[FolderGroupData],
    mode: ViewMode,
    count_label: &gtk::Label,
    content_title: &gtk::Label,
    photos_button: &gtk::Button,
    favorites_button: &gtk::Button,
    query: &str,
) {
    groups.remove_all();
    let mut photo_count = 0u32;
    let mut folder_count = 0u32;
    let needle = query.trim().to_lowercase();

    for group in master {
        let folder_match = needle.is_empty()
            || group.label.to_lowercase().contains(&needle)
            || group.folder.to_string_lossy().to_lowercase().contains(&needle);

        let filtered = gio::ListStore::new::<PhotoObject>();

        for position in 0..group.model.n_items() {
            let Some(photo) = group.model.item(position).and_downcast::<PhotoObject>() else {
                continue;
            };

            if mode == ViewMode::Favorites && !photo.favorite() {
                continue;
            }

            let photo_match = needle.is_empty()
                || folder_match
                || Path::new(&photo.path())
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.to_lowercase().contains(&needle))
                || photo.path().to_lowercase().contains(&needle);

            if photo_match {
                filtered.append(&photo);
            }
        }

        if filtered.n_items() == 0 {
            continue;
        }

        photo_count += filtered.n_items();
        folder_count += 1;
        groups.append(&glib::BoxedAnyObject::new(FolderGroupData {
            folder: group.folder.clone(),
            label: group.label.clone(),
            model: filtered,
        }));
    }

    match mode {
        ViewMode::Photos => {
            content_title.set_label(if needle.is_empty() { "Photos" } else { "Search · Photos" });
            photos_button.add_css_class("sidebar-active");
            favorites_button.remove_css_class("sidebar-active");
        }
        ViewMode::Favorites => {
            content_title.set_label(if needle.is_empty() { "Favourites" } else { "Search · Favourites" });
            favorites_button.add_css_class("sidebar-active");
            photos_button.remove_css_class("sidebar-active");
        }
    }

    if needle.is_empty() {
        count_label.set_label(&format!("{} photos · {} folders", photo_count, folder_count));
    } else {
        count_label.set_label(&format!("{} matches · {} folders", photo_count, folder_count));
    }
}

fn count_photos(groups: &[FolderGroupData]) -> u32 {
    groups.iter().map(|group| group.model.n_items()).sum()
}

fn count_favorites(groups: &[FolderGroupData]) -> u32 {
    groups
        .iter()
        .map(|group| {
            (0..group.model.n_items())
                .filter_map(|position| group.model.item(position).and_downcast::<PhotoObject>())
                .filter(|photo| photo.favorite())
                .count() as u32
        })
        .sum()
}

fn folder_nav_entries(groups: &[FolderGroupData], roots: &[PathBuf]) -> Vec<FolderNavEntry> {
    let mut entries = BTreeMap::<PathBuf, (u32, u32, usize)>::new();

    for (group_index, group) in groups.iter().enumerate() {
        let count = group.model.n_items();
        let matching_root = roots.iter().find(|root| group.folder.starts_with(root));

        if let Some(root) = matching_root {
            entries
                .entry(root.clone())
                .and_modify(|entry| entry.1 += count)
                .or_insert((0, count, group_index));

            if let Ok(relative) = group.folder.strip_prefix(root) {
                let mut current = root.clone();
                for (offset, component) in relative.components().enumerate() {
                    current.push(component.as_os_str());
                    entries
                        .entry(current.clone())
                        .and_modify(|entry| entry.1 += count)
                        .or_insert(((offset + 1) as u32, count, group_index));
                }
            }
        } else {
            entries.insert(group.folder.clone(), (0, count, group_index));
        }
    }

    entries
        .into_iter()
        .map(|(path, (depth, count, target_index))| FolderNavEntry {
            label: path
                .file_name()
                .and_then(|name| name.to_str())
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| path.to_str().unwrap_or("Folder"))
                .to_string(),
            path,
            depth,
            count,
            target_index,
        })
        .collect()
}

fn rebuild_folder_sidebar(
    folder_box: &gtk::Box,
    groups: &[FolderGroupData],
    roots: &[PathBuf],
    activate: &Rc<dyn Fn(usize)>,
) {
    while let Some(child) = folder_box.first_child() {
        folder_box.remove(&child);
    }

    for entry in folder_nav_entries(groups, roots) {
        let button = gtk::Button::new();
        button.add_css_class("folder-sidebar-row");
        button.set_tooltip_text(Some(&entry.path.display().to_string()));
        button.set_margin_start((6 + entry.depth as i32 * 14).min(70));

        let row = gtk::Box::new(gtk::Orientation::Horizontal, 7);
        let icon = gtk::Image::from_icon_name("folder-symbolic");
        icon.set_pixel_size(15);

        let label = gtk::Label::new(Some(&entry.label));
        label.set_xalign(0.0);
        label.set_hexpand(true);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);

        let count = gtk::Label::new(Some(&entry.count.to_string()));
        count.add_css_class("dim-label");
        count.add_css_class("sidebar-count");

        row.append(&icon);
        row.append(&label);
        row.append(&count);
        button.set_child(Some(&row));

        let activate = activate.clone();
        let target = entry.target_index;
        button.connect_clicked(move |_| activate(target));
        folder_box.append(&button);
    }
}

fn refresh_library_chrome(
    groups: &gio::ListStore,
    master: &Rc<RefCell<Vec<FolderGroupData>>>,
    roots: &[PathBuf],
    folder_box: &gtk::Box,
    folder_activate: &Rc<dyn Fn(usize)>,
    count_label: &gtk::Label,
    content_title: &gtk::Label,
    photos_button: &gtk::Button,
    favorites_button: &gtk::Button,
    photos_count: &gtk::Label,
    favorites_count: &gtk::Label,
) {
    let master_ref = master.borrow();
    photos_count.set_label(&count_photos(&master_ref).to_string());
    favorites_count.set_label(&count_favorites(&master_ref).to_string());

    rebuild_folder_sidebar(folder_box, &master_ref, roots, folder_activate);
    apply_view(
        groups,
        &master_ref,
        ViewMode::Photos,
        count_label,
        content_title,
        photos_button,
        favorites_button,
        "",
    );
}

fn is_displayable_photo(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
        return false;
    };
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "jpg" | "jpeg" | "png" | "webp" | "bmp" | "gif" | "tif" | "tiff"
    )
}

fn install_css() {
    let css = gtk::CssProvider::new();
    css.load_from_data(
        r#"
        .library-sidebar {
            background: alpha(@window_fg_color, 0.025);
            border-right: 1px solid alpha(@window_fg_color, 0.10);
            padding: 8px 6px 8px 6px;
        }

        .sidebar-heading {
            font-weight: 700;
            opacity: 0.66;
            margin: 8px 8px 5px 8px;
        }

        .sidebar-row,
        .folder-sidebar-row {
            min-height: 34px;
            padding: 3px 7px;
            border-radius: 7px;
            background: transparent;
            box-shadow: none;
            border: none;
        }

        .sidebar-row:hover,
        .folder-sidebar-row:hover {
            background: alpha(@window_fg_color, 0.06);
        }

        .sidebar-row.sidebar-active {
            background: alpha(@accent_bg_color, 0.18);
        }

        searchentry {
            min-height: 32px;
        }

        .sidebar-count {
            min-width: 34px;
            font-variant-numeric: tabular-nums;
        }

        .folder-list {
            background: @window_bg_color;
        }

        .folder-list > row {
            padding: 0;
            margin: 0;
            background: transparent;
        }

        .folder-section {
            padding: 14px 20px 18px 20px;
        }

        .folder-section-header {
            margin-top: 8px;
            margin-bottom: 2px;
        }

        .folder-section-title {
            font-weight: 700;
            font-size: 14px;
        }

        .folder-section-count {
            font-size: 12px;
            opacity: 0.58;
        }

        .folder-section-icon {
            opacity: 0.65;
        }

        .folder-section-separator {
            margin-top: 5px;
            margin-bottom: 8px;
            opacity: 0.45;
        }

        .folder-grid {
            background: transparent;
        }

        .folder-grid > child,
        .folder-grid > item {
            padding: 6px;
            margin: 0;
            min-height: 0;
            background: transparent;
        }

        .folder-grid > child:selected,
        .folder-grid > item:selected {
            background: transparent;
        }

        .folder-grid > child:selected .prototype-photo-tile,
        .folder-grid > item:selected .prototype-photo-tile {
            outline: 2px solid @accent_bg_color;
            outline-offset: -2px;
        }

        .prototype-photo-tile {
            background: alpha(@window_fg_color, 0.06);
            border-radius: 6px;
        }

        .prototype-photo-tile picture {
            border-radius: 6px;
        }

        .favorite-tile-button {
            min-width: 26px;
            min-height: 26px;
            padding: 0;
            border-radius: 9999px;
            background: alpha(@window_bg_color, 0.72);
            box-shadow: none;
        }

        .favorite-tile-button:hover {
            background: alpha(@window_bg_color, 0.94);
        }
        "#,
    );

    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}
