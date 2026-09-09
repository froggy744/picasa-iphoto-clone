fn connect_type_to_search(
    source: &impl IsA<gtk::Widget>,
    search: &gtk::SearchEntry,
    begin: Rc<dyn Fn() -> bool>,
) {
    // Bubble so normal text editors and existing shortcuts get first use of
    // the event. Only unhandled printable input starts a fresh search.
    let keyboard = gtk::EventControllerKey::new();
    keyboard.set_propagation_phase(gtk::PropagationPhase::Bubble);
    let search_weak = search.downgrade();
    keyboard.connect_key_pressed(move |controller, key, _, modifiers| {
        if modifiers.intersects(
            gtk::gdk::ModifierType::CONTROL_MASK
                | gtk::gdk::ModifierType::ALT_MASK
                | gtk::gdk::ModifierType::SUPER_MASK
                | gtk::gdk::ModifierType::META_MASK,
        ) {
            return glib::Propagation::Proceed;
        }
        let Some(character) = key
            .to_unicode()
            .filter(|c| !c.is_control() && !c.is_whitespace())
        else {
            return glib::Propagation::Proceed;
        };
        let Some(search) = search_weak.upgrade() else {
            return glib::Propagation::Proceed;
        };
        let Some(root) = search.root() else {
            return glib::Propagation::Proceed;
        };
        let mut focus = root.focus();
        while let Some(widget) = focus {
            if widget.is::<gtk::Editable>()
                || widget.is::<gtk::TextView>()
                || widget.is::<gtk::Popover>()
                || widget.is::<adw::Dialog>()
            {
                return glib::Propagation::Proceed;
            }
            focus = widget.parent();
        }
        if !begin() || !search.grab_focus() {
            return glib::Propagation::Proceed;
        }
        search.set_text("");
        // Forward the original event to GtkText so GTK handles the keyboard
        // layout and inserts the first character exactly once.
        let handled = root
            .focus()
            .is_some_and(|target| controller.forward(&target));
        if !handled {
            search.set_text(&character.to_string());
            search.set_position(-1);
        }
        if std::env::var_os("PICASA_TRACE").is_some() {
            eprintln!(
                "SEARCH TRACE type_to_search started chars={}",
                search.text().chars().count()
            );
        }
        glib::Propagation::Stop
    });
    source.add_controller(keyboard);
}

fn folder_suggestion_popup(search: &gtk::SearchEntry) -> (gtk::Popover, gtk::ListBox) {
    let popover = gtk::Popover::new();
    popover.set_has_arrow(false);
    popover.set_position(gtk::PositionType::Bottom);
    // Autohide popovers are modal: they grab keyboard input from the entry.
    // Completion must let the user keep typing while matches are visible.
    popover.set_autohide(false);
    popover.set_can_focus(false);
    popover.set_parent(search);
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::Single);
    list.set_can_focus(false);
    list.add_css_class("navigation-sidebar");
    list.set_size_request(320, -1);
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroll.set_propagate_natural_height(true);
    scroll.set_max_content_height(300);
    scroll.set_child(Some(&list));
    popover.set_child(Some(&scroll));

    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let popup_weak = popover.downgrade();
    let list_weak = list.downgrade();
    let scroll_weak = scroll.downgrade();
    keys.connect_key_pressed(move |_, key, _, modifiers| {
        let (Some(popup), Some(list), Some(scroll)) = (
            popup_weak.upgrade(),
            list_weak.upgrade(),
            scroll_weak.upgrade(),
        ) else {
            return glib::Propagation::Proceed;
        };
        if !popup.is_visible()
            || !list.is_sensitive()
            || modifiers.intersects(
                gtk::gdk::ModifierType::CONTROL_MASK
                    | gtk::gdk::ModifierType::ALT_MASK
                    | gtk::gdk::ModifierType::SUPER_MASK,
            )
        {
            return glib::Propagation::Proceed;
        }
        match key {
            gtk::gdk::Key::Down | gtk::gdk::Key::Up => {
                let last = list.last_child().and_downcast::<gtk::ListBoxRow>();
                let Some(last) = last else {
                    return glib::Propagation::Proceed;
                };
                let down = key == gtk::gdk::Key::Down;
                let index = list
                    .selected_row()
                    .map_or(if down { 0 } else { last.index() }, |row| {
                        (row.index() + if down { 1 } else { -1 }).clamp(0, last.index())
                    });
                if let Some(row) = list.row_at_index(index) {
                    list.select_row(Some(&row));
                    if let Some(bounds) = row.compute_bounds(&list) {
                        scroll
                            .vadjustment()
                            .clamp_page(bounds.y() as f64, (bounds.y() + bounds.height()) as f64);
                    }
                    if std::env::var_os("PICASA_TRACE").is_some() {
                        eprintln!(
                            "SEARCH TRACE suggestion_selected index={} scroll={}",
                            index,
                            scroll.vadjustment().value()
                        );
                    }
                }
                glib::Propagation::Stop
            }
            gtk::gdk::Key::Return | gtk::gdk::Key::KP_Enter => {
                if let Some(button) = list
                    .selected_row()
                    .and_then(|row| row.child())
                    .and_downcast::<gtk::Button>()
                {
                    button.emit_clicked();
                    glib::Propagation::Stop
                } else {
                    glib::Propagation::Proceed
                }
            }
            gtk::gdk::Key::Escape => {
                popup.popdown();
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        }
    });
    search.add_controller(keys);

    // A selected suggestion belongs to the previous text until the delayed
    // search-changed signal rebuilds the results. Never activate stale rows.
    let popup_weak = popover.downgrade();
    let list_weak = list.downgrade();
    search.connect_changed(move |entry| {
        if let Some(list) = list_weak.upgrade() {
            list.unselect_all();
            list.set_sensitive(false);
        }
        // Keep the completion surface mapped while typing. Repeatedly hiding
        // and reopening it disrupts native keyboard focus on some backends.
        if entry.text().is_empty() {
            if let Some(popup) = popup_weak.upgrade() {
                popup.popdown();
            }
        }
    });
    (popover, list)
}

fn trace_search_focus(event: &str, search: &gtk::SearchEntry, popup: &gtk::Popover) {
    if std::env::var_os("PICASA_TRACE").is_none() {
        return;
    }
    let focus = search.root().and_then(|root| root.focus());
    eprintln!(
        "SEARCH TRACE focus event={} widget={:?} in_entry={} cursor={} selection={:?} chars={} width={} popup_visible={} popup_mapped={} popup_modal={}",
        event,
        focus.as_ref().map(|widget| widget.type_().name()),
        focus.as_ref().is_some_and(|widget| widget == search.upcast_ref::<gtk::Widget>() || widget.is_ancestor(search)),
        search.position(), search.selection_bounds(), search.text().chars().count(),
        search.width(), popup.is_visible(), popup.is_mapped(), popup.is_autohide(),
    );
}

fn connect_search_popup_dismissal(
    window: &gtk::Window,
    search: &gtk::SearchEntry,
    popup: &gtk::Popover,
) {
    let click = gtk::GestureClick::new();
    click.set_button(0);
    click.set_propagation_phase(gtk::PropagationPhase::Capture);
    let search_weak = search.downgrade();
    let popup_weak = popup.downgrade();
    let window_weak = window.downgrade();
    click.connect_pressed(move |_, _, x, y| {
        let (Some(window), Some(search), Some(popup)) = (
            window_weak.upgrade(),
            search_weak.upgrade(),
            popup_weak.upgrade(),
        ) else {
            return;
        };
        let inside = window
            .pick(x, y, gtk::PickFlags::DEFAULT)
            .is_some_and(|picked| {
                picked == search.upcast_ref::<gtk::Widget>().clone()
                    || picked.is_ancestor(&search)
                    || picked == popup.upcast_ref::<gtk::Widget>().clone()
                    || picked.is_ancestor(&popup)
            });
        if !inside {
            popup.popdown();
        }
    });
    window.add_controller(click);

    let focus = gtk::EventControllerFocus::new();
    let popup_weak = popup.downgrade();
    let search_weak = search.downgrade();
    focus.connect_leave(move |_| {
        if let (Some(search), Some(popup)) = (search_weak.upgrade(), popup_weak.upgrade()) {
            trace_search_focus("leave", &search, &popup);
            popup.popdown();
        }
    });
    search.add_controller(focus);

    let popup_weak = popup.downgrade();
    search.connect_stop_search(move |search| {
        if let Some(popup) = popup_weak.upgrade() {
            if popup.is_visible() {
                popup.popdown();
            } else {
                search.set_text("");
            }
        }
    });
    let popup_weak = popup.downgrade();
    search.connect_unmap(move |_| {
        if let Some(popup) = popup_weak.upgrade() {
            popup.popdown();
        }
    });
}

fn update_folder_suggestions(
    popover: &gtk::Popover,
    list: &gtk::ListBox,
    folders: &[db::Folder],
    query: &str,
    on_folder: Rc<dyn Fn(i64)>,
) {
    list.set_sensitive(true);
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }

    if query.is_empty() {
        popover.popdown();
        return;
    }

    let query = normalized_search_text(query);
    if query.chars().count() < 2 {
        popover.popdown();
        return;
    }
    let matches = folders
        .iter()
        .filter(|folder| {
            normalized_search_text(&folder.name).contains(&query)
                || normalized_search_text(&folder.path).contains(&query)
        })
        .collect::<Vec<_>>();

    for folder in &matches {
        let button = gtk::Button::new();
        button.set_halign(gtk::Align::Fill);
        button.set_focusable(false);
        button.add_css_class("flat");
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.set_margin_start(8);
        row.set_margin_end(8);
        row.set_margin_top(5);
        row.set_margin_bottom(5);
        row.append(&gtk::Image::from_icon_name("folder-symbolic"));
        let labels = gtk::Box::new(gtk::Orientation::Vertical, 1);
        let name = gtk::Label::new(Some(&folder.name));
        name.set_xalign(0.0);
        let path = gtk::Label::new(Some(&folder.path));
        path.set_xalign(0.0);
        path.add_css_class("dim-label");
        path.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        labels.append(&name);
        labels.append(&path);
        labels.set_hexpand(true);
        row.append(&labels);
        button.set_child(Some(&row));

        let folder_id = folder.id;
        let popover = popover.clone();
        let on_folder = on_folder.clone();
        button.connect_clicked(move |_| {
            popover.popdown();
            if std::env::var_os("PICASA_TRACE").is_some() {
                eprintln!("SEARCH TRACE suggestion_activated folder_id={folder_id}");
            }
            on_folder(folder_id);
        });
        list.append(&button);
        if let Some(row) = button.parent().and_downcast::<gtk::ListBoxRow>() {
            row.set_can_focus(false);
        }
    }

    if matches.is_empty() || query.chars().count() < 2 {
        popover.popdown();
    } else {
        if !popover.is_visible() {
            popover.popup();
        }
    }
}

fn normalized_search_text(text: &str) -> String {
    text.chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect()
}
