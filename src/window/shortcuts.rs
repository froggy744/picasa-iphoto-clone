{
    // Handle viewer keyboard shortcuts at the window boundary as well as
    // inside the lightbox. Capture prevents GtkGridView from interpreting
    // Space as a selection toggle.
    let window_escape = gtk::EventControllerKey::new();
    window_escape.set_propagation_phase(gtk::PropagationPhase::Capture);
    let lightbox_for_window_escape = lightbox.clone();
    let space_open_slot_for_key = space_open_slot.clone();
    let edit_space_slot_for_key = edit_space_slot.clone();
    let one_to_one_for_key = info.one_to_one.clone();
    let space_toggle_in_progress_for_key = space_toggle_in_progress.clone();
    let search_popup_for_key = search_popup_slot.clone();
    let window_for_fullscreen_key = window.clone();
    window_escape.connect_key_pressed(move |_, key, _, _| {
        // Escape dismisses the photo context menu before it closes the
        // lightbox or affects the gallery. This mirrors normal context-menu
        // behaviour and keeps one Escape press scoped to one UI layer.
        if key == gtk::gdk::Key::Escape && dismiss_active_photo_context_menu() {
            let root = lightbox_for_window_escape.root.clone();
            glib::idle_add_local_once(move || {
                if root.is_visible() {
                    root.grab_focus();
                }
            });
            return glib::Propagation::Stop;
        }

        // Editing text must not invoke gallery Space/arrow-key shortcuts.
        if !lightbox_for_window_escape.root.is_visible()
            && gtk::prelude::RootExt::focus(&window_for_fullscreen_key)
                .is_some_and(|focus| focus.is::<gtk::Editable>())
            && key != gtk::gdk::Key::F11
        {
            return glib::Propagation::Proceed;
        }
        if (key == gtk::gdk::Key::Escape || key == gtk::gdk::Key::BackSpace)
            && lightbox_for_window_escape.root.is_visible()
        {
            
            lightbox_for_window_escape.close();
            glib::Propagation::Stop
        } else if key == gtk::gdk::Key::F11 {
            if window_for_fullscreen_key.is_fullscreen() {
                window_for_fullscreen_key.unfullscreen();
            } else {
                window_for_fullscreen_key.fullscreen();
            }
            glib::Propagation::Stop
        } else if lightbox_for_window_escape.root.is_visible()
            && (key == gtk::gdk::Key::Up || key == gtk::gdk::Key::Down)
            && search_popup_for_key
                .borrow()
                .as_ref()
                .is_some_and(|popup| popup.is_visible())
        {
            // Search suggestions own Up/Down while visible, even with the
            // photo viewer open underneath.
            glib::Propagation::Proceed
        } else if lightbox_for_window_escape.root.is_visible()
            && (key == gtk::gdk::Key::Left || key == gtk::gdk::Key::Right)
        {
            lightbox_for_window_escape.navigate_photo(if key == gtk::gdk::Key::Left { -1 } else { 1 });
            glib::Propagation::Stop
        } else if lightbox_for_window_escape.root.is_visible()
            && (key == gtk::gdk::Key::Up || key == gtk::gdk::Key::Down)
        {
            lightbox_for_window_escape.navigate_collection(if key == gtk::gdk::Key::Up { -1 } else { 1 });
            glib::Propagation::Stop
        } else if key == gtk::gdk::Key::space {
            if let Some(toggle_edit) = edit_space_slot_for_key.borrow().as_ref() {
                toggle_edit();
            } else if lightbox_for_window_escape.root.is_visible() {
                if one_to_one_for_key.is_active() {
                    space_toggle_in_progress_for_key.set(true);
                    one_to_one_for_key.set_active(false);
                    
                } else {
                    space_toggle_in_progress_for_key.set(true);
                    one_to_one_for_key.set_active(true);
                    
                }
            } else if let Some(open_selected) = space_open_slot_for_key.borrow().as_ref() {
                open_selected();
                
            }
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    window.add_controller(window_escape);

}
