use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Instant;

use gio::prelude::*;
use gtk::prelude::*;
use gtk4 as gtk;

#[derive(Clone, Debug)]
struct FolderSection {
    title: String,
    start: u32,
    count: u32,
}

const ZOOM_LEVELS: [i32; 8] = [100, 117, 137, 160, 187, 219, 256, 300];

fn next_zoom(current: i32, direction: f64) -> i32 {
    let nearest = ZOOM_LEVELS
        .iter()
        .enumerate()
        .min_by_key(|(_, value)| (**value - current).abs())
        .map(|(index, _)| index)
        .unwrap_or(0);
    let index = if direction < 0.0 {
        (nearest + 1).min(ZOOM_LEVELS.len() - 1)
    } else {
        nearest.saturating_sub(1)
    };
    ZOOM_LEVELS[index]
}

fn build_dataset() -> (gtk::StringList, Vec<FolderSection>) {
    // 50 folders / 5,000 photos. Folder 25 deliberately contains 2,000 photos
    // to expose the core FlowBox risk: bind_model eagerly creates one widget
    // for every item in a realized folder.
    let photos = gtk::StringList::new(&[]);
    let mut sections = Vec::with_capacity(50);
    let mut next_photo = 0_u32;
    let huge_folder = 24_usize;
    let normal_total = 5_000_u32 - 2_000;
    let normal_folders = 49_u32;
    let base = normal_total / normal_folders;
    let remainder = normal_total % normal_folders;

    let mut normal_index = 0_u32;
    for folder_index in 0..50_usize {
        let count = if folder_index == huge_folder {
            2_000
        } else {
            let extra = u32::from(normal_index < remainder);
            normal_index += 1;
            base + extra
        };
        let start = next_photo;
        for item in 0..count {
            photos.append(&format!(
                "F{:02} / photo {:04}",
                folder_index + 1,
                item + 1
            ));
            next_photo += 1;
        }
        sections.push(FolderSection {
            title: if folder_index == huge_folder {
                format!("Folder {:02} — STRESS TEST — {count} photos", folder_index + 1)
            } else {
                format!("Folder {:02} — {count} photos", folder_index + 1)
            },
            start,
            count,
        });
    }

    assert_eq!(next_photo, 5_000);
    (photos, sections)
}

fn apply_tile_size(tile: &gtk::Widget, width: i32) {
    let height = ((width as f64) * 0.68).round() as i32;
    tile.set_size_request(width, height);
}

fn main() {
    let app = gtk::Application::builder()
        .application_id("io.github.picrs.FlowBoxPrototype")
        .build();

    app.connect_activate(|app| {
        let (photos, sections_vec) = build_dataset();
        let sections = Rc::new(sections_vec);
        let folder_titles = gtk::StringList::new(&[]);
        for section in sections.iter() {
            folder_titles.append(&section.title);
        }

        let zoom = Rc::new(Cell::new(187_i32));
        let tile_refs: Rc<RefCell<Vec<glib::WeakRef<gtk::Widget>>>> =
            Rc::new(RefCell::new(Vec::new()));
        let flow_refs: Rc<RefCell<Vec<glib::WeakRef<gtk::FlowBox>>>> =
            Rc::new(RefCell::new(Vec::new()));
        let tiles_created = Rc::new(Cell::new(0_u64));
        let binds = Rc::new(Cell::new(0_u64));

        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup(|_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                return;
            };

            let header = gtk::Label::new(None);
            header.set_xalign(0.0);
            header.set_margin_start(20);
            header.set_margin_end(20);
            header.set_margin_top(14);
            header.set_margin_bottom(8);
            header.add_css_class("title-3");

            let flow = gtk::FlowBox::new();
            flow.set_selection_mode(gtk::SelectionMode::None);
            flow.set_homogeneous(true);
            flow.set_min_children_per_line(1);
            flow.set_max_children_per_line(20);
            flow.set_row_spacing(8);
            flow.set_column_spacing(8);
            flow.set_margin_start(20);
            flow.set_margin_end(20);
            flow.set_margin_bottom(16);
            flow.set_hexpand(true);
            flow.set_valign(gtk::Align::Start);

            let row = gtk::Box::new(gtk::Orientation::Vertical, 0);
            row.set_hexpand(true);
            row.append(&header);
            row.append(&flow);
            item.set_child(Some(&row));
        });

        {
            let sections = sections.clone();
            let photos = photos.clone();
            let zoom = zoom.clone();
            let tile_refs = tile_refs.clone();
            let flow_refs = flow_refs.clone();
            let tiles_created = tiles_created.clone();
            let binds = binds.clone();

            factory.connect_bind(move |_, item| {
                let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                    return;
                };
                let position = item.position() as usize;
                let Some(section) = sections.get(position).cloned() else {
                    return;
                };
                let Some(row) = item.child().and_downcast::<gtk::Box>() else {
                    return;
                };
                let Some(header) = row.first_child().and_downcast::<gtk::Label>() else {
                    return;
                };
                let Some(flow) = row.last_child().and_downcast::<gtk::FlowBox>() else {
                    return;
                };

                header.set_label(&section.title);
                flow.unbind_model();

                let started = Instant::now();
                let slice = gtk::SliceListModel::new(
                    Some(photos.clone()),
                    section.start,
                    section.count,
                );

                let zoom_for_tile = zoom.clone();
                let tile_refs_for_tile = tile_refs.clone();
                let tiles_created_for_tile = tiles_created.clone();

                flow.bind_model(Some(&slice), move |object| {
                    let label_text = object
                        .downcast_ref::<gtk::StringObject>()
                        .map(|value| value.string().to_string())
                        .unwrap_or_else(|| "photo".to_string());

                    let label = gtk::Label::new(Some(&label_text));
                    label.set_wrap(false);
                    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
                    label.set_margin_start(6);
                    label.set_margin_end(6);
                    label.set_margin_top(6);
                    label.set_margin_bottom(6);

                    let tile = gtk::Frame::new(None);
                    tile.set_child(Some(&label));
                    tile.add_css_class("card");
                    tile.set_hexpand(false);
                    tile.set_vexpand(false);

                    let widget: gtk::Widget = tile.clone().upcast();
                    apply_tile_size(&widget, zoom_for_tile.get());
                    tile_refs_for_tile.borrow_mut().push(widget.downgrade());
                    tiles_created_for_tile
                        .set(tiles_created_for_tile.get().wrapping_add(1));
                    widget
                });

                flow_refs.borrow_mut().push(flow.downgrade());
                binds.set(binds.get().wrapping_add(1));

                eprintln!(
                    "FLOWBOX_BIND folder={} photos={} elapsed_ms={} total_tiles_created={}",
                    position + 1,
                    section.count,
                    started.elapsed().as_millis(),
                    tiles_created.get(),
                );
            });
        }

        let selection = gtk::NoSelection::new(Some(folder_titles.clone()));
        let list = gtk::ListView::new(Some(selection.clone()), Some(factory.clone()));
        list.set_hexpand(true);
        list.set_vexpand(true);

        let scrolled = gtk::ScrolledWindow::new();
        scrolled.set_hexpand(true);
        scrolled.set_vexpand(true);
        scrolled.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scrolled.set_child(Some(&list));

        let status = gtk::Label::new(None);
        status.set_xalign(0.0);
        status.set_margin_start(12);
        status.set_margin_end(12);
        status.set_margin_top(8);
        status.set_margin_bottom(8);

        let refresh_status: Rc<dyn Fn()> = {
            let status = status.clone();
            let tile_refs = tile_refs.clone();
            let flow_refs = flow_refs.clone();
            let tiles_created = tiles_created.clone();
            let binds = binds.clone();
            let zoom = zoom.clone();
            Rc::new(move || {
                tile_refs.borrow_mut().retain(|weak| weak.upgrade().is_some());
                flow_refs.borrow_mut().retain(|weak| weak.upgrade().is_some());
                status.set_text(&format!(
                    "zoom={}px   live tiles={}   live FlowBoxes={}   total tiles created={}   binds={}",
                    zoom.get(),
                    tile_refs.borrow().len(),
                    flow_refs.borrow().len(),
                    tiles_created.get(),
                    binds.get(),
                ));
            })
        };
        refresh_status();

        let zoom_controller =
            gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        zoom_controller.set_propagation_phase(gtk::PropagationPhase::Capture);
        {
            let zoom = zoom.clone();
            let tile_refs = tile_refs.clone();
            let refresh_status = refresh_status.clone();
            zoom_controller.connect_scroll(move |controller, _, dy| {
                if !controller
                    .current_event_state()
                    .contains(gtk::gdk::ModifierType::CONTROL_MASK)
                {
                    return glib::Propagation::Proceed;
                }
                if dy == 0.0 {
                    return glib::Propagation::Stop;
                }

                let old = zoom.get();
                let new = next_zoom(old, dy);
                if new != old {
                    let started = Instant::now();
                    zoom.set(new);
                    let mut live = 0_usize;
                    tile_refs.borrow_mut().retain(|weak| {
                        let Some(tile) = weak.upgrade() else {
                            return false;
                        };
                        apply_tile_size(&tile, new);
                        live += 1;
                        true
                    });
                    eprintln!(
                        "FLOWBOX_ZOOM old={} new={} live_tiles={} apply_ms={}",
                        old,
                        new,
                        live,
                        started.elapsed().as_millis(),
                    );
                    refresh_status();
                }
                glib::Propagation::Stop
            });
        }
        scrolled.add_controller(zoom_controller);

        let stats_button = gtk::Button::with_label("Log live widget stats");
        {
            let refresh_status = refresh_status.clone();
            let tile_refs = tile_refs.clone();
            let flow_refs = flow_refs.clone();
            let tiles_created = tiles_created.clone();
            let binds = binds.clone();
            stats_button.connect_clicked(move |_| {
                refresh_status();
                eprintln!(
                    "FLOWBOX_STATS live_tiles={} live_flowboxes={} total_tiles_created={} binds={}",
                    tile_refs.borrow().iter().filter(|w| w.upgrade().is_some()).count(),
                    flow_refs.borrow().iter().filter(|w| w.upgrade().is_some()).count(),
                    tiles_created.get(),
                    binds.get(),
                );
            });
        }

        let jump_huge = gtk::Button::with_label("Jump to 2,000-photo folder");
        {
            let list = list.clone();
            jump_huge.connect_clicked(move |_| {
                let started = Instant::now();
                list.scroll_to(24, gtk::ListScrollFlags::FOCUS, None);
                eprintln!("FLOWBOX_JUMP requested folder=25 setup_us={}", started.elapsed().as_micros());
            });
        }

        let controls = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        controls.set_margin_start(12);
        controls.set_margin_end(12);
        controls.set_margin_top(8);
        controls.append(&jump_huge);
        controls.append(&stats_button);

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.append(&controls);
        root.append(&status);
        root.append(&scrolled);

        let css = gtk::CssProvider::new();
        css.load_from_string(
            r#"
            frame.card {
                border: 1px solid alpha(currentColor, 0.18);
                border-radius: 7px;
                background: alpha(currentColor, 0.045);
            }
            "#,
        );
        gtk::style_context_add_provider_for_display(
            &root.display(),
            &css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );

        let window = gtk::ApplicationWindow::builder()
            .application(app)
            .title("PIC Folder FlowBox Prototype")
            .default_width(1400)
            .default_height(900)
            .child(&root)
            .build();

        window.present();
        eprintln!(
            "FLOWBOX_PROTOTYPE ready folders=50 photos=5000 stress_folder=25 stress_photos=2000"
        );
    });

    app.run();
}
