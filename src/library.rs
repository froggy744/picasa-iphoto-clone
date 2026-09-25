use crate::photo::PhotoObject;
use libadwaita as adw;
use adw::prelude::*;
use gio::prelude::*;
use gtk4 as gtk;
use gtk::prelude::*;
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use walkdir::WalkDir;

const MIN_TILE: i32 = 84;
const MAX_TILE: i32 = 320;
const DEFAULT_TILE: i32 = 176;

pub fn build_window(app: &adw::Application) -> adw::ApplicationWindow {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("PIC Library Prototype")
        .default_width(1280)
        .default_height(820)
        .build();

    let model = gio::ListStore::new::<PhotoObject>();
    let selection = gtk::NoSelection::new(Some(model.clone()));

    let tile_size = Rc::new(Cell::new(DEFAULT_TILE));
    let live_tiles: Rc<RefCell<Vec<glib::WeakRef<gtk::Widget>>>> =
        Rc::new(RefCell::new(Vec::new()));

    let factory = gtk::SignalListItemFactory::new();

    {
        let tile_size = tile_size.clone();
        let live_tiles = live_tiles.clone();
        factory.connect_setup(move |_, list_item| {
            let frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
            frame.add_css_class("prototype-photo-tile");
            frame.set_overflow(gtk::Overflow::Hidden);
            frame.set_size_request(tile_size.get(), tile_size.get());

            let picture = gtk::Picture::new();
            picture.set_hexpand(true);
            picture.set_vexpand(true);
            picture.set_can_shrink(true);
            picture.set_content_fit(gtk::ContentFit::Cover);
            frame.append(&picture);

            let widget: gtk::Widget = frame.clone().upcast();
            live_tiles.borrow_mut().push(widget.downgrade());
            list_item.set_child(Some(&frame));
        });
    }

    factory.connect_bind(|_, list_item| {
        let Some(photo) = list_item.item().and_downcast::<PhotoObject>() else {
            return;
        };
        let Some(frame) = list_item.child().and_downcast::<gtk::Box>() else {
            return;
        };
        let Some(picture) = frame.first_child().and_downcast::<gtk::Picture>() else {
            return;
        };
        let file = gio::File::for_path(photo.path());
        picture.set_file(Some(&file));
        picture.set_tooltip_text(Some(&photo.path()));
    });

    factory.connect_unbind(|_, list_item| {
        let Some(frame) = list_item.child().and_downcast::<gtk::Box>() else {
            return;
        };
        let Some(picture) = frame.first_child().and_downcast::<gtk::Picture>() else {
            return;
        };
        picture.set_paintable(None::<&gtk::gdk::Paintable>);
        picture.set_tooltip_text(None);
    });

    let grid = gtk::GridView::new(Some(selection.clone()), Some(factory.clone()));
    grid.add_css_class("prototype-grid");
    grid.set_min_columns(1);
    grid.set_max_columns(30);
    grid.set_single_click_activate(false);
    grid.set_hexpand(true);
    grid.set_vexpand(true);

    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .child(&grid)
        .build();

    let header = adw::HeaderBar::new();

    let title = gtk::Label::new(Some("Library"));
    title.add_css_class("title");
    header.set_title_widget(Some(&title));

    let count_label = gtk::Label::new(Some("0 photos"));
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
        let grid = grid.clone();
        zoom.connect_value_changed(move |scale| {
            let size = scale.value().round() as i32;
            if size == tile_size.replace(size) {
                return;
            }

            let mut tiles = live_tiles.borrow_mut();
            tiles.retain(|weak| {
                let Some(widget) = weak.upgrade() else {
                    return false;
                };
                widget.set_size_request(size, size);
                true
            });

            // Critical prototype rule:
            // model / ordering / selection are untouched during zoom.
            grid.queue_resize();
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
        let model = model.clone();
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
            let model = model.clone();
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
                    load_directory(&model, &count_label, &path);
                },
            );
        });
    }

    let startup_path = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("PIC_LIBRARY_DIR").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Pictures")));

    if let Some(path) = startup_path.filter(|path| path.is_dir()) {
        load_directory(&model, &count_label, &path);
    }

    window
}

fn load_directory(model: &gio::ListStore, count_label: &gtk::Label, root: &Path) {
    let mut paths = WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| entry.into_path())
        .filter(|path| is_displayable_photo(path))
        .collect::<Vec<_>>();

    paths.sort_unstable();

    model.remove_all();
    for path in paths {
        model.append(&PhotoObject::new(path.to_string_lossy().into_owned()));
    }

    count_label.set_label(&format!("{} photos", model.n_items()));
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
        .prototype-grid {
            background: @window_bg_color;
            padding: 8px;
        }

        .prototype-grid > child {
            padding: 4px;
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
