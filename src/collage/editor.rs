use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;

use super::model::{AspectRatio, Background, CollageOrientation, CollageProject, LayoutKind};

const MAX_PREVIEW_CORNER_RADIUS: i32 = 48;

thread_local! {
    // Bumped on every preview rebuild so late decode results from an older
    // preview are discarded.
    static PREVIEW_GENERATION: Cell<u64> = const { Cell::new(0) };
    // The collage stylesheet is installed once per process; build() runs per
    // collage session and must not stack a new display provider each time.
    static COLLAGE_CSS_INSTALLED: Cell<bool> = const { Cell::new(false) };
    // Sharper preview textures keyed by (path, rotation, recipe). Reused across
    // preview rebuilds so dragging does not re-decode every photo.
    static PREVIEW_SHARP_CACHE: RefCell<std::collections::HashMap<(String, i32, String), gtk::gdk::Paintable>> =
        RefCell::new(std::collections::HashMap::new());
}

/// Decode a sharper preview source off the GTK thread. The editor preview
/// otherwise draws the 320 px cached thumbnail scaled up to the canvas tile.
fn schedule_preview_sharpen(
    picture: gtk::Picture,
    reference: String,
    rotation: i32,
    recipe: String,
    generation: u64,
) {
    let key = (reference.clone(), rotation, recipe.clone());
    if let Some(cached) = PREVIEW_SHARP_CACHE.with(|cache| cache.borrow().get(&key).cloned()) {
        picture.set_paintable(Some(&cached));
        return;
    }
    const PREVIEW_SHARP_SIZE: u32 = 1024;
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = crate::edit::render::render_for_viewer(
            &reference,
            rotation,
            &crate::edit::EditRecipe::decode(&recipe),
            PREVIEW_SHARP_SIZE,
            PREVIEW_SHARP_SIZE,
        );
        let _ = sender.send(result);
    });
    glib::timeout_add_local(Duration::from_millis(25), move || {
        match receiver.try_recv() {
            Ok(Ok(image)) => {
                if PREVIEW_GENERATION.with(Cell::get) == generation {
                    let (width, height) = image.dimensions();
                    let bytes = glib::Bytes::from_owned(image.into_raw());
                    let texture = gtk::gdk::MemoryTexture::new(
                        width as i32,
                        height as i32,
                        gtk::gdk::MemoryFormat::R8g8b8a8,
                        &bytes,
                        width as usize * 4,
                    );
                    let paintable: gtk::gdk::Paintable = texture.upcast();
                    picture.set_paintable(Some(&paintable));
                    PREVIEW_SHARP_CACHE.with(|cache| {
                        cache.borrow_mut().insert(key.clone(), paintable);
                    });
                }
                glib::ControlFlow::Break
            }
            Ok(Err(_)) => glib::ControlFlow::Break,
            Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
        }
    });
}

fn collage_css() -> String {
    let mut css = String::from(
        ".collage-canvas { border: 1px solid alpha(@theme_fg_color, 0.28); }\
         .collage-white { background: #ffffff; }\
         .collage-black { background: #000000; }\
         .collage-light-gray { background: #eeeeee; }\
         .collage-photo { border: 1px solid alpha(@theme_fg_color, 0.18); border-radius: 0; box-shadow: 0 3px 12px alpha(#000000, 0.28); }\
         .collage-contained-photo { border: none; box-shadow: none; }\
         .collage-dragging { opacity: 0.62; }\
         .collage-drop-target { border: 3px solid #4d9fdb; box-shadow: 0 0 0 3px alpha(#4d9fdb, 0.45), 0 3px 12px alpha(#000000, 0.35); }\
         .collage-photo-rounded { }\
         .collage-layout-tile { min-height: 54px; }\
         .collage-radius-row { margin-top: 2px; }\
         .collage-tabs button { min-height: 34px; padding: 4px 10px; }",
    );
    for radius in 0..=MAX_PREVIEW_CORNER_RADIUS {
        css.push_str(&format!(
            ".collage-photo-radius-{radius} {{ border-radius: {radius}px; }}"
        ));
    }
    css
}

struct PreviewFrame {
    outer: gtk::Widget,
    inner: gtk::Fixed,
    photo: gtk::Frame,
}

/// Host-application hooks for tile interactions (threaded into
/// refresh_preview so per-tile menus can act on the library).
#[derive(Clone)]
struct TileHooks {
    /// Opens the photo editor for a library photo id.
    open_edit: Rc<dyn Fn(i64)>,
}

pub struct CollageEditor {
    pub root: gtk::Box,
    project: Rc<RefCell<CollageProject>>,
    canvas: gtk::Fixed,
    frames: Rc<RefCell<Vec<PreviewFrame>>>,
    status: gtk::Label,
    hooks: TileHooks,
}

impl CollageEditor {
    pub fn photo_ids(&self) -> Vec<i64> {
        self.project
            .borrow()
            .items
            .iter()
            .map(|item| item.photo.id)
            .collect()
    }

    pub fn add_photos(&self, photos: Vec<crate::photo_object::PhotoObject>) {
        self.project.borrow_mut().add_photos(photos);
        self.status
            .set_text(&photo_count_text(self.project.borrow().items.len()));
        refresh_preview(&self.canvas, &self.frames, &self.project, &self.hooks);
    }

    pub fn set_photos(&self, photos: Vec<crate::photo_object::PhotoObject>) {
        self.project.borrow_mut().set_photos(photos);
        self.status
            .set_text(&photo_count_text(self.project.borrow().items.len()));
        refresh_preview(&self.canvas, &self.frames, &self.project, &self.hooks);
    }

    /// Serialized draft snapshot for persistence (see DRAFT_SETTING_KEY).
    pub fn draft_json(&self) -> String {
        super::model::draft_to_json(&self.project.borrow())
    }

    /// Refresh embedded photo metadata for the given ids (used when
    /// returning from the photo editor) and redraw the preview.
    pub fn refresh_photo_metadata(&self, photos: &[crate::photo_object::PhotoObject]) {
        self.project.borrow_mut().refresh_photo_metadata(photos);
        refresh_preview(&self.canvas, &self.frames, &self.project, &self.hooks);
    }
}

pub fn build(
    parent: &gtk::Window,
    photos: Vec<crate::photo_object::PhotoObject>,
    on_add_photos: Rc<dyn Fn()>,
    on_close: Rc<dyn Fn()>,
    draft: Option<super::model::CollageDraft>,
    on_edit_photo: Rc<dyn Fn(i64)>,
) -> CollageEditor {
    let display = gtk::prelude::WidgetExt::display(parent);
    if !COLLAGE_CSS_INSTALLED.with(Cell::get) {
        let css = gtk::CssProvider::new();
        let css_data = collage_css();
        css.load_from_data(&css_data);
        gtk::style_context_add_provider_for_display(
            &display,
            &css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        COLLAGE_CSS_INSTALLED.with(|cell| cell.set(true));
    }
    let project = Rc::new(RefCell::new(CollageProject::new(photos.clone())));
    // A resumed draft restores settings and the saved arrangement before
    // any widget reads project state, so every control reflects it.
    if let Some(draft) = draft.as_ref() {
        project.borrow_mut().apply_draft(draft, &photos);
    }
    let hooks = TileHooks {
        open_edit: on_edit_photo,
    };

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.set_hexpand(true);
    root.set_vexpand(true);
    let controls = gtk::Box::new(gtk::Orientation::Vertical, 12);
    controls.set_width_request(250);
    controls.set_margin_top(18);
    controls.set_margin_bottom(18);
    controls.set_margin_start(18);
    controls.set_margin_end(18);

    let heading = gtk::Label::new(Some("Collage"));
    heading.set_halign(gtk::Align::Start);
    heading.add_css_class("title-2");
    controls.append(&heading);
    let status = gtk::Label::new(Some(&photo_count_text(project.borrow().items.len())));
    status.set_halign(gtk::Align::Start);
    status.add_css_class("dim-label");
    controls.append(&status);

    let canvas = gtk::Fixed::new();
    canvas.set_hexpand(true);
    canvas.set_vexpand(true);
    canvas.add_css_class("collage-canvas");
    let aspect_frame =
        gtk::AspectFrame::new(0.5, 0.5, project.borrow().effective_aspect_ratio(), false);
    aspect_frame.set_hexpand(true);
    aspect_frame.set_vexpand(true);
    aspect_frame.set_halign(gtk::Align::Fill);
    aspect_frame.set_valign(gtk::Align::Fill);
    aspect_frame.set_margin_top(24);
    aspect_frame.set_margin_bottom(24);
    aspect_frame.set_margin_start(24);
    aspect_frame.set_margin_end(24);
    aspect_frame.set_child(Some(&preview_bounds(&canvas)));
    let frames: Rc<RefCell<Vec<PreviewFrame>>> = Rc::new(RefCell::new(Vec::new()));

    let refresh = {
        let project = project.clone();
        let canvas = canvas.clone();
        let frames = frames.clone();
        let hooks = hooks.clone();
        Rc::new(move || refresh_preview(&canvas, &frames, &project, &hooks))
    };
    refresh();
    {
        let last_size = Rc::new(Cell::new((0, 0)));
        let canvas_for_callback = canvas.clone();
        let frames = frames.clone();
        let project = project.clone();
        canvas.add_tick_callback(move |canvas, _| {
            let size = (canvas.width(), canvas.height());
            if last_size.get() != size {
                last_size.set(size);
                update_geometry(&canvas_for_callback, &frames, &project.borrow());
            }
            glib::ControlFlow::Continue
        });
    }

    // Row 1 — layout: three equal tiles with icon over title, radio
    // behaviour; edit-mode tab styling keeps every button row uniform.
    add_section_label(&controls, "LAYOUT");
    let mosaic_tile = gtk::ToggleButton::new();
    let smart_tile = gtk::ToggleButton::new();
    let grid_tile = gtk::ToggleButton::new();
    for (tile, icon, title, tooltip) in [
        (
            &mosaic_tile,
            "collage-mosaic-symbolic",
            "Mosaic",
            "Mosaic: varied tile sizes, no dominant photo",
        ),
        (
            &smart_tile,
            "collage-smart-mosaic-symbolic",
            "Smart",
            "Smart Mosaic: automatic layout with a dominant photo",
        ),
        (
            &grid_tile,
            "collage-grid-symbolic",
            "Grid",
            "Grid: uniform tiles, photos crop to fill",
        ),
    ] {
        let content = gtk::Box::new(gtk::Orientation::Vertical, 3);
        content.set_halign(gtk::Align::Center);
        content.append(&gtk::Image::from_icon_name(icon));
        content.append(&gtk::Label::new(Some(title)));
        tile.set_child(Some(&content));
        tile.set_tooltip_text(Some(tooltip));
        tile.add_css_class("collage-layout-tile");
    }
    smart_tile.set_group(Some(&mosaic_tile));
    grid_tile.set_group(Some(&mosaic_tile));
    match project.borrow().layout {
        LayoutKind::Mosaic => mosaic_tile.set_active(true),
        LayoutKind::SmartMosaic => smart_tile.set_active(true),
        LayoutKind::Grid => grid_tile.set_active(true),
    }
    let layout_tiles = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    layout_tiles.add_css_class("linked");
    layout_tiles.add_css_class("collage-tabs");
    // Homogeneous thirds so the layout tiles, the fit/orientation row and
    // the bottom actions all align to the same three column edges.
    layout_tiles.set_homogeneous(true);
    for tile in [&mosaic_tile, &smart_tile, &grid_tile] {
        layout_tiles.append(tile);
    }
    controls.append(&layout_tiles);

    // Row 2 — fit + orientation. Fit replaces the keep-photo-aspect
    // checkbox; it stays visible but insensitive for Grid, which always
    // crops photos to fill their tiles. Portrait/Landscape form a radio
    // pair of labelled buttons (icon+label would not fit the column).
    let fit_toggle = gtk::ToggleButton::new();
    {
        let fit_content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        fit_content.set_halign(gtk::Align::Center);
        fit_content.append(&gtk::Image::from_icon_name("zoom-fit-best-symbolic"));
        fit_content.append(&gtk::Label::new(Some("Fit")));
        fit_toggle.set_child(Some(&fit_content));
    }
    fit_toggle.set_tooltip_text(Some("Fit photos inside their tiles (keep aspect ratio)"));
    fit_toggle.set_active(project.borrow().keep_photo_aspect);
    fit_toggle.set_sensitive(!matches!(project.borrow().layout, LayoutKind::Grid));
    let portrait_btn = gtk::ToggleButton::new();
    {
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        content.set_halign(gtk::Align::Center);
        content.append(&gtk::Image::from_icon_name(
            "orientation-portrait-left-symbolic",
        ));
        content.append(&gtk::Label::new(Some("Portrait")));
        portrait_btn.set_child(Some(&content));
    }
    portrait_btn.set_tooltip_text(Some("Portrait canvas"));
    let landscape_btn = gtk::ToggleButton::new();
    {
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        content.set_halign(gtk::Align::Center);
        content.append(&gtk::Image::from_icon_name(
            "orientation-landscape-symbolic",
        ));
        content.append(&gtk::Label::new(Some("Landscape")));
        landscape_btn.set_child(Some(&content));
    }
    landscape_btn.set_tooltip_text(Some("Landscape canvas"));
    portrait_btn.set_group(Some(&landscape_btn));
    if matches!(project.borrow().orientation, CollageOrientation::Portrait) {
        portrait_btn.set_active(true);
    } else {
        landscape_btn.set_active(true);
    }
    let fit_orientation_row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    fit_orientation_row.add_css_class("linked");
    fit_orientation_row.add_css_class("collage-tabs");
    fit_orientation_row.set_homogeneous(true);
    for button in [&fit_toggle, &portrait_btn, &landscape_btn] {
        fit_orientation_row.append(button);
    }
    controls.append(&fit_orientation_row);
    {
        let project = project.clone();
        let refresh = refresh.clone();
        let fit_toggle = fit_toggle.clone();
        let apply_layout = Rc::new(move |layout_kind: LayoutKind| {
            {
                let mut project_data = project.borrow_mut();
                if project_data.layout == layout_kind {
                    return;
                }
                project_data.layout = layout_kind;
                project_data.relayout();
            }
            fit_toggle.set_sensitive(!matches!(layout_kind, LayoutKind::Grid));
            refresh();
        });
        for (button, kind) in [
            (mosaic_tile.clone(), LayoutKind::Mosaic),
            (smart_tile.clone(), LayoutKind::SmartMosaic),
            (grid_tile.clone(), LayoutKind::Grid),
        ] {
            let apply_layout = apply_layout.clone();
            button.connect_toggled(move |tile| {
                if tile.is_active() {
                    apply_layout(kind);
                }
            });
        }
    }
    {
        let project = project.clone();
        let refresh = refresh.clone();
        fit_toggle.connect_toggled(move |button| {
            let mut project_data = project.borrow_mut();
            project_data.keep_photo_aspect = button.is_active();
            if matches!(
                project_data.layout,
                LayoutKind::Mosaic | LayoutKind::SmartMosaic
            ) {
                project_data.relayout();
                drop(project_data);
                refresh();
            }
        });
    }
    for (button, value) in [
        (&portrait_btn, CollageOrientation::Portrait),
        (&landscape_btn, CollageOrientation::Landscape),
    ] {
        let project = project.clone();
        let refresh = refresh.clone();
        let aspect_frame = aspect_frame.clone();
        button.connect_toggled(move |btn| {
            if !btn.is_active() {
                return;
            }
            let ratio = {
                let mut project_data = project.borrow_mut();
                project_data.orientation = value;
                project_data.relayout();
                project_data.effective_aspect_ratio()
            };
            aspect_frame.set_ratio(ratio);
            refresh();
        });
    }

    // Row 3 — aspect ratio + background side by side, each with a small
    // title above the dropdown.
    let aspect = gtk::DropDown::from_strings(&["Square 1:1", "4:3", "3:2", "16:9", "Custom"]);
    aspect.set_selected(match project.borrow().aspect {
        AspectRatio::Square => 0,
        AspectRatio::FourThree => 1,
        AspectRatio::ThreeTwo => 2,
        AspectRatio::SixteenNine => 3,
        AspectRatio::Custom => 4,
    });
    aspect.set_tooltip_text(Some("Canvas aspect ratio"));
    let background = gtk::DropDown::from_strings(&["White", "Black", "Light Grey"]);
    background.set_selected(match project.borrow().background {
        Background::White => 0,
        Background::Black => 1,
        Background::LightGray => 2,
    });
    background.set_tooltip_text(Some("Canvas background"));
    // Equal halves via a homogeneous grid, matching the edit panel's
    // aspect-preset grid geometry; each dropdown keeps a small title.
    let aspect_background_grid = gtk::Grid::new();
    aspect_background_grid.set_column_homogeneous(true);
    aspect_background_grid.set_column_spacing(8);
    aspect_background_grid.set_hexpand(true);
    let aspect_column = titled_control("Aspect ratio", &aspect);
    let background_column = titled_control("Background", &background);
    aspect_background_grid.attach(&aspect_column, 0, 0, 1, 1);
    aspect_background_grid.attach(&background_column, 1, 0, 1, 1);
    controls.append(&aspect_background_grid);
    let custom_width = gtk::SpinButton::with_range(1.0, 10_000.0, 1.0);
    custom_width.set_value((project.borrow().custom_aspect * 9.0).round().max(1.0) as f64);
    custom_width.set_numeric(true);
    custom_width.set_digits(0);
    custom_width.set_tooltip_text(Some("Custom aspect width"));
    let custom_height = gtk::SpinButton::with_range(1.0, 10_000.0, 1.0);
    custom_height.set_value(9.0);
    custom_height.set_numeric(true);
    custom_height.set_digits(0);
    custom_height.set_tooltip_text(Some("Custom aspect height"));
    let custom_ratio = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    custom_ratio.append(&custom_width);
    custom_ratio.append(&gtk::Label::new(Some(":")));
    custom_ratio.append(&custom_height);
    custom_ratio.set_visible(project.borrow().aspect == AspectRatio::Custom);
    controls.append(&custom_ratio);
    {
        let project = project.clone();
        let refresh = refresh.clone();
        let aspect_frame = aspect_frame.clone();
        let custom_ratio = custom_ratio.clone();
        aspect.connect_selected_notify(move |dropdown| {
            let value = match dropdown.selected() {
                0 => AspectRatio::Square,
                1 => AspectRatio::FourThree,
                2 => AspectRatio::ThreeTwo,
                3 => AspectRatio::SixteenNine,
                _ => AspectRatio::Custom,
            };
            let ratio = {
                let mut project_data = project.borrow_mut();
                project_data.aspect = value;
                project_data.relayout();
                project_data.effective_aspect_ratio()
            };
            aspect_frame.set_ratio(ratio);
            custom_ratio.set_visible(value == AspectRatio::Custom);
            refresh();
        });
    }
    for custom_control in [&custom_width, &custom_height] {
        let project = project.clone();
        let refresh = refresh.clone();
        let aspect_frame = aspect_frame.clone();
        let custom_width = custom_width.clone();
        let custom_height = custom_height.clone();
        custom_control.connect_value_changed(move |_| {
            let ratio = custom_width.value() as f32 / custom_height.value().max(1.0) as f32;
            let is_custom = {
                let mut project_data = project.borrow_mut();
                project_data.custom_aspect = ratio;
                let is_custom = project_data.aspect == AspectRatio::Custom;
                if is_custom {
                    project_data.relayout();
                }
                is_custom
            };
            if is_custom {
                aspect_frame.set_ratio(project.borrow().effective_aspect_ratio());
                refresh();
            }
        });
    }

    {
        let project = project.clone();
        let refresh = refresh.clone();
        background.connect_selected_notify(move |dropdown| {
            project.borrow_mut().background = if dropdown.selected() == 1 {
                Background::Black
            } else if dropdown.selected() == 2 {
                Background::LightGray
            } else {
                Background::White
            };
            refresh();
        });
    }

    // Corners + slider share a single row under an edit-style heading:
    // on/off checkbox, then the radius slider bracketed by sharp/rounded
    // end icons. The icons state the actual range (0% .. 20%).
    add_section_label(&controls, "ROUND CORNERS");
    let round_corners = gtk::CheckButton::new();
    round_corners.set_tooltip_text(Some("Enable rounded corners"));
    round_corners.set_valign(gtk::Align::Center);
    round_corners.set_active(project.borrow().round_corners);
    let corner_sharp_icon = gtk::Image::from_icon_name("collage-corner-sharp-symbolic");
    corner_sharp_icon.set_tooltip_text(Some("No corner radius (0%)"));
    let corner_round_icon = gtk::Image::from_icon_name("collage-corner-round-symbolic");
    corner_round_icon.set_tooltip_text(Some("Maximum corner radius (20%)"));
    let corner_icons_active = project.borrow().round_corners;
    corner_sharp_icon.set_sensitive(corner_icons_active);
    corner_round_icon.set_sensitive(corner_icons_active);
    let corner_radius = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 0.2, 0.005);
    corner_radius.set_value(project.borrow().corner_radius as f64);
    corner_radius.set_digits(3);
    corner_radius.set_draw_value(false);
    corner_radius.set_sensitive(project.borrow().round_corners);
    corner_radius.set_tooltip_text(Some("Corner radius (0% to 20% of the tile's short side)"));
    corner_radius.set_hexpand(true);
    let radius_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    radius_row.add_css_class("collage-radius-row");
    radius_row.append(&round_corners);
    radius_row.append(&corner_sharp_icon);
    radius_row.append(&corner_radius);
    radius_row.append(&corner_round_icon);
    controls.append(&radius_row);
    {
        let project = project.clone();
        let refresh = refresh.clone();
        let corner_radius = corner_radius.clone();
        let corner_sharp_icon = corner_sharp_icon.clone();
        let corner_round_icon = corner_round_icon.clone();
        round_corners.connect_toggled(move |button| {
            project.borrow_mut().round_corners = button.is_active();
            corner_radius.set_sensitive(button.is_active());
            corner_sharp_icon.set_sensitive(button.is_active());
            corner_round_icon.set_sensitive(button.is_active());
            refresh();
        });
    }
    {
        let project = project.clone();
        let refresh = refresh.clone();
        corner_radius.connect_value_changed(move |scale| {
            project.borrow_mut().corner_radius = scale.value() as f32;
            refresh();
        });
    }

    add_section_label(&controls, "SPACING");
    let spacing = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 0.08, 0.002);
    spacing.set_value(project.borrow().spacing as f64);
    spacing.set_draw_value(false);
    controls.append(&spacing);
    {
        let project = project.clone();
        let canvas = canvas.clone();
        let frames = frames.clone();
        spacing.connect_value_changed(move |scale| {
            let mut project = project.borrow_mut();
            project.spacing = scale.value() as f32;
            project.relayout();
            update_geometry(&canvas, &frames, &project);
        });
    }

    // Bottom actions. Compact icon+label controls grouped right; Create is
    // the one full-width accent button with Exit beside it.
    let reset_defaults = icon_label_button(
        "view-refresh-symbolic",
        "Reset",
        "Restore default collage settings",
    );
    {
        let project = project.clone();
        let smart_tile = smart_tile.clone();
        let landscape_btn = landscape_btn.clone();
        let fit_toggle = fit_toggle.clone();
        let aspect = aspect.clone();
        let custom_width = custom_width.clone();
        let custom_height = custom_height.clone();
        let background = background.clone();
        let round_corners = round_corners.clone();
        let corner_radius = corner_radius.clone();
        let spacing = spacing.clone();
        let aspect_frame = aspect_frame.clone();
        let custom_ratio = custom_ratio.clone();
        let corner_sharp_icon = corner_sharp_icon.clone();
        let corner_round_icon = corner_round_icon.clone();
        reset_defaults.connect_clicked(move |_| {
            {
                let mut project_data = project.borrow_mut();
                project_data.layout = LayoutKind::SmartMosaic;
                project_data.orientation = CollageOrientation::Landscape;
                project_data.aspect = AspectRatio::SixteenNine;
                project_data.custom_aspect = 16.0 / 9.0;
                project_data.background = Background::White;
                project_data.round_corners = false;
                project_data.corner_radius = 0.06;
                project_data.spacing = 0.018;
                project_data.keep_photo_aspect = true;
            }
            smart_tile.set_active(true);
            landscape_btn.set_active(true);
            fit_toggle.set_active(true);
            // The tile handler skips no-op activations, so re-assert the
            // fit sensitivity explicitly (it may have been greyed by Grid).
            fit_toggle.set_sensitive(true);
            aspect.set_selected(3);
            custom_width.set_value(16.0);
            custom_height.set_value(9.0);
            background.set_selected(0);
            round_corners.set_active(false);
            corner_radius.set_value(0.06);
            spacing.set_value(0.018);
            custom_ratio.set_visible(false);
            aspect_frame.set_ratio(16.0 / 9.0);
            corner_sharp_icon.set_sensitive(false);
            corner_round_icon.set_sensitive(false);
            let mut project_data = project.borrow_mut();
            project_data.relayout();
        });
    }

    // Bottom actions share the top rows' look: one linked row of three
    // equal-width blocks spanning the panel width.
    let actions_row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    actions_row.add_css_class("linked");
    actions_row.add_css_class("collage-tabs");
    actions_row.set_homogeneous(true);
    let add_photos = icon_label_button("list-add-symbolic", "Add", "Add photos from the library");
    let shuffle = icon_label_button(
        "media-playlist-shuffle-symbolic",
        "Shuffle",
        "Shuffle photo order",
    );
    actions_row.append(&reset_defaults);
    actions_row.append(&add_photos);
    actions_row.append(&shuffle);
    controls.append(&actions_row);
    {
        let project = project.clone();
        let refresh = refresh.clone();
        shuffle.connect_clicked(move |_| {
            project.borrow_mut().shuffle();
            refresh();
        });
    }

    // Create spans the first two thirds; Exit occupies the last third at
    // exactly the same block size as Shuffle in the row above.
    let primary_grid = gtk::Grid::new();
    primary_grid.set_column_homogeneous(true);
    primary_grid.set_column_spacing(6);
    primary_grid.set_hexpand(true);
    primary_grid.add_css_class("collage-tabs");
    let export = gtk::Button::with_label("Create Collage…");
    export.add_css_class("suggested-action");
    export.set_tooltip_text(Some("Render and export the collage as a JPEG"));
    let close = icon_label_button("application-exit-symbolic", "Exit", "Exit collage");
    primary_grid.attach(&export, 0, 0, 2, 1);
    primary_grid.attach(&close, 2, 0, 1, 1);
    controls.append(&primary_grid);
    {
        let project = project.clone();
        let parent = parent.clone();
        export.connect_clicked(move |_| choose_export_path(&parent, project.clone()));
    }
    add_photos.connect_clicked(move |_| on_add_photos());
    close.connect_clicked(move |_| on_close());

    // The controls column is the collage editor's only min-height
    // contributor. Wrap it in a scrolled window so short windows (e.g.
    // 1366x768) never overflow the shared bottom bar; the column scrolls
    // instead while remaining fully expanded when space allows.
    let controls_scroll = gtk::ScrolledWindow::new();
    controls_scroll.set_hscrollbar_policy(gtk::PolicyType::Never);
    controls_scroll.set_vscrollbar_policy(gtk::PolicyType::Automatic);
    controls_scroll.set_propagate_natural_width(true);
    controls_scroll.set_propagate_natural_height(true);
    controls_scroll.set_min_content_width(320);
    controls_scroll.set_child(Some(&controls));
    // Same adjustable split pane as the edit mode tools panel: users can
    // drag the divider to give the controls or the canvas more room, the
    // panel starts at the same 360px width and never shrinks below 320px.
    let body = gtk::Paned::new(gtk::Orientation::Horizontal);
    body.set_hexpand(true);
    body.set_vexpand(true);
    body.set_position(360);
    body.set_resize_start_child(false);
    body.set_shrink_start_child(false);
    body.set_wide_handle(true);
    body.set_start_child(Some(&controls_scroll));
    body.set_end_child(Some(&aspect_frame));
    root.append(&body);
    CollageEditor {
        root,
        project,
        canvas,
        frames,
        status,
        hooks,
    }
}

fn add_section_label(parent: &gtk::Box, text: &str) {
    // Same section heading construction as the edit mode panels: small,
    // bold, dimmed uppercase text (pass the text already uppercased).
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.set_margin_top(8);
    label.add_css_class("heading");
    label.add_css_class("edit-section-label");
    parent.append(&label);
}

fn photo_count_text(count: usize) -> String {
    if count == 1 {
        "1 photo".to_string()
    } else {
        format!("{count} photos")
    }
}

fn icon_label_button(icon: &str, label: &str, tooltip: &str) -> gtk::Button {
    let button = gtk::Button::new();
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    content.set_halign(gtk::Align::Center);
    content.append(&gtk::Image::from_icon_name(icon));
    content.append(&gtk::Label::new(Some(label)));
    button.set_child(Some(&content));
    button.set_tooltip_text(Some(tooltip));
    button
}

/// Borderless menu entry for the per-tile popover menu.
fn flat_menu_button(icon: &str, label: &str) -> gtk::Button {
    let button = gtk::Button::new();
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    content.set_halign(gtk::Align::Center);
    content.append(&gtk::Image::from_icon_name(icon));
    content.append(&gtk::Label::new(Some(label)));
    button.set_child(Some(&content));
    button.add_css_class("flat");
    button
}

/// A small title above a control, for compact side-by-side rows where a
/// full-width section label would waste a row.
fn titled_control(title: &str, control: &impl IsA<gtk::Widget>) -> gtk::Box {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 3);
    let label = gtk::Label::new(Some(title));
    label.set_halign(gtk::Align::Start);
    label.add_css_class("dim-label");
    label.add_css_class("caption");
    column.append(&label);
    column.append(control);
    column
}

fn refresh_preview(
    canvas: &gtk::Fixed,
    frames: &Rc<RefCell<Vec<PreviewFrame>>>,
    project: &Rc<RefCell<CollageProject>>,
    hooks: &TileHooks,
) {
    let generation = PREVIEW_GENERATION.with(|cell| {
        let value = cell.get().wrapping_add(1);
        cell.set(value);
        value
    });
    while let Some(child) = canvas.first_child() {
        canvas.remove(&child);
    }
    frames.borrow_mut().clear();
    canvas.remove_css_class("collage-white");
    canvas.remove_css_class("collage-black");
    canvas.remove_css_class("collage-light-gray");
    let project_data = project.borrow();
    canvas.add_css_class(match project_data.background {
        Background::White => "collage-white",
        Background::Black => "collage-black",
        Background::LightGray => "collage-light-gray",
    });

    for (index, item) in project_data.items.iter().enumerate() {
        let frame = gtk::Frame::new(None);
        frame.add_css_class("collage-photo");
        // In Contain mode GtkPicture leaves the letterbox transparent. Put
        // that background on the rounded, clipping tile rather than on the
        // picture so its corners are part of the tile too.
        frame.add_css_class(match project_data.background {
            Background::White => "collage-white",
            Background::Black => "collage-black",
            Background::LightGray => "collage-light-gray",
        });
        frame.set_overflow(if project_data.round_corners {
            gtk::Overflow::Hidden
        } else {
            gtk::Overflow::Visible
        });
        if project_data.round_corners {
            frame.add_css_class("collage-photo-rounded");
        }
        // Keep the widget positioned by GtkFixed separate from the widget
        // that may receive a layout rotation transform.
        let inner = gtk::Fixed::new();
        inner.set_hexpand(true);
        inner.set_vexpand(true);
        let tile = gtk::Overlay::new();
        tile.set_child(Some(&inner));
        let picture = gtk::Picture::new();
        // The containing frame is sized to the fitted rectangle in
        // update_geometry, so the picture itself can always cover it.
        picture.set_content_fit(gtk::ContentFit::Cover);
        picture.set_can_shrink(true);
        picture.set_hexpand(true);
        picture.set_vexpand(true);
        if let Some(path) = item
            .photo
            .thumbnail_path
            .as_deref()
            .filter(|path| std::path::Path::new(path).is_file())
        {
            if let Some(rotated) = crate::photo_texture::edited_thumbnail(
                path,
                item.photo.library_rotation,
                &item.photo.edit_recipe,
            ) {
                picture.set_paintable(Some(&rotated));
            } else {
                picture.set_filename(Some(path));
            }
        } else {
            picture.set_paintable(gtk::gdk::Paintable::NONE);
            picture.set_tooltip_text(Some("Thumbnail unavailable"));
        }
        // The cached thumbnail is only 320 px; for the canvas tile decode a
        // sharper source from the original off-thread and swap it in.
        schedule_preview_sharpen(
            picture.clone(),
            item.photo.path.clone(),
            item.photo.library_rotation,
            item.photo.edit_recipe.clone(),
            generation,
        );
        let photo_frame = gtk::Frame::new(None);
        photo_frame.add_css_class("collage-contained-photo");
        photo_frame.set_child(Some(&picture));
        inner.put(&photo_frame, 0.0, 0.0);
        let offline_badge = gtk::Label::new(Some("!"));
        offline_badge.set_halign(gtk::Align::Start);
        offline_badge.set_valign(gtk::Align::Start);
        offline_badge.set_margin_top(8);
        offline_badge.set_margin_start(8);
        offline_badge.add_css_class("offline-badge");
        offline_badge.set_tooltip_text(Some("Original photo offline"));
        offline_badge.set_visible(!crate::source::folder_available(item.photo.folder_id));
        tile.add_overlay(&offline_badge);
        frame.set_child(Some(&tile));
        canvas.put(&frame, 0.0, 0.0);
        frame.set_cursor_from_name(Some("grab"));
        let drag = gtk::GestureDrag::new();
        let drag_start = Rc::new(Cell::new((0.0, 0.0)));
        let drop_target = Rc::new(Cell::new(None::<usize>));
        let frame_for_begin = frame.clone();
        let drop_target_for_begin = drop_target.clone();
        let drag_start_for_begin = drag_start.clone();
        drag.connect_drag_begin(move |_, start_x, start_y| {
            drag_start_for_begin.set((start_x, start_y));
            drop_target_for_begin.set(None);
            frame_for_begin.set_cursor_from_name(Some("grabbing"));
            frame_for_begin.add_css_class("collage-dragging");
        });
        let project_for_update = project.clone();
        let canvas_for_update = canvas.clone();
        let frames_for_update = frames.clone();
        let drop_target_for_update = drop_target.clone();
        let drag_start_for_update = drag_start.clone();
        drag.connect_drag_update(move |_, offset_x, offset_y| {
            let width = canvas_for_update.width().max(1) as f64;
            let height = canvas_for_update.height().max(1) as f64;
            let (start_x, start_y) = drag_start_for_update.get();
            let project = project_for_update.borrow();
            let Some(source) = project.items.get(index) else {
                return;
            };
            let pointer_x = source.x as f64 * width + start_x + offset_x;
            let pointer_y = source.y as f64 * height + start_y + offset_y;
            let normalized_x = pointer_x / width;
            let normalized_y = pointer_y / height;
            let target = project
                .items
                .iter()
                .enumerate()
                .rev()
                .find(|(candidate, item)| {
                    *candidate != index
                        && normalized_x >= item.x as f64
                        && normalized_x <= (item.x + item.width) as f64
                        && normalized_y >= item.y as f64
                        && normalized_y <= (item.y + item.height) as f64
                })
                .map(|(candidate, _)| candidate);
            let previous = drop_target_for_update.replace(target);
            if previous != target {
                if let Some(previous) = previous {
                    if let Some(frame) = frames_for_update.borrow().get(previous) {
                        frame.outer.remove_css_class("collage-drop-target");
                    }
                }
                if let Some(target) = target {
                    if let Some(frame) = frames_for_update.borrow().get(target) {
                        frame.outer.add_css_class("collage-drop-target");
                    }
                }
            }
        });
        let project_for_end = project.clone();
        let canvas_for_end = canvas.clone();
        let frames_for_end = frames.clone();
        let frame_for_end = frame.clone();
        let drop_target_for_end = drop_target.clone();
        drag.connect_drag_end(move |_, offset_x, offset_y| {
            frame_for_end.set_cursor_from_name(Some("grab"));
            frame_for_end.remove_css_class("collage-dragging");
            if let Some(target) = drop_target_for_end.take() {
                if let Some(frame) = frames_for_end.borrow().get(target) {
                    frame.outer.remove_css_class("collage-drop-target");
                }
            }
            let width = canvas_for_end.width().max(1) as f64;
            let height = canvas_for_end.height().max(1) as f64;
            let (start_x, start_y) = drag_start.get();
            let mut project = project_for_end.borrow_mut();
            let Some(source) = project.items.get(index) else {
                return;
            };
            let pointer_x = source.x as f64 * width + start_x + offset_x;
            let pointer_y = source.y as f64 * height + start_y + offset_y;
            let normalized_x = pointer_x / width;
            let normalized_y = pointer_y / height;
            let target = project
                .items
                .iter()
                .enumerate()
                .rev()
                .find(|(candidate, item)| {
                    *candidate != index
                        && normalized_x >= item.x as f64
                        && normalized_x <= (item.x + item.width) as f64
                        && normalized_y >= item.y as f64
                        && normalized_y <= (item.y + item.height) as f64
                })
                .map(|(candidate, _)| candidate);
            if let Some(target) = target {
                project.swap_item_positions(index, target);
            }
            update_geometry(&canvas_for_end, &frames_for_end, &project);
        });
        frame.add_controller(drag);
        // Right-click a tile for per-photo actions. The drag gesture above
        // only claims button 1, so the two controllers coexist.
        let menu = gtk::GestureClick::new();
        menu.set_button(3);
        {
            let project = project.clone();
            let canvas = canvas.clone();
            let frames = frames.clone();
            let frame_for_menu = frame.clone();
            let open_edit = hooks.open_edit.clone();
            let hooks = hooks.clone();
            let photo_id = item.photo.id;
            menu.connect_pressed(move |_, _, x, y| {
                let popover = gtk::Popover::new();
                let menu_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
                menu_box.set_margin_top(6);
                menu_box.set_margin_bottom(6);
                menu_box.set_margin_start(6);
                menu_box.set_margin_end(6);
                let edit_photo = flat_menu_button("document-edit-symbolic", "Edit Photo");
                let rotate_cw =
                    flat_menu_button("object-rotate-right-symbolic", "Rotate Clockwise");
                let rotate_ccw =
                    flat_menu_button("object-rotate-left-symbolic", "Rotate Counter-Clockwise");
                let remove = flat_menu_button("edit-delete-symbolic", "Remove Photo");
                menu_box.append(&edit_photo);
                menu_box.append(&rotate_cw);
                menu_box.append(&rotate_ccw);
                menu_box.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
                menu_box.append(&remove);
                popover.set_child(Some(&menu_box));
                popover.set_parent(&frame_for_menu);
                let pointing = gtk::gdk::Rectangle::new(x as i32 - 8, y as i32 - 8, 16, 16);
                popover.set_pointing_to(Some(&pointing));
                popover.connect_closed(|popover| popover.unparent());
                {
                    let open_edit = open_edit.clone();
                    let popover = popover.clone();
                    edit_photo.connect_clicked(move |_| {
                        popover.popdown();
                        open_edit(photo_id);
                    });
                }
                {
                    let project = project.clone();
                    let canvas = canvas.clone();
                    let frames = frames.clone();
                    let popover = popover.clone();
                    rotate_cw.connect_clicked(move |_| {
                        {
                            let mut project_data = project.borrow_mut();
                            if let Some(item) = project_data.items.get_mut(index) {
                                item.rotation = (item.rotation + 90.0) % 360.0;
                            }
                        }
                        update_geometry(&canvas, &frames, &project.borrow());
                        popover.popdown();
                    });
                }
                {
                    let project = project.clone();
                    let canvas = canvas.clone();
                    let frames = frames.clone();
                    let popover = popover.clone();
                    rotate_ccw.connect_clicked(move |_| {
                        {
                            let mut project_data = project.borrow_mut();
                            if let Some(item) = project_data.items.get_mut(index) {
                                item.rotation = (item.rotation + 270.0) % 360.0;
                            }
                        }
                        update_geometry(&canvas, &frames, &project.borrow());
                        popover.popdown();
                    });
                }
                {
                    let project = project.clone();
                    let canvas = canvas.clone();
                    let frames = frames.clone();
                    let popover = popover.clone();
                    let hooks = hooks.clone();
                    remove.connect_clicked(move |_| {
                        {
                            let mut project_data = project.borrow_mut();
                            if index < project_data.items.len() {
                                project_data.items.remove(index);
                                project_data.relayout();
                            }
                        }
                        refresh_preview(&canvas, &frames, &project, &hooks);
                        popover.popdown();
                    });
                }
                popover.popup();
            });
        }
        frame.add_controller(menu);
        frames.borrow_mut().push(PreviewFrame {
            outer: frame.upcast(),
            inner,
            photo: photo_frame,
        });
    }

    update_geometry(canvas, frames, &project_data);
}

fn update_geometry(
    canvas: &gtk::Fixed,
    frames: &Rc<RefCell<Vec<PreviewFrame>>>,
    project: &CollageProject,
) {
    let (width, height) = preview_geometry_size(canvas);
    if width < 1.0 || height < 1.0 {
        return; // Wait for the first real allocation; never invent a minimum.
    }
    for (item, preview) in project.items.iter().zip(frames.borrow().iter()) {
        let x = item.x * width;
        let y = item.y * height;
        let w = (item.width * width).round().clamp(1.0, width) as i32;
        let h = (item.height * height).round().clamp(1.0, height) as i32;
        for radius in 0..=MAX_PREVIEW_CORNER_RADIUS {
            preview
                .outer
                .remove_css_class(&format!("collage-photo-radius-{radius}"));
            preview
                .photo
                .remove_css_class(&format!("collage-photo-radius-{radius}"));
        }
        if project.round_corners {
            let radius = (w.min(h) as f32 * project.corner_radius)
                .round()
                .clamp(0.0, MAX_PREVIEW_CORNER_RADIUS as f32) as i32;
            preview
                .outer
                .add_css_class(&format!("collage-photo-radius-{radius}"));
        }
        preview.outer.set_size_request(w, h);
        let contain = matches!(project.layout, LayoutKind::Mosaic | LayoutKind::SmartMosaic)
            && project.keep_photo_aspect;
        let source_ratio = item.photo.aspect_ratio.max(0.01);
        let tile_ratio = w as f32 / h.max(1) as f32;
        let (photo_w, photo_h) = if contain && source_ratio > tile_ratio {
            (w, ((w as f32 / source_ratio).round() as i32).clamp(1, h))
        } else if contain {
            (((h as f32 * source_ratio).round() as i32).clamp(1, w), h)
        } else {
            (w, h)
        };
        let photo_x = (w - photo_w) / 2;
        let photo_y = (h - photo_h) / 2;
        preview.photo.set_size_request(photo_w, photo_h);
        preview
            .inner
            .move_(&preview.photo, photo_x as f64, photo_y as f64);
        preview
            .photo
            .set_overflow(if project.round_corners && contain {
                gtk::Overflow::Hidden
            } else {
                gtk::Overflow::Visible
            });
        if project.round_corners && contain {
            let radius = (photo_w.min(photo_h) as f32 * project.corner_radius)
                .round()
                .clamp(0.0, MAX_PREVIEW_CORNER_RADIUS as f32) as i32;
            preview
                .photo
                .add_css_class(&format!("collage-photo-radius-{radius}"));
        }
        canvas.move_(&preview.outer, x as f64, y as f64);
        if item.rotation.abs() > f32::EPSILON {
            let transform = gtk::gsk::Transform::new()
                .translate(&gtk::graphene::Point::new(w as f32 / 2.0, h as f32 / 2.0))
                .rotate(item.rotation)
                .translate(&gtk::graphene::Point::new(
                    -(w as f32 / 2.0),
                    -(h as f32 / 2.0),
                ));
            preview
                .inner
                .set_child_transform(&preview.photo, Some(&transform));
        } else {
            preview.inner.set_child_transform(&preview.photo, None);
        }
    }
}

// The canvas contains explicit tile sizes, but none may contribute to the
// editor/window minimum. The ordinary child measures zero; the overlay canvas
// receives exactly the allocation that GtkAspectFrame has fitted to its parent.
fn preview_bounds(canvas: &gtk::Fixed) -> gtk::Overlay {
    let bounds = gtk::Overlay::new();
    bounds.set_child(Some(&gtk::DrawingArea::new()));
    bounds.add_overlay(canvas);
    bounds.set_measure_overlay(canvas, false);
    bounds.set_clip_overlay(canvas, true);
    bounds.connect_get_child_position(|bounds, _| {
        Some(gtk::gdk::Rectangle::new(
            0,
            0,
            bounds.width(),
            bounds.height(),
        ))
    });
    bounds
}

fn preview_geometry_size(canvas: &gtk::Fixed) -> (f32, f32) {
    // The allocation already accounts for headers, sidebar, controls, margins
    // and canvas CSS. Do not feed a whole-window height estimate back into it.
    (canvas.width().max(0) as f32, canvas.height().max(0) as f32)
}

#[cfg(test)]
mod sizing_tests {
    use super::*;
    use crate::collage::model::{CollageItem, CollagePhoto};

    #[test]
    #[ignore = "requires a GTK display; run with --ignored --test-threads=1"]
    fn spacing_changes_do_not_propagate_preview_minimum_or_rebuild_tiles() {
        adw::init().expect("GTK display required");
        let window = gtk::Window::new();
        let editor = build(
            &window,
            Vec::new(),
            Rc::new(|| {}),
            Rc::new(|| {}),
            None,
            Rc::new(|_| {}),
        );
        editor.project.borrow_mut().items = (0..9)
            .map(|i| CollageItem {
                photo: CollagePhoto {
                    id: i,
                    folder_id: None,
                    path: String::new(),
                    filename: String::new(),
                    thumbnail_path: None,
                    library_rotation: 0,
                    edit_recipe: String::new(),
                    aspect_ratio: if i % 2 == 0 { 1.5 } else { 0.65 },
                },
                x: 0.0,
                y: 0.0,
                width: 1.0,
                height: 1.0,
                rotation: 0.0,
                z: i as usize,
            })
            .collect();
        editor.project.borrow_mut().relayout();
        refresh_preview(
            &editor.canvas,
            &editor.frames,
            &editor.project,
            &editor.hooks,
        );
        let tiles = editor
            .frames
            .borrow()
            .iter()
            .map(|f| f.outer.clone())
            .collect::<Vec<_>>();
        // The controls column lives in the start pane of the adjustable
        // split, inside a ScrolledWindow (its tall minimum must not leak
        // into the editor/window request). Descend through pane -> scroll
        // -> viewport -> controls box to find the scale.
        let controls = editor
            .root
            .first_child()
            .unwrap()
            .downcast::<gtk::Paned>()
            .unwrap()
            .start_child()
            .unwrap()
            .downcast::<gtk::ScrolledWindow>()
            .unwrap()
            .child()
            .and_then(|child| child.downcast::<gtk::Viewport>().ok())
            .and_then(|viewport| viewport.child())
            .expect("collage controls box");
        let mut child = controls.first_child();
        let spacing = loop {
            let widget = child.expect("Spacing control");
            if widget
                .clone()
                .downcast::<gtk::Label>()
                .is_ok_and(|l| l.text() == "SPACING")
            {
                break widget
                    .next_sibling()
                    .unwrap()
                    .downcast::<gtk::Scale>()
                    .unwrap();
            }
            child = widget.next_sibling();
        };
        let toast = adw::ToastOverlay::new();
        toast.set_child(Some(&editor.root));
        // Regression: the controls column must not leak its tall minimum
        // into the editor's size request. A 1366x768 window leaves roughly
        // 660px for the page stack once the header bar and the shared 58px
        // bottom bar are subtracted; GtkStack requests the max over all
        // pages (hidden ones included), so an unbounded editor minimum
        // pushes the bottom bar offscreen. The column scrolls instead.
        let (min_height, ..) = toast.measure(gtk::Orientation::Vertical, 1200);
        assert!(
            min_height <= 200,
            "collage editor minimum height {min_height} must stay bounded so short windows keep the bottom bar visible"
        );
        let (min_width, ..) = toast.measure(gtk::Orientation::Horizontal, -1);
        assert!(
            min_width <= 1200,
            "collage editor minimum width {min_width} must fit a 1366px window"
        );
        let baseline = toast.measure(gtk::Orientation::Vertical, 1200).0;
        for (width, height) in [(1200, 800), (900, 720), (1400, 850)] {
            toast.allocate(width, height, -1, None);
            // Manual allocation has no frame clock; perform the resize tick
            // that the live canvas runs before painting each new allocation.
            update_geometry(&editor.canvas, &editor.frames, &editor.project.borrow());
            assert!(editor.canvas.width() > 0 && editor.canvas.height() > 0);
            for step in (0..=20).chain((0..20).rev()) {
                spacing.set_value(step as f64 * 0.004);
                toast.allocate(width, height, -1, None);
                assert_eq!(toast.measure(gtk::Orientation::Vertical, width).0, baseline);
                assert!(editor.canvas.height() <= height);
                assert!(editor.canvas.width() <= width);
                for (frame, original) in editor.frames.borrow().iter().zip(&tiles) {
                    assert_eq!(&frame.outer, original);
                    assert!(frame.outer.width_request() <= editor.canvas.width());
                    assert!(
                        frame.outer.height_request() <= editor.canvas.height(),
                        "step={step} bounds={width}x{height} requested={} canvas={}x{}",
                        frame.outer.height_request(),
                        editor.canvas.width(),
                        editor.canvas.height()
                    );
                }
            }
        }
        window.close();
    }
}

/// Export options dialog (format, quality, size), then the destination
/// file chooser, then the background render with a completion dialog.
fn choose_export_path(window: &gtk::Window, project: Rc<RefCell<CollageProject>>) {
    let dialog = gtk::Dialog::new();
    dialog.set_title(Some("Export Collage"));
    dialog.set_transient_for(Some(window));
    dialog.set_modal(true);
    dialog.add_button("Cancel", gtk::ResponseType::Cancel);
    let export_button = dialog.add_button("Export", gtk::ResponseType::Ok);
    export_button.add_css_class("suggested-action");
    dialog.set_default_response(gtk::ResponseType::Ok);

    let content = dialog.content_area();
    content.set_spacing(12);
    content.set_margin_top(18);
    content.set_margin_bottom(6);
    content.set_margin_start(18);
    content.set_margin_end(18);

    let grid = gtk::Grid::new();
    grid.set_row_spacing(12);
    grid.set_column_spacing(12);

    fn grid_label(text: &str) -> gtk::Label {
        let label = gtk::Label::new(Some(text));
        label.set_xalign(0.0);
        label
    }

    // Format: JPEG (default) or PNG.
    let jpeg_toggle = gtk::ToggleButton::with_label("JPEG");
    let png_toggle = gtk::ToggleButton::with_label("PNG");
    png_toggle.set_group(Some(&jpeg_toggle));
    let format_row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    format_row.add_css_class("linked");
    format_row.append(&jpeg_toggle);
    format_row.append(&png_toggle);
    grid.attach(&grid_label("Format"), 0, 0, 1, 1);
    grid.attach(&format_row, 1, 0, 1, 1);

    // Quality: JPEG only, dimmed for PNG.
    let quality_scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 60.0, 100.0, 1.0);
    quality_scale.set_value(92.0);
    quality_scale.set_digits(0);
    quality_scale.set_draw_value(false);
    quality_scale.set_hexpand(true);
    let quality_value = gtk::Label::new(Some("92"));
    quality_value.set_width_request(30);
    quality_value.add_css_class("dim-label");
    quality_value.add_css_class("edit-adjustment-value");
    let quality_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    quality_row.append(&quality_scale);
    quality_row.append(&quality_value);
    grid.attach(&grid_label("Quality"), 0, 1, 1, 1);
    grid.attach(&quality_row, 1, 1, 1, 1);
    {
        let quality_scale = quality_scale.clone();
        let quality_value = quality_value.clone();
        quality_scale.connect_value_changed(move |scale| {
            quality_value.set_text(&format!("{:.0}", scale.value()));
        });
        let quality_scale = quality_scale.clone();
        png_toggle.connect_toggled(move |toggle| {
            quality_scale.set_sensitive(!toggle.is_active());
        });
    }

    // Long-edge size presets; the render clamps to the collage aspect.
    let size = gtk::DropDown::from_strings(&[
        "4K (3840 px)",
        "2048 px",
        "Full HD (1920 px)",
        "HD (1280 px)",
    ]);
    size.set_selected(0);
    grid.attach(&grid_label("Size"), 0, 2, 1, 1);
    grid.attach(&size, 1, 2, 1, 1);

    content.append(&grid);
    {
        let dialog_for_response = dialog.clone();
        let window = window.clone();
        let project = project.clone();
        let png_toggle = png_toggle.clone();
        let quality_scale = quality_scale.clone();
        let size = size.clone();
        dialog.connect_response(move |_, response| {
            if response != gtk::ResponseType::Ok {
                dialog_for_response.destroy();
                return;
            }
            let options = super::render::ExportOptions {
                max_edge: match size.selected() {
                    1 => 2048,
                    2 => 1920,
                    3 => 1280,
                    _ => 3840,
                },
                format: if png_toggle.is_active() {
                    super::render::ExportFormat::Png
                } else {
                    super::render::ExportFormat::Jpeg
                },
                jpeg_quality: quality_scale.value().round().clamp(60.0, 100.0) as u8,
            };
            dialog_for_response.destroy();
            choose_export_destination(&window, project.clone(), options);
        });
    }
    dialog.show();
}

fn choose_export_destination(
    window: &gtk::Window,
    project: Rc<RefCell<CollageProject>>,
    options: super::render::ExportOptions,
) {
    let extension = match options.format {
        super::render::ExportFormat::Jpeg => "jpg",
        super::render::ExportFormat::Png => "png",
    };
    let dialog = gtk::FileChooserNative::new(
        Some("Export Collage"),
        Some(window),
        gtk::FileChooserAction::Save,
        Some("Export"),
        Some("Cancel"),
    );
    dialog.set_current_name(&format!("collage.{extension}"));
    let filter = gtk::FileFilter::new();
    filter.set_name(Some(match options.format {
        super::render::ExportFormat::Jpeg => "JPEG image",
        super::render::ExportFormat::Png => "PNG image",
    }));
    filter.add_pattern(&format!("*.{extension}"));
    dialog.add_filter(&filter);
    let window = window.clone();
    dialog.connect_response(move |dialog, response| {
        if response != gtk::ResponseType::Accept {
            return;
        }
        let Some(path) = dialog.file().and_then(|file| file.path()) else {
            return;
        };
        let path = if path.extension().is_none() {
            path.with_extension(extension)
        } else {
            path
        };
        let project = project.borrow().clone();
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let result = super::render::export(&project, &path, &options);
            let _ = sender.send(result.map_err(|error| error.to_string()));
        });
        let window = window.clone();
        glib::timeout_add_local(Duration::from_millis(50), move || {
            match receiver.try_recv() {
                Ok(Ok(())) => {
                    let dialog = adw::AlertDialog::builder()
                        .heading("Collage exported")
                        .body("The collage was saved successfully.")
                        .close_response("close")
                        .build();
                    dialog.add_response("close", "Close");
                    dialog.present(Some(&window));
                    glib::ControlFlow::Break
                }
                Ok(Err(error)) => {
                    let dialog = adw::AlertDialog::builder()
                        .heading("Could not export collage")
                        .body(error)
                        .close_response("close")
                        .build();
                    dialog.add_response("close", "Close");
                    dialog.present(Some(&window));
                    glib::ControlFlow::Break
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
            }
        });
    });
    dialog.show();
}
