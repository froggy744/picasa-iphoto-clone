use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;
use rusqlite::Connection;

use super::model::{CropRect, EditRecipe, EditSession};

pub struct EditEditor {
    pub root: gtk::Box,
    photo_id: i64,
}

impl EditEditor {
    pub fn photo_id(&self) -> i64 {
        self.photo_id
    }
}

#[derive(Clone)]
struct Controls {
    straighten: gtk::Scale,
    exposure: gtk::Scale,
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
}

impl Controls {
    fn sync(&self, recipe: &EditRecipe) {
        self.straighten.set_value(recipe.straighten as f64);
        self.exposure.set_value(recipe.exposure as f64);
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
    let reset = gtk::Button::with_label("Reset");
    reset.set_tooltip_text(Some("Reset all edits to the original"));
    toolbar.append(&reset);

    let tools_toggle = gtk::ToggleButton::with_label("Tools");
    tools_toggle.set_active(true);
    tools_toggle.set_tooltip_text(Some("Show or hide editing controls"));
    toolbar.append(&tools_toggle);

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
    let tools_scroll = gtk::ScrolledWindow::new();
    tools_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    tools_scroll.set_width_request(315);
    tools_scroll.set_child(Some(&tools_box));
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
    preview_area.set_child(Some(&picture));

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

    let recipe = EditRecipe::decode(&photo.edit_recipe());
    let session = Rc::new(RefCell::new(EditSession::new(recipe)));
    let syncing = Rc::new(Cell::new(false));
    let pending_crop = Rc::new(RefCell::new(CropRect::default()));
    let preview_dimensions = Rc::new(Cell::new((1i32, 1i32)));
    let generation = Rc::new(Cell::new(0u64));
    let preview_debounce: Rc<RefCell<Option<glib::SourceId>>> = Rc::new(RefCell::new(None));

    add_section_label(&tools_box, "Basic fixes");
    let auto_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let auto_contrast = gtk::ToggleButton::with_label("Auto Contrast");
    let auto_color = gtk::ToggleButton::with_label("Auto Colour");
    auto_contrast.set_hexpand(true);
    auto_color.set_hexpand(true);
    auto_row.append(&auto_contrast);
    auto_row.append(&auto_color);
    tools_box.append(&auto_row);

    let effect_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let black_white = gtk::ToggleButton::with_label("B&W");
    let sepia = gtk::ToggleButton::with_label("Sepia");
    black_white.set_hexpand(true);
    sepia.set_hexpand(true);
    effect_row.append(&black_white);
    effect_row.append(&sepia);
    tools_box.append(&effect_row);

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
    let fill_light = add_slider(&tools_box, "Fill Light", -1.0, 1.0, 0.02, 2);
    let highlights = add_slider(&tools_box, "Highlights", -1.0, 1.0, 0.02, 2);
    let shadows = add_slider(&tools_box, "Shadows", -1.0, 1.0, 0.02, 2);
    let temperature = add_slider(&tools_box, "Colour Temperature", -1.0, 1.0, 0.02, 2);
    let saturation = add_slider(&tools_box, "Saturation", -1.0, 1.0, 0.02, 2);
    let sharpen = add_slider(&tools_box, "Sharpen", 0.0, 1.0, 0.02, 2);

    let controls = Controls {
        straighten,
        exposure,
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
    };
    controls.sync(&session.borrow().recipe);

    // Ordinary wheel scrolling over a slider should continue scrolling the
    // tools panel. Only the narrow track area in the middle of the scale keeps
    // the normal GTK wheel-to-adjust behavior.
    for scale in [
        &controls.straighten,
        &controls.exposure,
        &controls.fill_light,
        &controls.highlights,
        &controls.shadows,
        &controls.temperature,
        &controls.saturation,
        &controls.sharpen,
    ] {
        configure_scale_scroll(scale, &tools_scroll);
    }

    let queue_preview: Rc<dyn Fn()> = {
        let session = session.clone();
        let picture = picture.clone();
        let status = status.clone();
        let busy = busy.clone();
        let photo = photo.clone();
        let generation = generation.clone();
        let preview_dimensions = preview_dimensions.clone();
        let crop_overlay = crop_overlay.clone();
        let preview_debounce = preview_debounce.clone();
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
            let crop_overlay = crop_overlay.clone();
            let preview_debounce_for_fire = preview_debounce.clone();
            let source = glib::timeout_add_local_once(Duration::from_millis(90), move || {
                preview_debounce_for_fire.borrow_mut().take();
                let current_generation = generation.get().wrapping_add(1);
                generation.set(current_generation);
                let recipe = session.borrow().recipe.clone();
                let path = photo.path();
                let rotation = photo.rotation();
                busy.set_visible(true);
                busy.start();
                status.set_text("Rendering preview…");
                let (sender, receiver) = std::sync::mpsc::channel();
                std::thread::spawn(move || {
                    let result = super::render::render_for_viewer(
                        &path,
                        rotation,
                        &recipe,
                        1800,
                        1400,
                    )
                    .map(|image| (image.width(), image.height(), image.into_raw()));
                    let _ = sender.send(result);
                });
                let picture = picture.clone();
                let status = status.clone();
                let busy = busy.clone();
                let generation = generation.clone();
                let preview_dimensions = preview_dimensions.clone();
                let crop_overlay = crop_overlay.clone();
                glib::timeout_add_local(Duration::from_millis(25), move || {
                    let Ok(result) = receiver.try_recv() else {
                        return glib::ControlFlow::Continue;
                    };
                    if generation.get() != current_generation {
                        return glib::ControlFlow::Break;
                    }
                    busy.stop();
                    busy.set_visible(false);
                    match result {
                        Ok((width, height, pixels)) => {
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
                            status.set_text(&format!("{} × {} preview", width, height));
                            crop_overlay.queue_draw();
                        }
                        Err(error) => status.set_text(&format!("Preview failed: {error}")),
                    }
                    glib::ControlFlow::Break
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
        session.clone(), syncing.clone(), queue_preview.clone(), update_history_buttons.clone(),
        |recipe, value| recipe.straighten = value,
    );
    connect_scale(&controls.exposure, session.clone(), syncing.clone(), queue_preview.clone(), update_history_buttons.clone(), |r, v| r.exposure = v);
    connect_scale(&controls.fill_light, session.clone(), syncing.clone(), queue_preview.clone(), update_history_buttons.clone(), |r, v| r.fill_light = v);
    connect_scale(&controls.highlights, session.clone(), syncing.clone(), queue_preview.clone(), update_history_buttons.clone(), |r, v| r.highlights = v);
    connect_scale(&controls.shadows, session.clone(), syncing.clone(), queue_preview.clone(), update_history_buttons.clone(), |r, v| r.shadows = v);
    connect_scale(&controls.temperature, session.clone(), syncing.clone(), queue_preview.clone(), update_history_buttons.clone(), |r, v| r.temperature = v);
    connect_scale(&controls.saturation, session.clone(), syncing.clone(), queue_preview.clone(), update_history_buttons.clone(), |r, v| r.saturation = v);
    connect_scale(&controls.sharpen, session.clone(), syncing.clone(), queue_preview.clone(), update_history_buttons.clone(), |r, v| r.sharpen = v);

    connect_toggle(&controls.auto_contrast, session.clone(), syncing.clone(), queue_preview.clone(), update_history_buttons.clone(), |r, v| r.auto_contrast = v);
    connect_toggle(&controls.auto_color, session.clone(), syncing.clone(), queue_preview.clone(), update_history_buttons.clone(), |r, v| r.auto_color = v);
    connect_toggle(&controls.black_white, session.clone(), syncing.clone(), queue_preview.clone(), update_history_buttons.clone(), |r, v| r.black_white = v);
    connect_toggle(&controls.sepia, session.clone(), syncing.clone(), queue_preview.clone(), update_history_buttons.clone(), |r, v| r.sepia = v);

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
        reset.connect_clicked(move |_| {
            session.borrow_mut().reset();
            sync_controls();
            update_history_buttons();
            crop_overlay.set_visible(false);
            crop_actions.set_visible(false);
            queue_preview();
        });
    }

    {
        let tools_scroll = tools_scroll.clone();
        tools_toggle.connect_toggled(move |button| tools_scroll.set_visible(button.is_active()));
    }

    {
        let crop_overlay = crop_overlay.clone();
        let crop_actions = crop_actions.clone();
        let pending_crop = pending_crop.clone();
        crop.connect_clicked(move |_| {
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
            session.borrow_mut().mutate(|recipe| recipe.crop = recipe.crop.compose(child));
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
            session.borrow_mut().mutate(|recipe| recipe.crop = CropRect::default());
            pending_crop.replace(CropRect::default());
            crop_overlay.set_visible(false);
            crop_actions.set_visible(false);
            update_history_buttons();
            queue_preview();
        });
    }

    configure_crop_overlay(&crop_overlay, pending_crop.clone(), preview_dimensions.clone());

    {
        let on_close = on_close.clone();
        back.connect_clicked(move |_| on_close());
    }
    {
        let session = session.clone();
        let photo = photo.clone();
        let parent = parent.clone();
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
            let rotation = photo.rotation();
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
            if let Err(error) = crate::db::set_edit_recipe(&connection.borrow(), photo.id(), &encoded) {
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

    EditEditor {
        root,
        photo_id: photo.id(),
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
            session.borrow_mut().mutate_active(|recipe| assign(recipe, value));
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
                (adjustment.value() + dy.signum() * step * 2.0)
                    .clamp(adjustment.lower(), max),
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
        let (x, y, w, h) = contained_rect(width as f64, height as f64, image_width as f64, image_height as f64);
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
            pending.replace(CropRect {
                left: sx.min(cx) as f32,
                top: sy.min(cy) as f32,
                right: sx.max(cx) as f32,
                bottom: sy.max(cy) as f32,
            }.normalized());
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
    Some((((px - x) / width).clamp(0.0, 1.0), ((py - y) / height).clamp(0.0, 1.0)))
}

fn contained_rect(container_w: f64, container_h: f64, image_w: f64, image_h: f64) -> (f64, f64, f64, f64) {
    if container_w <= 0.0 || container_h <= 0.0 || image_w <= 0.0 || image_h <= 0.0 {
        return (0.0, 0.0, container_w.max(0.0), container_h.max(0.0));
    }
    let scale = (container_w / image_w).min(container_h / image_h);
    let width = image_w * scale;
    let height = image_h * scale;
    ((container_w - width) * 0.5, (container_h - height) * 0.5, width, height)
}
