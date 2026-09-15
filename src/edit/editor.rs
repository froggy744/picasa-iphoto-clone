use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use adw::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;
use rusqlite::Connection;

use super::filters::FilterPreset;
use super::model::{CropRect, EditRecipe, EditSession};

pub struct EditEditor {
    pub root: gtk::Box,
    photo_id: i64,
    zoom_in_action: Rc<dyn Fn()>,
    zoom_out_action: Rc<dyn Fn()>,
    fit_action: Rc<dyn Fn()>,
    one_to_one_action: Rc<dyn Fn(bool)>,
    set_library_rotation_action: Rc<dyn Fn(i32)>,
    one_to_one_sync: Rc<RefCell<Option<Box<dyn Fn(bool)>>>>,
}

impl EditEditor {
    pub fn photo_id(&self) -> i64 {
        self.photo_id
    }

    pub fn zoom_in(&self) {
        (self.zoom_in_action)();
    }

    pub fn zoom_out(&self) {
        (self.zoom_out_action)();
    }

    pub fn fit(&self) {
        (self.fit_action)();
    }

    pub fn set_one_to_one(&self, enabled: bool) {
        (self.one_to_one_action)(enabled);
    }

    pub fn set_library_rotation(&self, rotation: i32) {
        (self.set_library_rotation_action)(rotation);
    }

    pub fn set_one_to_one_sync_handler(&self, handler: impl Fn(bool) + 'static) {
        self.one_to_one_sync.replace(Some(Box::new(handler)));
    }
}

#[derive(Clone)]
struct PreviewBase {
    path: String,
    rotation: i32,
    target_width: u32,
    target_height: u32,
    image: image::RgbaImage,
}

impl PreviewBase {
    fn matches(&self, path: &str, rotation: i32, target_width: u32, target_height: u32) -> bool {
        self.path == path
            && self.rotation == rotation
            && self.target_width == target_width
            && self.target_height == target_height
    }
}

const PREVIEW_BASE_CACHE_CAPACITY: usize = 2;

fn cached_preview_base(
    cache: &Arc<Mutex<Vec<PreviewBase>>>,
    path: &str,
    rotation: i32,
    target_width: u32,
    target_height: u32,
) -> Option<image::RgbaImage> {
    cache
        .lock()
        .ok()?
        .iter()
        .find(|entry| entry.matches(path, rotation, target_width, target_height))
        .map(|entry| entry.image.clone())
}

fn store_preview_base(cache: &Arc<Mutex<Vec<PreviewBase>>>, entry: PreviewBase) {
    let Ok(mut cache) = cache.lock() else {
        return;
    };
    cache.retain(|existing| {
        !existing.matches(
            &entry.path,
            entry.rotation,
            entry.target_width,
            entry.target_height,
        )
    });
    cache.push(entry);
    while cache.len() > PREVIEW_BASE_CACHE_CAPACITY {
        cache.remove(0);
    }
}

#[derive(Clone)]
struct PreviewGeometry {
    path: String,
    rotation: i32,
    target_width: u32,
    target_height: u32,
    crop: CropRect,
    straighten: f32,
    image: image::RgbaImage,
}

impl PreviewGeometry {
    fn matches(
        &self,
        path: &str,
        rotation: i32,
        target_width: u32,
        target_height: u32,
        crop: CropRect,
        straighten: f32,
    ) -> bool {
        self.path == path
            && self.rotation == rotation
            && self.target_width == target_width
            && self.target_height == target_height
            && self.crop == crop.normalized()
            && self.straighten == straighten
    }
}

#[derive(Clone, Copy)]
enum OneToOneAnchor {
    Cursor {
        normalized_x: f64,
        normalized_y: f64,
        pointer_x: f64,
        pointer_y: f64,
    },
    Center,
}

struct PreviewJob {
    generation: u64,
    path: String,
    rotation: i32,
    target_width: u32,
    target_height: u32,
    recipe: EditRecipe,
    result_sender: std::sync::mpsc::Sender<anyhow::Result<(u64, u32, u32, Vec<u8>)>>,
}

fn spawn_preview_worker(
    base_cache: Arc<Mutex<Vec<PreviewBase>>>,
) -> std::sync::mpsc::Sender<PreviewJob> {
    let (job_sender, job_receiver) = std::sync::mpsc::channel::<PreviewJob>();
    std::thread::spawn(move || {
        let mut geometry_cache: Option<PreviewGeometry> = None;
        let mut pending: Option<PreviewJob> = None;

        loop {
            let mut job = match pending.take() {
                Some(job) => job,
                None => match job_receiver.recv() {
                    Ok(job) => job,
                    Err(_) => break,
                },
            };

            let mut coalesced = 0usize;
            while let Ok(newer) = job_receiver.try_recv() {
                job = newer;
                coalesced += 1;
            }
            if coalesced > 0 && std::env::var_os("PICASA_TRACE").is_some() {
                eprintln!(
                    "EDIT PREVIEW WORKER coalesced={} generation={}",
                    coalesced, job.generation
                );
            }

            let result = render_preview_job(&job, &base_cache, &mut geometry_cache).map(|image| {
                (
                    job.generation,
                    image.width(),
                    image.height(),
                    image.into_raw(),
                )
            });

            // If newer slider requests arrived while this frame was rendering,
            // keep only the newest one. The just-completed frame is stale, but
            // its decoded/geometry cache work remains useful to the next job.
            let mut newest = None;
            let mut superseded = 0usize;
            while let Ok(next) = job_receiver.try_recv() {
                newest = Some(next);
                superseded += 1;
            }
            if let Some(next) = newest {
                if std::env::var_os("PICASA_TRACE").is_some() {
                    eprintln!(
                        "EDIT PREVIEW WORKER superseded={} finished_generation={} next_generation={}",
                        superseded, job.generation, next.generation
                    );
                }
                pending = Some(next);
                continue;
            }

            let _ = job.result_sender.send(result);
        }
    });
    job_sender
}

fn render_preview_job(
    job: &PreviewJob,
    base_cache: &Arc<Mutex<Vec<PreviewBase>>>,
    geometry_cache: &mut Option<PreviewGeometry>,
) -> anyhow::Result<image::RgbaImage> {
    let total_started = Instant::now();
    let crop = job.recipe.crop.normalized();
    let straighten = job.recipe.straighten;
    let geometry_needed = straighten.abs() >= 0.01 || !crop.is_full();

    let geometry_hit = geometry_needed
        && geometry_cache.as_ref().is_some_and(|entry| {
            entry.matches(
                &job.path,
                job.rotation,
                job.target_width,
                job.target_height,
                crop,
                straighten,
            )
        });

    let mut base_hit = false;
    let mut decode_ms = 0u128;
    let mut geometry_ms = 0u128;

    let geometry = if geometry_hit {
        geometry_cache
            .as_ref()
            .expect("geometry cache matched")
            .image
            .clone()
    } else {
        let base = if let Some(image) = cached_preview_base(
            base_cache,
            &job.path,
            job.rotation,
            job.target_width,
            job.target_height,
        ) {
            base_hit = true;
            image
        } else {
            let decode_started = Instant::now();
            let image = super::render::decode_base_for_viewer(
                &job.path,
                job.rotation,
                job.target_width,
                job.target_height,
            )?;
            decode_ms = decode_started.elapsed().as_millis();
            store_preview_base(
                base_cache,
                PreviewBase {
                    path: job.path.clone(),
                    rotation: job.rotation,
                    target_width: job.target_width,
                    target_height: job.target_height,
                    image: image.clone(),
                },
            );
            image
        };

        if geometry_needed {
            let geometry_started = Instant::now();
            let image = super::render::apply_geometry(base, &job.recipe);
            geometry_ms = geometry_started.elapsed().as_millis();
            geometry_cache.replace(PreviewGeometry {
                path: job.path.clone(),
                rotation: job.rotation,
                target_width: job.target_width,
                target_height: job.target_height,
                crop,
                straighten,
                image: image.clone(),
            });
            image
        } else {
            base
        }
    };

    let tone_started = Instant::now();
    let rendered = super::render::apply_tone(geometry, &job.recipe);
    let tone_ms = tone_started.elapsed().as_millis();

    if std::env::var_os("PICASA_TRACE").is_some() {
        eprintln!(
            "EDIT PREVIEW base_cache={} geometry_cache={} decode_ms={} geometry_ms={} tone_ms={} total_ms={} target={}x{} path={}",
            if geometry_hit {
                "skip"
            } else if base_hit {
                "hit"
            } else {
                "miss"
            },
            if geometry_hit {
                "hit"
            } else if geometry_needed {
                "miss"
            } else {
                "bypass"
            },
            decode_ms,
            geometry_ms,
            tone_ms,
            total_started.elapsed().as_millis(),
            job.target_width,
            job.target_height,
            job.path
        );
    }

    Ok(rendered)
}

#[derive(Clone)]
struct Controls {
    straighten: gtk::Scale,
    exposure: gtk::Scale,
    contrast: gtk::Scale,
    fill_light: gtk::Scale,
    highlights: gtk::Scale,
    shadows: gtk::Scale,
    temperature: gtk::Scale,
    saturation: gtk::Scale,
    sharpen: gtk::Scale,
    auto_contrast: gtk::ToggleButton,
    auto_color: gtk::ToggleButton,
    black_white: gtk::ToggleButton,
    sepia: gtk::ToggleButton,
    filters: Vec<(FilterPreset, gtk::ToggleButton)>,
}

impl Controls {
    fn sync(&self, recipe: &EditRecipe) {
        self.straighten.set_value(recipe.straighten as f64);
        self.exposure.set_value(recipe.exposure as f64);
        self.contrast.set_value(recipe.contrast as f64);
        self.fill_light.set_value(recipe.fill_light as f64);
        self.highlights.set_value(recipe.highlights as f64);
        self.shadows.set_value(recipe.shadows as f64);
        self.temperature.set_value(recipe.temperature as f64);
        self.saturation.set_value(recipe.saturation as f64);
        self.sharpen.set_value(recipe.sharpen as f64);
        self.auto_contrast.set_active(recipe.auto_contrast);
        self.auto_color.set_active(recipe.auto_color);
        self.black_white.set_active(recipe.black_white);
        self.sepia.set_active(recipe.sepia);
        for (preset, button) in &self.filters {
            button.set_active(recipe.filter == *preset);
        }
    }
}

pub fn build(
    parent: &gtk::Window,
    connection: Rc<RefCell<Connection>>,
    photo: crate::photo_object::PhotoObject,
    on_close: Rc<dyn Fn()>,
    on_saved: Rc<dyn Fn(crate::photo_object::PhotoObject)>,
) -> EditEditor {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.set_hexpand(true);
    root.set_vexpand(true);

    let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    toolbar.set_margin_top(8);
    toolbar.set_margin_bottom(8);
    toolbar.set_margin_start(12);
    toolbar.set_margin_end(12);
    toolbar.add_css_class("toolbar");

    let back = gtk::Button::from_icon_name("go-previous-symbolic");
    back.set_tooltip_text(Some("Close editor without saving"));
    toolbar.append(&back);

    let title = gtk::Label::new(Some(&format!("Edit — {}", photo.filename())));
    title.set_xalign(0.0);
    title.set_hexpand(true);
    title.add_css_class("title-3");
    toolbar.append(&title);

    let undo = gtk::Button::from_icon_name("edit-undo-symbolic");
    undo.set_tooltip_text(Some("Undo"));
    toolbar.append(&undo);
    let redo = gtk::Button::from_icon_name("edit-redo-symbolic");
    redo.set_tooltip_text(Some("Redo"));
    toolbar.append(&redo);

    let toolbar_zoom_out = gtk::Button::with_label("−");
    toolbar_zoom_out.set_tooltip_text(Some("Zoom out (Ctrl + mouse wheel)"));
    toolbar.append(&toolbar_zoom_out);
    let toolbar_zoom_in = gtk::Button::with_label("+");
    toolbar_zoom_in.set_tooltip_text(Some("Zoom in (Ctrl + mouse wheel)"));
    toolbar.append(&toolbar_zoom_in);

    let reset = gtk::Button::with_label("Reset");
    reset.set_tooltip_text(Some("Reset all edits to the original"));
    toolbar.append(&reset);

    let tools_toggle = gtk::ToggleButton::with_label("Tools");
    tools_toggle.set_active(true);
    tools_toggle.set_tooltip_text(Some("Show editing controls"));
    toolbar.append(&tools_toggle);

    let filters_toggle = gtk::ToggleButton::with_label("Filters");
    filters_toggle.set_group(Some(&tools_toggle));
    filters_toggle.set_tooltip_text(Some("Show one-tap photo filters"));
    toolbar.append(&filters_toggle);

    let export = gtk::Button::with_label("Export");
    export.set_tooltip_text(Some("Export the current edited photo as a new JPEG"));
    toolbar.append(&export);

    let done = gtk::Button::with_label("Done");
    done.add_css_class("suggested-action");
    toolbar.append(&done);
    root.append(&toolbar);

    // The editor tools use a real split pane, matching the adjustable main
    // sidebar. Users can drag the divider to give the controls or preview more
    // room instead of being locked to one fixed editing-panel width.
    let body = gtk::Paned::new(gtk::Orientation::Horizontal);
    body.set_hexpand(true);
    body.set_vexpand(true);
    body.set_position(315);
    body.set_resize_start_child(false);
    body.set_shrink_start_child(false);
    body.set_wide_handle(true);
    root.append(&body);

    let tools_box = gtk::Box::new(gtk::Orientation::Vertical, 12);
    tools_box.set_width_request(285);
    tools_box.set_margin_top(14);
    tools_box.set_margin_bottom(14);
    tools_box.set_margin_start(14);
    tools_box.set_margin_end(14);
    let filters_box = gtk::Box::new(gtk::Orientation::Vertical, 12);
    filters_box.set_visible(true);
    filters_box.set_hexpand(true);
    filters_box.set_vexpand(true);
    filters_box.set_width_request(285);
    filters_box.set_margin_top(14);
    filters_box.set_margin_bottom(14);
    filters_box.set_margin_start(14);
    filters_box.set_margin_end(14);

    let panel_stack = gtk::Stack::new();
    panel_stack.set_hexpand(true);
    panel_stack.set_vexpand(true);
    panel_stack.add_named(&tools_box, Some("tools"));
    panel_stack.add_named(&filters_box, Some("filters"));
    panel_stack.set_visible_child(&tools_box);

    let tools_scroll = gtk::ScrolledWindow::new();
    tools_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    tools_scroll.set_width_request(315);
    tools_scroll.set_child(Some(&panel_stack));
    body.set_start_child(Some(&tools_scroll));

    let preview_area = gtk::Overlay::new();
    preview_area.set_hexpand(true);
    preview_area.set_vexpand(true);
    preview_area.set_margin_top(18);
    preview_area.set_margin_bottom(18);
    preview_area.set_margin_start(18);
    preview_area.set_margin_end(18);
    preview_area.add_css_class("edit-preview");
    body.set_end_child(Some(&preview_area));

    let picture = gtk::Picture::new();
    picture.set_content_fit(gtk::ContentFit::Contain);
    picture.set_can_shrink(true);
    picture.set_hexpand(true);
    picture.set_vexpand(true);

    let picture_scroll = gtk::ScrolledWindow::new();
    picture_scroll.set_hexpand(true);
    picture_scroll.set_vexpand(true);
    picture_scroll.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
    picture_scroll.set_child(Some(&picture));
    preview_area.set_child(Some(&picture_scroll));

    let crop_overlay = gtk::DrawingArea::new();
    crop_overlay.set_hexpand(true);
    crop_overlay.set_vexpand(true);
    crop_overlay.set_visible(false);
    crop_overlay.set_cursor_from_name(Some("crosshair"));
    preview_area.add_overlay(&crop_overlay);

    let busy = gtk::Spinner::new();
    busy.set_halign(gtk::Align::Center);
    busy.set_valign(gtk::Align::Center);
    busy.set_visible(false);
    preview_area.add_overlay(&busy);

    let status = gtk::Label::new(Some("Ready"));
    status.set_halign(gtk::Align::Start);
    status.set_valign(gtk::Align::End);
    status.set_margin_start(12);
    status.set_margin_bottom(10);
    status.add_css_class("dim-label");
    preview_area.add_overlay(&status);

    let mut recipe = EditRecipe::decode(&photo.edit_recipe());
    // Sepia is already a warm monochrome treatment, so presenting B&W and
    // Sepia as simultaneously active is misleading. Preserve old recipes by
    // letting Sepia win when both legacy flags are present.
    if recipe.sepia {
        recipe.black_white = false;
    }
    let session = Rc::new(RefCell::new(EditSession::new(recipe)));
    let syncing = Rc::new(Cell::new(false));
    let pending_crop = Rc::new(RefCell::new(CropRect::default()));
    let preview_dimensions = Rc::new(Cell::new((1i32, 1i32)));
    // 0.0 means fit-to-canvas. Positive values are display zoom factors.
    let canvas_zoom = Rc::new(Cell::new(0.0f64));
    let native_one_to_one = Rc::new(Cell::new(false));
    let active_rotation = Rc::new(Cell::new(photo.rotation().rem_euclid(360)));
    let pending_one_to_one_anchor: Rc<RefCell<Option<OneToOneAnchor>>> =
        Rc::new(RefCell::new(None));
    let one_to_one_sync: Rc<RefCell<Option<Box<dyn Fn(bool)>>>> = Rc::new(RefCell::new(None));
    let generation = Rc::new(Cell::new(0u64));
    let preview_debounce: Rc<RefCell<Option<glib::SourceId>>> = Rc::new(RefCell::new(None));
    // Keep both the normal Fit preview base and a prefetched native-resolution
    // base. Sharing this tiny cache with the prefetch thread lets 1:1 reuse the
    // decoded pixels without evicting the fast 1800x1400 editing base.
    let preview_base_cache: Arc<Mutex<Vec<PreviewBase>>> = Arc::new(Mutex::new(Vec::new()));
    let preview_worker = spawn_preview_worker(preview_base_cache.clone());

    // Drag-to-pan for the editing canvas. Keep this controller on the
    // stationary ScrolledWindow so pointer coordinates do not move with the
    // image. Crop mode sits in an overlay above it and therefore keeps its own
    // drag gesture without conflict.
    let pan_origin_h = Rc::new(Cell::new(0.0f64));
    let pan_origin_v = Rc::new(Cell::new(0.0f64));
    let pan_drag = gtk::GestureDrag::new();
    pan_drag.set_button(1);
    pan_drag.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let picture_scroll = picture_scroll.clone();
        let pan_origin_h = pan_origin_h.clone();
        let pan_origin_v = pan_origin_v.clone();
        pan_drag.connect_drag_begin(move |_, _, _| {
            let hadj = picture_scroll.hadjustment();
            let vadj = picture_scroll.vadjustment();
            pan_origin_h.set(hadj.value());
            pan_origin_v.set(vadj.value());
            if hadj.upper() > hadj.page_size() || vadj.upper() > vadj.page_size() {
                picture_scroll.set_cursor_from_name(Some("grabbing"));
            }
        });
    }
    {
        let picture_scroll = picture_scroll.clone();
        let pan_origin_h = pan_origin_h.clone();
        let pan_origin_v = pan_origin_v.clone();
        pan_drag.connect_drag_update(move |_, dx, dy| {
            let hadj = picture_scroll.hadjustment();
            let vadj = picture_scroll.vadjustment();
            let max_h = (hadj.upper() - hadj.page_size()).max(hadj.lower());
            let max_v = (vadj.upper() - vadj.page_size()).max(vadj.lower());
            hadj.set_value((pan_origin_h.get() - dx).clamp(hadj.lower(), max_h));
            vadj.set_value((pan_origin_v.get() - dy).clamp(vadj.lower(), max_v));
        });
    }
    {
        let picture_scroll = picture_scroll.clone();
        let canvas_zoom = canvas_zoom.clone();
        let native_one_to_one = native_one_to_one.clone();
        pan_drag.connect_drag_end(move |_, _, _| {
            picture_scroll.set_cursor_from_name(
                if native_one_to_one.get() || canvas_zoom.get() > 1.0 {
                    Some("grab")
                } else {
                    None
                },
            );
        });
    }
    picture_scroll.add_controller(pan_drag);

    add_section_label(&tools_box, "Basic fixes");
    let auto_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let auto_contrast = gtk::ToggleButton::with_label("Auto Contrast");
    let auto_color = gtk::ToggleButton::with_label("Auto Colour");
    auto_contrast.set_hexpand(true);
    auto_color.set_hexpand(true);
    auto_row.append(&auto_contrast);
    auto_row.append(&auto_color);
    tools_box.append(&auto_row);

    let black_white = gtk::ToggleButton::with_label("B&W");
    let sepia = gtk::ToggleButton::with_label("Sepia");

    add_section_label(&filters_box, "Filters");
    let filter_grid = gtk::Grid::new();
    filter_grid.set_row_spacing(6);
    filter_grid.set_column_spacing(6);
    filter_grid.set_column_homogeneous(true);
    let mut filter_buttons = Vec::new();
    let filter_presets = std::iter::once(FilterPreset::None)
        .chain(FilterPreset::ALL)
        .collect::<Vec<_>>();
    for (index, preset) in filter_presets.iter().copied().enumerate() {
        let button = gtk::ToggleButton::with_label(preset.label());
        button.set_hexpand(true);
        filter_grid.attach(&button, (index % 2) as i32, (index / 2) as i32, 1, 1);
        filter_buttons.push((preset, button));
    }
    let legacy_filter_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    black_white.set_hexpand(true);
    sepia.set_hexpand(true);
    legacy_filter_row.append(&black_white);
    legacy_filter_row.append(&sepia);
    filters_box.append(&filter_grid);
    filters_box.append(&legacy_filter_row);

    add_section_label(&tools_box, "Geometry");
    let crop = gtk::Button::with_label("Crop…");
    crop.set_halign(gtk::Align::Fill);
    tools_box.append(&crop);
    let crop_actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let crop_apply = gtk::Button::with_label("Apply Crop");
    crop_apply.add_css_class("suggested-action");
    let crop_cancel = gtk::Button::with_label("Cancel");
    let crop_reset = gtk::Button::with_label("Reset Crop");
    crop_actions.append(&crop_apply);
    crop_actions.append(&crop_cancel);
    crop_actions.append(&crop_reset);
    crop_actions.set_visible(false);
    tools_box.append(&crop_actions);

    let straighten = add_slider(&tools_box, "Straighten", -10.0, 10.0, 0.1, 1);

    add_section_label(&tools_box, "Tuning");
    let exposure = add_slider(&tools_box, "Exposure", -2.0, 2.0, 0.05, 2);
    let contrast = add_slider(&tools_box, "Contrast", -1.0, 1.0, 0.02, 2);
    let fill_light = add_slider(&tools_box, "Fill Light", -1.0, 1.0, 0.02, 2);
    let highlights = add_slider(&tools_box, "Highlights", -1.0, 1.0, 0.02, 2);
    let shadows = add_slider(&tools_box, "Shadows", -1.0, 1.0, 0.02, 2);
    let temperature = add_slider(&tools_box, "Colour Temperature", -1.0, 1.0, 0.02, 2);
    let saturation = add_slider(&tools_box, "Saturation", -1.0, 1.0, 0.02, 2);
    let sharpen = add_slider(&tools_box, "Sharpen", 0.0, 1.0, 0.02, 2);

    let controls = Controls {
        straighten,
        exposure,
        contrast,
        fill_light,
        highlights,
        shadows,
        temperature,
        saturation,
        sharpen,
        auto_contrast,
        auto_color,
        black_white,
        sepia,
        filters: filter_buttons,
    };
    controls.sync(&session.borrow().recipe);

    // Ordinary wheel scrolling over a slider should continue scrolling the
    // tools panel. Only the narrow track area in the middle of the scale keeps
    // the normal GTK wheel-to-adjust behavior.
    for scale in [
        &controls.straighten,
        &controls.exposure,
        &controls.contrast,
        &controls.fill_light,
        &controls.highlights,
        &controls.shadows,
        &controls.temperature,
        &controls.saturation,
        &controls.sharpen,
    ] {
        configure_scale_scroll(scale, &tools_scroll);
    }

    let active_rotation_for_queue = active_rotation.clone();
    let queue_preview: Rc<dyn Fn()> = {
        let session = session.clone();
        let picture = picture.clone();
        let status = status.clone();
        let busy = busy.clone();
        let photo = photo.clone();
        let generation = generation.clone();
        let preview_dimensions = preview_dimensions.clone();
        let picture_scroll = picture_scroll.clone();
        let canvas_zoom = canvas_zoom.clone();
        let native_one_to_one = native_one_to_one.clone();
        let pending_one_to_one_anchor = pending_one_to_one_anchor.clone();
        let crop_overlay = crop_overlay.clone();
        let preview_debounce = preview_debounce.clone();
        let preview_worker = preview_worker.clone();
        Rc::new(move || {
            if let Some(source) = preview_debounce.borrow_mut().take() {
                source.remove();
            }
            let session = session.clone();
            let picture = picture.clone();
            let status = status.clone();
            let busy = busy.clone();
            let photo = photo.clone();
            let generation = generation.clone();
            let preview_dimensions = preview_dimensions.clone();
            let picture_scroll = picture_scroll.clone();
            let canvas_zoom = canvas_zoom.clone();
            let native_one_to_one = native_one_to_one.clone();
            let pending_one_to_one_anchor = pending_one_to_one_anchor.clone();
            let crop_overlay = crop_overlay.clone();
            let preview_debounce_for_fire = preview_debounce.clone();
            let preview_worker = preview_worker.clone();
            let active_rotation = active_rotation_for_queue.clone();
            let source = glib::timeout_add_local_once(Duration::from_millis(90), move || {
                preview_debounce_for_fire.borrow_mut().take();
                let current_generation = generation.get().wrapping_add(1);
                generation.set(current_generation);
                let recipe = session.borrow().recipe.clone();
                let path = photo.path();
                let rotation = active_rotation.get();
                let native = native_one_to_one.get();
                let (target_width, target_height) = if native {
                    let mut width = if photo.width() > 0 {
                        photo.width() as u32
                    } else {
                        u32::MAX
                    };
                    let mut height = if photo.height() > 0 {
                        photo.height() as u32
                    } else {
                        u32::MAX
                    };
                    if matches!(rotation.rem_euclid(360), 90 | 270) {
                        std::mem::swap(&mut width, &mut height);
                    }
                    (width, height)
                } else {
                    (1800, 1400)
                };

                busy.set_visible(true);
                busy.start();
                status.set_text("Rendering preview…");

                let (result_sender, receiver) = std::sync::mpsc::channel();
                if preview_worker
                    .send(PreviewJob {
                        generation: current_generation,
                        path,
                        rotation,
                        target_width,
                        target_height,
                        recipe,
                        result_sender,
                    })
                    .is_err()
                {
                    busy.stop();
                    busy.set_visible(false);
                    status.set_text("Preview worker stopped");
                    return;
                }

                let picture = picture.clone();
                let status = status.clone();
                let busy = busy.clone();
                let generation = generation.clone();
                let preview_dimensions = preview_dimensions.clone();
                let picture_scroll = picture_scroll.clone();
                let canvas_zoom = canvas_zoom.clone();
                let pending_one_to_one_anchor = pending_one_to_one_anchor.clone();
                let crop_overlay = crop_overlay.clone();
                glib::timeout_add_local(Duration::from_millis(25), move || {
                    match receiver.try_recv() {
                        Ok(result) => {
                            if generation.get() != current_generation {
                                return glib::ControlFlow::Break;
                            }
                            busy.stop();
                            busy.set_visible(false);
                            match result {
                                Ok((result_generation, width, height, pixels)) => {
                                    if result_generation != current_generation {
                                        return glib::ControlFlow::Break;
                                    }
                                    let bytes = glib::Bytes::from_owned(pixels);
                                    let texture = gtk::gdk::MemoryTexture::new(
                                        width as i32,
                                        height as i32,
                                        gtk::gdk::MemoryFormat::R8g8b8a8,
                                        &bytes,
                                        width as usize * 4,
                                    );
                                    preview_dimensions.set((width as i32, height as i32));
                                    picture.set_paintable(Some(&texture));
                                    if native {
                                        apply_canvas_one_to_one(
                                            &picture,
                                            &picture_scroll,
                                            (width as i32, height as i32),
                                        );

                                        let anchor = pending_one_to_one_anchor.borrow_mut().take();
                                        let picture_scroll_for_restore = picture_scroll.clone();
                                        let picture_scroll_for_tick = picture_scroll.clone();
                                        picture_scroll_for_restore.add_tick_callback(move |_, _| {
                                            let hadj = picture_scroll_for_tick.hadjustment();
                                            let vadj = picture_scroll_for_tick.vadjustment();
                                            if hadj.upper() < width as f64 && vadj.upper() < height as f64 {
                                                return glib::ControlFlow::Continue;
                                            }
                                            let h_max = (hadj.upper() - hadj.page_size()).max(hadj.lower());
                                            let v_max = (vadj.upper() - vadj.page_size()).max(vadj.lower());
                                            let (target_h, target_v, anchored, trace_anchor) = match anchor {
                                                Some(OneToOneAnchor::Cursor {
                                                    normalized_x,
                                                    normalized_y,
                                                    pointer_x,
                                                    pointer_y,
                                                }) => (
                                                    normalized_scroll_target(
                                                        normalized_x,
                                                        pointer_x,
                                                        hadj.page_size(),
                                                        width as f64,
                                                    ),
                                                    normalized_scroll_target(
                                                        normalized_y,
                                                        pointer_y,
                                                        vadj.page_size(),
                                                        height as f64,
                                                    ),
                                                    true,
                                                    Some((normalized_x, normalized_y, pointer_x, pointer_y)),
                                                ),
                                                _ => (
                                                    one_to_one_scroll_target(
                                                        0.0,
                                                        0.0,
                                                        hadj.page_size(),
                                                        1.0,
                                                        width as f64,
                                                        false,
                                                    ),
                                                    one_to_one_scroll_target(
                                                        0.0,
                                                        0.0,
                                                        vadj.page_size(),
                                                        1.0,
                                                        height as f64,
                                                        false,
                                                    ),
                                                    false,
                                                    None,
                                                ),
                                            };
                                            hadj.set_value(target_h.clamp(hadj.lower(), h_max));
                                            vadj.set_value(target_v.clamp(vadj.lower(), v_max));
                                            if std::env::var_os("PICASA_TRACE").is_some() {
                                                eprintln!(
                                                    "EDIT 1TO1 anchor={} normalized={:?} scroll=({:.1},{:.1}) target={}x{}",
                                                    if anchored { "cursor" } else { "center" },
                                                    trace_anchor,
                                                    hadj.value(),
                                                    vadj.value(),
                                                    width,
                                                    height
                                                );
                                            }
                                            glib::ControlFlow::Break
                                        });
                                    } else {
                                        apply_canvas_zoom(
                                            &picture,
                                            &picture_scroll,
                                            (width as i32, height as i32),
                                            canvas_zoom.get(),
                                        );
                                    }
                                    status.set_text(&format!("{} × {} preview", width, height));
                                    crop_overlay.queue_draw();
                                }
                                Err(error) => {
                                    status.set_text(&format!("Preview failed: {error}"));
                                }
                            }
                            glib::ControlFlow::Break
                        }
                        Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                            // A disconnected per-request receiver normally means
                            // the worker coalesced this stale request into a newer
                            // one. Only change UI state if it was still current.
                            if generation.get() == current_generation {
                                busy.stop();
                                busy.set_visible(false);
                                status.set_text("Preview worker stopped");
                            }
                            glib::ControlFlow::Break
                        }
                    }
                });
            });
            preview_debounce.replace(Some(source));
        })
    };

    let update_history_buttons: Rc<dyn Fn()> = {
        let session = session.clone();
        let undo = undo.clone();
        let redo = redo.clone();
        Rc::new(move || {
            undo.set_sensitive(session.borrow().can_undo());
            redo.set_sensitive(session.borrow().can_redo());
        })
    };
    update_history_buttons();

    connect_scale(
        &controls.straighten,
        session.clone(),
        syncing.clone(),
        queue_preview.clone(),
        update_history_buttons.clone(),
        |recipe, value| recipe.straighten = value,
    );
    connect_scale(
        &controls.exposure,
        session.clone(),
        syncing.clone(),
        queue_preview.clone(),
        update_history_buttons.clone(),
        |r, v| r.exposure = v,
    );
    connect_scale(
        &controls.contrast,
        session.clone(),
        syncing.clone(),
        queue_preview.clone(),
        update_history_buttons.clone(),
        |r, v| r.contrast = v,
    );
    connect_scale(
        &controls.fill_light,
        session.clone(),
        syncing.clone(),
        queue_preview.clone(),
        update_history_buttons.clone(),
        |r, v| r.fill_light = v,
    );
    connect_scale(
        &controls.highlights,
        session.clone(),
        syncing.clone(),
        queue_preview.clone(),
        update_history_buttons.clone(),
        |r, v| r.highlights = v,
    );
    connect_scale(
        &controls.shadows,
        session.clone(),
        syncing.clone(),
        queue_preview.clone(),
        update_history_buttons.clone(),
        |r, v| r.shadows = v,
    );
    connect_scale(
        &controls.temperature,
        session.clone(),
        syncing.clone(),
        queue_preview.clone(),
        update_history_buttons.clone(),
        |r, v| r.temperature = v,
    );
    connect_scale(
        &controls.saturation,
        session.clone(),
        syncing.clone(),
        queue_preview.clone(),
        update_history_buttons.clone(),
        |r, v| r.saturation = v,
    );
    connect_scale(
        &controls.sharpen,
        session.clone(),
        syncing.clone(),
        queue_preview.clone(),
        update_history_buttons.clone(),
        |r, v| r.sharpen = v,
    );

    connect_toggle(
        &controls.auto_contrast,
        session.clone(),
        syncing.clone(),
        queue_preview.clone(),
        update_history_buttons.clone(),
        |r, v| r.auto_contrast = v,
    );
    connect_toggle(
        &controls.auto_color,
        session.clone(),
        syncing.clone(),
        queue_preview.clone(),
        update_history_buttons.clone(),
        |r, v| r.auto_color = v,
    );
    {
        let session = session.clone();
        let syncing = syncing.clone();
        let sepia = controls.sepia.clone();
        let filters = controls
            .filters
            .iter()
            .map(|(_, button)| button.clone())
            .collect::<Vec<_>>();
        let queue_preview = queue_preview.clone();
        let update_history_buttons = update_history_buttons.clone();
        controls.black_white.connect_toggled(move |button| {
            if syncing.get() {
                return;
            }
            let active = button.is_active();
            if active {
                syncing.set(true);
                sepia.set_active(false);
                for filter in &filters {
                    filter.set_active(false);
                }
                syncing.set(false);
            }
            session.borrow_mut().mutate(|recipe| {
                recipe.black_white = active;
                recipe.filter = FilterPreset::None;
                if active {
                    recipe.sepia = false;
                }
            });
            update_history_buttons();
            queue_preview();
        });
    }
    {
        let session = session.clone();
        let syncing = syncing.clone();
        let black_white = controls.black_white.clone();
        let filters = controls
            .filters
            .iter()
            .map(|(_, button)| button.clone())
            .collect::<Vec<_>>();
        let queue_preview = queue_preview.clone();
        let update_history_buttons = update_history_buttons.clone();
        controls.sepia.connect_toggled(move |button| {
            if syncing.get() {
                return;
            }
            let active = button.is_active();
            if active {
                syncing.set(true);
                black_white.set_active(false);
                for filter in &filters {
                    filter.set_active(false);
                }
                syncing.set(false);
            }
            session.borrow_mut().mutate(|recipe| {
                recipe.sepia = active;
                recipe.filter = FilterPreset::None;
                if active {
                    recipe.black_white = false;
                }
            });
            update_history_buttons();
            queue_preview();
        });
    }
    for (preset, button) in &controls.filters {
        let preset = *preset;
        let session = session.clone();
        let syncing = syncing.clone();
        let black_white = controls.black_white.clone();
        let sepia = controls.sepia.clone();
        let other_filters = controls
            .filters
            .iter()
            .filter(|(other, _)| *other != preset)
            .map(|(_, other)| other.clone())
            .collect::<Vec<_>>();
        let queue_preview = queue_preview.clone();
        let update_history_buttons = update_history_buttons.clone();
        button.connect_toggled(move |button| {
            if syncing.get() {
                return;
            }
            let active = button.is_active();
            if active {
                syncing.set(true);
                black_white.set_active(false);
                sepia.set_active(false);
                for other in &other_filters {
                    other.set_active(false);
                }
                syncing.set(false);
            }
            session.borrow_mut().mutate(|recipe| {
                recipe.filter = if active { preset } else { FilterPreset::None };
                recipe.black_white = false;
                recipe.sepia = false;
            });
            update_history_buttons();
            queue_preview();
        });
    }

    {
        let panel_stack = panel_stack.clone();
        let tools_box = tools_box.clone();
        let filters_toggle = filters_toggle.clone();
        tools_toggle.connect_toggled(move |button| {
            if button.is_active() {
                filters_toggle.set_active(false);
                panel_stack.set_visible_child(&tools_box);
            }
        });
    }
    {
        let panel_stack = panel_stack.clone();
        let filters_box = filters_box.clone();
        let tools_toggle = tools_toggle.clone();
        filters_toggle.connect_toggled(move |button| {
            if button.is_active() {
                tools_toggle.set_active(false);
                panel_stack.set_visible_child(&filters_box);
            }
        });
    }

    let sync_controls: Rc<dyn Fn()> = {
        let controls = controls.clone();
        let session = session.clone();
        let syncing = syncing.clone();
        Rc::new(move || {
            syncing.set(true);
            controls.sync(&session.borrow().recipe);
            syncing.set(false);
        })
    };

    {
        let session = session.clone();
        let sync_controls = sync_controls.clone();
        let queue_preview = queue_preview.clone();
        let update_history_buttons = update_history_buttons.clone();
        undo.connect_clicked(move |_| {
            if session.borrow_mut().undo() {
                sync_controls();
                update_history_buttons();
                queue_preview();
            }
        });
    }
    {
        let session = session.clone();
        let sync_controls = sync_controls.clone();
        let queue_preview = queue_preview.clone();
        let update_history_buttons = update_history_buttons.clone();
        redo.connect_clicked(move |_| {
            if session.borrow_mut().redo() {
                sync_controls();
                update_history_buttons();
                queue_preview();
            }
        });
    }
    {
        let session = session.clone();
        let sync_controls = sync_controls.clone();
        let queue_preview = queue_preview.clone();
        let update_history_buttons = update_history_buttons.clone();
        let crop_overlay = crop_overlay.clone();
        let crop_actions = crop_actions.clone();
        let picture = picture.clone();
        let picture_scroll = picture_scroll.clone();
        let preview_dimensions = preview_dimensions.clone();
        let canvas_zoom = canvas_zoom.clone();
        let native_one_to_one = native_one_to_one.clone();
        let one_to_one_sync = one_to_one_sync.clone();
        reset.connect_clicked(move |_| {
            session.borrow_mut().reset();
            sync_controls();
            update_history_buttons();
            crop_overlay.set_visible(false);
            crop_actions.set_visible(false);

            // Reset is a full editor reset: edits plus canvas presentation.
            // Return to Fit and clear any native 1:1 / panned viewport state.
            let was_one_to_one = native_one_to_one.replace(false);
            if was_one_to_one {
                if let Some(handler) = one_to_one_sync.borrow().as_ref() {
                    handler(false);
                }
            }
            canvas_zoom.set(0.0);
            apply_canvas_zoom(&picture, &picture_scroll, preview_dimensions.get(), 0.0);
            let hadj = picture_scroll.hadjustment();
            let vadj = picture_scroll.vadjustment();
            hadj.set_value(hadj.lower());
            vadj.set_value(vadj.lower());

            queue_preview();
        });
    }

    let fit_action: Rc<dyn Fn()> = {
        let picture = picture.clone();
        let picture_scroll = picture_scroll.clone();
        let preview_dimensions = preview_dimensions.clone();
        let canvas_zoom = canvas_zoom.clone();
        let native_one_to_one = native_one_to_one.clone();
        let one_to_one_sync = one_to_one_sync.clone();
        Rc::new(move || {
            let was_one_to_one = native_one_to_one.replace(false);
            if was_one_to_one {
                if let Some(handler) = one_to_one_sync.borrow().as_ref() {
                    handler(false);
                }
            }
            canvas_zoom.set(0.0);
            apply_canvas_zoom(&picture, &picture_scroll, preview_dimensions.get(), 0.0);
        })
    };
    // Canvas zoom is stored as a multiplier relative to Fit, not as an
    // absolute source-pixel scale. 0.0 means exactly Fit. +/- move in small
    // 10% Fit-relative steps so only the explicit 1:1 control can jump to
    // native pixels, even when the preview texture is close to viewport size.
    let zoom_out_action: Rc<dyn Fn()> = {
        let picture = picture.clone();
        let picture_scroll = picture_scroll.clone();
        let preview_dimensions = preview_dimensions.clone();
        let canvas_zoom = canvas_zoom.clone();
        let native_one_to_one = native_one_to_one.clone();
        let one_to_one_sync = one_to_one_sync.clone();
        Rc::new(move || {
            let was_one_to_one = native_one_to_one.replace(false);
            if was_one_to_one {
                if let Some(handler) = one_to_one_sync.borrow().as_ref() {
                    handler(false);
                }
            }
            let dimensions = preview_dimensions.get();
            let current_multiplier = if canvas_zoom.get() <= 0.0 {
                1.0
            } else {
                canvas_zoom.get()
            };
            let next_multiplier = (current_multiplier - 0.10).clamp(0.25, 8.0);
            canvas_zoom.set(next_multiplier);
            apply_canvas_zoom(&picture, &picture_scroll, dimensions, next_multiplier);
        })
    };
    let zoom_in_action: Rc<dyn Fn()> = {
        let picture = picture.clone();
        let picture_scroll = picture_scroll.clone();
        let preview_dimensions = preview_dimensions.clone();
        let canvas_zoom = canvas_zoom.clone();
        let native_one_to_one = native_one_to_one.clone();
        let one_to_one_sync = one_to_one_sync.clone();
        Rc::new(move || {
            let was_one_to_one = native_one_to_one.replace(false);
            if was_one_to_one {
                if let Some(handler) = one_to_one_sync.borrow().as_ref() {
                    handler(false);
                }
            }
            let dimensions = preview_dimensions.get();
            let current_multiplier = if canvas_zoom.get() <= 0.0 {
                1.0
            } else {
                canvas_zoom.get()
            };
            let next_multiplier = (current_multiplier + 0.10).clamp(0.25, 8.0);
            canvas_zoom.set(next_multiplier);
            apply_canvas_zoom(&picture, &picture_scroll, dimensions, next_multiplier);
        })
    };
    {
        let zoom_out_action = zoom_out_action.clone();
        toolbar_zoom_out.connect_clicked(move |_| zoom_out_action());
    }
    {
        let zoom_in_action = zoom_in_action.clone();
        toolbar_zoom_in.connect_clicked(move |_| zoom_in_action());
    }

    // Track the pointer in viewport-local coordinates so Ctrl+wheel can keep
    // the image point under the cursor fixed while the picture is resized.
    // Toolbar +/- deliberately keeps its existing centre-based behaviour.
    let canvas_pointer = Rc::new(Cell::new((f64::NAN, f64::NAN)));
    let canvas_motion = gtk::EventControllerMotion::new();
    {
        let canvas_pointer = canvas_pointer.clone();
        canvas_motion.connect_motion(move |_, x, y| canvas_pointer.set((x, y)));
    }
    {
        let canvas_pointer = canvas_pointer.clone();
        canvas_motion.connect_leave(move |_| canvas_pointer.set((f64::NAN, f64::NAN)));
    }
    picture_scroll.add_controller(canvas_motion);

    // Ctrl + mouse wheel uses the same incremental zoom actions as the toolbar,
    // then restores the scroll position around the pointer after GTK has laid
    // out the resized GtkPicture. Ordinary wheel input remains native scrolling.
    let canvas_zoom_scroll = gtk::EventControllerScroll::new(
        gtk::EventControllerScrollFlags::VERTICAL | gtk::EventControllerScrollFlags::DISCRETE,
    );
    canvas_zoom_scroll.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let zoom_in_action = zoom_in_action.clone();
        let zoom_out_action = zoom_out_action.clone();
        let picture_scroll = picture_scroll.clone();
        let preview_dimensions = preview_dimensions.clone();
        let canvas_zoom = canvas_zoom.clone();
        let native_one_to_one = native_one_to_one.clone();
        let canvas_pointer = canvas_pointer.clone();
        canvas_zoom_scroll.connect_scroll(move |controller, _, dy| {
            if !controller
                .current_event_state()
                .contains(gtk::gdk::ModifierType::CONTROL_MASK)
                || dy == 0.0
            {
                return glib::Propagation::Proceed;
            }

            let (pointer_x, pointer_y) = canvas_pointer.get();
            let pointer_valid = pointer_x.is_finite() && pointer_y.is_finite();
            let dimensions = preview_dimensions.get();
            let old_native = native_one_to_one.get();
            let old_multiplier = if canvas_zoom.get() <= 0.0 {
                1.0
            } else {
                canvas_zoom.get()
            };
            let (old_width, old_height) = if old_native {
                (dimensions.0.max(1) as f64, dimensions.1.max(1) as f64)
            } else {
                canvas_size_for_zoom(&picture_scroll, dimensions, old_multiplier)
            };
            let old_h_value = picture_scroll.hadjustment().value();
            let old_v_value = picture_scroll.vadjustment().value();
            let viewport_width = picture_scroll.width().max(1) as f64;
            let viewport_height = picture_scroll.height().max(1) as f64;

            if dy < 0.0 {
                zoom_in_action();
            } else {
                zoom_out_action();
            }

            if pointer_valid {
                let new_multiplier = if canvas_zoom.get() <= 0.0 {
                    1.0
                } else {
                    canvas_zoom.get()
                };
                let (new_width, new_height) =
                    canvas_size_for_zoom(&picture_scroll, dimensions, new_multiplier);
                let target_h = anchored_scroll_target(
                    old_h_value,
                    pointer_x,
                    viewport_width,
                    old_width,
                    new_width,
                );
                let target_v = anchored_scroll_target(
                    old_v_value,
                    pointer_y,
                    viewport_height,
                    old_height,
                    new_height,
                );
                let picture_scroll = picture_scroll.clone();
                glib::idle_add_local_once(move || {
                    let hadj = picture_scroll.hadjustment();
                    let vadj = picture_scroll.vadjustment();
                    let h_max = (hadj.upper() - hadj.page_size()).max(hadj.lower());
                    let v_max = (vadj.upper() - vadj.page_size()).max(vadj.lower());
                    hadj.set_value(target_h.clamp(hadj.lower(), h_max));
                    vadj.set_value(target_v.clamp(vadj.lower(), v_max));
                    if std::env::var_os("PICASA_TRACE").is_some() {
                        eprintln!(
                            "EDIT ZOOM anchor=({:.1},{:.1}) scroll=({:.1},{:.1}) zoom={:.2}",
                            pointer_x,
                            pointer_y,
                            hadj.value(),
                            vadj.value(),
                            new_multiplier
                        );
                    }
                });
            }

            glib::Propagation::Stop
        });
    }
    picture_scroll.add_controller(canvas_zoom_scroll);

    let one_to_one_action: Rc<dyn Fn(bool)> = {
        let native_one_to_one = native_one_to_one.clone();
        let pending_one_to_one_anchor = pending_one_to_one_anchor.clone();
        let canvas_zoom = canvas_zoom.clone();
        let canvas_pointer = canvas_pointer.clone();
        let preview_dimensions = preview_dimensions.clone();
        let picture_scroll = picture_scroll.clone();
        let queue_preview = queue_preview.clone();
        let fit_action = fit_action.clone();
        Rc::new(move |enabled| {
            if enabled {
                let dimensions = preview_dimensions.get();
                let multiplier = if canvas_zoom.get() <= 0.0 {
                    1.0
                } else {
                    canvas_zoom.get()
                };
                let (old_width, old_height) = if native_one_to_one.get() {
                    (dimensions.0.max(1) as f64, dimensions.1.max(1) as f64)
                } else {
                    canvas_size_for_zoom(&picture_scroll, dimensions, multiplier)
                };
                let viewport_width = picture_scroll.width().max(1) as f64;
                let viewport_height = picture_scroll.height().max(1) as f64;
                let (pointer_x, pointer_y) = canvas_pointer.get();
                let (photo_x, photo_y, photo_width, photo_height) = if canvas_zoom.get() <= 0.0 {
                    fit_photo_rect(
                        viewport_width,
                        viewport_height,
                        dimensions.0.max(1) as f64,
                        dimensions.1.max(1) as f64,
                    )
                } else {
                    (
                        ((viewport_width - old_width) * 0.5).max(0.0),
                        ((viewport_height - old_height) * 0.5).max(0.0),
                        old_width,
                        old_height,
                    )
                };
                let normalized = normalized_image_point(
                    pointer_x,
                    pointer_y,
                    photo_x,
                    photo_y,
                    photo_width,
                    photo_height,
                    if canvas_zoom.get() <= 0.0 {
                        0.0
                    } else {
                        picture_scroll.hadjustment().value()
                    },
                    if canvas_zoom.get() <= 0.0 {
                        0.0
                    } else {
                        picture_scroll.vadjustment().value()
                    },
                );

                let anchor = if let Some((normalized_x, normalized_y)) = normalized {
                    if std::env::var_os("PICASA_TRACE").is_some() {
                        eprintln!(
                            "EDIT 1TO1 request normalized=({:.4},{:.4}) cursor=({:.1},{:.1}) rect=({:.1},{:.1},{:.1},{:.1})",
                            normalized_x, normalized_y, pointer_x, pointer_y,
                            photo_x, photo_y, photo_width, photo_height
                        );
                    }
                    OneToOneAnchor::Cursor {
                        normalized_x,
                        normalized_y,
                        pointer_x,
                        pointer_y,
                    }
                } else {
                    OneToOneAnchor::Center
                };
                pending_one_to_one_anchor.replace(Some(anchor));

                native_one_to_one.set(true);
                // Native 1:1 has its own sizing path. Do not encode it as a
                // normal Fit-relative zoom multiplier.
                canvas_zoom.set(0.0);
                queue_preview();
            } else {
                pending_one_to_one_anchor.borrow_mut().take();
                fit_action();
            }
        })
    };

    let set_library_rotation_action: Rc<dyn Fn(i32)> = {
        let active_rotation = active_rotation.clone();
        let native_one_to_one = native_one_to_one.clone();
        let pending_one_to_one_anchor = pending_one_to_one_anchor.clone();
        let canvas_zoom = canvas_zoom.clone();
        let one_to_one_sync = one_to_one_sync.clone();
        let picture = picture.clone();
        let picture_scroll = picture_scroll.clone();
        let preview_dimensions = preview_dimensions.clone();
        let queue_preview = queue_preview.clone();
        Rc::new(move |rotation| {
            active_rotation.set(rotation.rem_euclid(360));
            pending_one_to_one_anchor.borrow_mut().take();
            let was_one_to_one = native_one_to_one.replace(false);
            if was_one_to_one {
                if let Some(handler) = one_to_one_sync.borrow().as_ref() {
                    handler(false);
                }
            }
            canvas_zoom.set(0.0);
            apply_canvas_zoom(&picture, &picture_scroll, preview_dimensions.get(), 0.0);
            queue_preview();
        })
    };

    {
        let crop_overlay = crop_overlay.clone();
        let crop_actions = crop_actions.clone();
        let pending_crop = pending_crop.clone();
        let canvas_zoom = canvas_zoom.clone();
        let native_one_to_one = native_one_to_one.clone();
        let one_to_one_sync = one_to_one_sync.clone();
        let picture = picture.clone();
        let picture_scroll = picture_scroll.clone();
        let preview_dimensions = preview_dimensions.clone();
        crop.connect_clicked(move |_| {
            canvas_zoom.set(0.0);
            let was_one_to_one = native_one_to_one.replace(false);
            if was_one_to_one {
                if let Some(handler) = one_to_one_sync.borrow().as_ref() {
                    handler(false);
                }
            }
            apply_canvas_zoom(&picture, &picture_scroll, preview_dimensions.get(), 0.0);
            pending_crop.replace(CropRect::default());
            crop_overlay.set_visible(true);
            crop_actions.set_visible(true);
            crop_overlay.queue_draw();
        });
    }
    {
        let crop_overlay = crop_overlay.clone();
        let crop_actions = crop_actions.clone();
        let pending_crop = pending_crop.clone();
        crop_cancel.connect_clicked(move |_| {
            pending_crop.replace(CropRect::default());
            crop_overlay.set_visible(false);
            crop_actions.set_visible(false);
        });
    }
    let apply_crop: Rc<dyn Fn()> = {
        let session = session.clone();
        let crop_overlay = crop_overlay.clone();
        let crop_actions = crop_actions.clone();
        let pending_crop = pending_crop.clone();
        let queue_preview = queue_preview.clone();
        let update_history_buttons = update_history_buttons.clone();
        Rc::new(move || {
            if !crop_overlay.is_visible() {
                return;
            }
            let child = *pending_crop.borrow();
            session
                .borrow_mut()
                .mutate(|recipe| recipe.crop = recipe.crop.compose(child));
            pending_crop.replace(CropRect::default());
            crop_overlay.set_visible(false);
            crop_actions.set_visible(false);
            update_history_buttons();
            queue_preview();
        })
    };
    {
        let apply_crop = apply_crop.clone();
        crop_apply.connect_clicked(move |_| apply_crop());
    }
    {
        let apply_crop = apply_crop.clone();
        let crop_overlay = crop_overlay.clone();
        let key = gtk::EventControllerKey::new();
        key.set_propagation_phase(gtk::PropagationPhase::Capture);
        key.connect_key_pressed(move |_, key, _, _| {
            if crop_overlay.is_visible()
                && matches!(key, gtk::gdk::Key::Return | gtk::gdk::Key::KP_Enter)
            {
                apply_crop();
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        root.add_controller(key);
    }
    {
        let session = session.clone();
        let crop_overlay = crop_overlay.clone();
        let crop_actions = crop_actions.clone();
        let pending_crop = pending_crop.clone();
        let queue_preview = queue_preview.clone();
        let update_history_buttons = update_history_buttons.clone();
        crop_reset.connect_clicked(move |_| {
            session
                .borrow_mut()
                .mutate(|recipe| recipe.crop = CropRect::default());
            pending_crop.replace(CropRect::default());
            crop_overlay.set_visible(false);
            crop_actions.set_visible(false);
            update_history_buttons();
            queue_preview();
        });
    }

    configure_crop_overlay(
        &crop_overlay,
        pending_crop.clone(),
        preview_dimensions.clone(),
    );

    {
        let on_close = on_close.clone();
        back.connect_clicked(move |_| on_close());
    }
    {
        let session = session.clone();
        let photo = photo.clone();
        let parent = parent.clone();
        let active_rotation = active_rotation.clone();
        export.connect_clicked(move |_| {
            let dialog = gtk::FileChooserNative::new(
                Some("Export Edited Photo"),
                Some(&parent),
                gtk::FileChooserAction::Save,
                Some("Export"),
                Some("Cancel"),
            );
            let filename = std::path::Path::new(&photo.filename())
                .file_stem()
                .and_then(|value| value.to_str())
                .map(|stem| format!("{stem}-edited.jpg"))
                .unwrap_or_else(|| "export-edited.jpg".to_string());
            dialog.set_current_name(&filename);

            let reference = photo.path();
            let rotation = active_rotation.get();
            let source_width = photo.width();
            let source_height = photo.height();
            let session = session.clone();
            dialog.connect_response(move |dialog, response| {
                if response == gtk::ResponseType::Accept {
                    if let Some(destination) = dialog.file().and_then(|file| file.path()) {
                        let edit_recipe = session.borrow().recipe.encode();
                        let reference = reference.clone();
                        std::thread::spawn(move || {
                            let result = super::render::render_for_export(
                                &reference,
                                rotation,
                                &edit_recipe,
                                source_width,
                                source_height,
                            )
                            .and_then(|image| super::render::save_jpeg(&image, &destination, 92));
                            if let Err(error) = result {
                                eprintln!("Could not export edited photo: {error:#}");
                            }
                        });
                    }
                }
                dialog.destroy();
            });
            dialog.show();
        });
    }

    {
        let connection = connection.clone();
        let session = session.clone();
        let photo = photo.clone();
        let on_saved = on_saved.clone();
        let on_close = on_close.clone();
        let parent = parent.clone();
        done.connect_clicked(move |_| {
            let encoded = session.borrow().recipe.encode();
            if let Err(error) =
                crate::db::set_edit_recipe(&connection.borrow(), photo.id(), &encoded)
            {
                let dialog = libadwaita::AlertDialog::builder()
                    .heading("Could not save edits")
                    .body(error.to_string())
                    .close_response("close")
                    .build();
                dialog.add_response("close", "Close");
                dialog.present(Some(&parent));
                return;
            }
            photo.set_edit_recipe(encoded);
            on_saved(photo.clone());
            on_close();
        });
    }

    queue_preview();

    // Phase 5: warm the native-resolution decoded base shortly after the fast
    // Fit preview has been requested. The decode happens on its own thread so
    // slider rendering remains responsive. The shared two-entry cache keeps
    // both Fit and native bases, so warming 1:1 never makes normal editing pay
    // another decode.
    {
        let cache = preview_base_cache.clone();
        let path = photo.path();
        let rotation = photo.rotation();
        let mut target_width = if photo.width() > 0 {
            photo.width() as u32
        } else {
            u32::MAX
        };
        let mut target_height = if photo.height() > 0 {
            photo.height() as u32
        } else {
            u32::MAX
        };
        if matches!(rotation.rem_euclid(360), 90 | 270) {
            std::mem::swap(&mut target_width, &mut target_height);
        }

        glib::timeout_add_local_once(Duration::from_millis(300), move || {
            if cached_preview_base(&cache, &path, rotation, target_width, target_height).is_some() {
                if std::env::var_os("PICASA_TRACE").is_some() {
                    eprintln!(
                        "EDIT PREFETCH cache=hit target={}x{} path={}",
                        target_width, target_height, path
                    );
                }
                return;
            }

            std::thread::spawn(move || {
                let started = Instant::now();
                match super::render::decode_base_for_viewer(
                    &path,
                    rotation,
                    target_width,
                    target_height,
                ) {
                    Ok(image) => {
                        let decode_ms = started.elapsed().as_millis();
                        store_preview_base(
                            &cache,
                            PreviewBase {
                                path: path.clone(),
                                rotation,
                                target_width,
                                target_height,
                                image,
                            },
                        );
                        if std::env::var_os("PICASA_TRACE").is_some() {
                            eprintln!(
                                "EDIT PREFETCH cache=ready decode_ms={} target={}x{} path={}",
                                decode_ms, target_width, target_height, path
                            );
                        }
                    }
                    Err(error) => {
                        if std::env::var_os("PICASA_TRACE").is_some() {
                            eprintln!(
                                "EDIT PREFETCH cache=failed target={}x{} path={} error={:#}",
                                target_width, target_height, path, error
                            );
                        }
                    }
                }
            });
        });
    }

    EditEditor {
        root,
        photo_id: photo.id(),
        zoom_in_action,
        zoom_out_action,
        fit_action,
        one_to_one_action,
        set_library_rotation_action,
        one_to_one_sync,
    }
}

fn fit_zoom_for_canvas(scroll: &gtk::ScrolledWindow, dimensions: (i32, i32)) -> f64 {
    let source_w = dimensions.0.max(1) as f64;
    let source_h = dimensions.1.max(1) as f64;
    // Leave a small breathing margin so the first + click visibly enlarges
    // the photo instead of jumping straight from Fit to native 100%.
    let viewport_w = (scroll.width() - 24).max(1) as f64;
    let viewport_h = (scroll.height() - 24).max(1) as f64;
    (viewport_w / source_w)
        .min(viewport_h / source_h)
        .min(1.0)
        .max(0.01)
}

fn canvas_size_for_zoom(
    scroll: &gtk::ScrolledWindow,
    dimensions: (i32, i32),
    zoom: f64,
) -> (f64, f64) {
    let fit = fit_zoom_for_canvas(scroll, dimensions);
    let effective_zoom = (fit * zoom).clamp(0.01, 8.0);
    (
        (dimensions.0.max(1) as f64 * effective_zoom).max(1.0),
        (dimensions.1.max(1) as f64 * effective_zoom).max(1.0),
    )
}

fn anchored_scroll_target(
    current_scroll: f64,
    pointer: f64,
    viewport: f64,
    old_content: f64,
    new_content: f64,
) -> f64 {
    let old_content = old_content.max(1.0);
    let new_content = new_content.max(1.0);
    let old_padding = ((viewport - old_content) * 0.5).max(0.0);
    let new_padding = ((viewport - new_content) * 0.5).max(0.0);
    let image_position = (current_scroll + pointer - old_padding).clamp(0.0, old_content);
    let normalized = image_position / old_content;
    normalized * new_content + new_padding - pointer
}

fn pointer_over_content(pointer: f64, viewport: f64, content: f64) -> bool {
    if !pointer.is_finite() || !viewport.is_finite() || !content.is_finite() {
        return false;
    }
    let padding = ((viewport - content) * 0.5).max(0.0);
    let visible_end = if content >= viewport {
        viewport
    } else {
        padding + content
    };
    pointer >= padding && pointer <= visible_end
}

fn fit_photo_rect(
    container_w: f64,
    container_h: f64,
    image_w: f64,
    image_h: f64,
) -> (f64, f64, f64, f64) {
    contained_rect(container_w, container_h, image_w, image_h)
}

fn normalized_image_point(
    pointer_x: f64,
    pointer_y: f64,
    photo_x: f64,
    photo_y: f64,
    photo_width: f64,
    photo_height: f64,
    h_scroll: f64,
    v_scroll: f64,
) -> Option<(f64, f64)> {
    if !pointer_x.is_finite()
        || !pointer_y.is_finite()
        || photo_width <= 0.0
        || photo_height <= 0.0
        || pointer_x < photo_x
        || pointer_y < photo_y
        || pointer_x > photo_x + photo_width
        || pointer_y > photo_y + photo_height
    {
        return None;
    }
    Some((
        ((h_scroll + pointer_x - photo_x) / photo_width).clamp(0.0, 1.0),
        ((v_scroll + pointer_y - photo_y) / photo_height).clamp(0.0, 1.0),
    ))
}

fn normalized_scroll_target(
    normalized: f64,
    pointer: f64,
    viewport: f64,
    native_content: f64,
) -> f64 {
    let padding = ((viewport - native_content) * 0.5).max(0.0);
    normalized.clamp(0.0, 1.0) * native_content + padding - pointer
}

fn one_to_one_scroll_target(
    current_scroll: f64,
    pointer: f64,
    viewport: f64,
    old_content: f64,
    native_content: f64,
    anchor_cursor: bool,
) -> f64 {
    if anchor_cursor {
        anchored_scroll_target(
            current_scroll,
            pointer,
            viewport,
            old_content,
            native_content,
        )
    } else {
        ((native_content - viewport) * 0.5).max(0.0)
    }
}

fn apply_canvas_zoom(
    picture: &gtk::Picture,
    scroll: &gtk::ScrolledWindow,
    dimensions: (i32, i32),
    zoom: f64,
) {
    if zoom <= 0.0 {
        picture.set_can_shrink(true);
        picture.set_hexpand(true);
        picture.set_vexpand(true);
        picture.set_halign(gtk::Align::Fill);
        picture.set_valign(gtk::Align::Fill);
        picture.set_size_request(1, 1);
        scroll.set_cursor_from_name(None);
        scroll.hadjustment().set_value(0.0);
        scroll.vadjustment().set_value(0.0);
        return;
    }
    // Positive zoom values are multipliers of the current Fit scale.
    let (width, height) = canvas_size_for_zoom(scroll, dimensions, zoom);
    let width = width.round() as i32;
    let height = height.round() as i32;
    // Keep shrinking enabled: disabling it makes GtkPicture insist on the
    // paintable's intrinsic size and defeats incremental size requests.
    picture.set_can_shrink(true);
    picture.set_hexpand(false);
    picture.set_vexpand(false);
    picture.set_halign(gtk::Align::Center);
    picture.set_valign(gtk::Align::Center);
    picture.set_size_request(width, height);
    scroll.set_cursor_from_name(if zoom > 1.0 { Some("grab") } else { None });
    picture.queue_resize();
}

fn apply_canvas_one_to_one(
    picture: &gtk::Picture,
    scroll: &gtk::ScrolledWindow,
    dimensions: (i32, i32),
) {
    picture.set_can_shrink(true);
    picture.set_hexpand(false);
    picture.set_vexpand(false);
    picture.set_halign(gtk::Align::Center);
    picture.set_valign(gtk::Align::Center);
    picture.set_size_request(dimensions.0.max(1), dimensions.1.max(1));
    scroll.set_cursor_from_name(Some("grab"));
    picture.queue_resize();
}

#[cfg(test)]
mod panel_tests {
    use super::*;

    #[test]
    #[ignore = "requires a GTK display; run with --ignored --test-threads=1"]
    fn filters_switch_keeps_sidebar_visible() {
        gtk::init().unwrap();
        let parent = gtk::Window::new();
        let photo: crate::photo_object::PhotoObject = glib::Object::new();
        let editor = build(
            &parent,
            Rc::new(RefCell::new(Connection::open_in_memory().unwrap())),
            photo,
            Rc::new(|| {}),
            Rc::new(|_| {}),
        );
        let toolbar = editor.root.first_child().unwrap();
        let body = toolbar
            .next_sibling()
            .unwrap()
            .downcast::<gtk::Paned>()
            .unwrap();
        let sidebar = body.start_child().unwrap();
        let scroll = sidebar.clone().downcast::<gtk::ScrolledWindow>().unwrap();
        let viewport = scroll.child().unwrap().downcast::<gtk::Viewport>().unwrap();
        let stack = viewport.child().unwrap().downcast::<gtk::Stack>().unwrap();
        let mut buttons = Vec::new();
        let mut child = toolbar.first_child();
        while let Some(widget) = child {
            child = widget.next_sibling();
            if let Ok(button) = widget.downcast::<gtk::ToggleButton>() {
                buttons.push(button);
            }
        }
        let tools = buttons
            .iter()
            .find(|b| b.label().as_deref() == Some("Tools"))
            .unwrap();
        let filters = buttons
            .iter()
            .find(|b| b.label().as_deref() == Some("Filters"))
            .unwrap();
        for _ in 0..3 {
            filters.emit_clicked();
            assert!(
                sidebar.is_visible(),
                "Filters must not hide the shared sidebar"
            );
            assert_eq!(stack.visible_child_name().as_deref(), Some("filters"));
            assert!(!tools.is_active());
            tools.emit_clicked();
            assert!(sidebar.is_visible());
            assert_eq!(stack.visible_child_name().as_deref(), Some("tools"));
            assert!(!filters.is_active());
        }
    }
}

fn add_section_label(parent: &gtk::Box, text: &str) {
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.set_margin_top(6);
    label.add_css_class("heading");
    parent.append(&label);
}

fn add_slider(
    parent: &gtk::Box,
    text: &str,
    min: f64,
    max: f64,
    step: f64,
    digits: i32,
) -> gtk::Scale {
    let row = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, min, max, step);
    scale.set_digits(digits);
    scale.set_draw_value(true);
    scale.set_hexpand(true);
    row.append(&label);
    row.append(&scale);
    parent.append(&row);
    scale
}

fn connect_scale(
    scale: &gtk::Scale,
    session: Rc<RefCell<EditSession>>,
    syncing: Rc<Cell<bool>>,
    queue_preview: Rc<dyn Fn()>,
    update_history: Rc<dyn Fn()>,
    assign: impl Fn(&mut EditRecipe, f32) + 'static,
) {
    let dragging = Rc::new(Cell::new(false));
    let press = gtk::GestureClick::new();
    press.set_button(1);
    press.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let session = session.clone();
        let dragging = dragging.clone();
        press.connect_pressed(move |_, _, _, _| {
            dragging.set(true);
            session.borrow_mut().begin_action();
        });
    }
    {
        let session = session.clone();
        let dragging = dragging.clone();
        let update_history = update_history.clone();
        press.connect_released(move |_, _, _, _| {
            if dragging.replace(false) {
                session.borrow_mut().end_action();
                update_history();
            }
        });
    }
    scale.add_controller(press);

    scale.connect_value_changed(move |scale| {
        if syncing.get() {
            return;
        }
        let value = scale.value() as f32;
        if dragging.get() {
            session
                .borrow_mut()
                .mutate_active(|recipe| assign(recipe, value));
        } else {
            // Keyboard steps and deliberate wheel steps remain normal single
            // undoable actions; only a continuous mouse drag is coalesced.
            session.borrow_mut().mutate(|recipe| assign(recipe, value));
            update_history();
        }
        queue_preview();
    });
}

fn configure_scale_scroll(scale: &gtk::Scale, tools_scroll: &gtk::ScrolledWindow) {
    let pointer_y = Rc::new(Cell::new(f64::NAN));
    let motion = gtk::EventControllerMotion::new();
    {
        let pointer_y = pointer_y.clone();
        motion.connect_motion(move |_, _, y| pointer_y.set(y));
    }
    {
        let pointer_y = pointer_y.clone();
        motion.connect_leave(move |_| pointer_y.set(f64::NAN));
    }
    scale.add_controller(motion);

    let controller = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
    controller.set_propagation_phase(gtk::PropagationPhase::Capture);
    let scale_for_scroll = scale.clone();
    let tools_scroll = tools_scroll.clone();
    controller.connect_scroll(move |_, _, dy| {
        // GTK Scale reacts to wheel input across its full allocation, including
        // value text and padding. Reserve only a narrow central track band for
        // adjustment; everywhere else scrolls the editing tools normally.
        let y = pointer_y.get();
        let center = scale_for_scroll.height() as f64 / 2.0;
        if y.is_finite() && (y - center).abs() <= 7.0 {
            return glib::Propagation::Proceed;
        }

        if dy != 0.0 {
            let adjustment = tools_scroll.vadjustment();
            let max = (adjustment.upper() - adjustment.page_size()).max(adjustment.lower());
            let step = adjustment.step_increment().max(24.0);
            adjustment.set_value(
                (adjustment.value() + dy.signum() * step * 2.0).clamp(adjustment.lower(), max),
            );
        }
        glib::Propagation::Stop
    });
    scale.add_controller(controller);
}

fn connect_toggle(
    button: &gtk::ToggleButton,
    session: Rc<RefCell<EditSession>>,
    syncing: Rc<Cell<bool>>,
    queue_preview: Rc<dyn Fn()>,
    update_history: Rc<dyn Fn()>,
    assign: impl Fn(&mut EditRecipe, bool) + 'static,
) {
    button.connect_toggled(move |button| {
        if syncing.get() {
            return;
        }
        let value = button.is_active();
        session.borrow_mut().mutate(|recipe| assign(recipe, value));
        update_history();
        queue_preview();
    });
}

fn configure_crop_overlay(
    overlay: &gtk::DrawingArea,
    pending: Rc<RefCell<CropRect>>,
    preview_dimensions: Rc<Cell<(i32, i32)>>,
) {
    let pending_for_draw = pending.clone();
    let preview_for_draw = preview_dimensions.clone();
    overlay.set_draw_func(move |_, context, width, height| {
        let (image_width, image_height) = preview_for_draw.get();
        let (x, y, w, h) = contained_rect(
            width as f64,
            height as f64,
            image_width as f64,
            image_height as f64,
        );
        let crop = pending_for_draw.borrow().normalized();
        let sx = x + crop.left as f64 * w;
        let sy = y + crop.top as f64 * h;
        let sw = (crop.right - crop.left) as f64 * w;
        let sh = (crop.bottom - crop.top) as f64 * h;

        context.set_source_rgba(0.0, 0.0, 0.0, 0.52);
        context.rectangle(x, y, w, (sy - y).max(0.0));
        context.rectangle(x, sy + sh, w, (y + h - sy - sh).max(0.0));
        context.rectangle(x, sy, (sx - x).max(0.0), sh);
        context.rectangle(sx + sw, sy, (x + w - sx - sw).max(0.0), sh);
        let _ = context.fill();
        context.set_source_rgba(1.0, 1.0, 1.0, 0.95);
        context.set_line_width(2.0);
        context.rectangle(sx, sy, sw, sh);
        let _ = context.stroke();
        context.set_line_width(1.0);
        context.set_source_rgba(1.0, 1.0, 1.0, 0.55);
        for third in [1.0 / 3.0, 2.0 / 3.0] {
            context.move_to(sx + sw * third, sy);
            context.line_to(sx + sw * third, sy + sh);
            context.move_to(sx, sy + sh * third);
            context.line_to(sx + sw, sy + sh * third);
        }
        let _ = context.stroke();
    });

    let drag = gtk::GestureDrag::new();
    drag.set_button(1);
    let origin = Rc::new(Cell::new((0.0f64, 0.0f64)));
    {
        let origin = origin.clone();
        drag.connect_drag_begin(move |_, x, y| origin.set((x, y)));
    }
    {
        let overlay = overlay.clone();
        let pending = pending.clone();
        let preview_dimensions = preview_dimensions.clone();
        let origin = origin.clone();
        drag.connect_drag_update(move |_, dx, dy| {
            let (ox, oy) = origin.get();
            let start = point_to_normalized(&overlay, preview_dimensions.get(), ox, oy);
            let current = point_to_normalized(&overlay, preview_dimensions.get(), ox + dx, oy + dy);
            let (Some((sx, sy)), Some((cx, cy))) = (start, current) else {
                return;
            };
            pending.replace(
                CropRect {
                    left: sx.min(cx) as f32,
                    top: sy.min(cy) as f32,
                    right: sx.max(cx) as f32,
                    bottom: sy.max(cy) as f32,
                }
                .normalized(),
            );
            overlay.queue_draw();
        });
    }
    overlay.add_controller(drag);
}

fn point_to_normalized(
    overlay: &gtk::DrawingArea,
    preview: (i32, i32),
    px: f64,
    py: f64,
) -> Option<(f64, f64)> {
    let (x, y, width, height) = contained_rect(
        overlay.width() as f64,
        overlay.height() as f64,
        preview.0 as f64,
        preview.1 as f64,
    );
    if width <= 0.0 || height <= 0.0 {
        return None;
    }
    Some((
        ((px - x) / width).clamp(0.0, 1.0),
        ((py - y) / height).clamp(0.0, 1.0),
    ))
}

fn contained_rect(
    container_w: f64,
    container_h: f64,
    image_w: f64,
    image_h: f64,
) -> (f64, f64, f64, f64) {
    if container_w <= 0.0 || container_h <= 0.0 || image_w <= 0.0 || image_h <= 0.0 {
        return (0.0, 0.0, container_w.max(0.0), container_h.max(0.0));
    }
    let scale = (container_w / image_w).min(container_h / image_h);
    let width = image_w * scale;
    let height = image_h * scale;
    (
        (container_w - width) * 0.5,
        (container_h - height) * 0.5,
        width,
        height,
    )
}

#[cfg(test)]
mod zoom_anchor_tests {
    use super::{
        anchored_scroll_target, fit_photo_rect, normalized_image_point, normalized_scroll_target,
        one_to_one_scroll_target, pointer_over_content,
    };

    #[test]
    fn fit_anchor_uses_letterboxed_photo_rectangle() {
        let rect = fit_photo_rect(400.0, 400.0, 1000.0, 800.0);
        assert_eq!(rect, (0.0, 40.0, 400.0, 320.0));
        let normalized =
            normalized_image_point(200.0, 160.0, rect.0, rect.1, rect.2, rect.3, 0.0, 0.0)
                .expect("cursor is over photo");
        let target = (
            normalized_scroll_target(normalized.0, 200.0, 400.0, 1000.0),
            normalized_scroll_target(normalized.1, 160.0, 400.0, 800.0),
        );
        assert!((target.0 - 300.0).abs() < 0.001);
        assert!((target.1 - 140.0).abs() < 0.001);
    }

    #[test]
    fn fit_anchor_returns_none_when_pointer_is_in_margin() {
        assert!(normalized_image_point(10.0, 10.0, 0.0, 40.0, 400.0, 320.0, 0.0, 0.0).is_none());
    }

    #[test]
    fn zoom_anchor_keeps_centered_image_point_under_pointer() {
        let target = anchored_scroll_target(0.0, 500.0, 1000.0, 800.0, 1200.0);
        assert!((target - 100.0).abs() < 0.001);
    }

    #[test]
    fn zoom_anchor_preserves_off_center_scrolled_point() {
        let target = anchored_scroll_target(250.0, 125.0, 500.0, 1000.0, 2000.0);
        assert!((target - 625.0).abs() < 0.001);
    }

    #[test]
    fn zoom_anchor_handles_zooming_out_to_content_smaller_than_viewport() {
        let target = anchored_scroll_target(250.0, 250.0, 500.0, 1000.0, 400.0);
        assert!((target - 0.0).abs() < 0.001);
    }

    #[test]
    fn one_to_one_anchor_preserves_cursor_image_point() {
        let target = one_to_one_scroll_target(0.0, 400.0, 1000.0, 800.0, 4000.0, true);
        assert!((target - 1100.0).abs() < 0.001);
    }

    #[test]
    fn one_to_one_without_image_pointer_centers_native_image() {
        let target = one_to_one_scroll_target(0.0, 0.0, 1000.0, 800.0, 4000.0, false);
        assert!((target - 1500.0).abs() < 0.001);
    }

    #[test]
    fn pointer_must_be_over_centered_image_to_anchor_one_to_one() {
        assert!(!pointer_over_content(50.0, 1000.0, 800.0));
        assert!(pointer_over_content(100.0, 1000.0, 800.0));
        assert!(pointer_over_content(900.0, 1000.0, 800.0));
        assert!(!pointer_over_content(950.0, 1000.0, 800.0));
    }
}
