{
    let settings = gtk::MenuButton::new();
    settings.set_icon_name("emblem-system-symbolic");
    settings.set_tooltip_text(Some("Settings"));

    // The iPhone presentation is the app's default. Keep the stock GTK4 /
    // libadwaita presentation immediately available as an opt-in overlay,
    // rather than making users restart or changing their system theme.
    let standard_theme_provider = gtk::CssProvider::new();
    standard_theme_provider.load_from_data(STANDARD_GTK4_CSS);
    let display = gtk::gdk::Display::default().expect("a display is required");

    let settings_popover = gtk::Popover::new();
    let settings_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    settings_box.set_margin_top(8);
    settings_box.set_margin_bottom(8);
    settings_box.set_margin_start(8);
    settings_box.set_margin_end(8);

    let appearance = gtk::Label::new(Some("Appearance"));
    appearance.set_xalign(0.0);
    appearance.add_css_class("heading");

    let saved_theme = db::setting(&connection.borrow(), THEME_SETTING_KEY)
        .ok()
        .flatten()
        .unwrap_or_else(|| "standard".to_string());

    let iphone_theme = gtk::CheckButton::with_label("iPhoto Dark");
    iphone_theme.set_tooltip_text(Some("Dark iPhoto-inspired gallery"));

    let standard_theme = gtk::CheckButton::with_label("Standard GTK4");
    standard_theme.set_group(Some(&iphone_theme));
    if saved_theme == "iphone" {
        iphone_theme.set_active(true);
    } else {
        standard_theme.set_active(true);
    }
    standard_theme.set_tooltip_text(Some("Use the regular GTK4 / libadwaita appearance"));

    let style_manager = adw::StyleManager::default();
    if saved_theme == "iphone" {
        lightbox.use_iphone_backdrop();
    } else {
        lightbox.use_standard_backdrop(style_manager.is_dark());
    }

    let display_for_standard = display.clone();
    let provider_for_standard = standard_theme_provider.clone();
    let connection_for_standard = connection.clone();
    let lightbox_for_standard = lightbox.clone();
    let style_manager_for_standard = style_manager.clone();
    let albums_for_standard = albums_home_refresh_slot.clone();
    standard_theme.connect_toggled(move |button| {
        if button.is_active() {
            gtk::style_context_add_provider_for_display(
                &display_for_standard,
                &provider_for_standard,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
            );
            lightbox_for_standard.use_standard_backdrop(style_manager_for_standard.is_dark());
            if let Err(error) = db::set_setting(
                &connection_for_standard.borrow(),
                THEME_SETTING_KEY,
                "standard",
            ) {
                eprintln!("Could not save appearance theme: {error}");
            }
            let albums = db::albums(&connection_for_standard.borrow()).unwrap_or_default();
            if let Some(refresh) = albums_for_standard.borrow().as_ref() {
                refresh(&albums);
            }
        }
    });

    let display_for_iphone = display.clone();
    let provider_for_iphone = standard_theme_provider.clone();
    let connection_for_iphone = connection.clone();
    let lightbox_for_iphone = lightbox.clone();
    let albums_for_iphone = albums_home_refresh_slot.clone();
    iphone_theme.connect_toggled(move |button| {
        if button.is_active() {
            gtk::style_context_remove_provider_for_display(
                &display_for_iphone,
                &provider_for_iphone,
            );
            lightbox_for_iphone.use_iphone_backdrop();
            if let Err(error) =
                db::set_setting(&connection_for_iphone.borrow(), THEME_SETTING_KEY, "iphone")
            {
                eprintln!("Could not save appearance theme: {error}");
            }
            let albums = db::albums(&connection_for_iphone.borrow()).unwrap_or_default();
            if let Some(refresh) = albums_for_iphone.borrow().as_ref() {
                refresh(&albums);
            }
        }
    });

    let standard_theme_for_dark = standard_theme.clone();
    let lightbox_for_dark = lightbox.clone();
    style_manager.connect_dark_notify(move |manager| {
        if standard_theme_for_dark.is_active() {
            lightbox_for_dark.use_standard_backdrop(manager.is_dark());
        }
    });

    let clear_thumbnails = gtk::Button::with_label("Clear thumbnails");
    clear_thumbnails.set_halign(gtk::Align::Fill);
    clear_thumbnails.add_css_class("clear-action-button");
    let clear_database = gtk::Button::with_label("Clear database");
    clear_database.set_halign(gtk::Align::Fill);
    clear_database.add_css_class("clear-action-button");
    let clear_all = gtk::Button::with_label("Clear all");
    clear_all.set_halign(gtk::Align::Fill);
    clear_all.add_css_class("clear-action-button");
    settings_box.append(&appearance);
    settings_box.append(&iphone_theme);
    settings_box.append(&standard_theme);
    settings_box.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    settings_box.append(&clear_thumbnails);
    settings_box.append(&clear_database);
    settings_box.append(&clear_all);
    settings_popover.set_child(Some(&settings_box));
    settings.set_popover(Some(&settings_popover));

    (
        settings,
        standard_theme_provider,
        display,
        saved_theme,
        clear_thumbnails,
        clear_database,
        clear_all,
    )
}
