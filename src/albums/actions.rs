#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AlbumAppearanceAction {
    ToggleBookshelf,
    NextBackground,
    ToggleCovers,
    NextCovers,
    DisableAll,
    OpenThemeSettings,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct AlbumContextMenuItem {
    label: &'static str,
    action: AlbumAppearanceAction,
    checked: Option<bool>,
    sensitive: bool,
}

fn album_context_menu_items(appearance: AlbumAppearance) -> Vec<AlbumContextMenuItem> {
    vec![
        AlbumContextMenuItem {
            label: "Bookshelf",
            action: AlbumAppearanceAction::ToggleBookshelf,
            checked: Some(appearance.bookshelf_enabled),
            sensitive: true,
        },
        AlbumContextMenuItem {
            label: "Next Background",
            action: AlbumAppearanceAction::NextBackground,
            checked: None,
            sensitive: true,
        },
        AlbumContextMenuItem {
            label: "Album Covers",
            action: AlbumAppearanceAction::ToggleCovers,
            checked: Some(appearance.covers_enabled),
            sensitive: true,
        },
        AlbumContextMenuItem {
            label: "Next Album Covers",
            action: AlbumAppearanceAction::NextCovers,
            checked: None,
            sensitive: true,
        },
        AlbumContextMenuItem {
            label: "Disable All Themes",
            action: AlbumAppearanceAction::DisableAll,
            checked: None,
            sensitive: appearance.bookshelf_enabled || appearance.covers_enabled,
        },
        AlbumContextMenuItem {
            label: "Theme Settings…",
            action: AlbumAppearanceAction::OpenThemeSettings,
            checked: None,
            sensitive: true,
        },
    ]
}

// Alpha below this value counts as part of a frame's transparent opening.
fn install_context_menu(
    scrolled: &gtk::ScrolledWindow,
    connection: Rc<RefCell<Connection>>,
    on_appearance_changed: Rc<dyn Fn()>,
    open_theme_settings: Rc<dyn Fn()>,
) {
    let click = gtk::GestureClick::new();
    click.set_button(gtk::gdk::BUTTON_SECONDARY);
    click.set_propagation_phase(gtk::PropagationPhase::Bubble);
    let anchor = scrolled.clone();
    click.connect_pressed(move |_, _, x, y| {
        show_context_menu(
            &anchor,
            x,
            y,
            connection.clone(),
            on_appearance_changed.clone(),
            open_theme_settings.clone(),
        );
    });
    scrolled.add_controller(click);
}

fn install_album_context_menu(
    card: &gtk::Button,
    connection: Rc<RefCell<Connection>>,
    album: Album,
    on_appearance_changed: Rc<dyn Fn()>,
) {
    let click = gtk::GestureClick::new();
    click.set_button(gtk::gdk::BUTTON_SECONDARY);
    let anchor = card.clone();
    click.connect_pressed(move |gesture, _, x, y| {
        gesture.set_state(gtk::EventSequenceState::Claimed);
        show_album_context_menu(
            &anchor,
            x,
            y,
            connection.clone(),
            album.clone(),
            on_appearance_changed.clone(),
        );
    });
    card.add_controller(click);
}

fn show_album_context_menu(
    anchor: &gtk::Button,
    x: f64,
    y: f64,
    connection: Rc<RefCell<Connection>>,
    album: Album,
    on_appearance_changed: Rc<dyn Fn()>,
) {
    let frame_root = Path::new(ALBUM_COVER_THEME_DIRECTORY);
    let frames = album_frame_paths_in(frame_root);
    let popover = gtk::Popover::new();
    popover.set_autohide(true);
    popover.set_has_arrow(false);
    popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(
        x.round() as i32,
        y.round() as i32,
        1,
        1,
    )));

    let menu = gtk::Box::new(gtk::Orientation::Vertical, 2);
    menu.set_margin_top(6);
    menu.set_margin_bottom(6);
    menu.set_margin_start(6);
    menu.set_margin_end(6);
    let heading = gtk::Label::new(Some(&album.name));
    heading.set_xalign(0.0);
    heading.set_margin_start(8);
    heading.set_margin_end(8);
    heading.set_margin_top(4);
    heading.set_margin_bottom(4);
    heading.add_css_class("heading");
    menu.append(&heading);

    let rename = gtk::Button::with_label("Rename Album…");
    rename.set_halign(gtk::Align::Fill);
    rename.add_css_class("flat");
    rename.add_css_class("rename-album-action");
    let popover_for_rename = popover.clone();
    let rename_connection = connection.clone();
    let rename_album = album.clone();
    let rename_changed = on_appearance_changed.clone();
    let rename_anchor = anchor.clone();
    rename.connect_clicked(move |_| {
        popover_for_rename.popdown();
        show_rename_album_dialog(
            &rename_anchor,
            rename_connection.clone(),
            rename_album.clone(),
            rename_changed.clone(),
        );
    });
    menu.append(&rename);
    menu.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

    let next_frame = gtk::Button::with_label("Next Album Cover");
    next_frame.set_halign(gtk::Align::Fill);
    next_frame.set_sensitive(!frames.is_empty());
    next_frame.add_css_class("flat");
    next_frame.add_css_class("next-album-frame-action");
    let popover_for_action = popover.clone();
    let reset_connection = connection.clone();
    let reset_album = album.clone();
    let reset_appearance_changed = on_appearance_changed.clone();
    let album_has_own_cover = album.cover_frame.is_some() || album.cover_photo_id.is_some();
    next_frame.connect_clicked(move |_| {
        popover_for_action.popdown();
        let result = (|| {
            settings::set_covers_enabled(&connection.borrow(), true)?;
            let cover_index = settings::album_appearance(&connection.borrow()).cover_index;
            change_to_next_album_cover(
                &connection.borrow(),
                &album,
                frame_root,
                &frames,
                cover_index,
            )?;
            anyhow::Ok(())
        })();
        if let Err(error) = result {
            eprintln!("Could not change album cover: {error}");
            return;
        }
        on_appearance_changed();
    });
    menu.append(&next_frame);

    // Only an album with a cover of its own can lose one, so the reset stays
    // disabled for albums that already follow the page-wide design and its
    // automatic thumbnail.
    let reset_frame = gtk::Button::with_label("Reset Album Cover");
    reset_frame.set_halign(gtk::Align::Fill);
    reset_frame.set_sensitive(album_has_own_cover);
    reset_frame.add_css_class("flat");
    reset_frame.add_css_class("reset-album-frame-action");
    let popover_for_reset = popover.clone();
    reset_frame.connect_clicked(move |_| {
        popover_for_reset.popdown();
        let result = (|| {
            let connection = reset_connection.borrow();
            db::clear_album_cover_frame(&connection, reset_album.id)?;
            db::clear_album_cover_photo(&connection, reset_album.id)?;
            anyhow::Ok(())
        })();
        if let Err(error) = result {
            eprintln!("Could not reset album cover: {error}");
            return;
        }
        reset_appearance_changed();
    });
    menu.append(&reset_frame);

    popover.set_child(Some(&menu));
    popover.set_parent(anchor);
    popover.connect_closed(|popover| popover.unparent());
    popover.popup();
}

fn show_rename_album_dialog(
    parent: &gtk::Button,
    connection: Rc<RefCell<Connection>>,
    album: Album,
    on_changed: Rc<dyn Fn()>,
) {
    let entry = gtk::Entry::new();
    entry.set_text(&album.name);
    entry.set_activates_default(true);
    entry.select_region(0, -1);
    let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
    content.append(&gtk::Label::new(Some("Enter a new album name:")));
    content.append(&entry);
    let dialog = adw::AlertDialog::builder()
        .heading("Rename Album")
        .extra_child(&content)
        .close_response("cancel")
        .default_response("rename")
        .build();
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("rename", "Rename");
    dialog.set_response_appearance("rename", adw::ResponseAppearance::Suggested);
    let parent_for_response = parent.clone();
    let entry_for_response = entry.clone();
    dialog.connect_response(Some("rename"), move |dialog, _| {
        if let Err(error) = db::rename_album(
            &connection.borrow(),
            album.id,
            entry_for_response.text().as_str(),
        ) {
            let error_dialog = adw::AlertDialog::builder()
                .heading("Could not rename album")
                .body(error.to_string())
                .close_response("close")
                .build();
            error_dialog.add_response("close", "Close");
            error_dialog.present(Some(&parent_for_response));
            return;
        }
        dialog.close();
        on_changed();
    });
    dialog.present(Some(parent));
    entry.grab_focus();
}

fn show_context_menu(
    anchor: &gtk::ScrolledWindow,
    x: f64,
    y: f64,
    connection: Rc<RefCell<Connection>>,
    on_appearance_changed: Rc<dyn Fn()>,
    open_theme_settings: Rc<dyn Fn()>,
) {
    let appearance = settings::album_appearance(&connection.borrow());
    let popover = gtk::Popover::new();
    popover.set_autohide(true);
    popover.set_has_arrow(false);
    popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(
        x.round() as i32,
        y.round() as i32,
        1,
        1,
    )));

    let menu = gtk::Box::new(gtk::Orientation::Vertical, 2);
    menu.set_margin_top(6);
    menu.set_margin_bottom(6);
    menu.set_margin_start(6);
    menu.set_margin_end(6);

    let heading = gtk::Label::new(Some("Album Appearance"));
    heading.set_xalign(0.0);
    heading.set_margin_start(8);
    heading.set_margin_end(8);
    heading.set_margin_top(4);
    heading.set_margin_bottom(4);
    heading.add_css_class("heading");
    menu.append(&heading);

    for item in album_context_menu_items(appearance) {
        if matches!(
            item.action,
            AlbumAppearanceAction::DisableAll | AlbumAppearanceAction::OpenThemeSettings
        ) {
            menu.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        }
        let button = gtk::Button::new();
        button.set_halign(gtk::Align::Fill);
        button.set_sensitive(item.sensitive);
        button.add_css_class("flat");

        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        if let Some(checked) = item.checked {
            let mark = gtk::Image::from_icon_name("object-select-symbolic");
            mark.set_visible(checked);
            mark.set_size_request(16, 16);
            row.append(&mark);
        } else {
            let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            spacer.set_size_request(16, 16);
            row.append(&spacer);
        }
        let label = gtk::Label::new(Some(item.label));
        label.set_xalign(0.0);
        label.set_hexpand(true);
        row.append(&label);
        button.set_child(Some(&row));

        let popover_for_action = popover.clone();
        let connection = connection.clone();
        let on_appearance_changed = on_appearance_changed.clone();
        let open_theme_settings = open_theme_settings.clone();
        button.connect_clicked(move |_| {
            popover_for_action.popdown();
            if item.action == AlbumAppearanceAction::OpenThemeSettings {
                open_theme_settings();
                return;
            }
            let result = match item.action {
                AlbumAppearanceAction::ToggleBookshelf => settings::set_bookshelf_enabled(
                    &connection.borrow(),
                    !settings::album_appearance(&connection.borrow()).bookshelf_enabled,
                ),
                AlbumAppearanceAction::NextBackground => settings::next_bookshelf_background(
                    &connection.borrow(),
                    bookshelf_background_count(),
                )
                .map(|_| ()),
                AlbumAppearanceAction::ToggleCovers => settings::set_covers_enabled(
                    &connection.borrow(),
                    !settings::album_appearance(&connection.borrow()).covers_enabled,
                ),
                AlbumAppearanceAction::NextCovers => {
                    settings::next_album_covers(&connection.borrow(), album_cover_theme_count())
                        .map(|_| ())
                }
                AlbumAppearanceAction::DisableAll => {
                    settings::disable_all_album_themes(&connection.borrow())
                }
                AlbumAppearanceAction::OpenThemeSettings => unreachable!(),
            };
            if let Err(error) = result {
                eprintln!("Could not update album appearance: {error}");
                return;
            }
            on_appearance_changed();
        });
        menu.append(&button);
    }

    popover.set_child(Some(&menu));
    popover.set_parent(anchor);
    popover.connect_closed(|popover| popover.unparent());
    popover.popup();
}
