use crate::photo::PhotoObject;
use adw::prelude::*;
use gio::prelude::*;
use gtk4 as gtk;
use gtk::prelude::*;
use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Instant;
use walkdir::WalkDir;

const MIN_TILE: i32 = 84;
const MAX_TILE: i32 = 320;
const DEFAULT_TILE: i32 = 176;

#[derive(Clone)]
struct FolderGroupData {
    folder: PathBuf,
    label: String,
    model: gio::ListStore,
}

fn trace_enabled() -> bool {
    std::env::var_os("PICASA_TRACE").is_some()
}

fn images_disabled() -> bool {
    std::env::var_os("PIC_NO_IMAGES").is_some()
}

pub fn build_window(app: &adw::Application) -> adw::ApplicationWindow {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("PIC Library Prototype · Folder Groups")
        .default_width(1280)
        .default_height(820)
        .build();

    let groups = gio::ListStore::new::<glib::BoxedAnyObject>();
    let group_selection = gtk::NoSelection::new(Some(groups.clone()));

    let tile_size = Rc::new(Cell::new(DEFAULT_TILE));
    let live_tiles: Rc<RefCell<Vec<glib::WeakRef<gtk::Widget>>>> =
        Rc::new(RefCell::new(Vec::new()));

    let folder_factory = gtk::SignalListItemFactory::new();

    {
        let tile_size = tile_size.clone();
        let live_tiles = live_tiles.clone();

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

            let flow = gtk::FlowBox::new();
            flow.add_css_class("folder-grid");
            flow.set_selection_mode(gtk::SelectionMode::None);
            flow.set_homogeneous(false);
            flow.set_row_spacing(8);
            flow.set_column_spacing(8);
            flow.set_min_children_per_line(1);
            flow.set_max_children_per_line(30);
            flow.set_hexpand(true);
            flow.set_vexpand(false);
            flow.set_halign(gtk::Align::Fill);
            flow.set_valign(gtk::Align::Start);

            section.append(&header_line);
            section.append(&separator);
            section.append(&flow);

            list_item.set_child(Some(&section));
        });
    }

    {
        let tile_size = tile_size.clone();
        let live_tiles = live_tiles.clone();
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
        let Some(flow) = separator.next_sibling().and_downcast::<gtk::FlowBox>() else {
            return;
        };

        title.set_label(&group.label);
        count.set_label(&format!("{} photos", group.model.n_items()));

        while let Some(child) = flow.first_child() {
            flow.remove(&child);
        }

        for index in 0..group.model.n_items() {
            let Some(photo) = group.model.item(index).and_downcast::<PhotoObject>() else {
                continue;
            };
            let frame = make_photo_tile(&photo, tile_size.get(), &live_tiles);
            flow.insert(&frame, -1);
        }

        if trace_enabled() {
            eprintln!(
                "PIC_GROUP bind folder={} photos={}",
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
        let Some(flow) = separator.next_sibling().and_downcast::<gtk::FlowBox>() else {
            return;
        };
        while let Some(child) = flow.first_child() {
            flow.remove(&child);
        }
    });

    let list = gtk::ListView::new(Some(group_selection.clone()), Some(folder_factory.clone()));
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
                "PIC_GROUP scroll value={:.0} upper={:.0} page={:.0}",
                adj.value(),
                adj.upper(),
                adj.page_size()
            );
        });
    }

    let header = adw::HeaderBar::new();

    let title = gtk::Label::new(Some("Library · Folders"));
    title.add_css_class("title");
    header.set_title_widget(Some(&title));

    let count_label = gtk::Label::new(Some("0 photos · 0 folders"));
    count_label.add_css_class("dim-label");
    header.pack_start(&count_label);

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
    let minus = gtk::Button::from_icon_name("zoom-out-symbolic");
    header.pack_end(&plus);
    header.pack_end(&minus);

    let open = gtk::Button::with_label("Open Folder");
    header.pack_start(&open);

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.append(&header);
    root.append(&scroller);
    window.set_content(Some(&root));

    install_css();

    {
        let live_tiles = live_tiles.clone();
        let tile_size = tile_size.clone();
        let list = list.clone();
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
            list.queue_resize();

            if trace_enabled() {
                eprintln!(
                    "PIC_GROUP zoom size={} realized_tiles={} elapsed_us={}",
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
        let groups = groups.clone();
        let count_label = count_label.clone();
        let window_weak = window.downgrade();
        open.connect_clicked(move |_| {
            let Some(window) = window_weak.upgrade() else {
                return;
            };
            let dialog = gtk::FileDialog::builder()
                .title("Choose photo library folder")
                .modal(true)
                .build();
            let groups = groups.clone();
            let count_label = count_label.clone();
            dialog.select_folder(
                Some(&window),
                gio::Cancellable::NONE,
                move |result| {
                    let Ok(folder) = result else {
                        return;
                    };
                    let Some(path) = folder.path() else {
                        return;
                    };
                    load_grouped_directories(&groups, &count_label, &[path]);
                },
            );
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
        load_grouped_directories(&groups, &count_label, &startup_paths);
    }

    window
}

fn make_photo_tile(
    photo: &PhotoObject,
    size: i32,
    live_tiles: &Rc<RefCell<Vec<glib::WeakRef<gtk::Widget>>>>,
) -> gtk::Box {
    let frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
    frame.add_css_class("prototype-photo-tile");
    frame.set_overflow(gtk::Overflow::Hidden);
    frame.set_size_request(size, size);
    frame.set_hexpand(false);
    frame.set_vexpand(false);
    frame.set_halign(gtk::Align::Start);
    frame.set_valign(gtk::Align::Start);

    let picture = gtk::Picture::new();
    picture.set_size_request(1, 1);
    picture.set_hexpand(true);
    picture.set_vexpand(true);
    picture.set_halign(gtk::Align::Fill);
    picture.set_valign(gtk::Align::Fill);
    picture.set_can_shrink(true);
    picture.set_content_fit(gtk::ContentFit::Cover);
    frame.append(&picture);

    let widget: gtk::Widget = frame.clone().upcast();
    live_tiles.borrow_mut().push(widget.downgrade());

    let path = photo.path();
    picture.set_tooltip_text(Some(&path));

    if !images_disabled() {
        let started = Instant::now();
        let file = gio::File::for_path(&path);
        picture.set_file(Some(&file));
        if trace_enabled() {
            eprintln!(
                "PIC_GROUP photo_bind set_file_us={} path={}",
                started.elapsed().as_micros(),
                path
            );
        }
    }

    frame
}

fn load_grouped_directories(
    groups: &gio::ListStore,
    count_label: &gtk::Label,
    roots: &[PathBuf],
) {
    let started = Instant::now();

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

    groups.remove_all();

    let mut total_photos = 0u32;

    for (folder, mut paths) in by_folder {
        paths.sort_unstable();

        let model = gio::ListStore::new::<PhotoObject>();
        for path in paths {
            model.append(&PhotoObject::new(path.to_string_lossy().into_owned()));
        }

        total_photos += model.n_items();

        let label = folder
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or("Photos")
            .to_string();

        groups.append(&glib::BoxedAnyObject::new(FolderGroupData {
            folder,
            label,
            model,
        }));
    }

    count_label.set_label(&format!(
        "{} photos · {} folders",
        total_photos,
        groups.n_items()
    ));

    if trace_enabled() {
        eprintln!(
            "PIC_GROUP library_loaded photos={} folders={} roots={} elapsed_ms={}",
            total_photos,
            groups.n_items(),
            roots.len(),
            started.elapsed().as_millis()
        );
    }
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
        .folder-list {
            background: @window_bg_color;
        }

        .folder-list > row {
            padding: 0;
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

        .folder-grid > flowboxchild {
            padding: 0;
            margin: 0;
            min-height: 0;
            background: transparent;
        }

        .prototype-photo-tile {
            background: alpha(@window_fg_color, 0.06);
            border-radius: 6px;
        }

        .prototype-photo-tile picture {
            border-radius: 6px;
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
