use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::time::Instant;

use gtk::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;
use rusqlite::Connection;

use crate::photo_object::PhotoObject;

const GAP: f64 = 8.0;
const SIDE_MARGIN: f64 = 20.0;
const HEADER_H: f64 = 42.0;
const SECTION_GAP: f64 = 10.0;
const OVERSCAN_PX: f64 = 320.0;
const MAX_TILE_POOL: usize = 140;
const MAX_HEADER_POOL: usize = 8;
const PAINTABLE_CACHE_CAP: usize = 256;

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
    picture: gtk::Picture,
    caption: gtk::Label,
    key: Rc<RefCell<Option<String>>>,
}

fn current_rss_kb() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status.lines().find_map(|line| {
        let value = line.strip_prefix("VmRSS:")?;
        value.split_whitespace().next()?.parse::<u64>().ok()
    })
}

fn next_zoom(current: i32, dy: f64) -> i32 {
    let levels = crate::grid::ZOOM_LEVELS;
    let index = levels
        .iter()
        .enumerate()
        .min_by_key(|(_, value)| (**value - current).abs())
        .map(|(i, _)| i)
        .unwrap_or(0);
    if dy < 0.0 {
        levels[(index + 1).min(levels.len() - 1)]
    } else {
        levels[index.saturating_sub(1)]
    }
}

fn rebuild_geometry(
    state: &mut GeometryState,
    sections: &[FolderSection],
    width: i32,
    tile: i32,
) {
    let started = Instant::now();
    let usable = (f64::from(width) - SIDE_MARGIN * 2.0).max(f64::from(tile));
    let columns = ((usable + GAP) / (f64::from(tile) + GAP))
        .floor()
        .max(1.0) as u32;
    let tile_height = ((f64::from(tile) * 0.68).round() as i32).max(1);
    let row_h = f64::from(tile_height) + GAP;

    let mut y = 0.0;
    let mut geometry = Vec::with_capacity(sections.len());
    for section in sections {
        let rows = section.count.div_ceil(columns);
        let header_y = y;
        let first_photo_y = header_y + HEADER_H;
        let end_y = first_photo_y + f64::from(rows) * row_h + SECTION_GAP;
        geometry.push(SectionGeometry {
            header_y,
            first_photo_y,
            end_y,
        });
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

fn photo_y(
    photo_index: u32,
    sections: &[FolderSection],
    geometry: &GeometryState,
) -> Option<f64> {
    let row_h = f64::from(geometry.tile_height) + GAP;
    for (section, geom) in sections.iter().zip(&geometry.sections) {
        if photo_index >= section.start && photo_index < section.start + section.count {
            let local = photo_index - section.start;
            return Some(
                geom.first_photo_y
                    + f64::from(local / geometry.columns.max(1)) * row_h,
            );
        }
    }
    None
}

fn anchor_photo_at_view_offset(
    scroll_y: f64,
    view_offset: f64,
    sections: &[FolderSection],
    geometry: &GeometryState,
) -> Option<(u32, f64)> {
    let target_y = scroll_y + view_offset;
    let row_h = f64::from(geometry.tile_height) + GAP;
    for (section, geom) in sections.iter().zip(&geometry.sections) {
        if target_y >= geom.end_y || section.count == 0 {
            continue;
        }
        let row = if target_y <= geom.first_photo_y {
            0
        } else {
            ((target_y - geom.first_photo_y) / row_h)
                .floor()
                .max(0.0) as u32
        };
        let local = (row * geometry.columns.max(1)).min(section.count - 1);
        let index = section.start + local;
        let y = geom.first_photo_y + f64::from(row) * row_h;
        return Some((index, y - scroll_y));
    }
    None
}

fn display_request(photo: &PhotoObject) -> Option<crate::thumbnail_display::DisplayRequest> {
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

fn make_tile() -> TileWidget {
    let picture = gtk::Picture::new();
    picture.set_content_fit(gtk::ContentFit::Cover);
    picture.set_can_shrink(true);
    picture.set_hexpand(true);
    picture.set_vexpand(true);

    let caption = gtk::Label::new(None);
    caption.set_xalign(0.0);
    caption.set_ellipsize(gtk::pango::EllipsizeMode::End);
    caption.set_single_line_mode(true);
    caption.set_margin_start(4);
    caption.set_margin_end(4);

    let box_ = gtk::Box::new(gtk::Orientation::Vertical, 2);
    box_.append(&picture);
    box_.append(&caption);

    let frame = gtk::Frame::new(None);
    frame.add_css_class("sectioned-real-card");
    frame.set_child(Some(&box_));

    TileWidget {
        frame,
        picture,
        caption,
        key: Rc::new(RefCell::new(None)),
    }
}

fn paintable_from_pixels(width: i32, height: i32, pixels: Vec<u8>) -> gtk::gdk::Paintable {
    let bytes = glib::Bytes::from_owned(pixels);
    let texture = gtk::gdk::MemoryTexture::new(
        width,
        height,
        gtk::gdk::MemoryFormat::R8g8b8a8,
        &bytes,
        width as usize * 4,
    );
    texture.upcast()
}

fn build_data(connection: &Connection) -> (Vec<PhotoObject>, Vec<FolderSection>) {
    let mut photos = crate::db::photos(connection, None, false, None).unwrap_or_default();

    // Folder mode is a continuous folder stream. Keep all photos as stable
    // objects; sorting here changes ordering only once at prototype startup.
    photos.sort_by(|left, right| {
        left.folder_path
            .as_deref()
            .unwrap_or("")
            .cmp(right.folder_path.as_deref().unwrap_or(""))
            .then_with(|| right.taken_at.cmp(&left.taken_at))
            .then_with(|| left.path.cmp(&right.path))
    });

    let objects = photos.iter().map(PhotoObject::from_photo).collect::<Vec<_>>();
    let mut sections = Vec::new();
    let mut start = 0usize;

    while start < photos.len() {
        let folder_id = photos[start].folder_id.unwrap_or_default();
        let folder_path = photos[start]
            .folder_path
            .clone()
            .unwrap_or_else(|| "Unfiled".to_string());
        let mut end = start + 1;
        while end < photos.len() && photos[end].folder_id.unwrap_or_default() == folder_id {
            end += 1;
        }
        let title = if folder_path == "Unfiled" {
            folder_path
        } else {
            crate::db::folder_name(&folder_path)
        };
        sections.push(FolderSection {
            folder_id,
            title,
            start: start as u32,
            count: (end - start) as u32,
        });
        start = end;
    }

    (objects, sections)
}

pub fn build(app: &adw::Application, connection: Connection) -> adw::ApplicationWindow {
    let started = Instant::now();
    let (photos, sections_vec) = build_data(&connection);
    let photos = Rc::new(photos);
    let sections = Rc::new(sections_vec);

    let zoom = Rc::new(Cell::new(187i32));
    let geometry = Rc::new(RefCell::new(GeometryState::default()));
    let zoom_anchor: Rc<RefCell<Option<(u32, f64)>>> = Rc::new(RefCell::new(None));

    let fixed = gtk::Fixed::new();
    fixed.set_hexpand(true);
    let spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    spacer.set_can_target(false);
    fixed.put(&spacer, 0.0, 0.0);

    let scrolled = gtk::ScrolledWindow::new();
    scrolled.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scrolled.set_hexpand(true);
    scrolled.set_vexpand(true);
    scrolled.set_child(Some(&fixed));

    let live_tiles: Rc<RefCell<HashMap<u32, TileWidget>>> =
        Rc::new(RefCell::new(HashMap::new()));
    let tile_pool: Rc<RefCell<VecDeque<TileWidget>>> =
        Rc::new(RefCell::new(VecDeque::new()));
    let live_headers: Rc<RefCell<HashMap<usize, gtk::Label>>> =
        Rc::new(RefCell::new(HashMap::new()));
    let header_pool: Rc<RefCell<VecDeque<gtk::Label>>> =
        Rc::new(RefCell::new(VecDeque::new()));
    let total_tiles_created = Rc::new(Cell::new(0u64));

    let paintables: Rc<RefCell<VecDeque<(String, gtk::gdk::Paintable)>>> =
        Rc::new(RefCell::new(VecDeque::new()));

    let status = gtk::Label::new(None);
    status.set_xalign(0.0);
    status.set_margin_start(12);
    status.set_margin_end(12);
    status.set_margin_top(8);
    status.set_margin_bottom(8);

    let refresh: Rc<dyn Fn()> = {
        let fixed = fixed.clone();
        let spacer = spacer.clone();
        let scrolled = scrolled.clone();
        let photos = photos.clone();
        let sections = sections.clone();
        let geometry = geometry.clone();
        let zoom = zoom.clone();
        let live_tiles = live_tiles.clone();
        let tile_pool = tile_pool.clone();
        let live_headers = live_headers.clone();
        let header_pool = header_pool.clone();
        let total_tiles_created = total_tiles_created.clone();
        let paintables = paintables.clone();
        let status = status.clone();

        Rc::new(move || {
            let width = scrolled.width().max(1);
            if geometry.borrow().sections.is_empty() || geometry.borrow().width != width {
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
            for (section_index, (section, geom)) in
                sections.iter().zip(g.sections.iter()).enumerate()
            {
                if geom.end_y < top || geom.header_y > bottom {
                    continue;
                }
                wanted_headers.push(section_index);

                let start_row = if top <= geom.first_photo_y {
                    0
                } else {
                    ((top - geom.first_photo_y) / row_h).floor().max(0.0) as u32
                };
                let end_row = (((bottom - geom.first_photo_y) / row_h)
                    .ceil()
                    .max(0.0) as u32)
                    .min(section.count.div_ceil(g.columns.max(1)));

                for row in start_row..end_row {
                    let row_start = section.start + row * g.columns.max(1);
                    for col in 0..g.columns.max(1) {
                        let index = row_start + col;
                        if index >= section.start + section.count {
                            break;
                        }
                        wanted_tiles.push((index, section_index, row, col));
                    }
                }
            }

            let wanted_ids = wanted_tiles
                .iter()
                .map(|entry| entry.0)
                .collect::<std::collections::HashSet<_>>();
            let stale = live_tiles
                .borrow()
                .keys()
                .copied()
                .filter(|id| !wanted_ids.contains(id))
                .collect::<Vec<_>>();
            for id in stale {
                if let Some(tile) = live_tiles.borrow_mut().remove(&id) {
                    fixed.remove(&tile.frame);
                    let mut pool = tile_pool.borrow_mut();
                    if pool.len() < MAX_TILE_POOL {
                        pool.push_back(tile);
                    }
                }
            }

            let mut requests = Vec::new();
            for (index, section_index, row, col) in wanted_tiles {
                let existing = {
                    let live = live_tiles.borrow();
                    live.get(&index).cloned()
                };
                let tile = if let Some(tile) = existing {
                    tile
                } else {
                    let tile = tile_pool.borrow_mut().pop_front().unwrap_or_else(|| {
                        total_tiles_created.set(total_tiles_created.get() + 1);
                        make_tile()
                    });
                    let Some(photo) = photos.get(index as usize) else {
                        continue;
                    };
                    tile.caption.set_label(&photo.filename());
                    tile.picture.set_paintable(gtk::gdk::Paintable::NONE);

                    if let Some(request) = display_request(photo) {
                        *tile.key.borrow_mut() = Some(request.key.clone());
                        let cached = {
                            let mut cache = paintables.borrow_mut();
                            cache
                                .iter()
                                .position(|(key, _)| key == &request.key)
                                .and_then(|position| cache.remove(position))
                        };
                        if let Some((key, paintable)) = cached {
                            tile.picture.set_paintable(Some(&paintable));
                            let mut cache = paintables.borrow_mut();
                            cache.push_back((key, paintable));
                        } else {
                            requests.push(request);
                        }
                    } else {
                        tile.key.borrow_mut().take();
                    }

                    fixed.put(&tile.frame, 0.0, 0.0);
                    live_tiles.borrow_mut().insert(index, tile.clone());
                    tile
                };

                tile.frame.set_size_request(g.tile_width, g.tile_height + 22);
                let geom = g.sections[section_index];
                let x = SIDE_MARGIN + f64::from(col) * (f64::from(g.tile_width) + GAP);
                let y = geom.first_photo_y + f64::from(row) * row_h;
                fixed.move_(&tile.frame, x, y);
            }

            if !requests.is_empty() {
                crate::thumbnail_display::replace_visible_requests(requests);
            }

            let wanted_header_ids = wanted_headers
                .iter()
                .copied()
                .collect::<std::collections::HashSet<_>>();
            let stale_headers = live_headers
                .borrow()
                .keys()
                .copied()
                .filter(|id| !wanted_header_ids.contains(id))
                .collect::<Vec<_>>();
            for id in stale_headers {
                if let Some(label) = live_headers.borrow_mut().remove(&id) {
                    fixed.remove(&label);
                    let mut pool = header_pool.borrow_mut();
                    if pool.len() < MAX_HEADER_POOL {
                        pool.push_back(label);
                    }
                }
            }

            for section_index in wanted_headers {
                let existing = {
                    let live = live_headers.borrow();
                    live.get(&section_index).cloned()
                };
                let label = if let Some(label) = existing {
                    label
                } else {
                    let label = header_pool.borrow_mut().pop_front().unwrap_or_else(|| {
                        let label = gtk::Label::new(None);
                        label.set_xalign(0.0);
                        label.add_css_class("title-3");
                        label
                    });
                    let section = &sections[section_index];
                    label.set_label(&format!("{}  ·  {} photos", section.title, section.count));
                    fixed.put(&label, SIDE_MARGIN, 0.0);
                    live_headers.borrow_mut().insert(section_index, label.clone());
                    label
                };
                label.set_size_request(
                    (width - (SIDE_MARGIN * 2.0) as i32).max(1),
                    HEADER_H as i32,
                );
                fixed.move_(&label, SIDE_MARGIN, g.sections[section_index].header_y);
            }

            status.set_text(&format!(
                "REAL photos={} folders={} live_tiles={} pool={} created={} rss={:.1}MB pending={} cols={} geometry={}us",
                photos.len(),
                sections.len(),
                live_tiles.borrow().len(),
                tile_pool.borrow().len(),
                total_tiles_created.get(),
                current_rss_kb().unwrap_or(0) as f64 / 1024.0,
                crate::thumbnail_display::pending_count(),
                g.columns,
                g.last_rebuild_us,
            ));
        })
    };

    {
        let refresh = refresh.clone();
        scrolled.vadjustment().connect_value_changed(move |_| refresh());
    }

    // Width changes only rebuild geometry. They never rebuild PhotoObjects or
    // FolderSection membership.
    {
        let refresh = refresh.clone();
        let geometry = geometry.clone();
        let sections = sections.clone();
        let scrolled_for_tick = scrolled.clone();
        let zoom = zoom.clone();
        let last_width = Rc::new(Cell::new(0i32));
        scrolled.add_tick_callback(move |_, _| {
            let width = scrolled_for_tick.width();
            if width > 0 && width != last_width.get() {
                last_width.set(width);
                rebuild_geometry(&mut geometry.borrow_mut(), &sections, width, zoom.get());
                refresh();
            }
            glib::ControlFlow::Continue
        });
    }

    // Poll the existing thumbnail-display completion queue and apply only to
    // currently bound recycled tiles. Stale completions are harmless.
    {
        let live_tiles = live_tiles.clone();
        let paintables = paintables.clone();
        status.add_tick_callback(move |_, _| {
            for completion in crate::thumbnail_display::take_completions() {
                let crate::thumbnail_display::DisplayOutcome::Loaded {
                    width,
                    height,
                    pixels,
                } = completion.outcome
                else {
                    continue;
                };
                let paintable = paintable_from_pixels(width, height, pixels);
                {
                    let mut cache = paintables.borrow_mut();
                    cache.retain(|(key, _)| key != &completion.key);
                    cache.push_back((completion.key.clone(), paintable.clone()));
                    while cache.len() > PAINTABLE_CACHE_CAP {
                        cache.pop_front();
                    }
                }
                for tile in live_tiles.borrow().values() {
                    if tile.key.borrow().as_deref() == Some(completion.key.as_str()) {
                        tile.picture.set_paintable(Some(&paintable));
                    }
                }
            }
            glib::ControlFlow::Continue
        });
    }

    let zoom_controller =
        gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
    zoom_controller.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let zoom = zoom.clone();
        let geometry = geometry.clone();
        let sections = sections.clone();
        let scrolled = scrolled.clone();
        let refresh = refresh.clone();
        let zoom_anchor = zoom_anchor.clone();
        zoom_controller.connect_scroll(move |controller, _, dy| {
            if !controller
                .current_event_state()
                .contains(gtk::gdk::ModifierType::CONTROL_MASK)
            {
                zoom_anchor.borrow_mut().take();
                return glib::Propagation::Proceed;
            }
            if dy == 0.0 {
                return glib::Propagation::Stop;
            }

            let old_zoom = zoom.get();
            let new_zoom = next_zoom(old_zoom, dy);
            if new_zoom == old_zoom {
                return glib::Propagation::Stop;
            }

            let adj = scrolled.vadjustment();
            let scroll_y = adj.value();
            let existing = {
                let anchor = zoom_anchor.borrow();
                *anchor
            };
            let before = if let Some(anchor) = existing {
                Some(anchor)
            } else {
                let picked = {
                    let g = geometry.borrow();
                    anchor_photo_at_view_offset(
                        scroll_y,
                        adj.page_size() * 0.5,
                        &sections,
                        &g,
                    )
                };
                if let Some(anchor) = picked {
                    *zoom_anchor.borrow_mut() = Some(anchor);
                }
                picked
            };

            zoom.set(new_zoom);
            rebuild_geometry(
                &mut geometry.borrow_mut(),
                &sections,
                scrolled.width().max(1),
                new_zoom,
            );

            if let Some((photo_id, offset)) = before {
                if let Some(new_y) = photo_y(photo_id, &sections, &geometry.borrow()) {
                    let upper = (adj.upper() - adj.page_size()).max(adj.lower());
                    adj.set_value((new_y - offset).clamp(adj.lower(), upper));
                    let actual_offset = new_y - adj.value();
                    *zoom_anchor.borrow_mut() = Some((photo_id, actual_offset));
                    eprintln!(
                        "SECTIONED_REAL_ZOOM old={} new={} anchor={} error_px={:.3} rss_kb={}",
                        old_zoom,
                        new_zoom,
                        photo_id,
                        (actual_offset - offset).abs(),
                        current_rss_kb().unwrap_or(0),
                    );
                }
            }
            refresh();
            glib::Propagation::Stop
        });
    }
    scrolled.add_controller(zoom_controller);

    let log = gtk::Button::with_label("Log real stats");
    {
        let photos = photos.clone();
        let sections = sections.clone();
        let live_tiles = live_tiles.clone();
        let tile_pool = tile_pool.clone();
        let total_tiles_created = total_tiles_created.clone();
        let geometry = geometry.clone();
        log.connect_clicked(move |_| {
            eprintln!(
                "SECTIONED_REAL_STATS photos={} folders={} live_tiles={} pool={} created={} rss_kb={} pending={} geometry_us={} rebuilds={}",
                photos.len(),
                sections.len(),
                live_tiles.borrow().len(),
                tile_pool.borrow().len(),
                total_tiles_created.get(),
                current_rss_kb().unwrap_or(0),
                crate::thumbnail_display::pending_count(),
                geometry.borrow().last_rebuild_us,
                geometry.borrow().rebuilds,
            );
        });
    }

    let controls = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    controls.set_margin_start(12);
    controls.set_margin_end(12);
    controls.set_margin_top(8);
    controls.append(&log);

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.append(&controls);
    root.append(&status);
    root.append(&scrolled);

    let css = gtk::CssProvider::new();
    css.load_from_string(
        r#"
        frame.sectioned-real-card {
            border: 1px solid alpha(currentColor, 0.18);
            border-radius: 6px;
            background: alpha(currentColor, 0.035);
        }
        "#,
    );
    gtk::style_context_add_provider_for_display(
        &root.display(),
        &css,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("PIC Real Sectioned Folder Prototype")
        .default_width(1400)
        .default_height(900)
        .content(&root)
        .build();

    rebuild_geometry(&mut geometry.borrow_mut(), &sections, 1400, zoom.get());
    refresh();

    eprintln!(
        "SECTIONED_REAL_READY photos={} folders={} build_ms={} rss_kb={}",
        photos.len(),
        sections.len(),
        started.elapsed().as_millis(),
        current_rss_kb().unwrap_or(0),
    );

    window
}
