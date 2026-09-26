use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::time::Instant;

use gtk::prelude::*;
use gtk4 as gtk;

const ZOOM_LEVELS: [i32; 8] = [100, 117, 137, 160, 187, 219, 256, 300];
const GAP: f64 = 8.0;
const SIDE_MARGIN: f64 = 20.0;
const HEADER_H: f64 = 42.0;
const SECTION_GAP: f64 = 10.0;
const OVERSCAN_PX: f64 = 320.0;

#[derive(Clone, Debug)]
struct FolderSection {
    folder_id: i64,
    title: String,
    start: u32,
    count: u32,
}

#[derive(Clone, Copy, Debug)]
struct SectionGeometry {
    header_y: f64,
    first_photo_y: f64,
    end_y: f64,
}

#[derive(Default)]
struct GeometryState {
    width: i32,
    tile_width: i32,
    tile_height: i32,
    columns: u32,
    total_height: f64,
    sections: Vec<SectionGeometry>,
    rebuilds: u64,
    last_rebuild_us: u128,
}

#[derive(Clone)]
struct TileWidget {
    frame: gtk::Frame,
    label: gtk::Label,
}

fn dataset() -> Vec<FolderSection> {
    let huge = 24usize;
    let normal_total = 3_000u32;
    let base = normal_total / 49;
    let remainder = normal_total % 49;
    let mut sections = Vec::with_capacity(50);
    let mut start = 0u32;
    let mut normal = 0u32;

    for folder in 0..50usize {
        let count = if folder == huge {
            2_000
        } else {
            let extra = if normal < remainder { 1 } else { 0 };
            normal += 1;
            base + extra
        };
        sections.push(FolderSection {
            folder_id: folder as i64 + 1,
            title: if folder == huge {
                format!("Folder {:02} — STRESS TEST — {count} photos", folder + 1)
            } else {
                format!("Folder {:02} — {count} photos", folder + 1)
            },
            start,
            count,
        });
        start += count;
    }
    assert_eq!(start, 5_000);
    sections
}

fn next_zoom(current: i32, dy: f64) -> i32 {
    let index = ZOOM_LEVELS
        .iter()
        .enumerate()
        .min_by_key(|(_, value)| (**value - current).abs())
        .map(|(i, _)| i)
        .unwrap_or(0);
    if dy < 0.0 {
        ZOOM_LEVELS[(index + 1).min(ZOOM_LEVELS.len() - 1)]
    } else {
        ZOOM_LEVELS[index.saturating_sub(1)]
    }
}

fn rebuild_geometry(state: &mut GeometryState, sections: &[FolderSection], width: i32, tile: i32) {
    let started = Instant::now();
    let usable = (f64::from(width) - SIDE_MARGIN * 2.0).max(f64::from(tile));
    let columns = ((usable + GAP) / (f64::from(tile) + GAP)).floor().max(1.0) as u32;
    let tile_height = ((f64::from(tile) * 0.68).round() as i32).max(1);
    let row_h = f64::from(tile_height) + GAP;

    let mut y = 0.0;
    let mut geometry = Vec::with_capacity(sections.len());
    for section in sections {
        let rows = section.count.div_ceil(columns);
        let header_y = y;
        let first_photo_y = header_y + HEADER_H;
        let end_y = first_photo_y + f64::from(rows) * row_h + SECTION_GAP;
        geometry.push(SectionGeometry { header_y, first_photo_y, end_y });
        y = end_y;
    }

    state.width = width;
    state.tile_width = tile;
    state.tile_height = tile_height;
    state.columns = columns;
    state.total_height = y.max(1.0);
    state.sections = geometry;
    state.rebuilds += 1;
    state.last_rebuild_us = started.elapsed().as_micros();
}

fn photo_y(photo_index: u32, sections: &[FolderSection], g: &GeometryState) -> Option<f64> {
    let row_h = f64::from(g.tile_height) + GAP;
    for (section, geom) in sections.iter().zip(g.sections.iter()) {
        if photo_index >= section.start && photo_index < section.start + section.count {
            let local = photo_index - section.start;
            return Some(geom.first_photo_y + f64::from(local / g.columns.max(1)) * row_h);
        }
    }
    None
}

fn anchor_photo(scroll_y: f64, sections: &[FolderSection], g: &GeometryState) -> Option<(u32, f64)> {
    let row_h = f64::from(g.tile_height) + GAP;
    for (section, geom) in sections.iter().zip(g.sections.iter()) {
        if scroll_y >= geom.end_y { continue; }
        if section.count == 0 { continue; }
        let row = if scroll_y <= geom.first_photo_y {
            0
        } else {
            ((scroll_y - geom.first_photo_y) / row_h).floor().max(0.0) as u32
        };
        let local = (row * g.columns.max(1)).min(section.count - 1);
        let index = section.start + local;
        let y = geom.first_photo_y + f64::from(row) * row_h;
        return Some((index, y - scroll_y));
    }
    None
}

fn photo_label(index: u32, sections: &[FolderSection]) -> String {
    let folder = sections.iter().find(|s| index >= s.start && index < s.start + s.count);
    match folder {
        Some(s) => format!("F{:02} • {:04}", s.folder_id, index - s.start + 1),
        None => format!("Photo {index}"),
    }
}

fn main() {
    let app = gtk::Application::builder()
        .application_id("io.github.picrs.SectionedViewPrototype")
        .build();

    app.connect_activate(|app| {
        let sections = Rc::new(dataset());
        let zoom = Rc::new(Cell::new(187i32));
        let geometry = Rc::new(RefCell::new(GeometryState::default()));

        let fixed = gtk::Fixed::new();
        fixed.set_hexpand(true);

        let spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
        spacer.set_can_target(false);
        fixed.put(&spacer, 0.0, 0.0);

        let scrolled = gtk::ScrolledWindow::new();
        scrolled.set_hexpand(true);
        scrolled.set_vexpand(true);
        scrolled.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scrolled.set_child(Some(&fixed));

        let live_tiles: Rc<RefCell<HashMap<u32, TileWidget>>> = Rc::new(RefCell::new(HashMap::new()));
        let tile_pool: Rc<RefCell<VecDeque<TileWidget>>> = Rc::new(RefCell::new(VecDeque::new()));
        let live_headers: Rc<RefCell<HashMap<usize, gtk::Label>>> = Rc::new(RefCell::new(HashMap::new()));
        let header_pool: Rc<RefCell<VecDeque<gtk::Label>>> = Rc::new(RefCell::new(VecDeque::new()));
        let total_tiles_created = Rc::new(Cell::new(0u64));
        let total_headers_created = Rc::new(Cell::new(0u64));
        let last_anchor_ok = Rc::new(Cell::new(true));

        let status = gtk::Label::new(None);
        status.set_xalign(0.0);
        status.set_margin_start(12);
        status.set_margin_end(12);
        status.set_margin_top(8);
        status.set_margin_bottom(8);

        let refresh: Rc<dyn Fn()> = {
            let fixed = fixed.clone();
            let scrolled = scrolled.clone();
            let sections = sections.clone();
            let geometry = geometry.clone();
            let live_tiles = live_tiles.clone();
            let tile_pool = tile_pool.clone();
            let live_headers = live_headers.clone();
            let header_pool = header_pool.clone();
            let total_tiles_created = total_tiles_created.clone();
            let total_headers_created = total_headers_created.clone();
            let status = status.clone();
            let spacer = spacer.clone();
            let last_anchor_ok = last_anchor_ok.clone();
            let zoom = zoom.clone();

            Rc::new(move || {
                let width = scrolled.width().max(1);
                let g = geometry.borrow();
                if g.sections.is_empty() || g.width != width {
                    drop(g);
                    rebuild_geometry(&mut geometry.borrow_mut(), &sections, width, zoom.get());
                }
                let g = geometry.borrow();

                spacer.set_size_request(width, g.total_height.ceil() as i32);

                let adj = scrolled.vadjustment();
                let top = (adj.value() - OVERSCAN_PX).max(0.0);
                let bottom = adj.value() + adj.page_size() + OVERSCAN_PX;
                let row_h = f64::from(g.tile_height) + GAP;

                let mut wanted_headers = Vec::new();
                let mut wanted_tiles = Vec::new();

                for (section_index, (section, geom)) in sections.iter().zip(g.sections.iter()).enumerate() {
                    if geom.end_y < top || geom.header_y > bottom { continue; }
                    wanted_headers.push(section_index);

                    let start_row = if top <= geom.first_photo_y { 0 } else {
                        ((top - geom.first_photo_y) / row_h).floor().max(0.0) as u32
                    };
                    let end_row = (((bottom - geom.first_photo_y) / row_h).ceil().max(0.0) as u32)
                        .min(section.count.div_ceil(g.columns.max(1)));

                    for row in start_row..end_row {
                        let row_start = section.start + row * g.columns.max(1);
                        for col in 0..g.columns.max(1) {
                            let index = row_start + col;
                            if index >= section.start + section.count { break; }
                            wanted_tiles.push((index, section_index, row, col));
                        }
                    }
                }

                let wanted_ids = wanted_tiles.iter().map(|x| x.0).collect::<std::collections::HashSet<_>>();
                let stale_tiles = live_tiles.borrow().keys().copied()
                    .filter(|id| !wanted_ids.contains(id))
                    .collect::<Vec<_>>();
                for id in stale_tiles {
                    if let Some(tile) = live_tiles.borrow_mut().remove(&id) {
                        fixed.remove(&tile.frame);
                        tile_pool.borrow_mut().push_back(tile);
                    }
                }

                for (index, section_index, row, col) in wanted_tiles {
                    let tile = if let Some(existing) = live_tiles.borrow().get(&index).cloned() {
                        existing
                    } else {
                        let tile = tile_pool.borrow_mut().pop_front().unwrap_or_else(|| {
                            let label = gtk::Label::new(None);
                            label.set_ellipsize(gtk::pango::EllipsizeMode::End);
                            let frame = gtk::Frame::new(None);
                            frame.add_css_class("card");
                            frame.set_child(Some(&label));
                            total_tiles_created.set(total_tiles_created.get() + 1);
                            TileWidget { frame, label }
                        });
                        tile.label.set_label(&photo_label(index, &sections));
                        fixed.put(&tile.frame, 0.0, 0.0);
                        live_tiles.borrow_mut().insert(index, tile.clone());
                        tile
                    };

                    tile.frame.set_size_request(g.tile_width, g.tile_height);
                    let geom = g.sections[section_index];
                    let x = SIDE_MARGIN + f64::from(col) * (f64::from(g.tile_width) + GAP);
                    let y = geom.first_photo_y + f64::from(row) * row_h;
                    fixed.move_(&tile.frame, x, y);
                }

                let wanted_header_ids = wanted_headers.iter().copied().collect::<std::collections::HashSet<_>>();
                let stale_headers = live_headers.borrow().keys().copied()
                    .filter(|id| !wanted_header_ids.contains(id))
                    .collect::<Vec<_>>();
                for id in stale_headers {
                    if let Some(label) = live_headers.borrow_mut().remove(&id) {
                        fixed.remove(&label);
                        header_pool.borrow_mut().push_back(label);
                    }
                }

                for section_index in wanted_headers {
                    let label = if let Some(existing) = live_headers.borrow().get(&section_index).cloned() {
                        existing
                    } else {
                        let label = header_pool.borrow_mut().pop_front().unwrap_or_else(|| {
                            let label = gtk::Label::new(None);
                            label.set_xalign(0.0);
                            label.add_css_class("title-3");
                            total_headers_created.set(total_headers_created.get() + 1);
                            label
                        });
                        label.set_label(&sections[section_index].title);
                        fixed.put(&label, SIDE_MARGIN, 0.0);
                        live_headers.borrow_mut().insert(section_index, label.clone());
                        label
                    };
                    label.set_size_request((width - (SIDE_MARGIN * 2.0) as i32).max(1), HEADER_H as i32);
                    fixed.move_(&label, SIDE_MARGIN, g.sections[section_index].header_y);
                }

                status.set_text(&format!(
                    "photos=5000  live_tiles={}  pool={}  live_headers={}  created_tiles={}  created_headers={}  cols={}  geometry_us={}  geometry_rebuilds={}  anchor_ok={}",
                    live_tiles.borrow().len(),
                    tile_pool.borrow().len(),
                    live_headers.borrow().len(),
                    total_tiles_created.get(),
                    total_headers_created.get(),
                    g.columns,
                    g.last_rebuild_us,
                    g.rebuilds,
                    last_anchor_ok.get(),
                ));
            })
        };

        {
            let refresh = refresh.clone();
            scrolled.vadjustment().connect_value_changed(move |_| refresh());
        }

        {
            let refresh = refresh.clone();
            let geometry = geometry.clone();
            let sections = sections.clone();
            let scrolled = scrolled.clone();
            let zoom = zoom.clone();
            let last_width = Rc::new(Cell::new(0i32));
            let last_width_tick = last_width.clone();
            scrolled.add_tick_callback(move |_, _| {
                let width = scrolled.width();
                if width > 0 && width != last_width_tick.get() {
                    last_width_tick.set(width);
                    rebuild_geometry(&mut geometry.borrow_mut(), &sections, width, zoom.get());
                    refresh();
                }
                glib::ControlFlow::Continue
            });
        }

        let zoom_controller = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        zoom_controller.set_propagation_phase(gtk::PropagationPhase::Capture);
        {
            let zoom = zoom.clone();
            let geometry = geometry.clone();
            let sections = sections.clone();
            let scrolled = scrolled.clone();
            let refresh = refresh.clone();
            let last_anchor_ok = last_anchor_ok.clone();
            zoom_controller.connect_scroll(move |controller, _, dy| {
                if !controller.current_event_state().contains(gtk::gdk::ModifierType::CONTROL_MASK) {
                    return glib::Propagation::Proceed;
                }
                if dy == 0.0 { return glib::Propagation::Stop; }

                let old_zoom = zoom.get();
                let new_zoom = next_zoom(old_zoom, dy);
                if new_zoom == old_zoom { return glib::Propagation::Stop; }

                let adj = scrolled.vadjustment();
                let before_scroll = adj.value();
                let before = {
                    let g = geometry.borrow();
                    anchor_photo(before_scroll, &sections, &g)
                };

                zoom.set(new_zoom);
                rebuild_geometry(&mut geometry.borrow_mut(), &sections, scrolled.width().max(1), new_zoom);

                if let Some((photo_id, offset)) = before {
                    if let Some(new_y) = photo_y(photo_id, &sections, &geometry.borrow()) {
                        let upper = (adj.upper() - adj.page_size()).max(adj.lower());
                        adj.set_value((new_y - offset).clamp(adj.lower(), upper));
                        let after = anchor_photo(adj.value(), &sections, &geometry.borrow()).map(|x| x.0);
                        last_anchor_ok.set(after == Some(photo_id));
                        eprintln!(
                            "SECTIONED_ZOOM old={} new={} anchor={} anchor_after={:?} geometry_us={} live_target_scroll={:.1}",
                            old_zoom,
                            new_zoom,
                            photo_id,
                            after,
                            geometry.borrow().last_rebuild_us,
                            adj.value(),
                        );
                    }
                }
                refresh();
                glib::Propagation::Stop
            });
        }
        scrolled.add_controller(zoom_controller);

        let jump = gtk::Button::with_label("Jump to 2,000-photo folder");
        {
            let geometry = geometry.clone();
            let scrolled = scrolled.clone();
            let refresh = refresh.clone();
            jump.connect_clicked(move |_| {
                let started = Instant::now();
                let target = geometry.borrow().sections.get(24).map(|g| g.header_y).unwrap_or(0.0);
                scrolled.vadjustment().set_value(target);
                refresh();
                eprintln!("SECTIONED_JUMP folder=25 setup_us={}", started.elapsed().as_micros());
            });
        }

        let log_stats = gtk::Button::with_label("Log stats");
        {
            let live_tiles = live_tiles.clone();
            let tile_pool = tile_pool.clone();
            let live_headers = live_headers.clone();
            let total_tiles_created = total_tiles_created.clone();
            let geometry = geometry.clone();
            log_stats.connect_clicked(move |_| {
                let g = geometry.borrow();
                eprintln!(
                    "SECTIONED_STATS photos=5000 live_tiles={} pool_tiles={} live_headers={} total_tiles_created={} geometry_us={} geometry_rebuilds={}",
                    live_tiles.borrow().len(),
                    tile_pool.borrow().len(),
                    live_headers.borrow().len(),
                    total_tiles_created.get(),
                    g.last_rebuild_us,
                    g.rebuilds,
                );
            });
        }

        let controls = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        controls.set_margin_start(12);
        controls.set_margin_end(12);
        controls.set_margin_top(8);
        controls.append(&jump);
        controls.append(&log_stats);

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.append(&controls);
        root.append(&status);
        root.append(&scrolled);

        let css = gtk::CssProvider::new();
        css.load_from_string(
            r#"
            frame.card {
                border: 1px solid alpha(currentColor, 0.20);
                border-radius: 7px;
                background: alpha(currentColor, 0.05);
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
            .title("PIC SectionedPhotoView Prototype B")
            .default_width(1400)
            .default_height(900)
            .child(&root)
            .build();

        window.present();
        rebuild_geometry(&mut geometry.borrow_mut(), &sections, 1400, zoom.get());
        refresh();
        eprintln!("SECTIONED_PROTOTYPE ready folders=50 photos=5000 stress_folder=25 stress_photos=2000");
    });

    app.run();
}
