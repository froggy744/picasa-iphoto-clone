use crate::photo::PhotoObject;
use adw::prelude::*;
use gio::prelude::*;
use gtk::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::cmp::Ordering;
use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

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
    RecentlyAdded,
    Album(i64),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SortField {
    DateTaken,
    Name,
    FileSize,
    Dimensions,
    DateAdded,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SortDirection {
    Ascending,
    Descending,
}

#[derive(Clone, Copy)]
struct PhotoSort {
    field: SortField,
    direction: SortDirection,
}

impl PhotoSort {
    fn load() -> Self {
        let field = match crate::catalog::setting("photo-sort-field")
            .ok()
            .flatten()
            .as_deref()
        {
            Some("name") => SortField::Name,
            Some("file-size") => SortField::FileSize,
            Some("dimensions") => SortField::Dimensions,
            Some("date-added") => SortField::DateAdded,
            _ => SortField::DateTaken,
        };
        let direction = match crate::catalog::setting("photo-sort-direction")
            .ok()
            .flatten()
            .as_deref()
        {
            Some("ascending") => SortDirection::Ascending,
            _ => SortDirection::Descending,
        };
        Self { field, direction }
    }
}

struct FolderNavEntry {
    path: PathBuf,
    label: String,
    depth: u32,
    count: u32,
    target_index: usize,
}

#[derive(Clone)]
struct ThumbJob {
    path: String,
    mtime: i64,
    size_bytes: i64,
    rotation: i32,
    cache_key: String,
}

struct ThumbResult {
    path: String,
    cache_key: String,
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
        return PathBuf::from(dir).join("picasa-rs");
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".cache")
        .join("picasa-rs")
}

fn thumbnail_identity(source: &str, mtime: i64, size_bytes: i64, rotation: i32) -> String {
    format!("{source}\0{mtime}\0{size_bytes}\0{}", rotation.rem_euclid(360))
}

fn thumbnail_cache_path(
    source: &str,
    mtime: i64,
    size_bytes: i64,
    cache_dir: &Path,
) -> PathBuf {
    let mut hasher = DefaultHasher::new();
    source.hash(&mut hasher);
    mtime.hash(&mut hasher);
    size_bytes.hash(&mut hasher);
    cache_dir.join(format!("{:016x}.png", hasher.finish()))
}

fn decode_thumb_cached(job: &ThumbJob, cache_dir: &Path) -> ThumbResult {
    let cached = thumbnail_cache_path(&job.path, job.mtime, job.size_bytes, cache_dir);

    let disk_hit = cached.is_file();
    if trace_enabled() {
        eprintln!(
            "PIC_TRACE thumb_disk_cache hit={} source={} cache={}",
            disk_hit,
            job.path,
            cached.display()
        );
    }

    let decoded = if disk_hit {
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
        let source = image::ImageReader::open(&job.path)
            .ok()
            .and_then(|reader| reader.with_guessed_format().ok())
            .and_then(|reader| reader.decode().ok());

        let Some(source) = source else {
            return ThumbResult {
                path: job.path.clone(),
                cache_key: job.cache_key.clone(),
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

    let image = match job.rotation.rem_euclid(360) {
        90 => image.rotate90(),
        180 => image.rotate180(),
        270 => image.rotate270(),
        _ => image,
    };
    let rgba = image.to_rgba8();
    let (width, height) = rgba.dimensions();
    ThumbResult {
        path: job.path.clone(),
        cache_key: job.cache_key.clone(),
        width: width as i32,
        height: height as i32,
        stride: width as usize * 4,
        rgba: rgba.into_raw(),
    }
}

pub fn build_window(app: &adw::Application) -> adw::ApplicationWindow {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Picasa iPhoto Clone")
        .default_width(1440)
        .default_height(900)
        .build();

    let groups = gio::ListStore::new::<glib::BoxedAnyObject>();
    let group_selection = gtk::NoSelection::new(Some(groups.clone()));
    let master_groups: Rc<RefCell<Vec<FolderGroupData>>> = Rc::new(RefCell::new(Vec::new()));
    let roots: Rc<RefCell<Vec<PathBuf>>> = Rc::new(RefCell::new(Vec::new()));
    let mode = Rc::new(Cell::new(ViewMode::Photos));
    let search_text = Rc::new(RefCell::new(String::new()));
    let favorite_paths = Rc::new(RefCell::new(crate::catalog::favorite_paths().unwrap_or_default()));

    let tile_size = Rc::new(Cell::new(DEFAULT_TILE));
    let live_tiles: Rc<RefCell<Vec<glib::WeakRef<gtk::Widget>>>> =
        Rc::new(RefCell::new(Vec::new()));
    let live_grids: Rc<RefCell<Vec<(glib::WeakRef<gtk::GridView>, gio::ListStore)>>> =
        Rc::new(RefCell::new(Vec::new()));
    let current_columns = Rc::new(Cell::new(6u32));
    let favorite_changed: Rc<RefCell<Option<Rc<dyn Fn()>>>> =
        Rc::new(RefCell::new(None));
    let selection_changed: Rc<RefCell<Option<Rc<dyn Fn(Option<PhotoObject>)>>>> =
        Rc::new(RefCell::new(None));
    let selected_photo: Rc<RefCell<Option<PhotoObject>>> = Rc::new(RefCell::new(None));

    let viewer = Rc::new(crate::viewer::Viewer::new());

    let thumb_cache: Rc<RefCell<HashMap<String, gtk::gdk::Texture>>> =
        Rc::new(RefCell::new(HashMap::new()));
    let thumb_pending: Rc<RefCell<HashMap<String, Vec<glib::WeakRef<gtk::Picture>>>>> =
        Rc::new(RefCell::new(HashMap::new()));
    let thumb_inflight: Rc<RefCell<HashSet<String>>> =
        Rc::new(RefCell::new(HashSet::new()));

    let (thumb_job_tx, thumb_job_rx) = mpsc::channel::<ThumbJob>();
    let (thumb_result_tx, thumb_result_rx) = mpsc::channel::<ThumbResult>();
    let shared_jobs = Arc::new(Mutex::new(thumb_job_rx));
    let disk_cache_dir = app_cache_dir().join("thumbs");
    let _ = fs::create_dir_all(&disk_cache_dir);

    for _ in 0..3 {
        let jobs = shared_jobs.clone();
        let results = thumb_result_tx.clone();
        let cache_dir = disk_cache_dir.clone();
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

            let decoded = decode_thumb_cached(&job, &cache_dir);
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
                inflight.borrow_mut().remove(&result.cache_key);
                let waiters = pending.borrow_mut().remove(&result.cache_key).unwrap_or_default();

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
                    cache.insert(result.cache_key.clone(), texture.clone());
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
    sidebar.add_css_class("navigation-sidebar");
    sidebar.set_size_request(250, -1);

    let library_heading = gtk::Label::new(Some("Library"));
    library_heading.add_css_class("sidebar-heading");
    library_heading.set_xalign(0.0);
    sidebar.append(&library_heading);

    let (photos_button, photos_count) =
        make_sidebar_button("image-x-generic-symbolic", "Photos", "0");
    let (favorites_button, favorites_count) =
        make_sidebar_button("emote-love-symbolic", "Favourites", "0");
    let (recent_button, recent_count) =
        make_sidebar_button("appointment-soon-symbolic", "Recently Added", "0");
    sidebar.append(&photos_button);
    sidebar.append(&favorites_button);
    sidebar.append(&recent_button);

    let albums_heading = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    albums_heading.set_margin_top(8);
    let albums_label = gtk::Label::new(Some("Albums"));
    albums_label.add_css_class("sidebar-heading");
    albums_label.set_xalign(0.0);
    albums_label.set_hexpand(true);
    let create_album_button = gtk::Button::from_icon_name("list-add-symbolic");
    create_album_button.add_css_class("flat");
    create_album_button.set_tooltip_text(Some("Create Album"));
    albums_heading.append(&albums_label);
    albums_heading.append(&create_album_button);
    sidebar.append(&albums_heading);

    let album_box = gtk::Box::new(gtk::Orientation::Vertical, 1);
    sidebar.append(&album_box);

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
        let viewer = viewer.clone();

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

            let viewer = viewer.clone();
            grid.connect_activate(move |grid, position| {
                let Some(model) = grid.model() else {
                    return;
                };
                let photos = (0..model.n_items())
                    .filter_map(|item| model.item(item).and_downcast::<PhotoObject>())
                    .collect::<Vec<_>>();
                viewer.open(photos, position as usize);
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
        let selection_changed = selection_changed.clone();

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
            {
                let selection_changed = selection_changed.clone();
                selection.connect_selection_changed(move |selection, _, _| {
                    let selected = selection.selected_item().and_downcast::<PhotoObject>();
                    if let Some(callback) = selection_changed.borrow().as_ref().cloned() {
                        callback(selected);
                    }
                });
            }
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
    list.add_css_class("photo-grid");
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

    let left_header = adw::HeaderBar::new();
    left_header.set_height_request(46);
    left_header.set_show_start_title_buttons(true);
    left_header.set_show_end_title_buttons(false);
    left_header.add_css_class("layout-left-header");

    let app_title = gtk::Label::new(Some("PIC"));
    app_title.add_css_class("app-title");
    left_header.set_title_widget(Some(&app_title));

    let hide_sidebar = gtk::Button::from_icon_name("sidebar-hide-symbolic");
    hide_sidebar.set_tooltip_text(Some("Hide sidebar"));
    hide_sidebar.add_css_class("flat");
    hide_sidebar.set_size_request(28, 28);
    left_header.pack_end(&hide_sidebar);

    let left_column = gtk::Box::new(gtk::Orientation::Vertical, 0);
    left_column.set_vexpand(true);
    left_column.add_css_class("layout-left-column");
    sidebar.set_vexpand(true);
    left_column.append(&left_header);
    left_column.append(&sidebar);

    let right_header = adw::HeaderBar::new();
    right_header.set_height_request(46);
    right_header.set_hexpand(true);
    right_header.set_show_start_title_buttons(false);
    right_header.set_show_end_title_buttons(true);
    right_header.add_css_class("layout-right-header");

    let show_sidebar = gtk::Button::from_icon_name("sidebar-show-symbolic");
    show_sidebar.set_tooltip_text(Some("Show sidebar"));
    show_sidebar.add_css_class("flat");
    show_sidebar.set_visible(false);
    right_header.pack_start(&show_sidebar);

    let search = gtk::SearchEntry::new();
    search.set_placeholder_text(Some("Search photos"));
    search.set_width_chars(18);
    search.set_size_request(220, -1);
    search.set_hexpand(true);
    search.add_css_class("search-field");

    let search_area = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    search_area.set_valign(gtk::Align::Center);
    search_area.set_size_request(220, -1);
    search_area.set_hexpand(true);
    search_area.append(&search);
    right_header.set_title_widget(Some(&search_area));

    let theme_button = gtk::MenuButton::new();
    theme_button.set_icon_name("emblem-system-symbolic");
    theme_button.set_tooltip_text(Some("Appearance"));
    theme_button.add_css_class("flat");
    right_header.pack_end(&theme_button);

    let refresh = gtk::Button::from_icon_name("view-refresh-symbolic");
    refresh.set_tooltip_text(Some("Refresh library folders"));
    refresh.add_css_class("flat");
    right_header.pack_end(&refresh);

    let open = gtk::Button::from_icon_name("folder-new-symbolic");
    open.set_tooltip_text(Some("Import Folder"));
    open.add_css_class("flat");
    right_header.pack_end(&open);

    let zoom = gtk::Scale::with_range(
        gtk::Orientation::Horizontal,
        f64::from(MIN_TILE),
        f64::from(MAX_TILE),
        1.0,
    );
    zoom.set_value(f64::from(DEFAULT_TILE));
    zoom.set_draw_value(false);
    zoom.set_size_request(150, -1);
    zoom.set_tooltip_text(Some("Thumbnail size"));

    let plus = gtk::Button::with_label("+");
    plus.add_css_class("photo-action-button");
    plus.set_tooltip_text(Some("Larger thumbnails"));
    let minus = gtk::Button::with_label("−");
    minus.add_css_class("photo-action-button");
    minus.set_tooltip_text(Some("Smaller thumbnails"));

    let bottom_bar = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    bottom_bar.set_height_request(58);
    bottom_bar.set_margin_start(16);
    bottom_bar.set_margin_end(16);
    bottom_bar.add_css_class("photo-info-bar");

    let status = gtk::Box::new(gtk::Orientation::Vertical, 1);
    status.set_valign(gtk::Align::Center);
    content_title.set_xalign(0.0);
    content_title.add_css_class("info-title");
    count_label.set_xalign(0.0);
    count_label.add_css_class("dim-label");
    status.append(&content_title);
    status.append(&count_label);
    bottom_bar.append(&status);

    let selected_info = gtk::Box::new(gtk::Orientation::Horizontal, 18);
    selected_info.set_valign(gtk::Align::Center);
    selected_info.set_visible(false);

    let selected_text = gtk::Box::new(gtk::Orientation::Vertical, 1);
    let selected_name = gtk::Label::new(None);
    selected_name.set_xalign(0.0);
    selected_name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    selected_name.add_css_class("info-title");
    let selected_folder = gtk::Label::new(None);
    selected_folder.set_xalign(0.0);
    selected_folder.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    selected_folder.add_css_class("dim-label");
    selected_text.append(&selected_name);
    selected_text.append(&selected_folder);
    selected_info.append(&selected_text);

    let dimensions_label = gtk::Label::new(None);
    dimensions_label.add_css_class("metric-val");
    let size_label = gtk::Label::new(None);
    size_label.add_css_class("metric-val");
    selected_info.append(&dimensions_label);
    selected_info.append(&size_label);
    bottom_bar.append(&selected_info);

    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    bottom_bar.append(&spacer);

    let favorite_action = gtk::Button::from_icon_name("non-starred-symbolic");
    favorite_action.add_css_class("photo-action-button");
    favorite_action.add_css_class("favorite-btn");
    favorite_action.set_tooltip_text(Some("Add to Favourites"));
    favorite_action.set_sensitive(false);
    bottom_bar.append(&favorite_action);

    let export_action = gtk::Button::from_icon_name("document-save-symbolic");
    export_action.add_css_class("photo-action-button");
    export_action.set_tooltip_text(Some("Export photo"));
    export_action.set_sensitive(false);
    bottom_bar.append(&export_action);

    let rotate_action = gtk::Button::from_icon_name("object-rotate-right-symbolic");
    rotate_action.add_css_class("photo-action-button");
    rotate_action.set_tooltip_text(Some("Rotate clockwise"));
    rotate_action.set_sensitive(false);
    bottom_bar.append(&rotate_action);

    let one_to_one = gtk::ToggleButton::with_label("1:1");
    one_to_one.add_css_class("photo-action-button");
    one_to_one.set_tooltip_text(Some("Show photo at 100%"));
    bottom_bar.append(&one_to_one);
    bottom_bar.append(&minus);
    bottom_bar.append(&zoom);
    bottom_bar.append(&plus);

    let right_column = gtk::Box::new(gtk::Orientation::Vertical, 0);
    right_column.set_hexpand(true);
    right_column.set_vexpand(true);
    right_column.append(&right_header);

    let gallery_overlay = gtk::Overlay::new();
    gallery_overlay.set_hexpand(true);
    gallery_overlay.set_vexpand(true);
    gallery_overlay.set_child(Some(&scroller));
    gallery_overlay.add_overlay(&viewer.root);
    right_column.append(&gallery_overlay);
    right_column.append(&bottom_bar);

    let main_split = adw::OverlaySplitView::new();
    main_split.set_sidebar(Some(&left_column));
    main_split.set_content(Some(&right_column));
    main_split.set_min_sidebar_width(200.0);
    main_split.set_max_sidebar_width(600.0);
    main_split.set_sidebar_width_fraction(0.22);
    main_split.set_show_sidebar(true);
    main_split.set_enable_show_gesture(true);
    main_split.set_enable_hide_gesture(true);

    {
        let selected_photo = selected_photo.clone();
        let selected_info = selected_info.clone();
        let selected_name = selected_name.clone();
        let selected_folder = selected_folder.clone();
        let dimensions_label = dimensions_label.clone();
        let size_label = size_label.clone();
        let favorite_action = favorite_action.clone();
        let export_action = export_action.clone();
        let rotate_action = rotate_action.clone();

        selection_changed.replace(Some(Rc::new(move |photo| {
            selected_photo.replace(photo.clone());
            let Some(photo) = photo else {
                selected_info.set_visible(false);
                favorite_action.set_sensitive(false);
                export_action.set_sensitive(false);
                rotate_action.set_sensitive(false);
                favorite_action.remove_css_class("active");
                return;
            };

            let metadata = crate::catalog::photo_by_id(photo.id()).ok().flatten();
            let path = photo.path();
            let name = Path::new(&path)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("Photo");
            selected_name.set_label(name);

            if let Some(record) = metadata {
                selected_folder.set_label(&record.folder_path);
                let dimensions = match (record.width, record.height) {
                    (Some(width), Some(height)) if width > 0 && height > 0 => {
                        format!("{width} × {height}")
                    }
                    _ => "Unknown size".to_string(),
                };
                dimensions_label.set_label(&dimensions);
                size_label.set_label(&format_file_size(record.size_bytes.unwrap_or_default()));
            } else {
                selected_folder.set_label("");
                dimensions_label.set_label("");
                size_label.set_label("");
            }

            favorite_action.set_sensitive(true);
            export_action.set_sensitive(true);
            rotate_action.set_sensitive(true);
            if photo.favorite() {
                favorite_action.set_icon_name("starred-symbolic");
                favorite_action.add_css_class("active");
                favorite_action.set_tooltip_text(Some("Remove from Favourites"));
            } else {
                favorite_action.set_icon_name("non-starred-symbolic");
                favorite_action.remove_css_class("active");
                favorite_action.set_tooltip_text(Some("Add to Favourites"));
            }
            selected_info.set_visible(true);
        })));
    }

    {
        let selection_changed = selection_changed.clone();
        viewer.set_photo_changed_handler(move |photo| {
            if let Some(callback) = selection_changed.borrow().as_ref().cloned() {
                callback(Some(photo));
            }
        });
    }

    {
        let one_to_one_button = one_to_one.clone();
        viewer.set_one_to_one_changed_handler(move |enabled| {
            if one_to_one_button.is_active() != enabled {
                one_to_one_button.set_active(enabled);
            }
        });
    }

    {
        let selected_photo = selected_photo.clone();
        let favorite_changed = favorite_changed.clone();
        let favorite_paths = favorite_paths.clone();
        favorite_action.connect_clicked(move |button| {
            let Some(photo) = selected_photo.borrow().clone() else {
                return;
            };
            let favorite = !photo.favorite();
            if let Err(error) = crate::catalog::set_favorite_by_path(&photo.path(), favorite) {
                eprintln!("PIC_REBUILD favourite_update_failed error={error:#}");
                return;
            }
            photo.set_favorite(favorite);
            {
                let mut paths = favorite_paths.borrow_mut();
                let path = photo.path();
                if favorite {
                    paths.insert(path);
                } else {
                    paths.remove(&path);
                }
            }
            button.set_icon_name(if favorite {
                "starred-symbolic"
            } else {
                "non-starred-symbolic"
            });
            if favorite {
                button.add_css_class("active");
            } else {
                button.remove_css_class("active");
            }
            if let Some(callback) = favorite_changed.borrow().as_ref().cloned() {
                glib::idle_add_local_once(move || callback());
            }
        });
    }

    {
        let selected_photo = selected_photo.clone();
        let viewer = viewer.clone();
        let thumb_cache = thumb_cache.clone();
        let groups = groups.clone();
        let master = master_groups.clone();
        let mode = mode.clone();
        let search_text = search_text.clone();
        let count_label = count_label.clone();
        let title = content_title.clone();
        let photos_button = photos_button.clone();
        let favorites_button = favorites_button.clone();
        let recent_button = recent_button.clone();

        rotate_action.connect_clicked(move |_| {
            let photo = viewer
                .current_photo()
                .or_else(|| selected_photo.borrow().clone());
            let Some(photo) = photo else {
                return;
            };
            let rotation = (photo.rotation() + 90).rem_euclid(360);
            if let Err(error) = crate::catalog::set_rotation(photo.id(), rotation) {
                eprintln!("PIC_REBUILD rotate_failed error={error:#}");
                return;
            }
            photo.set_rotation(rotation);
            thumb_cache.borrow_mut().clear();
            apply_view(
                &groups,
                &master.borrow(),
                mode.get(),
                &search_text.borrow(),
                &count_label,
                &title,
                &photos_button,
                &favorites_button,
                &recent_button,
            );
            viewer.refresh_current();
        });
    }

    {
        let selected_photo = selected_photo.clone();
        let parent = window.clone();
        export_action.connect_clicked(move |_| {
            let Some(photo) = selected_photo.borrow().clone() else {
                return;
            };
            let source = gio::File::for_path(photo.path());
            let path = photo.path();
            let suggested_name = Path::new(&path)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("photo.jpg")
                .to_string();
            let dialog = gtk::FileDialog::builder()
                .title("Export Photo")
                .initial_name(&suggested_name)
                .build();
            dialog.save(Some(&parent), gio::Cancellable::NONE, move |result| {
                let Ok(destination) = result else {
                    return;
                };
                if let Err(error) = source.copy(
                    &destination,
                    gio::FileCopyFlags::OVERWRITE,
                    gio::Cancellable::NONE,
                    None,
                ) {
                    eprintln!("PIC_REBUILD export_failed error={error}");
                }
            });
        });
    }

    {
        let viewer = viewer.clone();
        one_to_one.connect_toggled(move |button| viewer.set_one_to_one(button.is_active()));
    }
    {
        let one_to_one = one_to_one.clone();
        viewer.root.connect_visible_notify(move |root| {
            if !root.is_visible() {
                one_to_one.set_active(false);
            }
        });
    }

    {
        let main_split = main_split.clone();
        hide_sidebar.connect_clicked(move |_| main_split.set_show_sidebar(false));
    }
    {
        let main_split = main_split.clone();
        show_sidebar.connect_clicked(move |_| main_split.set_show_sidebar(true));
    }
    {
        let show_sidebar = show_sidebar.clone();
        let right_header = right_header.clone();
        main_split.connect_show_sidebar_notify(move |split| {
            let visible = split.shows_sidebar();
            show_sidebar.set_visible(!visible);
            right_header.set_show_start_title_buttons(!visible);
        });
    }

    window.set_content(Some(&main_split));

    install_css();

    let theme_manager = crate::theme::ThemeManager::new().map(Rc::new);
    let saved_theme = crate::catalog::setting("appearance-theme")
        .ok()
        .flatten()
        .unwrap_or_else(|| "standard".to_string());
    if let Some(manager) = theme_manager.as_ref() {
        if let Err(error) = manager.apply(&saved_theme) {
            eprintln!("PIC_REBUILD theme_load_failed theme={saved_theme} error={error:#}");
            let _ = manager.apply("standard");
        }

        let menu = gtk::Box::new(gtk::Orientation::Vertical, 2);
        menu.set_margin_top(6);
        menu.set_margin_bottom(6);
        menu.set_margin_start(6);
        menu.set_margin_end(6);

        let heading = gtk::Label::new(Some("Theme"));
        heading.set_xalign(0.0);
        heading.add_css_class("dim-label");
        menu.append(&heading);

        let list = gtk::Box::new(gtk::Orientation::Vertical, 1);
        for theme_name in crate::theme::ThemeManager::available_themes() {
            let button = gtk::Button::with_label(&crate::theme::pretty_name(&theme_name));
            button.add_css_class("flat");
            if theme_name == saved_theme {
                button.add_css_class("suggested-action");
            }
            let manager = manager.clone();
            let popover_button = theme_button.clone();
            button.connect_clicked(move |_| {
                if let Err(error) = manager.apply(&theme_name) {
                    eprintln!("PIC_REBUILD theme_apply_failed theme={theme_name} error={error:#}");
                    return;
                }
                if let Err(error) = crate::catalog::set_setting("appearance-theme", &theme_name) {
                    eprintln!("PIC_REBUILD theme_save_failed error={error:#}");
                }
                if let Some(popover) = popover_button.popover() {
                    popover.popdown();
                }
            });
            list.append(&button);
        }

        let scroll = gtk::ScrolledWindow::new();
        scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroll.set_max_content_height(420);
        scroll.set_propagate_natural_height(true);
        scroll.set_child(Some(&list));
        menu.append(&scroll);

        let popover = gtk::Popover::new();
        popover.set_child(Some(&menu));
        theme_button.set_popover(Some(&popover));
    }

    {
        let groups = groups.clone();
        let master = master_groups.clone();
        let mode = mode.clone();
        let count_label = count_label.clone();
        let content_title = content_title.clone();
        let photos_button = photos_button.clone();
        let favorites_button = favorites_button.clone();
        let recent_button = recent_button.clone();
        let favorites_count = favorites_count.clone();
        let search_text = search_text.clone();

        let refresh: Rc<dyn Fn()> = Rc::new(move || {
            let favorite_total = count_favorites(&master.borrow());
            favorites_count.set_label(&favorite_total.to_string());

            apply_view(
                &groups,
                &master.borrow(),
                mode.get(),
                &search_text.borrow(),
                &count_label,
                &content_title,
                &photos_button,
                &favorites_button,
                &recent_button,
            );
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
        let recent_button_for_style = recent_button.clone();
        let search_text = search_text.clone();

        photos_button.connect_clicked(move |_| {
            mode.set(ViewMode::Photos);
            apply_view(
                &groups,
                &master.borrow(),
                ViewMode::Photos,
                &search_text.borrow(),
                &count_label,
                &title,
                &photos_button_for_style,
                &favorites_button_for_style,
                &recent_button_for_style,
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
        let recent_button_for_style = recent_button.clone();
        let search_text = search_text.clone();

        favorites_button.connect_clicked(move |_| {
            mode.set(ViewMode::Favorites);
            apply_view(
                &groups,
                &master.borrow(),
                ViewMode::Favorites,
                &search_text.borrow(),
                &count_label,
                &title,
                &photos_button_for_style,
                &favorites_button_for_style,
                &recent_button_for_style,
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
        let recent_button_for_style = recent_button.clone();
        let search_text = search_text.clone();

        recent_button.connect_clicked(move |_| {
            mode.set(ViewMode::RecentlyAdded);
            apply_view(
                &groups,
                &master.borrow(),
                ViewMode::RecentlyAdded,
                &search_text.borrow(),
                &count_label,
                &title,
                &photos_button,
                &favorites_button,
                &recent_button_for_style,
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
        let recent_button = recent_button.clone();
        let search_text = search_text.clone();

        search.connect_search_changed(move |entry| {
            search_text.replace(entry.text().trim().to_string());
            apply_view(
                &groups,
                &master.borrow(),
                mode.get(),
                &search_text.borrow(),
                &count_label,
                &title,
                &photos_button,
                &favorites_button,
                &recent_button,
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
        let search = search.clone();
        let photos_button = photos_button.clone();
        let favorites_button = favorites_button.clone();
        let recent_button = recent_button.clone();
        let search_text = search_text.clone();

        Rc::new(move |index| {
            if trace_enabled() {
                eprintln!("PIC_TRACE folder_jump target_index={}", index);
            }

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
                    "",
                    &count_label,
                    &title,
                    &photos_button,
                    &favorites_button,
                    &recent_button,
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

    let album_activate: Rc<dyn Fn(i64)> = {
        let groups = groups.clone();
        let master = master_groups.clone();
        let mode = mode.clone();
        let count_label = count_label.clone();
        let title = content_title.clone();
        let photos_button = photos_button.clone();
        let favorites_button = favorites_button.clone();
        let recent_button = recent_button.clone();
        let search = search.clone();
        let search_text = search_text.clone();

        Rc::new(move |album_id| {
            if !search_text.borrow().is_empty() {
                search_text.borrow_mut().clear();
                search.set_text("");
            }
            mode.set(ViewMode::Album(album_id));
            apply_view(
                &groups,
                &master.borrow(),
                ViewMode::Album(album_id),
                "",
                &count_label,
                &title,
                &photos_button,
                &favorites_button,
                &recent_button,
            );
        })
    };
    rebuild_album_sidebar(&album_box, &album_activate);

    {
        let parent = window.clone();
        let album_box = album_box.clone();
        let album_activate = album_activate.clone();
        create_album_button.connect_clicked(move |_| {
            let dialog = gtk::Dialog::builder()
                .transient_for(&parent)
                .modal(true)
                .title("Create Album")
                .build();
            dialog.add_button("Cancel", gtk::ResponseType::Cancel);
            dialog.add_button("Create", gtk::ResponseType::Accept);
            let entry = gtk::Entry::new();
            entry.set_placeholder_text(Some("Album name"));
            entry.set_activates_default(true);
            entry.set_margin_top(12);
            entry.set_margin_bottom(12);
            entry.set_margin_start(12);
            entry.set_margin_end(12);
            dialog.content_area().append(&entry);
            dialog.set_default_response(gtk::ResponseType::Accept);

            entry.grab_focus();
            let entry_for_response = entry.clone();
            let album_box = album_box.clone();
            let album_activate = album_activate.clone();
            dialog.connect_response(move |dialog, response| {
                if response == gtk::ResponseType::Accept {
                    let name = entry_for_response.text();
                    if let Err(error) = crate::catalog::create_album(name.as_str()) {
                        eprintln!("PIC_REBUILD album_create_failed error={error:#}");
                    } else {
                        rebuild_album_sidebar(&album_box, &album_activate);
                    }
                }
                dialog.close();
            });
            dialog.present();
        });
    }

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

            let tile_widgets = {
                let mut tiles = live_tiles.borrow_mut();
                let mut widgets = Vec::with_capacity(tiles.len());
                tiles.retain(|weak| {
                    let Some(widget) = weak.upgrade() else {
                        return false;
                    };
                    widgets.push(widget);
                    true
                });
                widgets
            };

            let touched = tile_widgets.len();
            for widget in tile_widgets {
                widget.set_size_request(size, size);
            }

            update_grid_layout(
                scroller.width(),
                size,
                &current_columns,
                &live_grids,
                &list,
            );

            if trace_enabled() {
                eprintln!(
                    "PIC_TRACE zoom size={} columns={} width={} realized_tiles={} elapsed_us={}",
                    size,
                    current_columns.get(),
                    scroller.width(),
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
        let recent_count = recent_count.clone();
        let recent_button = recent_button.clone();
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
            let recent_count = recent_count.clone();
            let recent_button = recent_button.clone();
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

                let (finished_tx, finished_rx) = mpsc::channel();
                std::thread::spawn(move || {
                    let result = crate::catalog::import_root(&path);
                    let _ = finished_tx.send(result);
                });

                let groups = groups.clone();
                let master = master.clone();
                let roots_state = roots_state.clone();
                let count_label = count_label.clone();
                let title = title.clone();
                let photos_button = photos_button.clone();
                let favorites_button = favorites_button.clone();
                let photos_count = photos_count.clone();
                let favorites_count = favorites_count.clone();
                let recent_count = recent_count.clone();
                let recent_button = recent_button.clone();
                let folder_box = folder_box.clone();
                let folder_activate = folder_activate.clone();
                let mode = mode.clone();
                let favorite_paths = favorite_paths.clone();

                glib::timeout_add_local(Duration::from_millis(50), move || {
                    match finished_rx.try_recv() {
                        Ok(Ok(summary)) => {
                            if trace_enabled() {
                                eprintln!(
                                    "PIC_REBUILD import_complete photos={} folders={}",
                                    summary.photos_seen, summary.folders_seen
                                );
                            }
                            let new_roots = crate::catalog::roots().unwrap_or_default();
                            favorite_paths.replace(
                                crate::catalog::favorite_paths().unwrap_or_default(),
                            );
                            roots_state.replace(new_roots.clone());
                            master.replace(load_catalog_groups());
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
                                &recent_count,
                                &recent_button,
                            );
                            glib::ControlFlow::Break
                        }
                        Ok(Err(error)) => {
                            eprintln!("PIC_REBUILD import_failed error={error:#}");
                            glib::ControlFlow::Break
                        }
                        Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                        Err(mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
                    }
                });
            });
        });
    }

    {
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
        let recent_count = recent_count.clone();
        let recent_button = recent_button.clone();
        let folder_box = folder_box.clone();
        let folder_activate = folder_activate.clone();
        let mode = mode.clone();
        let refresh_button = refresh.clone();

        refresh.connect_clicked(move |_| {
            let roots_to_refresh = crate::catalog::roots().unwrap_or_default();
            if roots_to_refresh.is_empty() {
                return;
            }
            refresh_button.set_sensitive(false);

            let (finished_tx, finished_rx) = mpsc::channel();
            std::thread::spawn(move || {
                let mut failed = 0usize;
                for root in roots_to_refresh {
                    if let Err(error) = crate::catalog::import_root(&root) {
                        failed += 1;
                        eprintln!(
                            "PIC_REBUILD refresh_root_failed path={} error={error:#}",
                            root.display()
                        );
                    }
                }
                let _ = finished_tx.send(failed);
            });

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
            let recent_count = recent_count.clone();
            let recent_button = recent_button.clone();
            let folder_box = folder_box.clone();
            let folder_activate = folder_activate.clone();
            let mode = mode.clone();
            let refresh_button = refresh_button.clone();

            glib::timeout_add_local(Duration::from_millis(50), move || {
                match finished_rx.try_recv() {
                    Ok(failed) => {
                        let new_roots = crate::catalog::roots().unwrap_or_default();
                        favorite_paths
                            .replace(crate::catalog::favorite_paths().unwrap_or_default());
                        roots_state.replace(new_roots.clone());
                        master.replace(load_catalog_groups());
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
                            &recent_count,
                            &recent_button,
                        );
                        refresh_button.set_sensitive(true);
                        if trace_enabled() {
                            eprintln!("PIC_REBUILD refresh_complete failed_roots={failed}");
                        }
                        glib::ControlFlow::Break
                    }
                    Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        refresh_button.set_sensitive(true);
                        glib::ControlFlow::Break
                    }
                }
            });
        });
    }

    let explicit_roots = std::env::args_os()
        .skip(1)
        .map(PathBuf::from)
        .filter(|path| path.is_dir())
        .collect::<Vec<_>>();
    for root in explicit_roots {
        if let Err(error) = crate::catalog::import_root(&root) {
            eprintln!("PIC_REBUILD startup_import_failed path={} error={error:#}", root.display());
        }
    }

    let startup_paths = crate::catalog::roots().unwrap_or_default();
    roots.replace(startup_paths.clone());
    master_groups.replace(load_catalog_groups());

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
        &recent_count,
        &recent_button,
    );

    if trace_enabled() {
        eprintln!(
            "PIC_REBUILD library_loaded photos={} folders={} roots={}",
            count_photos(&master_groups.borrow()),
            master_groups.borrow().len(),
            startup_paths.len()
        );
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
    thumb_job_tx: mpsc::Sender<ThumbJob>,
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
            overlay.add_css_class("photo-frame");
            overlay.add_css_class("photo-tile");
            overlay.set_overflow(gtk::Overflow::Hidden);
            overlay.set_size_request(tile_size.get(), tile_size.get());
            overlay.set_hexpand(false);
            overlay.set_vexpand(false);

            let picture = gtk::Picture::new();
            picture.add_css_class("thumbnail");
            picture.set_size_request(1, 1);
            picture.set_hexpand(true);
            picture.set_vexpand(true);
            picture.set_halign(gtk::Align::Fill);
            picture.set_valign(gtk::Align::Fill);
            picture.set_can_shrink(true);
            picture.set_content_fit(gtk::ContentFit::Cover);
            overlay.set_child(Some(&picture));

            let offline_badge = gtk::Button::with_label("!");
            offline_badge.add_css_class("offline-badge");
            offline_badge.set_halign(gtk::Align::End);
            offline_badge.set_valign(gtk::Align::Start);
            offline_badge.set_margin_top(6);
            offline_badge.set_margin_end(38);
            offline_badge.set_tooltip_text(Some("Original photo unavailable; cached thumbnail shown"));
            offline_badge.set_sensitive(false);
            offline_badge.set_visible(false);
            overlay.add_overlay(&offline_badge);

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
                    let _ = crate::catalog::set_favorite_by_path(&path, new_value);
                }

                if trace_enabled() {
                    eprintln!(
                        "PIC_TRACE favourite changed={} path={}",
                        new_value,
                        path
                    );
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

            let list_item_for_context = list_item.clone();
            let overlay_for_context = overlay.clone();
            let right_click = gtk::GestureClick::new();
            right_click.set_button(3);
            right_click.connect_pressed(move |gesture, _, _, _| {
                let Some(photo) = list_item_for_context
                    .item()
                    .and_downcast::<PhotoObject>()
                else {
                    return;
                };
                let albums = crate::catalog::albums().unwrap_or_default();
                if albums.is_empty() {
                    return;
                }

                let popover = gtk::Popover::new();
                popover.set_has_arrow(true);
                popover.set_parent(&overlay_for_context);
                let menu = gtk::Box::new(gtk::Orientation::Vertical, 2);
                menu.set_margin_top(6);
                menu.set_margin_bottom(6);
                menu.set_margin_start(6);
                menu.set_margin_end(6);

                let heading = gtk::Label::new(Some("Add to Album"));
                heading.set_xalign(0.0);
                heading.add_css_class("dim-label");
                menu.append(&heading);

                for album in albums {
                    let button = gtk::Button::with_label(&album.name);
                    button.add_css_class("flat");
                    let popover_for_click = popover.clone();
                    let photo_id = photo.id();
                    button.connect_clicked(move |_| {
                        if let Err(error) =
                            crate::catalog::add_photo_to_album(album.id, photo_id)
                        {
                            eprintln!("PIC_REBUILD album_add_failed error={error:#}");
                        }
                        popover_for_click.popdown();
                    });
                    menu.append(&button);
                }
                popover.set_child(Some(&menu));
                popover.popup();
                gesture.set_state(gtk::EventSequenceState::Claimed);
            });
            overlay.add_controller(right_click);

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
            let offline_badge = favorite_button
                .prev_sibling()
                .and_downcast::<gtk::Button>();

            let path = photo.path();
            let cache_key =
                thumbnail_identity(&path, photo.mtime(), photo.size_bytes(), photo.rotation());
            if let Some(badge) = offline_badge {
                badge.set_visible(!Path::new(&path).is_file());
            }
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

            if let Some(texture) = cache.borrow().get(&cache_key).cloned() {
                picture.set_paintable(Some(&texture));
                if trace_enabled() {
                    eprintln!(
                        "PIC_TRACE thumb_memory_cache_hit position={} path={}",
                        list_item.position(),
                        path
                    );
                }
                return;
            }

            pending
                .borrow_mut()
                .entry(cache_key.clone())
                .or_default()
                .push(picture.downgrade());

            if inflight.borrow_mut().insert(cache_key.clone()) {
                let _ = jobs.send(ThumbJob {
                    path: path.clone(),
                    mtime: photo.mtime(),
                    size_bytes: photo.size_bytes(),
                    rotation: photo.rotation(),
                    cache_key: cache_key.clone(),
                });
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
    let previous_columns = current_columns.replace(columns);

    let grids_to_update = {
        let mut grids = live_grids.borrow_mut();
        let mut realized = Vec::with_capacity(grids.len());
        grids.retain(|(weak, model)| {
            let Some(grid) = weak.upgrade() else {
                return false;
            };
            realized.push((grid, model.clone()));
            true
        });
        realized
    };

    let realized_grid_count = grids_to_update.len();
    for (grid, model) in grids_to_update {
        grid.set_min_columns(columns);
        grid.set_max_columns(columns);
        let rows = (model.n_items() + columns - 1) / columns;
        grid.set_height_request((rows as i32 * (size + 12)).max(size + 12));
        grid.queue_resize();
    }

    list.queue_resize();

    if trace_enabled() && previous_columns != columns {
        eprintln!(
            "PIC_TRACE layout width={} tile={} columns={} previous_columns={} realized_grids={}",
            width,
            size,
            columns,
            previous_columns,
            realized_grid_count
        );
    }
}

fn sort_photo_records(
    photos: &mut [crate::catalog::PhotoRecord],
    sort: PhotoSort,
) {
    photos.sort_by(|left, right| {
        let ordering = match sort.field {
            SortField::DateTaken => left
                .taken_at
                .as_deref()
                .unwrap_or("")
                .cmp(right.taken_at.as_deref().unwrap_or("")),
            SortField::Name => {
                let left_name = Path::new(&left.path)
                    .file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or(&left.path);
                let right_name = Path::new(&right.path)
                    .file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or(&right.path);
                left_name.to_ascii_lowercase().cmp(&right_name.to_ascii_lowercase())
            }
            SortField::FileSize => left
                .size_bytes
                .unwrap_or_default()
                .cmp(&right.size_bytes.unwrap_or_default()),
            SortField::Dimensions => {
                let left_area = left.width.unwrap_or_default()
                    .saturating_mul(left.height.unwrap_or_default());
                let right_area = right.width.unwrap_or_default()
                    .saturating_mul(right.height.unwrap_or_default());
                left_area.cmp(&right_area)
            }
            SortField::DateAdded => left.added_at.cmp(&right.added_at),
        };
        let ordering = if ordering == Ordering::Equal {
            left.path.cmp(&right.path)
        } else {
            ordering
        };
        match sort.direction {
            SortDirection::Ascending => ordering,
            SortDirection::Descending => ordering.reverse(),
        }
    });
}

fn load_catalog_groups() -> Vec<FolderGroupData> {
    let mut by_folder = BTreeMap::<PathBuf, Vec<crate::catalog::PhotoRecord>>::new();

    for photo in crate::catalog::photos().unwrap_or_default() {
        let folder = if photo.folder_path.is_empty() {
            Path::new(&photo.path)
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default()
        } else {
            PathBuf::from(&photo.folder_path)
        };
        by_folder.entry(folder).or_default().push(photo);
    }

    let mut groups = Vec::with_capacity(by_folder.len());
    let sort = PhotoSort::load();
    for (folder, mut photos) in by_folder {
        sort_photo_records(&mut photos, sort);
        let model = gio::ListStore::new::<PhotoObject>();
        for photo in photos {
            model.append(&PhotoObject::new(
                photo.id,
                photo.path,
                photo.favorite,
                photo.rotation,
                photo.mtime,
                photo.size_bytes,
            ));
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
    query: &str,
    count_label: &gtk::Label,
    content_title: &gtk::Label,
    photos_button: &gtk::Button,
    favorites_button: &gtk::Button,
    recent_button: &gtk::Button,
) {
    let started = Instant::now();
    groups.remove_all();
    let mut photo_count = 0u32;
    let mut folder_count = 0u32;
    let needle = query.trim().to_ascii_lowercase();
    let membership = match mode {
        ViewMode::RecentlyAdded => {
            Some(crate::catalog::recently_added_ids(500).unwrap_or_default())
        }
        ViewMode::Album(id) => Some(crate::catalog::album_photo_ids(id).unwrap_or_default()),
        _ => None,
    };

    for group in master {
        let folder_match = !needle.is_empty()
            && (group.label.to_ascii_lowercase().contains(&needle)
                || group
                    .folder
                    .to_string_lossy()
                    .to_ascii_lowercase()
                    .contains(&needle));

        let filtered = gio::ListStore::new::<PhotoObject>();
        for position in 0..group.model.n_items() {
            let Some(photo) = group.model.item(position).and_downcast::<PhotoObject>() else {
                continue;
            };

            if mode == ViewMode::Favorites && !photo.favorite() {
                continue;
            }
            if membership
                .as_ref()
                .is_some_and(|members| !members.contains(&photo.id()))
            {
                continue;
            }

            let path = photo.path();
            let photo_match = needle.is_empty()
                || folder_match
                || Path::new(&path)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(|name| name.to_ascii_lowercase().contains(&needle))
                    .unwrap_or(false)
                || path.to_ascii_lowercase().contains(&needle);

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

    photos_button.remove_css_class("sidebar-active");
    favorites_button.remove_css_class("sidebar-active");
    recent_button.remove_css_class("sidebar-active");

    match mode {
        ViewMode::Photos => {
            content_title.set_label(if needle.is_empty() { "Photos" } else { "Search · Photos" });
            photos_button.add_css_class("sidebar-active");
        }
        ViewMode::Favorites => {
            content_title.set_label(if needle.is_empty() {
                "Favourites"
            } else {
                "Search · Favourites"
            });
            favorites_button.add_css_class("sidebar-active");
        }
        ViewMode::RecentlyAdded => {
            content_title.set_label(if needle.is_empty() {
                "Recently Added"
            } else {
                "Search · Recently Added"
            });
            recent_button.add_css_class("sidebar-active");
        }
        ViewMode::Album(id) => {
            let name = crate::catalog::album_name(id)
                .ok()
                .flatten()
                .unwrap_or_else(|| "Album".to_string());
            if needle.is_empty() {
                content_title.set_label(&name);
            } else {
                content_title.set_label(&format!("Search · {name}"));
            }
        }
    }

    count_label.set_label(&format!("{} photos · {} folders", photo_count, folder_count));

    if trace_enabled() {
        let mode_name = match mode {
            ViewMode::Photos => "photos",
            ViewMode::Favorites => "favourites",
            ViewMode::RecentlyAdded => "recent",
            ViewMode::Album(_) => "album",
        };
        eprintln!(
            "PIC_TRACE view_apply mode={} query={:?} photos={} folders={} elapsed_us={}",
            mode_name,
            query,
            photo_count,
            folder_count,
            started.elapsed().as_micros()
        );
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

fn rebuild_album_sidebar(album_box: &gtk::Box, activate: &Rc<dyn Fn(i64)>) {
    while let Some(child) = album_box.first_child() {
        album_box.remove(&child);
    }

    for album in crate::catalog::albums().unwrap_or_default() {
        let (button, _) = make_sidebar_button(
            "folder-documents-symbolic",
            &album.name,
            &album.photo_count.to_string(),
        );
        button.set_tooltip_text(Some(&format!(
            "{} · {} photos",
            album.name, album.photo_count
        )));
        let activate = activate.clone();
        let album_id = album.id;
        button.connect_clicked(move |_| activate(album_id));
        album_box.append(&button);
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
    recent_count: &gtk::Label,
    recent_button: &gtk::Button,
) {
    let master_ref = master.borrow();
    photos_count.set_label(&count_photos(&master_ref).to_string());
    favorites_count.set_label(&count_favorites(&master_ref).to_string());
    recent_count.set_label(
        &crate::catalog::recently_added_ids(500)
            .unwrap_or_default()
            .len()
            .to_string(),
    );

    rebuild_folder_sidebar(folder_box, &master_ref, roots, folder_activate);
    apply_view(
        groups,
        &master_ref,
        ViewMode::Photos,
        "",
        count_label,
        content_title,
        photos_button,
        favorites_button,
        recent_button,
    );
}

fn format_file_size(size: i64) -> String {
    if size >= 1_000_000_000 {
        format!("{:.1} GB", size as f64 / 1_000_000_000.0)
    } else if size >= 1_000_000 {
        format!("{:.1} MB", size as f64 / 1_000_000.0)
    } else if size >= 1_000 {
        format!("{:.1} KB", size as f64 / 1_000.0)
    } else {
        format!("{size} B")
    }
}

fn install_css() {
    let theme = gtk::CssProvider::new();
    theme.load_from_data(include_str!("../themes/standard/theme.css"));

    let compatibility = gtk::CssProvider::new();
    compatibility.load_from_data(
        r#"
        .library-sidebar {
            padding: 8px 6px;
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
            background: alpha(@theme_fg_color, 0.06);
        }

        .sidebar-row.sidebar-active {
            background: alpha(@accent_bg_color, 0.15);
        }

        .sidebar-count {
            min-width: 34px;
            font-variant-numeric: tabular-nums;
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
            border-radius: 10px;
        }

        .prototype-photo-tile picture {
            border-radius: 8px;
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

        .photo-info-bar {
            min-height: 58px;
        }

        .photo-info-bar scale {
            min-width: 120px;
        }

        .lightbox-backdrop {
            background: #292929;
        }

        .lightbox-picture {
            background: transparent;
        }

        .lightbox-backdrop + * {
            background: transparent;
        }
        "#,
    );

    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &theme,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        gtk::style_context_add_provider_for_display(
            &display,
            &compatibility,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 5,
        );
    }
}
