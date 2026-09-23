use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use adw::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;
use rusqlite::Connection;

use super::filters::FilterPreset;
use super::model::{
    CropRect, EditRecipe, EditSession, OverlayAnchor, OverlaySpec, TextAlign, TextLayerSpec,
};

pub struct EditEditor {
    pub root: gtk::Box,
    photo_id: i64,
    back_button: gtk::Button,
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

    /// Relabels the toolbar Back button for the context the editor was
    /// opened from (e.g. "Back to Collage").
    pub fn set_back_label(&self, label: &str, tooltip: &str) {
        self.back_button.set_label(label);
        self.back_button.set_tooltip_text(Some(tooltip));
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

// Editor implementation fragments remain in this module so the split preserves
// the existing Rc/RefCell callback graph and private helper visibility.
include!("preview.rs");
include!("zoom.rs");
include!("crop.rs");
include!("controls.rs");
include!("undo.rs");
include!("export.rs");
include!("overlays_ui.rs");
include!("text_ui.rs");

pub fn build(
    parent: &gtk::Window,
    connection: Rc<RefCell<Connection>>,
    photo: crate::photo_object::PhotoObject,
    on_close: Rc<dyn Fn()>,
    on_saved: Rc<dyn Fn(crate::photo_object::PhotoObject)>,
    start_export: Rc<dyn Fn(Vec<super::export_batch::ExportJob>)>,
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

    let export = gtk::Button::with_label("Export");
    export.set_tooltip_text(Some("Export the current edited photo (size and file type)"));
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
    body.set_position(360);
    body.set_resize_start_child(false);
    body.set_shrink_start_child(false);
    body.set_wide_handle(true);
    root.append(&body);

    let sidebar = gtk::Box::new(gtk::Orientation::Vertical, 0);
    sidebar.set_width_request(320);

    let panel_tabs = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    panel_tabs.set_margin_top(12);
    panel_tabs.set_margin_start(14);
    panel_tabs.set_margin_end(14);
    panel_tabs.set_margin_bottom(4);
    panel_tabs.add_css_class("linked");
    panel_tabs.add_css_class("edit-panel-tabs");

    let tools_toggle = gtk::ToggleButton::with_label("Tools");
    tools_toggle.set_active(true);
    tools_toggle.set_hexpand(true);
    tools_toggle.set_tooltip_text(Some("Show editing controls"));
    let filters_toggle = gtk::ToggleButton::with_label("Filters");
    filters_toggle.set_group(Some(&tools_toggle));
    filters_toggle.set_hexpand(true);
    filters_toggle.set_tooltip_text(Some("Show one-tap photo filters"));
    let crop_toggle = gtk::ToggleButton::with_label("Crop");
    crop_toggle.set_group(Some(&tools_toggle));
    crop_toggle.set_hexpand(true);
    crop_toggle.set_tooltip_text(Some("Crop, straighten and compose the photo"));
    let overlays_toggle = gtk::ToggleButton::with_label("Overlays");
    overlays_toggle.set_group(Some(&tools_toggle));
    overlays_toggle.set_hexpand(true);
    overlays_toggle.set_tooltip_text(Some("Place PNG/JPG logos, badges and banners on the photo"));
    let text_toggle = gtk::ToggleButton::with_label("Text");
    text_toggle.set_group(Some(&tools_toggle));
    text_toggle.set_hexpand(true);
    text_toggle.set_tooltip_text(Some("Place editable text layers on the photo"));
    panel_tabs.append(&tools_toggle);
    panel_tabs.append(&filters_toggle);
    panel_tabs.append(&crop_toggle);
    panel_tabs.append(&overlays_toggle);
    panel_tabs.append(&text_toggle);
    sidebar.append(&panel_tabs);

    let tools_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    tools_box.set_margin_top(12);
    tools_box.set_margin_bottom(16);
    tools_box.set_margin_start(14);
    tools_box.set_margin_end(14);
    let filters_box = gtk::Box::new(gtk::Orientation::Vertical, 10);
    filters_box.set_hexpand(true);
    filters_box.set_vexpand(true);
    filters_box.set_margin_top(12);
    filters_box.set_margin_bottom(16);
    filters_box.set_margin_start(14);
    filters_box.set_margin_end(14);
    let crop_box = gtk::Box::new(gtk::Orientation::Vertical, 10);
    crop_box.set_hexpand(true);
    crop_box.set_vexpand(true);
    crop_box.set_margin_top(12);
    crop_box.set_margin_bottom(16);
    crop_box.set_margin_start(14);
    crop_box.set_margin_end(14);
    let overlays_box = gtk::Box::new(gtk::Orientation::Vertical, 10);
    overlays_box.set_hexpand(true);
    overlays_box.set_vexpand(true);
    overlays_box.set_margin_top(12);
    overlays_box.set_margin_bottom(16);
    overlays_box.set_margin_start(14);
    overlays_box.set_margin_end(14);
    let text_box = gtk::Box::new(gtk::Orientation::Vertical, 10);
    text_box.set_hexpand(true);
    text_box.set_vexpand(true);
    text_box.set_margin_top(12);
    text_box.set_margin_bottom(16);
    text_box.set_margin_start(14);
    text_box.set_margin_end(14);
    let tools_scroll = gtk::ScrolledWindow::new();
    tools_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    tools_scroll.set_child(Some(&tools_box));
    let filters_scroll = gtk::ScrolledWindow::new();
    filters_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    filters_scroll.set_child(Some(&filters_box));
    let crop_scroll = gtk::ScrolledWindow::new();
    crop_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    crop_scroll.set_child(Some(&crop_box));
    let overlays_scroll = gtk::ScrolledWindow::new();
    overlays_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    overlays_scroll.set_child(Some(&overlays_box));
    let text_scroll = gtk::ScrolledWindow::new();
    text_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    text_scroll.set_child(Some(&text_box));
    let panel_stack = gtk::Stack::new();
    panel_stack.set_hexpand(true);
    panel_stack.set_vexpand(true);
    panel_stack.add_css_class("edit-panel-pages");
    panel_stack.add_named(&tools_scroll, Some("tools"));
    panel_stack.add_named(&filters_scroll, Some("filters"));
    panel_stack.add_named(&crop_scroll, Some("crop"));
    panel_stack.add_named(&overlays_scroll, Some("overlays"));
    panel_stack.add_named(&text_scroll, Some("text"));
    panel_stack.set_visible_child(&tools_scroll);
    sidebar.append(&panel_stack);
    body.set_start_child(Some(&sidebar));

    let preview_area = gtk::Overlay::new();
    preview_area.set_hexpand(true);
    preview_area.set_vexpand(true);
    preview_area.set_margin_top(12);
    preview_area.set_margin_bottom(12);
    preview_area.set_margin_start(12);
    preview_area.set_margin_end(12);
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
    picture_scroll.add_css_class("edit-canvas-stage");
    picture_scroll.set_child(Some(&picture));
    preview_area.set_child(Some(&picture_scroll));

    let crop_overlay = gtk::DrawingArea::new();
    crop_overlay.set_hexpand(true);
    crop_overlay.set_vexpand(true);
    crop_overlay.set_visible(false);
    crop_overlay.set_cursor_from_name(Some("crosshair"));
    preview_area.add_overlay(&crop_overlay);

    // Overlay layer sits above the photo (and above the crop overlay in
    // stacking order) but yields pointer events unless the Overlays tab is
    // active and the recipe actually has overlays. Drawing still happens so
    // overlays remain visible on Tools/Filters tabs.
    let overlay_layer = gtk::DrawingArea::new();
    overlay_layer.set_hexpand(true);
    overlay_layer.set_vexpand(true);
    overlay_layer.set_can_target(false);
    preview_area.add_overlay(&overlay_layer);

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
    // Index into recipe.overlays of the overlay selected for canvas/panel
    // editing; None when nothing is selected.
    let selected_overlay: Rc<Cell<Option<usize>>> = Rc::new(Cell::new(None));
    // Index into recipe.text_layers of the text layer selected while the Text
    // tab owns the canvas; per-tab selection so switching tabs restores each
    // tab's own selected layer.
    let selected_text: Rc<Cell<Option<usize>>> = Rc::new(Cell::new(None));
    // Filled once the Overlays panel is built so sync_controls (undo/redo,
    // reset, crop commits) can refresh the list without a dependency cycle.
    let sync_overlays_slot: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));
    let sync_text_slot: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));
    let pending_crop = Rc::new(RefCell::new(CropRect::default()));
    let crop_aspect_preset = Rc::new(Cell::new(CropAspectPreset::Free));
    let crop_portrait = Rc::new(Cell::new(false));
    let crop_aspect_ratio = Rc::new(Cell::new(None::<f64>));
    let preview_dimensions = Rc::new(Cell::new((1i32, 1i32)));
    // 0.0 means fit-to-canvas. Positive values are display zoom factors.
    let canvas_zoom = Rc::new(Cell::new(0.0f64));
    // Preview renders are throttled to this cadence while sliders move;
    // see the queue_preview closure below.
    const PREVIEW_THROTTLE_MS: u64 = 40;
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

    add_section_label(&filters_box, "QUICK FIXES");
    let auto_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    auto_row.add_css_class("quick-fix-row");
    let auto_contrast = gtk::ToggleButton::with_label("Auto Contrast");
    let auto_color = gtk::ToggleButton::with_label("Auto Colour");
    auto_contrast.set_hexpand(true);
    auto_color.set_hexpand(true);
    auto_row.append(&auto_contrast);
    auto_row.append(&auto_color);
    filters_box.append(&auto_row);

    add_section_label(&filters_box, "FILTERS");
    let filter_grid = gtk::FlowBox::new();
    filter_grid.set_row_spacing(10);
    filter_grid.set_column_spacing(6);
    // Homogeneous tiles reflow cleanly as the inspector is resized: two
    // columns at the default pane width, growing to three, four and five on
    // wider panes and collapsing back to one when the pane is dragged narrow.
    // The count is driven purely by the fixed tile minimum width, so no child
    // ever requests a width derived from the current pane size and widening
    // the inspector can never trap the divider.
    filter_grid.set_homogeneous(true);
    filter_grid.set_min_children_per_line(1);
    filter_grid.set_max_children_per_line(5);
    filter_grid.set_selection_mode(gtk::SelectionMode::None);
    filter_grid.add_css_class("filter-grid");
    let mut filter_buttons = Vec::new();
    let mut filter_pictures = Vec::new();
    let mut black_white = None;
    let mut sepia = None;
    for effect in FilterTileEffect::all() {
        let (button, picture) = filter_tile(effect);
        filter_grid.insert(&button, -1);
        filter_pictures.push(picture);
        match effect {
            FilterTileEffect::Preset(preset) => filter_buttons.push((preset, button)),
            FilterTileEffect::BlackWhite => black_white = Some(button),
            FilterTileEffect::Sepia => sepia = Some(button),
        }
    }
    filters_box.append(&filter_grid);

    let black_white = black_white.expect("B&W filter tile");
    let sepia = sepia.expect("Sepia filter tile");

    // Crop has its own geometry workspace again. The tab earns its space by
    // grouping aspect ratio, orientation, straighten and crop reset controls.
    add_section_label(&crop_box, "ASPECT RATIO");
    let aspect_grid = gtk::Grid::new();
    aspect_grid.set_column_spacing(6);
    aspect_grid.set_row_spacing(6);
    aspect_grid.set_column_homogeneous(true);
    aspect_grid.add_css_class("crop-aspect-grid");

    let aspect_presets = [
        CropAspectPreset::Free,
        CropAspectPreset::Original,
        CropAspectPreset::Square,
        CropAspectPreset::FourThree,
        CropAspectPreset::ThreeTwo,
        CropAspectPreset::SixteenNine,
        CropAspectPreset::A4,
        CropAspectPreset::UsLetter,
    ];
    let mut aspect_buttons = Vec::new();
    let first_aspect = gtk::ToggleButton::with_label(aspect_presets[0].label());
    first_aspect.set_active(true);
    first_aspect.set_hexpand(true);
    first_aspect.add_css_class("crop-aspect-button");
    aspect_grid.attach(&first_aspect, 0, 0, 1, 1);
    aspect_buttons.push((aspect_presets[0], first_aspect.clone()));
    for (index, preset) in aspect_presets.iter().copied().enumerate().skip(1) {
        let button = gtk::ToggleButton::with_label(preset.label());
        button.set_group(Some(&first_aspect));
        button.set_hexpand(true);
        button.add_css_class("crop-aspect-button");
        aspect_grid.attach(&button, (index % 2) as i32, (index / 2) as i32, 1, 1);
        aspect_buttons.push((preset, button));
    }
    crop_box.append(&aspect_grid);

    add_section_label(&crop_box, "ORIENTATION");
    let orientation_row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    orientation_row.add_css_class("linked");
    orientation_row.add_css_class("crop-orientation-row");
    let landscape = gtk::ToggleButton::with_label("Landscape");
    landscape.set_hexpand(true);
    landscape.set_active(true);
    let portrait = gtk::ToggleButton::with_label("Portrait");
    portrait.set_group(Some(&landscape));
    portrait.set_hexpand(true);
    orientation_row.append(&landscape);
    orientation_row.append(&portrait);
    crop_box.append(&orientation_row);

    add_section_label(&crop_box, "STRAIGHTEN");
    let straighten_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    straighten_box.add_css_class("crop-straighten");
    let straighten = add_slider(&straighten_box, "Angle", -10.0, 10.0, 0.1, 1);
    straighten.add_css_class("crop-straighten-scale");
    crop_box.append(&straighten_box);

    let crop_action_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let apply_crop_button = gtk::Button::with_label("Apply Crop");
    apply_crop_button.set_hexpand(true);
    apply_crop_button.set_tooltip_text(Some("Commit the pending crop and return to the tools"));
    apply_crop_button.add_css_class("suggested-action");
    apply_crop_button.add_css_class("crop-apply-button");
    let reset_crop = gtk::Button::with_label("Reset Crop");
    reset_crop.set_hexpand(true);
    reset_crop.set_tooltip_text(Some("Remove the crop and return to the full photo"));
    reset_crop.add_css_class("crop-reset-button");
    crop_action_row.append(&apply_crop_button);
    crop_action_row.append(&reset_crop);
    crop_box.append(&crop_action_row);

    add_section_label(&tools_box, "LIGHT");
    let light_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    light_box.add_css_class("edit-adjustment-group");
    tools_box.append(&light_box);

    let exposure = add_slider(&light_box, "Exposure", -2.0, 2.0, 0.05, 2);
    let contrast = add_slider(&light_box, "Contrast", -1.0, 1.0, 0.02, 2);
    let fill_light = add_slider(&light_box, "Fill Light", -1.0, 1.0, 0.02, 2);
    let highlights = add_slider(&light_box, "Highlights", -1.0, 1.0, 0.02, 2);
    let shadows = add_slider(&light_box, "Shadows", -1.0, 1.0, 0.02, 2);

    add_section_label(&tools_box, "COLOUR");
    let colour_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    colour_box.add_css_class("edit-adjustment-group");
    tools_box.append(&colour_box);

    let saturation = add_slider(&colour_box, "Saturation", -1.0, 1.0, 0.02, 2);
    let temperature = add_slider(&colour_box, "Warmth", -1.0, 1.0, 0.02, 2);

    add_section_label(&tools_box, "DETAIL");
    let detail_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    detail_box.add_css_class("edit-adjustment-group");
    tools_box.append(&detail_box);

    let sharpen = add_slider(&detail_box, "Sharpen", 0.0, 1.0, 0.02, 2);

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

    // Generate all quick-look tiles away from the GTK thread. The previews
    // use the current tool recipe while substituting only the tile's filter,
    // so they remain representative without delaying editor startup.
    {
        let path = photo.path();
        let rotation = active_rotation.get();
        let recipe = session.borrow().recipe.clone();
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(render_filter_thumbnails(&path, rotation, &recipe));
        });
        glib::timeout_add_local(Duration::from_millis(25), move || {
            match receiver.try_recv() {
                Ok(Ok(thumbnails)) => {
                    for (picture, (width, height, pixels)) in filter_pictures.iter().zip(thumbnails)
                    {
                        let bytes = glib::Bytes::from_owned(pixels);
                        let texture = gtk::gdk::MemoryTexture::new(
                            width as i32,
                            height as i32,
                            gtk::gdk::MemoryFormat::R8g8b8a8,
                            &bytes,
                            width as usize * 4,
                        );
                        picture.set_paintable(Some(&texture));
                    }
                    glib::ControlFlow::Break
                }
                Ok(Err(_)) | Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    glib::ControlFlow::Break
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            }
        });
    }

    // Ordinary wheel scrolling over a slider should continue scrolling the
    // tools panel. Only the narrow track area in the middle of the scale keeps
    // the normal GTK wheel-to-adjust behavior.
    for scale in [
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
        let overlay_layer = overlay_layer.clone();
        let preview_debounce = preview_debounce.clone();
        let preview_worker = preview_worker.clone();
        Rc::new(move || {
            // Throttle, not debounce: the old code reset the timer on every
            // slider tick, so a continuous drag skipped every intermediate
            // value and the preview jumped 0 -> 0.5 -> 1.0. If a render is
            // already scheduled, keep it — it reads the live session state
            // when it fires, so it always picks up the newest values, and the
            // preview worker coalesces any jobs that overlap.
            if preview_debounce.borrow().is_some() {
                return;
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
            let overlay_layer = overlay_layer.clone();
            let preview_debounce_for_fire = preview_debounce.clone();
            let preview_worker = preview_worker.clone();
            let active_rotation = active_rotation_for_queue.clone();
            let source = glib::timeout_add_local_once(
                Duration::from_millis(PREVIEW_THROTTLE_MS),
                move || {
                    preview_debounce_for_fire.borrow_mut().take();
                    let current_generation = generation.get().wrapping_add(1);
                    generation.set(current_generation);
                    let recipe = session.borrow().recipe.clone();
                    // While a slider drag is in progress, render half-resolution
                    // draft frames: 4x fewer pixels keeps the drag preview light
                    // and fast. The release handler queues a full-resolution
                    // settle render once the drag ends.
                    let interactive = session.borrow().action_active();
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
                    } else if interactive {
                        (1800 / 2, 1400 / 2)
                    } else {
                        (1800, 1400)
                    };

                    if !interactive {
                        busy.set_visible(true);
                        busy.start();
                        status.set_text("Rendering preview…");
                    }

                    let (result_sender, receiver) = std::sync::mpsc::channel();
                    if preview_worker
                        .send(PreviewJob {
                            generation: current_generation,
                            path,
                            rotation,
                            target_width,
                            target_height,
                            recipe,
                            interactive,
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
                    let overlay_layer = overlay_layer.clone();
                    glib::timeout_add_local(Duration::from_millis(25), move || {
                        match receiver.try_recv() {
                            Ok(result) => {
                                if generation.get() != current_generation {
                                    return glib::ControlFlow::Break;
                                }
                                if !interactive {
                                    busy.stop();
                                    busy.set_visible(false);
                                }
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

                                            let anchor =
                                                pending_one_to_one_anchor.borrow_mut().take();
                                            let picture_scroll_for_restore = picture_scroll.clone();
                                            let picture_scroll_for_tick = picture_scroll.clone();
                                            picture_scroll_for_restore.add_tick_callback(
                                                move |_, _| {
                                                    let hadj =
                                                        picture_scroll_for_tick.hadjustment();
                                                    let vadj =
                                                        picture_scroll_for_tick.vadjustment();
                                                    if hadj.upper() < width as f64
                                                        && vadj.upper() < height as f64
                                                    {
                                                        return glib::ControlFlow::Continue;
                                                    }
                                                    let h_max = (hadj.upper() - hadj.page_size())
                                                        .max(hadj.lower());
                                                    let v_max = (vadj.upper() - vadj.page_size())
                                                        .max(vadj.lower());
                                                    let (target_h, target_v) = match anchor {
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
                                                        ),
                                                    };
                                                    hadj.set_value(
                                                        target_h.clamp(hadj.lower(), h_max),
                                                    );
                                                    vadj.set_value(
                                                        target_v.clamp(vadj.lower(), v_max),
                                                    );

                                                    glib::ControlFlow::Break
                                                },
                                            );
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
                                        overlay_layer.queue_draw();
                                    }
                                    Err(error) => {
                                        status.set_text(&format!("Preview failed: {error}"));
                                    }
                                }
                                glib::ControlFlow::Break
                            }
                            Err(std::sync::mpsc::TryRecvError::Empty) => {
                                glib::ControlFlow::Continue
                            }
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
                },
            );
            preview_debounce.replace(Some(source));
        })
    };

    let update_history_buttons =
        history_button_updater(session.clone(), undo.clone(), redo.clone());
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
        let none_filter = controls
            .filters
            .iter()
            .find(|(preset, _)| *preset == FilterPreset::None)
            .expect("None filter tile")
            .1
            .clone();
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
            } else {
                syncing.set(true);
                none_filter.set_active(true);
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
        let none_filter = controls
            .filters
            .iter()
            .find(|(preset, _)| *preset == FilterPreset::None)
            .expect("None filter tile")
            .1
            .clone();
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
            } else {
                syncing.set(true);
                none_filter.set_active(true);
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
        let none_filter = controls
            .filters
            .iter()
            .find(|(candidate, _)| *candidate == FilterPreset::None)
            .expect("None filter tile")
            .1
            .clone();
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
            } else {
                syncing.set(true);
                none_filter.set_active(true);
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
        tools_toggle.connect_toggled(move |button| {
            if button.is_active() {
                panel_stack.set_visible_child_name("tools");
            }
        });
    }
    {
        let panel_stack = panel_stack.clone();
        filters_toggle.connect_toggled(move |button| {
            if button.is_active() {
                panel_stack.set_visible_child_name("filters");
            }
        });
    }
    {
        let panel_stack = panel_stack.clone();
        crop_toggle.connect_toggled(move |button| {
            if button.is_active() {
                panel_stack.set_visible_child_name("crop");
            }
        });
    }
    {
        let panel_stack = panel_stack.clone();
        overlays_toggle.connect_toggled(move |button| {
            if button.is_active() {
                panel_stack.set_visible_child_name("overlays");
            }
        });
    }
    {
        let panel_stack = panel_stack.clone();
        text_toggle.connect_toggled(move |button| {
            if button.is_active() {
                panel_stack.set_visible_child_name("text");
            }
        });
    }

    let sync_controls: Rc<dyn Fn()> = {
        let controls = controls.clone();
        let session = session.clone();
        let syncing = syncing.clone();
        let sync_overlays_slot = sync_overlays_slot.clone();
        let sync_text_slot = sync_text_slot.clone();
        Rc::new(move || {
            syncing.set(true);
            controls.sync(&session.borrow().recipe);
            syncing.set(false);
            if let Some(sync_overlays) = sync_overlays_slot.borrow().as_ref() {
                sync_overlays();
            }
            if let Some(sync_text) = sync_text_slot.borrow().as_ref() {
                sync_text();
            }
        })
    };

    connect_history_actions(
        &undo,
        &redo,
        session.clone(),
        sync_controls.clone(),
        queue_preview.clone(),
        update_history_buttons.clone(),
    );
    {
        let session = session.clone();
        let sync_controls = sync_controls.clone();
        let queue_preview = queue_preview.clone();
        let update_history_buttons = update_history_buttons.clone();
        let crop_overlay = crop_overlay.clone();
        let pending_crop = pending_crop.clone();
        let crop_aspect_preset = crop_aspect_preset.clone();
        let crop_portrait = crop_portrait.clone();
        let crop_aspect_ratio = crop_aspect_ratio.clone();
        let first_aspect = first_aspect.clone();
        let landscape = landscape.clone();
        let tools_toggle = tools_toggle.clone();
        let picture = picture.clone();
        let picture_scroll = picture_scroll.clone();
        let preview_dimensions = preview_dimensions.clone();
        let canvas_zoom = canvas_zoom.clone();
        let native_one_to_one = native_one_to_one.clone();
        let one_to_one_sync = one_to_one_sync.clone();
        let selected_overlay = selected_overlay.clone();
        let selected_text = selected_text.clone();
        reset.connect_clicked(move |_| {
            session.borrow_mut().reset();
            selected_overlay.set(None);
            selected_text.set(None);
            sync_controls();
            update_history_buttons();
            pending_crop.replace(CropRect::default());
            crop_aspect_preset.set(CropAspectPreset::Free);
            crop_portrait.set(false);
            crop_aspect_ratio.set(None);
            first_aspect.set_active(true);
            landscape.set_active(true);
            crop_overlay.set_visible(false);
            tools_toggle.set_active(true);

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

    let cancel_crop: Rc<dyn Fn()> = {
        let crop_overlay = crop_overlay.clone();
        let pending_crop = pending_crop.clone();
        let tools_toggle = tools_toggle.clone();
        Rc::new(move || {
            pending_crop.replace(CropRect::default());
            crop_overlay.set_visible(false);
            tools_toggle.set_active(true);
        })
    };

    let apply_crop: Rc<dyn Fn()> = {
        let session = session.clone();
        let crop_overlay = crop_overlay.clone();
        let pending_crop = pending_crop.clone();
        let queue_preview = queue_preview.clone();
        let update_history_buttons = update_history_buttons.clone();
        Rc::new(move || {
            if !crop_overlay.is_visible() {
                return;
            }

            let child = *pending_crop.borrow();
            let changed = !child.is_full();
            if changed {
                session
                    .borrow_mut()
                    .mutate(|recipe| recipe.crop = recipe.crop.compose(child));
            }

            pending_crop.replace(CropRect::default());
            crop_overlay.set_visible(false);

            if changed {
                update_history_buttons();
                queue_preview();
            }
        })
    };

    // Entering the Crop tab activates the on-canvas crop overlay. Leaving the
    // tab, the Apply Crop button or the Enter key commits the pending crop.
    {
        let crop_overlay = crop_overlay.clone();
        let overlay_layer = overlay_layer.clone();
        let pending_crop = pending_crop.clone();
        let canvas_zoom = canvas_zoom.clone();
        let native_one_to_one = native_one_to_one.clone();
        let one_to_one_sync = one_to_one_sync.clone();
        let picture = picture.clone();
        let picture_scroll = picture_scroll.clone();
        let preview_dimensions = preview_dimensions.clone();
        let apply_crop = apply_crop.clone();
        crop_toggle.connect_toggled(move |button| {
            if button.is_active() {
                canvas_zoom.set(0.0);
                let was_one_to_one = native_one_to_one.replace(false);
                if was_one_to_one {
                    if let Some(handler) = one_to_one_sync.borrow().as_ref() {
                        handler(false);
                    }
                }
                apply_canvas_zoom(&picture, &picture_scroll, preview_dimensions.get(), 0.0);
                pending_crop.replace(CropRect::default());
                // The crop rubber-band owns the canvas while Crop is active;
                // overlay bitmaps would only obscure the dimming mask.
                overlay_layer.set_visible(false);
                crop_overlay.set_visible(true);
                crop_overlay.queue_draw();
            } else {
                apply_crop();
                overlay_layer.set_visible(true);
                overlay_layer.queue_draw();
            }
        });
    }

    // Aspect presets immediately reshape the crop rectangle and constrain new
    // drag selections to the chosen ratio.
    for (preset, button) in &aspect_buttons {
        let preset = *preset;
        let pending_crop = pending_crop.clone();
        let preview_dimensions = preview_dimensions.clone();
        let crop_aspect_preset = crop_aspect_preset.clone();
        let crop_portrait = crop_portrait.clone();
        let crop_aspect_ratio = crop_aspect_ratio.clone();
        let crop_overlay = crop_overlay.clone();
        button.connect_toggled(move |button| {
            if !button.is_active() {
                return;
            }
            crop_aspect_preset.set(preset);
            let dimensions = preview_dimensions.get();
            let ratio = preset.ratio(dimensions.0, dimensions.1, crop_portrait.get());
            crop_aspect_ratio.set(ratio);
            pending_crop.replace(
                ratio
                    .map(|ratio| centered_crop_for_aspect(dimensions.0, dimensions.1, ratio))
                    .unwrap_or_default(),
            );
            crop_overlay.queue_draw();
        });
    }

    let refresh_crop_orientation: Rc<dyn Fn(bool)> = {
        let pending_crop = pending_crop.clone();
        let preview_dimensions = preview_dimensions.clone();
        let crop_aspect_preset = crop_aspect_preset.clone();
        let crop_portrait = crop_portrait.clone();
        let crop_aspect_ratio = crop_aspect_ratio.clone();
        let crop_overlay = crop_overlay.clone();
        Rc::new(move |portrait_mode| {
            crop_portrait.set(portrait_mode);
            let dimensions = preview_dimensions.get();
            let ratio = crop_aspect_preset
                .get()
                .ratio(dimensions.0, dimensions.1, portrait_mode);
            crop_aspect_ratio.set(ratio);
            pending_crop.replace(
                ratio
                    .map(|ratio| centered_crop_for_aspect(dimensions.0, dimensions.1, ratio))
                    .unwrap_or_default(),
            );
            crop_overlay.queue_draw();
        })
    };
    {
        let refresh_crop_orientation = refresh_crop_orientation.clone();
        landscape.connect_toggled(move |button| {
            if button.is_active() {
                refresh_crop_orientation(false);
            }
        });
    }
    {
        let refresh_crop_orientation = refresh_crop_orientation.clone();
        portrait.connect_toggled(move |button| {
            if button.is_active() {
                refresh_crop_orientation(true);
            }
        });
    }

    {
        let session = session.clone();
        let pending_crop = pending_crop.clone();
        let crop_overlay = crop_overlay.clone();
        let crop_aspect_preset = crop_aspect_preset.clone();
        let crop_portrait = crop_portrait.clone();
        let crop_aspect_ratio = crop_aspect_ratio.clone();
        let first_aspect = first_aspect.clone();
        let landscape = landscape.clone();
        let queue_preview = queue_preview.clone();
        let update_history_buttons = update_history_buttons.clone();
        reset_crop.connect_clicked(move |_| {
            let had_crop = !session.borrow().recipe.crop.is_full();
            if had_crop {
                session
                    .borrow_mut()
                    .mutate(|recipe| recipe.crop = CropRect::default());
            }
            pending_crop.replace(CropRect::default());
            crop_aspect_preset.set(CropAspectPreset::Free);
            crop_portrait.set(false);
            crop_aspect_ratio.set(None);
            first_aspect.set_active(true);
            landscape.set_active(true);
            crop_overlay.queue_draw();
            if had_crop {
                update_history_buttons();
                queue_preview();
            }
        });
    }

    {
        let apply_crop = apply_crop.clone();
        let tools_toggle = tools_toggle.clone();
        apply_crop_button.connect_clicked(move |_| {
            apply_crop();
            tools_toggle.set_active(true);
        });
    }

    {
        let apply_crop = apply_crop.clone();
        let cancel_crop = cancel_crop.clone();
        let crop_overlay = crop_overlay.clone();
        let tools_toggle = tools_toggle.clone();
        let key = gtk::EventControllerKey::new();
        key.set_propagation_phase(gtk::PropagationPhase::Capture);
        key.connect_key_pressed(move |_, key, _, _| {
            if !crop_overlay.is_visible() {
                return glib::Propagation::Proceed;
            }
            match key {
                gtk::gdk::Key::Return | gtk::gdk::Key::KP_Enter => {
                    apply_crop();
                    tools_toggle.set_active(true);
                    glib::Propagation::Stop
                }
                gtk::gdk::Key::Escape => {
                    cancel_crop();
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
        root.add_controller(key);
    }

    configure_crop_overlay(
        &crop_overlay,
        pending_crop.clone(),
        preview_dimensions.clone(),
        crop_aspect_ratio.clone(),
    );

    // Overlay canvas layer + Overlays panel. Built after the zoom actions so
    // Ctrl+wheel on the layer can reuse the toolbar zoom closures, and after
    // sync_controls so undo/redo can refresh the panel through the slot.
    let redraw_overlay_canvas: Rc<dyn Fn()> = {
        let overlay_layer = overlay_layer.clone();
        Rc::new(move || overlay_layer.queue_draw())
    };
    let update_canvas_input: Rc<dyn Fn()> = {
        let overlay_layer = overlay_layer.clone();
        let overlays_toggle = overlays_toggle.clone();
        let text_toggle = text_toggle.clone();
        let crop_overlay = crop_overlay.clone();
        let session = session.clone();
        Rc::new(move || {
            let in_crop = crop_overlay.is_visible();
            overlay_layer.set_visible(!in_crop);
            let recipe = session.borrow();
            let enable = (overlays_toggle.is_active() && !recipe.recipe.overlays.is_empty()
                || text_toggle.is_active() && !recipe.recipe.text_layers.is_empty())
                && !in_crop;
            drop(recipe);
            overlay_layer.set_can_target(enable);
            if enable {
                overlay_layer.set_cursor_from_name(Some("default"));
            } else {
                overlay_layer.set_cursor_from_name(None);
            }
            overlay_layer.queue_draw();
        })
    };
    {
        let update_canvas_input = update_canvas_input.clone();
        overlays_toggle.connect_toggled(move |button| {
            if button.is_active() {
                update_canvas_input();
            }
        });
    }
    {
        let update_canvas_input = update_canvas_input.clone();
        text_toggle.connect_toggled(move |button| {
            if button.is_active() {
                update_canvas_input();
            }
        });
    }
    {
        let update_canvas_input = update_canvas_input.clone();
        tools_toggle.connect_toggled(move |button| {
            if button.is_active() {
                update_canvas_input();
            }
        });
    }
    {
        let update_canvas_input = update_canvas_input.clone();
        filters_toggle.connect_toggled(move |button| {
            if button.is_active() {
                update_canvas_input();
            }
        });
    }

    let sync_overlays_panel = build_overlays_panel(
        &overlays_box,
        session.clone(),
        connection.clone(),
        parent.clone(),
        selected_overlay.clone(),
        syncing.clone(),
        preview_dimensions.clone(),
        update_history_buttons.clone(),
        redraw_overlay_canvas.clone(),
        update_canvas_input.clone(),
        overlays_toggle.clone(),
    );
    sync_overlays_slot.replace(Some(sync_overlays_panel.clone()));

    let sync_text_panel = build_text_panel(
        &text_box,
        session.clone(),
        selected_text.clone(),
        syncing.clone(),
        update_history_buttons.clone(),
        redraw_overlay_canvas.clone(),
        update_canvas_input.clone(),
        text_toggle.clone(),
    );
    sync_text_slot.replace(Some(sync_text_panel.clone()));

    configure_overlay_canvas(
        &overlay_layer,
        session.clone(),
        selected_overlay.clone(),
        selected_text.clone(),
        preview_dimensions.clone(),
        canvas_zoom.clone(),
        native_one_to_one.clone(),
        picture_scroll.clone(),
        overlays_toggle.clone(),
        text_toggle.clone(),
        zoom_in_action.clone(),
        zoom_out_action.clone(),
        update_history_buttons.clone(),
        sync_overlays_panel.clone(),
        sync_text_panel.clone(),
        update_canvas_input.clone(),
    );
    update_canvas_input();
    redraw_overlay_canvas();

    {
        let on_close = on_close.clone();
        back.connect_clicked(move |_| on_close());
    }
    connect_export_action(
        &export,
        photo.clone(),
        session.clone(),
        active_rotation.clone(),
        apply_crop.clone(),
        start_export.clone(),
    );

    {
        let connection = connection.clone();
        let session = session.clone();
        let photo = photo.clone();
        let on_saved = on_saved.clone();
        let on_close = on_close.clone();
        let parent = parent.clone();
        let apply_crop = apply_crop.clone();
        done.connect_clicked(move |_| {
            apply_crop();
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
                return;
            }

            std::thread::spawn(move || {
                if let Ok(image) = super::render::decode_base_for_viewer(
                    &path,
                    rotation,
                    target_width,
                    target_height,
                ) {
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
                }
            });
        });
    }

    EditEditor {
        root,
        photo_id: photo.id(),
        back_button: back,
        zoom_in_action,
        zoom_out_action,
        fit_action,
        one_to_one_action,
        set_library_rotation_action,
        one_to_one_sync,
    }
}

#[cfg(test)]
mod panel_tests {
    use super::*;

    fn descendants(root: &impl IsA<gtk::Widget>) -> Vec<gtk::Widget> {
        let mut widgets = Vec::new();
        let mut child = root.as_ref().first_child();
        while let Some(widget) = child {
            child = widget.next_sibling();
            widgets.extend(descendants(&widget));
            widgets.push(widget);
        }
        widgets
    }

    fn settle_gtk() {
        let context = glib::MainContext::default();
        for _ in 0..8 {
            while context.pending() {
                context.iteration(false);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn first_row_count(flow: &gtk::FlowBox) -> usize {
        let Some(first) = flow.first_child() else {
            return 0;
        };
        let row_y = first.allocation().y();
        let mut count = 0;
        let mut child = Some(first);
        while let Some(widget) = child {
            child = widget.next_sibling();
            if widget.allocation().y() == row_y {
                count += 1;
            }
        }
        count
    }

    #[test]
    fn filter_tile_recipes_replace_only_the_filter_effect() {
        let mut base = EditRecipe::default();
        base.filter = FilterPreset::Clarendon;
        base.black_white = true;
        base.exposure = 0.4;
        base.saturation = 0.2;

        let none = FilterTileEffect::Preset(FilterPreset::None).recipe(&base);
        let sepia = FilterTileEffect::Sepia.recipe(&base);

        assert_eq!(none.filter, FilterPreset::None);
        assert!(!none.black_white);
        assert!(!none.sepia);
        assert!((none.exposure - 0.4).abs() < 0.001);
        assert!((none.saturation - 0.2).abs() < 0.001);
        assert_eq!(sepia.filter, FilterPreset::None);
        assert!(!sepia.black_white);
        assert!(sepia.sepia);
        assert!((sepia.exposure - 0.4).abs() < 0.001);
    }

    #[test]
    fn original_filter_tile_uses_photo_editor_language() {
        assert_eq!(
            FilterTileEffect::Preset(FilterPreset::None).label(),
            "Original"
        );
    }

    #[test]
    fn filter_tiles_keep_a_compact_natural_width() {
        assert_eq!(FILTER_TILE_WIDTH, 160);
    }

    #[test]
    fn filter_thumbnail_renderer_builds_every_quick_look() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/samples/AutoHide.jpg");
        let thumbnails = render_filter_thumbnails(path, 0, &EditRecipe::default()).unwrap();

        assert_eq!(thumbnails.len(), FilterPreset::ALL.len() + 3);
        assert!(thumbnails.iter().all(|(width, height, pixels)| *width > 0
            && *height > 0
            && pixels.len() == *width as usize * *height as usize * 4));
        assert_ne!(thumbnails[0].2, thumbnails[1].2);
    }

    #[test]
    fn crop_aspect_presets_produce_expected_centered_rectangles() {
        let four_three = centered_crop_for_aspect(1600, 900, 4.0 / 3.0);
        assert!((four_three.left - 0.125).abs() < 0.001);
        assert!((four_three.right - 0.875).abs() < 0.001);
        assert_eq!(four_three.top, 0.0);
        assert_eq!(four_three.bottom, 1.0);

        let portrait = centered_crop_for_aspect(1600, 900, 3.0 / 4.0);
        // A 3:4 (portrait) crop out of a 16:9 landscape must trim the wide
        // dimension: keep the full height and cut the sides to 900 * 3/4 px.
        assert_eq!(portrait.top, 0.0);
        assert_eq!(portrait.bottom, 1.0);
        assert!((portrait.left - 0.2890625).abs() < 0.001);
        assert!((portrait.right - 0.7109375).abs() < 0.001);
    }

    #[test]
    fn crop_aspect_orientation_swaps_non_square_ratios() {
        assert_eq!(
            CropAspectPreset::FourThree.ratio(1600, 900, false),
            Some(4.0 / 3.0)
        );
        assert_eq!(
            CropAspectPreset::FourThree.ratio(1600, 900, true),
            Some(3.0 / 4.0)
        );
        assert_eq!(CropAspectPreset::Square.ratio(1600, 900, true), Some(1.0));
        assert_eq!(CropAspectPreset::Free.ratio(1600, 900, false), None);
        assert_eq!(
            CropAspectPreset::A4.ratio(1600, 900, false),
            Some(297.0 / 210.0)
        );
        assert_eq!(
            CropAspectPreset::A4.ratio(1600, 900, true),
            Some(210.0 / 297.0)
        );
        assert_eq!(
            CropAspectPreset::UsLetter.ratio(1600, 900, false),
            Some(11.0 / 8.5)
        );
        assert_eq!(
            CropAspectPreset::UsLetter.ratio(1600, 900, true),
            Some(8.5 / 11.0)
        );
    }

    #[test]
    #[ignore = "requires a GTK display; run with --ignored --test-threads=1"]
    fn editor_uses_three_tab_sidebar_with_crop_workspace() {
        if !gtk::is_initialized() {
            gtk::init().unwrap();
        }
        // Apply the same structural edit-panel CSS the real window installs,
        // so tile geometry matches production instead of stock theme padding.
        let edit_css = gtk::CssProvider::new();
        edit_css.load_from_data(crate::window::EDIT_PANEL_CSS);
        gtk::style_context_add_provider_for_display(
            &gtk::gdk::Display::default().unwrap(),
            &edit_css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        let parent = gtk::Window::new();
        let photo: crate::photo_object::PhotoObject = glib::Object::new();
        photo.set_path(concat!(env!("CARGO_MANIFEST_DIR"), "/samples/AutoHide.jpg").to_string());
        let editor = build(
            &parent,
            Rc::new(RefCell::new(Connection::open_in_memory().unwrap())),
            photo,
            Rc::new(|| {}),
            Rc::new(|_| {}),
            Rc::new(|_| {}),
        );
        parent.set_default_size(1500, 900);
        parent.set_child(Some(&editor.root));
        parent.present();
        settle_gtk();

        let toolbar = editor.root.first_child().unwrap();
        let body = toolbar
            .next_sibling()
            .unwrap()
            .downcast::<gtk::Paned>()
            .unwrap();
        let sidebar = body.start_child().unwrap();
        let sidebar_widgets = descendants(&sidebar);
        let buttons = sidebar_widgets
            .iter()
            .filter_map(|widget| widget.clone().downcast::<gtk::ToggleButton>().ok())
            .collect::<Vec<_>>();
        let stack = sidebar_widgets
            .iter()
            .find(|widget| widget.has_css_class("edit-panel-pages"))
            .unwrap()
            .clone()
            .downcast::<gtk::Stack>()
            .unwrap();

        // The filter grid must reflow responsively: two columns at the default
        // pane width, one when the pane is dragged narrow, up to five on wide
        // panes, with preview thumbnails that have a real visible height.
        let flow = sidebar_widgets
            .iter()
            .find_map(|widget| widget.clone().downcast::<gtk::FlowBox>().ok())
            .unwrap();
        assert_eq!(flow.min_children_per_line(), 1);
        assert_eq!(flow.max_children_per_line(), 5);
        assert!(flow.is_homogeneous());

        sidebar_widgets
            .iter()
            .filter(|widget| {
                widget
                    .parent()
                    .is_some_and(|parent| parent.has_css_class("edit-panel-tabs"))
            })
            .find_map(|widget| {
                let button = widget.clone().downcast::<gtk::ToggleButton>().ok()?;
                (button.label().as_deref() == Some("Filters")).then_some(button)
            })
            .unwrap()
            .emit_clicked();
        settle_gtk();
        // Window mapping and the first allocation passes are asynchronous and
        // can be slow on headless displays without a window manager; wait for
        // the grid to actually be laid out before asserting on its geometry.
        for _ in 0..100 {
            if flow.allocation().width() > 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
            settle_gtk();
        }

        let columns_at = |position: i32| {
            body.set_position(position);
            // set_position only stores the value; the start child is re-allocated
            // on the next frame-clock layout pass, which requires a live display
            // (this is why the test is #[ignored] and run on a real session).
            body.queue_resize();
            for _ in 0..100 {
                settle_gtk();
                // The stack pages carry 14px side margins (matching the tab
                // row), so the grid is 28px narrower than the paned position.
                if (flow.allocation().width() - (position - 28)).abs() <= 8 {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            first_row_count(&flow)
        };
        assert_eq!(
            columns_at(360),
            2,
            "default pane width shows two columns (flowbox alloc was {:?})",
            flow.allocation()
        );
        assert!(
            columns_at(540) >= 3,
            "wider pane must expose a third column"
        );
        assert!(
            columns_at(720) >= 4,
            "wider pane must expose a fourth column"
        );
        assert_eq!(columns_at(1200), 5, "widest pane caps at five columns");

        let first_child = flow.first_child().unwrap();
        let tile_button = first_child.first_child().unwrap();
        let tile_content = tile_button.first_child().unwrap();
        let tile_picture = tile_content
            .first_child()
            .unwrap()
            .downcast::<gtk::Picture>()
            .unwrap();
        let (tile_min_width, tile_min_height) = tile_picture.size_request();
        assert_eq!(tile_min_width, FILTER_TILE_PREVIEW_WIDTH);
        assert!(
            tile_min_height >= 100,
            "preview needs a visible minimum height"
        );
        assert!(tile_picture.allocation().height() >= tile_min_height);

        let top_level_tab_labels = buttons
            .iter()
            .filter(|button| {
                button
                    .parent()
                    .is_some_and(|parent| parent.has_css_class("edit-panel-tabs"))
            })
            .filter_map(|button| button.label().map(|label| label.to_string()))
            .collect::<Vec<_>>();
        assert_eq!(
            top_level_tab_labels,
            vec![
                "Tools".to_string(),
                "Filters".to_string(),
                "Crop".to_string(),
                "Overlays".to_string(),
                "Text".to_string()
            ]
        );

        let overlays_tab = buttons
            .iter()
            .find(|button| button.label().as_deref() == Some("Overlays"))
            .unwrap();
        overlays_tab.emit_clicked();
        settle_gtk();
        assert_eq!(
            stack.visible_child_name().as_deref(),
            Some("overlays"),
            "Overlays tab must show its panel page"
        );

        let text_tab = buttons
            .iter()
            .find(|button| button.label().as_deref() == Some("Text"))
            .unwrap();
        text_tab.emit_clicked();
        settle_gtk();
        assert_eq!(
            stack.visible_child_name().as_deref(),
            Some("text"),
            "Text tab must show its panel page"
        );

        let crop_tab = buttons
            .iter()
            .find(|button| button.label().as_deref() == Some("Crop"))
            .unwrap();
        crop_tab.emit_clicked();
        assert_eq!(stack.visible_child_name().as_deref(), Some("crop"));

        for label in [
            "Free",
            "Original",
            "1:1",
            "4:3",
            "3:2",
            "16:9",
            "Landscape",
            "Portrait",
        ] {
            assert!(
                buttons
                    .iter()
                    .any(|button| button.label().as_deref() == Some(label)),
                "missing crop control {label}"
            );
        }
        assert!(sidebar_widgets
            .iter()
            .any(|widget| widget.has_css_class("crop-straighten-scale")));
        assert!(sidebar_widgets.iter().any(|widget| {
            widget
                .clone()
                .downcast::<gtk::Button>()
                .ok()
                .and_then(|button| button.label())
                .as_deref()
                == Some("Reset Crop")
        }));
        let apply_crop_button = sidebar_widgets.iter().find_map(|widget| {
            let button = widget.clone().downcast::<gtk::Button>().ok()?;
            (button.label().as_deref() == Some("Apply Crop")).then_some(button)
        });
        assert!(
            apply_crop_button.is_some(),
            "crop tab needs an Apply Crop button"
        );
        assert!(apply_crop_button.unwrap().has_css_class("suggested-action"));
        settle_gtk();
    }

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
