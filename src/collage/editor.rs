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
         .collage-photo-rounded { }",
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

pub struct CollageEditor {
    pub root: gtk::Box,
    project: Rc<RefCell<CollageProject>>,
    canvas: gtk::Fixed,
    frames: Rc<RefCell<Vec<PreviewFrame>>>,
    status: gtk::Label,
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
            .set_text(&format!("{} photos", self.project.borrow().items.len()));
        refresh_preview(&self.canvas, &self.frames, &self.project);
    }

    pub fn set_photos(&self, photos: Vec<crate::photo_object::PhotoObject>) {
        self.project.borrow_mut().set_photos(photos);
        self.status
            .set_text(&format!("{} photos", self.project.borrow().items.len()));
        refresh_preview(&self.canvas, &self.frames, &self.project);
    }
}

pub fn build(
    parent: &gtk::Window,
    photos: Vec<crate::photo_object::PhotoObject>,
    on_add_photos: Rc<dyn Fn()>,
    on_close: Rc<dyn Fn()>,
) -> CollageEditor {
    let css = gtk::CssProvider::new();
    let css_data = collage_css();
    css.load_from_data(&css_data);
    gtk::style_context_add_provider_for_display(
        &gtk::prelude::WidgetExt::display(parent),
        &css,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
    let project = Rc::new(RefCell::new(CollageProject::new(photos)));

    let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    root.set_hexpand(true);
    root.set_vexpand(true);
    let controls = gtk::Box::new(gtk::Orientation::Vertical, 12);
    controls.set_width_request(230);
    controls.set_margin_top(18);
    controls.set_margin_bottom(18);
    controls.set_margin_start(18);
    controls.set_margin_end(18);

    let heading = gtk::Label::new(Some("Collage"));
    heading.set_halign(gtk::Align::Start);
    heading.add_css_class("title-2");
    controls.append(&heading);
    let status = gtk::Label::new(Some(&format!("{} photos", project.borrow().items.len())));
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
        Rc::new(move || refresh_preview(&canvas, &frames, &project))
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

    add_section_label(&controls, "Layout");
    let layout = gtk::DropDown::from_strings(&["Mosaic", "Smart Mosaic", "Grid"]);
    layout.set_selected(match project.borrow().layout {
        LayoutKind::Mosaic => 0,
        LayoutKind::SmartMosaic => 1,
        LayoutKind::Grid => 2,
    });
    let orientation = gtk::DropDown::from_strings(&["Landscape", "Portrait"]);
    orientation
        .set_selected(matches!(project.borrow().orientation, CollageOrientation::Portrait) as u32);
    let layout_controls = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    layout_controls.append(&layout);
    layout_controls.append(&orientation);
    controls.append(&layout_controls);
    let keep_photo_aspect = gtk::CheckButton::with_label("Keep photo aspect ratio");
    keep_photo_aspect.set_active(project.borrow().keep_photo_aspect);
    keep_photo_aspect.set_visible(matches!(
        project.borrow().layout,
        LayoutKind::Mosaic | LayoutKind::SmartMosaic
    ));
    controls.append(&keep_photo_aspect);
    {
        let project = project.clone();
        let refresh = refresh.clone();
        let keep_photo_aspect = keep_photo_aspect.clone();
        layout.connect_selected_notify(move |dropdown| {
            let layout_kind = match dropdown.selected() {
                0 => LayoutKind::Mosaic,
                1 => LayoutKind::SmartMosaic,
                _ => LayoutKind::Grid,
            };
            let mut project_data = project.borrow_mut();
            project_data.layout = layout_kind;
            project_data.relayout();
            drop(project_data);
            keep_photo_aspect.set_visible(matches!(
                layout_kind,
                LayoutKind::Mosaic | LayoutKind::SmartMosaic
            ));
            refresh();
        });
    }
    {
        let project = project.clone();
        let refresh = refresh.clone();
        keep_photo_aspect.connect_toggled(move |button| {
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

    add_section_label(&controls, "Aspect ratio");
    let aspect = gtk::DropDown::from_strings(&["Square 1:1", "4:3", "3:2", "16:9", "Custom"]);
    aspect.set_selected(match project.borrow().aspect {
        AspectRatio::Square => 0,
        AspectRatio::FourThree => 1,
        AspectRatio::ThreeTwo => 2,
        AspectRatio::SixteenNine => 3,
        AspectRatio::Custom => 4,
    });
    controls.append(&aspect);
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
    {
        let project = project.clone();
        let refresh = refresh.clone();
        let aspect_frame = aspect_frame.clone();
        orientation.connect_selected_notify(move |dropdown| {
            let orientation = if dropdown.selected() == 1 {
                CollageOrientation::Portrait
            } else {
                CollageOrientation::Landscape
            };
            let ratio = {
                let mut project_data = project.borrow_mut();
                project_data.orientation = orientation;
                project_data.relayout();
                project_data.effective_aspect_ratio()
            };
            aspect_frame.set_ratio(ratio);
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

    add_section_label(&controls, "Background");
    let background = gtk::DropDown::from_strings(&["White", "Black", "Light Grey"]);
    background.set_selected(match project.borrow().background {
        Background::White => 0,
        Background::Black => 1,
        Background::LightGray => 2,
    });
    controls.append(&background);
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

    add_section_label(&controls, "Photo corners");
    let round_corners = gtk::CheckButton::with_label("Round corners");
    round_corners.set_active(project.borrow().round_corners);
    controls.append(&round_corners);
    let corner_radius = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 0.12, 0.005);
    corner_radius.set_value(project.borrow().corner_radius as f64);
    corner_radius.set_digits(3);
    corner_radius.set_draw_value(false);
    corner_radius.set_sensitive(project.borrow().round_corners);
    corner_radius.set_tooltip_text(Some("Corner radius"));
    controls.append(&corner_radius);
    {
        let project = project.clone();
        let refresh = refresh.clone();
        let corner_radius = corner_radius.clone();
        round_corners.connect_toggled(move |button| {
            project.borrow_mut().round_corners = button.is_active();
            corner_radius.set_sensitive(button.is_active());
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

    add_section_label(&controls, "Spacing");
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

    let reset_defaults = gtk::Button::with_label("Reset defaults");
    reset_defaults.set_width_request(120);
    reset_defaults.set_height_request(30);
    reset_defaults.set_tooltip_text(Some("Restore default collage settings"));
    {
        let project = project.clone();
        let layout = layout.clone();
        let orientation = orientation.clone();
        let aspect = aspect.clone();
        let custom_width = custom_width.clone();
        let custom_height = custom_height.clone();
        let background = background.clone();
        let round_corners = round_corners.clone();
        let corner_radius = corner_radius.clone();
        let spacing = spacing.clone();
        let aspect_frame = aspect_frame.clone();
        let custom_ratio = custom_ratio.clone();
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
            layout.set_selected(1);
            orientation.set_selected(0);
            aspect.set_selected(3);
            custom_width.set_value(16.0);
            custom_height.set_value(9.0);
            background.set_selected(0);
            round_corners.set_active(false);
            corner_radius.set_value(0.06);
            spacing.set_value(0.018);
            custom_ratio.set_visible(false);
            aspect_frame.set_ratio(16.0 / 9.0);
            let mut project_data = project.borrow_mut();
            project_data.relayout();
        });
    }

    let shuffle = gtk::Button::with_label("Shuffle");
    shuffle.set_sensitive(true);
    shuffle.set_hexpand(true);
    let reset_shuffle_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    reset_shuffle_row.set_hexpand(true);
    reset_shuffle_row.append(&reset_defaults);
    reset_shuffle_row.append(&shuffle);
    controls.append(&reset_shuffle_row);
    {
        let project = project.clone();
        let refresh = refresh.clone();
        shuffle.connect_clicked(move |_| {
            project.borrow_mut().shuffle();
            refresh();
        });
    }

    let export = gtk::Button::with_label("Create / Export Collage…");
    export.add_css_class("suggested-action");
    controls.append(&export);
    let add_photos = gtk::Button::with_label("Add Photos…");
    controls.append(&add_photos);
    let close = gtk::Button::with_label("Exit Collage");
    controls.append(&close);
    controls.append(&gtk::Box::new(gtk::Orientation::Vertical, 0));
    {
        let project = project.clone();
        let parent = parent.clone();
        export.connect_clicked(move |_| choose_export_path(&parent, project.clone()));
    }
    add_photos.connect_clicked(move |_| on_add_photos());
    close.connect_clicked(move |_| on_close());

    root.append(&controls);
    root.append(&aspect_frame);
    CollageEditor {
        root,
        project,
        canvas,
        frames,
        status,
    }
}

fn add_section_label(parent: &gtk::Box, text: &str) {
    let label = gtk::Label::new(Some(text));
    label.set_halign(gtk::Align::Start);
    label.add_css_class("heading");
    parent.append(&label);
}

fn refresh_preview(
    canvas: &gtk::Fixed,
    frames: &Rc<RefCell<Vec<PreviewFrame>>>,
    project: &Rc<RefCell<CollageProject>>,
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
        offline_badge.set_visible(!crate::source::cached_file_available(&item.photo.path));
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
        frames.borrow_mut().push(PreviewFrame {
            outer: frame.upcast(),
            inner,
            photo: photo_frame,
        });
    }
    if std::env::var_os("PICASA_TRACE").is_some() {
        let mut child_count = 0;
        let mut child = canvas.first_child();
        while let Some(current) = child {
            child_count += 1;
            child = current.next_sibling();
        }
        eprintln!(
            "COLLAGE TRACE preview items={} widgets={} canvas={}x{}",
            project_data.items.len(),
            child_count,
            canvas.width(),
            canvas.height()
        );
        for item in &project_data.items {
            eprintln!(
                "COLLAGE TRACE item id={} x={:.4} y={:.4} w={:.4} h={:.4} rotation={:.2}",
                item.photo.id, item.x, item.y, item.width, item.height, item.rotation
            );
        }
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
        let editor = build(&window, Vec::new(), Rc::new(|| {}), Rc::new(|| {}));
        editor.project.borrow_mut().items = (0..9)
            .map(|i| CollageItem {
                photo: CollagePhoto {
                    id: i,
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
        refresh_preview(&editor.canvas, &editor.frames, &editor.project);
        let tiles = editor
            .frames
            .borrow()
            .iter()
            .map(|f| f.outer.clone())
            .collect::<Vec<_>>();
        let controls = editor.root.first_child().unwrap();
        let mut child = controls.first_child();
        let spacing = loop {
            let widget = child.expect("Spacing control");
            if widget
                .clone()
                .downcast::<gtk::Label>()
                .is_ok_and(|l| l.text() == "Spacing")
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

fn choose_export_path(window: &gtk::Window, project: Rc<RefCell<CollageProject>>) {
    let dialog = gtk::FileChooserNative::new(
        Some("Export Collage"),
        Some(window),
        gtk::FileChooserAction::Save,
        Some("Export"),
        Some("Cancel"),
    );
    dialog.set_current_name("collage.jpg");
    let window = window.clone();
    dialog.connect_response(move |dialog, response| {
        if response != gtk::ResponseType::Accept {
            return;
        }
        let Some(path) = dialog.file().and_then(|file| file.path()) else {
            return;
        };
        let path = if path.extension().is_none() {
            path.with_extension("jpg")
        } else {
            path
        };
        let project = project.borrow().clone();
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let result = super::render::export(&project, &path);
            let _ = sender.send(result.map_err(|error| error.to_string()));
        });
        let window = window.clone();
        glib::timeout_add_local(Duration::from_millis(50), move || {
            match receiver.try_recv() {
                Ok(Ok(())) => {
                    let dialog = adw::AlertDialog::builder()
                        .heading("Collage exported")
                        .body("The JPEG collage was saved successfully.")
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
