// Editor fragment included by editor.rs. Provides the Text panel tab for
// creating and editing text layers.

#[allow(clippy::too_many_arguments)]
fn build_text_panel(
    parent: &gtk::Box,
    session: Rc<RefCell<EditSession>>,
    selected: Rc<Cell<Option<usize>>>,
    syncing: Rc<Cell<bool>>,
    update_history_buttons: Rc<dyn Fn()>,
    redraw_canvas: Rc<dyn Fn()>,
    update_canvas_input: Rc<dyn Fn()>,
    text_toggle: gtk::ToggleButton,
    photo_dimensions: Rc<Cell<(i32, i32)>>,
) -> Rc<dyn Fn()> {
    use gtk::pango::prelude::*;

    let action_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let add_text = gtk::Button::with_label("Add Text");
    add_text.set_hexpand(true);
    add_text.set_tooltip_text(Some("Add a text layer on top of this photo"));
    add_text.add_css_class("crop-apply-button");
    let remove = gtk::Button::with_label("Remove Text");
    remove.set_hexpand(true);
    remove.set_sensitive(false);
    remove.set_tooltip_text(Some("Delete this text layer from the photo"));
    remove.add_css_class("crop-reset-button");
    action_row.append(&add_text);
    action_row.append(&remove);
    parent.append(&action_row);

    add_section_label(parent, "TEXT LAYERS");
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::Single);
    list.add_css_class("overlay-list");
    list.set_margin_bottom(6);
    parent.append(&list);

    let selected_section = gtk::Box::new(gtk::Orientation::Vertical, 0);
    selected_section.add_css_class("overlay-selected-section");
    parent.append(&selected_section);

    add_section_label(&selected_section, "TEXT CONTENT");
    let text_scroll = gtk::ScrolledWindow::new();
    text_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    text_scroll.set_min_content_height(76);
    let text_view = gtk::TextView::new();
    text_view.set_wrap_mode(gtk::WrapMode::WordChar);
    text_view.set_hexpand(true);
    text_view.set_top_margin(6);
    text_view.set_bottom_margin(6);
    text_view.set_left_margin(6);
    text_view.set_right_margin(6);
    text_scroll.set_child(Some(&text_view));
    selected_section.append(&text_scroll);

    let font_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    font_row.set_margin_top(6);
    let font_combo = gtk::ComboBoxText::new();
    font_combo.set_hexpand(true);
    font_combo.set_tooltip_text(Some("Font family"));
    let font_map = pangocairo::FontMap::new();
    let mut families = font_map
        .list_families()
        .into_iter()
        .map(|family| family.name().to_string())
        .collect::<Vec<_>>();
    families.sort_by_key(|name| name.to_lowercase());
    for family in &families {
        font_combo.append(Some(family), family);
    }
    font_combo.set_active(Some(0));
    let size_spin = gtk::SpinButton::with_range(0.5, 50.0, 0.5);
    size_spin.set_tooltip_text(Some("Font size as a percentage of the photo height"));
    font_row.append(&font_combo);
    font_row.append(&size_spin);
    selected_section.append(&font_row);

    let colour_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    colour_row.set_margin_top(10);
    let colour_label = gtk::Label::new(Some("Colour"));
    colour_label.set_xalign(0.0);
    colour_label.set_width_chars(6);
    colour_label.set_halign(gtk::Align::Start);
    let colour_button = gtk::ColorDialogButton::new(Some(gtk::ColorDialog::new()));
    colour_button.set_tooltip_text(Some("Choose custom colour"));
    colour_button.set_rgba(&gtk::gdk::RGBA::new(1.0, 1.0, 1.0, 1.0));
    colour_row.append(&colour_label);

    let colour_swatches = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    colour_swatches.set_hexpand(true);
    let mut swatch_buttons = Vec::new();
    for (name, class, color) in [
        ("White", "text-swatch-white", (1.0, 1.0, 1.0)),
        ("Black", "text-swatch-black", (0.05, 0.05, 0.05)),
        ("Red", "text-swatch-red", (0.95, 0.12, 0.12)),
        ("Orange", "text-swatch-orange", (1.0, 0.48, 0.0)),
        ("Yellow", "text-swatch-yellow", (1.0, 0.9, 0.05)),
        ("Green", "text-swatch-green", (0.12, 0.72, 0.28)),
        ("Blue", "text-swatch-blue", (0.15, 0.42, 1.0)),
        ("Purple", "text-swatch-purple", (0.68, 0.3, 0.88)),
    ] {
        let swatch = gtk::Button::new();
        swatch.set_hexpand(true);
        swatch.set_size_request(0, 30);
        swatch.set_tooltip_text(Some(name));
        swatch.add_css_class("text-colour-swatch");
        swatch.add_css_class(class);
        colour_swatches.append(&swatch);
        swatch_buttons.push((swatch, color));
    }
    let custom_swatch = gtk::Button::new();
    custom_swatch.set_hexpand(true);
    custom_swatch.set_size_request(0, 30);
    custom_swatch.set_tooltip_text(Some("Choose custom colour"));
    custom_swatch.add_css_class("text-colour-swatch");
    let custom_icon = gtk::Image::from_icon_name("color-select-symbolic");
    custom_icon.set_pixel_size(16);
    custom_swatch.set_child(Some(&custom_icon));
    colour_row.append(&colour_swatches);
    colour_swatches.append(&custom_swatch);
    selected_section.append(&colour_row);

    let style_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    style_row.set_margin_top(10);
    let bold_button = gtk::ToggleButton::with_label("Bold");
    bold_button.set_hexpand(true);
    bold_button.set_tooltip_text(Some("Bold"));
    let italic_button = gtk::ToggleButton::with_label("Italic");
    italic_button.set_hexpand(true);
    italic_button.set_tooltip_text(Some("Italic"));
    let style_group = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    style_group.add_css_class("linked");
    style_group.set_hexpand(true);
    style_group.append(&bold_button);
    style_group.append(&italic_button);
    let style_separator = gtk::Separator::new(gtk::Orientation::Vertical);
    style_separator.set_margin_start(4);
    style_separator.set_margin_end(4);
    let align_group = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    align_group.add_css_class("linked");
    align_group.add_css_class("overlay-anchor-row");
    let left_button = gtk::ToggleButton::with_label("Left");
    left_button.set_active(true);
    left_button.set_hexpand(true);
    let center_button = gtk::ToggleButton::with_label("Centre");
    center_button.set_group(Some(&left_button));
    center_button.set_hexpand(true);
    let right_button = gtk::ToggleButton::with_label("Right");
    right_button.set_group(Some(&left_button));
    right_button.set_hexpand(true);
    align_group.append(&left_button);
    align_group.append(&center_button);
    align_group.append(&right_button);
    let align_buttons = vec![
        (TextAlign::Left, left_button.clone()),
        (TextAlign::Center, center_button),
        (TextAlign::Right, right_button),
    ];
    style_row.append(&style_group);
    style_row.append(&style_separator);
    style_row.append(&align_group);
    selected_section.append(&style_row);

    let opacity_row = add_text_opacity_slider(&selected_section);

    let position_column = gtk::Box::new(gtk::Orientation::Vertical, 2);
    position_column.set_hexpand(false);
    add_section_label(&position_column, "POSITION");
    let anchor_buttons = build_text_position_grid(&position_column);
    let fit_column = gtk::Box::new(gtk::Orientation::Vertical, 6);
    fit_column.set_hexpand(true);
    add_section_label(&fit_column, "FIT");
    let fit_width = gtk::Button::with_label("Fit Width");
    fit_width.set_hexpand(true);
    fit_width.set_tooltip_text(Some("Scale text to span the photo width"));
    fit_width.add_css_class("crop-reset-button");
    let fit_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let reset_placement = gtk::Button::with_label("Reset Placement");
    reset_placement.set_hexpand(true);
    reset_placement.set_tooltip_text(Some("Centre this text and reset its position offsets"));
    reset_placement.add_css_class("crop-reset-button");
    let fit_screen = gtk::Button::with_label("Fit Screen");
    fit_screen.set_hexpand(true);
    fit_screen.set_tooltip_text(Some("Scale text to fit inside the photo"));
    fit_screen.add_css_class("crop-reset-button");
    fit_row.append(&fit_width);
    fit_row.append(&fit_screen);
    fit_column.append(&fit_row);
    fit_column.append(&reset_placement);
    let position_fit_row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    position_fit_row.set_margin_top(10);
    position_fit_row.append(&position_column);
    position_fit_row.append(&fit_column);
    selected_section.append(&position_fit_row);

    add_section_label(&selected_section, "OFFSET");
    let offset_x = gtk::SpinButton::with_range(-100000.0, 100000.0, 1.0);
    let offset_y = gtk::SpinButton::with_range(-100000.0, 100000.0, 1.0);
    offset_x.set_tooltip_text(Some(
        "Horizontal offset from the selected position, in photo pixels",
    ));
    offset_y.set_tooltip_text(Some(
        "Vertical offset from the selected position, in photo pixels",
    ));
    let offset_row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    offset_row.set_margin_top(4);
    offset_row.set_margin_bottom(10);
    let offset_cell_x = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    offset_cell_x.set_hexpand(true);
    let offset_label_x = gtk::Label::new(Some("X offset"));
    offset_label_x.set_xalign(0.0);
    offset_label_x.set_hexpand(true);
    offset_cell_x.append(&offset_label_x);
    offset_cell_x.append(&offset_x);
    offset_row.append(&offset_cell_x);
    let offset_cell_y = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    offset_cell_y.set_hexpand(true);
    let offset_label_y = gtk::Label::new(Some("Y offset"));
    offset_label_y.set_xalign(0.0);
    offset_label_y.set_hexpand(true);
    offset_cell_y.append(&offset_label_y);
    offset_cell_y.append(&offset_y);
    offset_row.append(&offset_cell_y);
    selected_section.append(&offset_row);

    let sync: Rc<dyn Fn()> = {
        let session = session.clone();
        let selected = selected.clone();
        let syncing = syncing.clone();
        let list = list.clone();
        let selected_section = selected_section.clone();
        let text_view = text_view.clone();
        let font_combo = font_combo.clone();
        let colour_button = colour_button.clone();
        let size_spin = size_spin.clone();
        let bold_button = bold_button.clone();
        let italic_button = italic_button.clone();
        let align_buttons = align_buttons.clone();
        let opacity_row = opacity_row.clone();
        let anchor_buttons = anchor_buttons.clone();
        let remove = remove.clone();
        let offset_x = offset_x.clone();
        let offset_y = offset_y.clone();
        let photo_dimensions = photo_dimensions.clone();
        let redraw_canvas = redraw_canvas.clone();
        let update_canvas_input = update_canvas_input.clone();
        Rc::new(move || {
            syncing.set(true);
            let layers = session.borrow().recipe.text_layers.clone();
            if selected.get().is_some_and(|index| index >= layers.len()) {
                selected.set(if layers.is_empty() { None } else { Some(0) });
            }
            if selected.get().is_none() && !layers.is_empty() {
                selected.set(Some(0));
            }
            let active = selected.get();
            remove.set_sensitive(active.is_some());
            selected_section.set_visible(true);
            selected_section.set_sensitive(active.is_some());
            list.set_visible(true);

            while let Some(child) = list.first_child() {
                list.remove(&child);
            }
            for (index, layer) in layers.iter().enumerate() {
                let row = gtk::ListBoxRow::new();
                let label = gtk::Label::new(None);
                label.set_xalign(0.0);
                label.set_hexpand(true);
                let first_line = layer.text.lines().next().unwrap_or("");
                let mut preview = first_line.chars().take(40).collect::<String>();
                if first_line.chars().count() > 40 {
                    preview.push('…');
                }
                let name = if preview.is_empty() {
                    "Text".to_string()
                } else {
                    format!("Text: {preview}")
                };
                let visibility = if layer.visible { "" } else { " · hidden" };
                label.set_text(&format!("{name}{visibility}"));
                row.set_child(Some(&label));
                list.append(&row);
                if Some(index) == active {
                    list.select_row(Some(&row));
                }
            }

            if let Some(layer) = active.and_then(|index| layers.get(index)) {
                let buffer = text_view.buffer();
                let start = buffer.start_iter();
                let end = buffer.end_iter();
                let current = buffer.text(&start, &end, false);
                if current.as_str() != layer.text {
                    buffer.set_text(&layer.text);
                }
                size_spin.set_value(f64::from(layer.size) * 100.0);
                bold_button.set_active(layer.bold);
                italic_button.set_active(layer.italic);
                for (align, button) in &align_buttons {
                    button.set_active(*align == layer.align);
                }
                font_combo.set_active_id(Some(&layer.font_family));
                if font_combo.active_id().as_deref() != Some(layer.font_family.as_str()) {
                    font_combo.append(Some(&layer.font_family), &layer.font_family);
                    font_combo.set_active_id(Some(&layer.font_family));
                }
                let (red, green, blue, alpha) = layer.color_rgba();
                colour_button.set_rgba(&gtk::gdk::RGBA::new(red, green, blue, alpha));
                opacity_row.set_value(f64::from(layer.opacity));
                for (anchor, button) in &anchor_buttons {
                    let (width, height) = photo_dimensions.get();
                    let (col, row, x_offset, y_offset) = text_grid_position(layer, width, height);
                    button.set_active((col, row) == *anchor);
                    offset_x.set_value(x_offset);
                    offset_y.set_value(y_offset);
                }
            } else {
                colour_button.set_rgba(&gtk::gdk::RGBA::new(1.0, 1.0, 1.0, 1.0));
                opacity_row.set_value(1.0);
                for ((col, row), button) in &anchor_buttons {
                    button.set_active((*col, *row) == (1, 1));
                }
                offset_x.set_value(0.0);
                offset_y.set_value(0.0);
            }
            syncing.set(false);
            redraw_canvas();
            update_canvas_input();
        })
    };

    {
        let selected = selected.clone();
        let syncing = syncing.clone();
        let sync = sync.clone();
        list.connect_row_selected(move |_, row| {
            if syncing.get() {
                return;
            }
            // Rebuilding list rows during sync can emit a delayed `None`
            // notification. A single-selection list cannot be user-cleared,
            // so only a real row should update the active text layer.
            let Some(row) = row else {
                if crate::diagnostics::trace_enabled() {
                    eprintln!(
                        "PIC_TEXT list_selection_cleared_preserved selected={:?}",
                        selected.get()
                    );
                }
                return;
            };
            let index = Some(row.index() as usize);
            if selected.get() == index {
                return;
            }
            selected.set(index);
            sync();
        });
    }

    {
        let session = session.clone();
        let selected = selected.clone();
        let update_history_buttons = update_history_buttons.clone();
        let sync = sync.clone();
        let text_toggle = text_toggle.clone();
        add_text.connect_clicked(move |_| {
            session.borrow_mut().mutate(|recipe| {
                recipe.text_layers.push(TextLayerSpec::new_default());
            });
            let index = session.borrow().recipe.text_layers.len().saturating_sub(1);
            selected.set(Some(index));
            text_toggle.set_active(true);
            update_history_buttons();
            sync();
        });
    }

    {
        let session = session.clone();
        let selected = selected.clone();
        let update_history_buttons = update_history_buttons.clone();
        let sync = sync.clone();
        remove.connect_clicked(move |_| {
            let Some(index) = selected.get() else {
                return;
            };
            session.borrow_mut().mutate(move |recipe| {
                if index < recipe.text_layers.len() {
                    recipe.text_layers.remove(index);
                }
            });
            selected.set(None);
            update_history_buttons();
            sync();
        });
    }

    {
        let session = session.clone();
        let selected = selected.clone();
        let syncing = syncing.clone();
        let update_history_buttons = update_history_buttons.clone();
        let redraw_canvas = redraw_canvas.clone();
        let sync = sync.clone();
        let typing = Rc::new(Cell::new(false));
        let typing_source: Rc<RefCell<Option<glib::SourceId>>> = Rc::new(RefCell::new(None));
        text_view.buffer().connect_changed(move |buffer| {
            if syncing.get() {
                return;
            }
            let Some(index) = selected.get() else {
                return;
            };
            let text = {
                let start = buffer.start_iter();
                let end = buffer.end_iter();
                buffer.text(&start, &end, false).to_string()
            };
            {
                let mut session = session.borrow_mut();
                if !typing.get() {
                    typing.set(true);
                    session.begin_action();
                }
                session.mutate_active(move |recipe| {
                    if let Some(layer) = recipe.text_layers.get_mut(index) {
                        layer.text = text.clone();
                    }
                });
            }
            redraw_canvas();
            if let Some(previous) = typing_source.borrow_mut().take() {
                previous.remove();
            }
            let session = session.clone();
            let typing = typing.clone();
            let typing_source_for_timer = typing_source.clone();
            let update_history_buttons = update_history_buttons.clone();
            let sync = sync.clone();
            let source = glib::timeout_add_local(Duration::from_millis(700), move || {
                if typing.replace(false) {
                    session.borrow_mut().end_action();
                    update_history_buttons();
                    sync();
                }
                let _ = typing_source_for_timer.borrow_mut().take();
                glib::ControlFlow::Break
            });
            typing_source.borrow_mut().replace(source);
        });
    }

    {
        let session = session.clone();
        let selected = selected.clone();
        let syncing = syncing.clone();
        let update_history_buttons = update_history_buttons.clone();
        let redraw_canvas = redraw_canvas.clone();
        font_combo.connect_changed(move |combo| {
            if syncing.get() {
                return;
            }
            let Some(index) = selected.get() else {
                return;
            };
            let Some(family) = combo.active_id() else {
                return;
            };
            let family = family.to_string();
            session.borrow_mut().mutate(move |recipe| {
                if let Some(layer) = recipe.text_layers.get_mut(index) {
                    layer.font_family = family.clone();
                }
            });
            update_history_buttons();
            redraw_canvas();
        });
    }

    {
        let session = session.clone();
        let selected = selected.clone();
        let syncing = syncing.clone();
        let update_history_buttons = update_history_buttons.clone();
        let redraw_canvas = redraw_canvas.clone();
        size_spin.connect_value_changed(move |spin| {
            if syncing.get() {
                return;
            }
            let Some(index) = selected.get() else {
                return;
            };
            let size = (spin.value() / 100.0) as f32;
            let size = size.clamp(TextLayerSpec::MIN_SIZE, TextLayerSpec::MAX_SIZE);
            session.borrow_mut().mutate(move |recipe| {
                if let Some(layer) = recipe.text_layers.get_mut(index) {
                    layer.size = size;
                }
            });
            update_history_buttons();
            redraw_canvas();
        });
    }

    {
        let session = session.clone();
        let selected = selected.clone();
        let syncing = syncing.clone();
        let update_history_buttons = update_history_buttons.clone();
        let redraw_canvas = redraw_canvas.clone();
        colour_button.connect_rgba_notify(move |button| {
            if syncing.get() {
                return;
            }
            let Some(index) = selected.get() else {
                return;
            };
            let rgba = button.rgba();
            let (red, green, blue, alpha) = (rgba.red(), rgba.green(), rgba.blue(), rgba.alpha());
            session.borrow_mut().mutate(move |recipe| {
                if let Some(layer) = recipe.text_layers.get_mut(index) {
                    layer.set_color_rgba(red, green, blue, alpha);
                }
            });
            update_history_buttons();
            redraw_canvas();
        });
    }

    for (swatch, color) in swatch_buttons {
        let colour_button = colour_button.clone();
        swatch.connect_clicked(move |_| {
            colour_button.set_rgba(&gtk::gdk::RGBA::new(color.0, color.1, color.2, 1.0));
        });
    }

    {
        let colour_button = colour_button.clone();
        let chooser = gtk::ColorDialog::new();
        let colour_row = colour_row.clone();
        custom_swatch.connect_clicked(move |_| {
            let initial = colour_button.rgba();
            let dialog = chooser.clone();
            let result_button = colour_button.clone();
            let parent_window = colour_row
                .root()
                .and_then(|root| root.downcast::<gtk::Window>().ok());
            if let Some(parent_window) = parent_window.as_ref() {
                dialog.choose_rgba(
                    Some(parent_window),
                    Some(&initial),
                    None::<&gtk::gio::Cancellable>,
                    move |result| {
                        if let Ok(color) = result {
                            result_button.set_rgba(&color);
                        }
                    },
                );
            } else {
                dialog.choose_rgba(
                    None::<&gtk::Window>,
                    Some(&initial),
                    None::<&gtk::gio::Cancellable>,
                    move |result| {
                        if let Ok(color) = result {
                            result_button.set_rgba(&color);
                        }
                    },
                );
            }
        });
    }

    for (button, apply) in [(bold_button.clone(), true), (italic_button.clone(), false)] {
        let session = session.clone();
        let selected = selected.clone();
        let syncing = syncing.clone();
        let update_history_buttons = update_history_buttons.clone();
        let redraw_canvas = redraw_canvas.clone();
        button.connect_toggled(move |button| {
            if syncing.get() {
                return;
            }
            let Some(index) = selected.get() else {
                return;
            };
            let active = button.is_active();
            session.borrow_mut().mutate(move |recipe| {
                let Some(layer) = recipe.text_layers.get_mut(index) else {
                    return;
                };
                if apply {
                    layer.bold = active;
                } else {
                    layer.italic = active;
                }
            });
            update_history_buttons();
            redraw_canvas();
        });
    }

    for (align, button) in &align_buttons {
        let align = *align;
        let session = session.clone();
        let selected = selected.clone();
        let syncing = syncing.clone();
        let update_history_buttons = update_history_buttons.clone();
        let redraw_canvas = redraw_canvas.clone();
        button.connect_toggled(move |button| {
            if syncing.get() || !button.is_active() {
                return;
            }
            let Some(index) = selected.get() else {
                return;
            };
            session.borrow_mut().mutate(move |recipe| {
                if let Some(layer) = recipe.text_layers.get_mut(index) {
                    layer.align = align;
                }
            });
            update_history_buttons();
            redraw_canvas();
        });
    }

    {
        let session = session.clone();
        let selected = selected.clone();
        let syncing = syncing.clone();
        let update_history_buttons = update_history_buttons.clone();
        let redraw_canvas = redraw_canvas.clone();
        let dragging = Rc::new(Cell::new(false));
        let press = gtk::GestureClick::new();
        press.set_button(1);
        press.set_propagation_phase(gtk::PropagationPhase::Capture);
        {
            let session = session.clone();
            let selected = selected.clone();
            let dragging = dragging.clone();
            press.connect_pressed(move |_, _, _, _| {
                if selected.get().is_some() {
                    dragging.set(true);
                    session.borrow_mut().begin_action();
                }
            });
        }
        {
            let session = session.clone();
            let dragging = dragging.clone();
            let update_history_buttons = update_history_buttons.clone();
            let redraw_canvas = redraw_canvas.clone();
            press.connect_released(move |_, _, _, _| {
                if dragging.replace(false) {
                    session.borrow_mut().end_action();
                    update_history_buttons();
                    redraw_canvas();
                }
            });
        }
        opacity_row.add_controller(press);
        opacity_row.connect_value_changed(move |scale| {
            if syncing.get() {
                return;
            }
            let Some(index) = selected.get() else {
                return;
            };
            let value = (scale.value() as f32).clamp(0.0, 1.0);
            let was_dragging = dragging.get();
            {
                let mut session = session.borrow_mut();
                if was_dragging {
                    session.mutate_active(move |recipe| {
                        if let Some(layer) = recipe.text_layers.get_mut(index) {
                            layer.opacity = value;
                        }
                    });
                } else {
                    session.mutate(move |recipe| {
                        if let Some(layer) = recipe.text_layers.get_mut(index) {
                            layer.opacity = value;
                        }
                    });
                }
            }
            if !was_dragging {
                update_history_buttons();
            }
            redraw_canvas();
        });
    }

    for ((col, row), button) in &anchor_buttons {
        let (col, row) = (*col, *row);
        let session = session.clone();
        let selected = selected.clone();
        let syncing = syncing.clone();
        let update_history_buttons = update_history_buttons.clone();
        let redraw_canvas = redraw_canvas.clone();
        let photo_dimensions = photo_dimensions.clone();
        button.connect_toggled(move |button| {
            if syncing.get() || !button.is_active() {
                return;
            }
            let Some(index) = selected.get() else {
                return;
            };
            let dims = photo_dimensions.get();
            session.borrow_mut().mutate(move |recipe| {
                let Some(layer) = recipe.text_layers.get_mut(index) else {
                    return;
                };
                place_text_grid(layer, dims, col, row, 0.0, 0.0);
            });
            update_history_buttons();
            redraw_canvas();
        });
    }

    for (spin, horizontal) in [(offset_x.clone(), true), (offset_y.clone(), false)] {
        let session = session.clone();
        let selected = selected.clone();
        let syncing = syncing.clone();
        let update_history_buttons = update_history_buttons.clone();
        let redraw_canvas = redraw_canvas.clone();
        let photo_dimensions = photo_dimensions.clone();
        let offset_x = offset_x.clone();
        let offset_y = offset_y.clone();
        spin.connect_value_changed(move |spin| {
            if syncing.get() {
                return;
            }
            let Some(index) = selected.get() else { return };
            let dims = photo_dimensions.get();
            let x_offset = if horizontal {
                spin.value()
            } else {
                offset_x.value()
            };
            let y_offset = if horizontal {
                offset_y.value()
            } else {
                spin.value()
            };
            session.borrow_mut().mutate(move |recipe| {
                if let Some(layer) = recipe.text_layers.get_mut(index) {
                    let (col, row, _, _) = text_grid_position(layer, dims.0, dims.1);
                    place_text_grid(layer, dims, col, row, x_offset, y_offset);
                }
            });
            update_history_buttons();
            redraw_canvas();
        });
    }

    for (button, fit_screen_mode) in [(&fit_width, false), (&fit_screen, true)] {
        let session = session.clone();
        let selected = selected.clone();
        let dimensions = photo_dimensions.clone();
        let update_history_buttons = update_history_buttons.clone();
        let sync = sync.clone();
        button.connect_clicked(move |_| {
            let Some(index) = selected.get() else { return };
            let (photo_w, photo_h) = dimensions.get();
            if photo_w <= 0 || photo_h <= 0 {
                return;
            }
            session.borrow_mut().mutate(move |recipe| {
                let Some(layer) = recipe.text_layers.get_mut(index) else {
                    return;
                };
                let Some(metrics) = super::text_render::measure_text_px(layer, photo_h as f64)
                else {
                    return;
                };
                if metrics.width <= 0 || metrics.height <= 0 {
                    return;
                }
                let scale_x = photo_w as f64 / metrics.width as f64;
                let scale_y = photo_h as f64 / metrics.height as f64;
                let scale = if fit_screen_mode {
                    scale_x.min(scale_y)
                } else {
                    scale_x
                };
                layer.size = (f64::from(layer.size) * scale).clamp(
                    f64::from(TextLayerSpec::MIN_SIZE),
                    f64::from(TextLayerSpec::MAX_SIZE),
                ) as f32;
            });
            update_history_buttons();
            sync();
        });
    }

    {
        let session = session.clone();
        let selected = selected.clone();
        let update_history_buttons = update_history_buttons.clone();
        let sync = sync.clone();
        reset_placement.connect_clicked(move |_| {
            let Some(index) = selected.get() else {
                return;
            };
            session.borrow_mut().mutate(move |recipe| {
                if let Some(layer) = recipe.text_layers.get_mut(index) {
                    layer.anchor = super::model::OverlayAnchor::Center;
                    layer.x = 0.5;
                    layer.y = 0.5;
                }
            });
            update_history_buttons();
            sync();
        });
    }

    sync();
    sync
}

fn build_text_position_grid(parent: &gtk::Box) -> Vec<((u8, u8), gtk::ToggleButton)> {
    let grid = gtk::Grid::new();
    grid.add_css_class("text-position-grid");
    let mut buttons = Vec::with_capacity(9);
    let glyphs = [["↖", "↑", "↗"], ["←", "•", "→"], ["↙", "↓", "↘"]];
    for row in 0..3u8 {
        for col in 0..3u8 {
            let button = gtk::ToggleButton::with_label(glyphs[row as usize][col as usize]);
            button.set_size_request(34, 34);
            button.set_tooltip_text(Some(match (col, row) {
                (0, 0) => "Top left",
                (1, 0) => "Top centre",
                (2, 0) => "Top right",
                (0, 1) => "Centre left",
                (1, 1) => "Centre",
                (2, 1) => "Centre right",
                (0, 2) => "Bottom left",
                (1, 2) => "Bottom centre",
                _ => "Bottom right",
            }));
            if let Some((_, first)) = buttons.first() {
                button.set_group(Some(first));
            }
            grid.attach(&button, i32::from(col), i32::from(row), 1, 1);
            buttons.push(((col, row), button));
        }
    }
    if let Some((_, centre)) = buttons.get(4) {
        centre.set_active(true);
    }
    parent.append(&grid);
    buttons
}

fn text_grid_position(layer: &TextLayerSpec, photo_w: i32, photo_h: i32) -> (u8, u8, f64, f64) {
    let photo_w = photo_w.max(1) as f64;
    let photo_h = photo_h.max(1) as f64;
    let Some(rect) = super::text_render::text_rect(layer, photo_w, photo_h) else {
        return (1, 1, 0.0, 0.0);
    };
    let xs = [
        f64::from(rect.left),
        f64::from(rect.left + rect.width * 0.5),
        f64::from(rect.right()),
    ];
    let ys = [
        f64::from(rect.top),
        f64::from(rect.top + rect.height * 0.5),
        f64::from(rect.bottom()),
    ];
    let targets = [
        f64::from(TextLayerSpec::POSITION_MARGIN),
        0.5,
        1.0 - f64::from(TextLayerSpec::POSITION_MARGIN),
    ];
    let col = (0..3)
        .min_by(|a, b| {
            (xs[*a] - targets[*a])
                .abs()
                .total_cmp(&(xs[*b] - targets[*b]).abs())
        })
        .unwrap_or(1);
    let row = (0..3)
        .min_by(|a, b| {
            (ys[*a] - targets[*a])
                .abs()
                .total_cmp(&(ys[*b] - targets[*b]).abs())
        })
        .unwrap_or(1);
    (
        ((col) as u8),
        ((row) as u8),
        (xs[col] - targets[col]) * photo_w,
        (ys[row] - targets[row]) * photo_h,
    )
}

fn place_text_grid(
    layer: &mut TextLayerSpec,
    dims: (i32, i32),
    col: u8,
    row: u8,
    x_offset: f64,
    y_offset: f64,
) {
    let photo_w = dims.0.max(1) as f64;
    let photo_h = dims.1.max(1) as f64;
    let (width, height) = super::text_render::measure_text_px(layer, photo_h)
        .map(|metrics| {
            (
                metrics.width as f32 / photo_w as f32,
                metrics.height as f32 / photo_h as f32,
            )
        })
        .unwrap_or((0.0, 0.0));
    let margin = TextLayerSpec::POSITION_MARGIN;
    let target_x = [margin, 0.5, 1.0 - margin][usize::from(col.min(2))];
    let target_y = [margin, 0.5, 1.0 - margin][usize::from(row.min(2))];
    let left = match col {
        0 => target_x,
        1 => target_x - width * 0.5,
        _ => target_x - width,
    } + (x_offset / photo_w) as f32;
    let top = match row {
        0 => target_y,
        1 => target_y - height * 0.5,
        _ => target_y - height,
    } + (y_offset / photo_h) as f32;
    layer.anchor = super::model::OverlayAnchor::Center;
    layer.x = left + width * 0.5;
    layer.y = top + height * 0.5;
}

fn add_text_opacity_slider(parent: &gtk::Box) -> gtk::Scale {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.add_css_class("edit-adjustment-row");
    row.set_margin_top(10);
    let label = gtk::Label::new(Some("Opacity"));
    label.set_xalign(0.0);
    label.set_width_chars(8);
    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 0.01);
    scale.set_digits(2);
    scale.set_draw_value(false);
    scale.set_hexpand(true);
    scale.add_css_class("edit-adjustment-scale");
    let value = gtk::Label::new(Some("1.00"));
    value.set_xalign(1.0);
    value.set_width_chars(4);
    value.add_css_class("dim-label");
    value.add_css_class("edit-adjustment-value");
    let value_for_update = value.clone();
    scale.connect_value_changed(move |scale| {
        value_for_update.set_text(&format!("{:.2}", scale.value()))
    });
    row.append(&label);
    row.append(&scale);
    row.append(&value);
    parent.append(&row);
    scale
}

#[cfg(test)]
mod text_position_tests {
    use super::*;

    #[test]
    fn all_nine_position_cells_place_and_report_the_text_layer() {
        let dims = (2400, 1600);
        let margin = TextLayerSpec::POSITION_MARGIN;
        let targets = [margin, 0.5, 1.0 - margin];
        for row in 0..3 {
            for col in 0..3 {
                let mut layer = TextLayerSpec::new_default();
                place_text_grid(&mut layer, dims, col, row, 0.0, 0.0);
                let rect = super::super::text_render::text_rect(
                    &layer,
                    f64::from(dims.0),
                    f64::from(dims.1),
                )
                .unwrap();
                let x = [rect.left, rect.left + rect.width * 0.5, rect.right()][col as usize];
                let y = [rect.top, rect.top + rect.height * 0.5, rect.bottom()][row as usize];
                assert!((x - targets[col as usize]).abs() < 0.001);
                assert!((y - targets[row as usize]).abs() < 0.001);
                assert_eq!(
                    (
                        text_grid_position(&layer, dims.0, dims.1).0,
                        text_grid_position(&layer, dims.0, dims.1).1
                    ),
                    (col, row)
                );
            }
        }
    }

    #[test]
    fn offsets_are_photo_relative_and_survive_position_sync() {
        let dims = (2400, 1600);
        let mut layer = TextLayerSpec::new_default();
        place_text_grid(&mut layer, dims, 2, 0, 120.0, -50.0);
        let (col, row, x, y) = text_grid_position(&layer, dims.0, dims.1);
        assert_eq!((col, row), (2, 0));
        assert!((x - 120.0).abs() < 1.0);
        assert!((y + 50.0).abs() < 1.0);
    }
}
