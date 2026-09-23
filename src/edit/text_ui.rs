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
) -> Rc<dyn Fn()> {
    use gtk::pango::prelude::*;

    let add_text = gtk::Button::with_label("Add Text");
    add_text.set_hexpand(true);
    add_text.set_tooltip_text(Some("Add a text layer on top of this photo"));
    add_text.add_css_class("crop-apply-button");
    parent.append(&add_text);

    let empty_hint = gtk::Label::new(Some(
        "No text yet.\nAdd text, then edit it here or drag it into place on the photo.",
    ));
    empty_hint.set_xalign(0.0);
    empty_hint.set_justify(gtk::Justification::Left);
    empty_hint.add_css_class("dim-label");
    empty_hint.set_margin_top(8);
    parent.append(&empty_hint);

    add_section_label(parent, "TEXT LAYERS");
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::Single);
    list.add_css_class("overlay-list");
    list.set_margin_bottom(6);
    parent.append(&list);

    let selected_section = gtk::Box::new(gtk::Orientation::Vertical, 0);
    selected_section.add_css_class("overlay-selected-section");
    parent.append(&selected_section);

    add_section_label(&selected_section, "SELECTED TEXT");
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
    colour_row.set_margin_top(6);
    let colour_label = gtk::Label::new(Some("Colour"));
    colour_label.set_xalign(0.0);
    colour_label.set_hexpand(true);
    let colour_button = gtk::ColorDialogButton::new(Some(gtk::ColorDialog::new()));
    colour_button.set_tooltip_text(Some("Text colour"));
    colour_button.set_rgba(&gtk::gdk::RGBA::new(1.0, 1.0, 1.0, 1.0));
    colour_row.append(&colour_label);
    colour_row.append(&colour_button);
    selected_section.append(&colour_row);

    let style_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    style_row.set_margin_top(6);
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

    let opacity_row = add_slider(&selected_section, "Opacity", 0.0, 1.0, 0.01, 2);

    add_section_label(&selected_section, "POSITION");
    let (anchor_buttons, first_anchor) = build_position_row(&selected_section);

    let action_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    action_row.set_margin_top(6);
    let reset_placement = gtk::Button::with_label("Reset Placement");
    reset_placement.set_hexpand(true);
    reset_placement.set_tooltip_text(Some("Centre this text and restore full opacity"));
    reset_placement.add_css_class("crop-reset-button");
    let remove = gtk::Button::with_label("Remove Text");
    remove.set_hexpand(true);
    remove.set_tooltip_text(Some("Delete this text layer from the photo"));
    remove.add_css_class("crop-reset-button");
    action_row.append(&reset_placement);
    action_row.append(&remove);
    selected_section.append(&action_row);

    let sync: Rc<dyn Fn()> = {
        let session = session.clone();
        let selected = selected.clone();
        let syncing = syncing.clone();
        let list = list.clone();
        let empty_hint = empty_hint.clone();
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
        let first_anchor = first_anchor.clone();
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
            empty_hint.set_visible(layers.is_empty());
            selected_section.set_visible(!layers.is_empty());
            list.set_visible(!layers.is_empty());

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
                    button.set_active(*anchor == layer.anchor);
                }
            } else {
                colour_button.set_rgba(&gtk::gdk::RGBA::new(1.0, 1.0, 1.0, 1.0));
                opacity_row.set_value(1.0);
                first_anchor.set_active(true);
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
            let index = row.map(|row| row.index() as usize);
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

    for (anchor, button) in &anchor_buttons {
        let anchor = *anchor;
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
                let Some(layer) = recipe.text_layers.get_mut(index) else {
                    return;
                };
                layer.position_at(anchor);
            });
            update_history_buttons();
            redraw_canvas();
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
                    layer.reset_placement();
                }
            });
            update_history_buttons();
            sync();
        });
    }

    sync();
    sync
}
