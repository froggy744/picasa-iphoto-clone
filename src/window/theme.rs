{
    let settings = gtk::MenuButton::new();
    settings.set_icon_name("emblem-system-symbolic");
    settings.set_tooltip_text(Some("Settings"));

    // The iPhone presentation is the app's default. Keep the stock GTK4 /
    // libadwaita presentation immediately available as an opt-in overlay,
    // rather than making users restart or changing their system theme.
    let standard_theme_provider = gtk::CssProvider::new();
    standard_theme_provider.load_from_data(STANDARD_GTK4_CSS);
    let teal_theme_provider = gtk::CssProvider::new();
    teal_theme_provider.load_from_data(TEAL_GTK4_CSS);
    let blue_theme_provider = gtk::CssProvider::new();
    blue_theme_provider.load_from_data(BLUE_GTK4_CSS);
    let glass_theme_provider = gtk::CssProvider::new();
    glass_theme_provider.load_from_data(GLASS_GTK4_CSS);
    let superman_theme_provider = gtk::CssProvider::new();
    superman_theme_provider.load_from_data(SUPERMAN_GTK4_CSS);
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
    let teal_theme = gtk::CheckButton::with_label("Teal");
    teal_theme.set_group(Some(&iphone_theme));
    let blue_theme = gtk::CheckButton::with_label("Deep Ocean");
    blue_theme.set_group(Some(&iphone_theme));
    let glass_theme = gtk::CheckButton::with_label("Apple Glass");
    glass_theme.set_group(Some(&iphone_theme));
    let superman_theme = gtk::CheckButton::with_label("Superman");
    superman_theme.set_group(Some(&iphone_theme));
    if saved_theme == "iphone" {
        iphone_theme.set_active(true);
    } else if saved_theme == "teal" {
        teal_theme.set_active(true);
    } else if saved_theme == "blue" {
        blue_theme.set_active(true);
    } else if saved_theme == "glass" {
        glass_theme.set_active(true);
    } else if saved_theme == "superman" {
        superman_theme.set_active(true);
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
    // Force the app-wide color scheme so that widgets the theme CSS does
    // not cover (title bar, popovers, dialogs, settings window) follow
    // the dark appearance when iPhoto Dark is selected.
    if saved_theme == "iphone" {
        style_manager.set_color_scheme(adw::ColorScheme::PreferDark);
    } else {
        style_manager.set_color_scheme(adw::ColorScheme::Default);
    }
    // Load the overlay provider for Standard / Teal / Blue / Glass / Superman.
    if saved_theme != "iphone" {
        if saved_theme == "teal" {
            gtk::style_context_add_provider_for_display(
                &display,
                &teal_theme_provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
            );
        } else if saved_theme == "blue" {
            gtk::style_context_add_provider_for_display(
                &display,
                &blue_theme_provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
            );
        } else if saved_theme == "glass" {
            gtk::style_context_add_provider_for_display(
                &display,
                &glass_theme_provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
            );
        } else if saved_theme == "superman" {
            gtk::style_context_add_provider_for_display(
                &display,
                &superman_theme_provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
            );
        } else {
            gtk::style_context_add_provider_for_display(
                &display,
                &standard_theme_provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
            );
        }
    }

    let display_for_standard = display.clone();
    let provider_for_standard = standard_theme_provider.clone();
    let provider_for_teal = teal_theme_provider.clone();
    let provider_for_blue = blue_theme_provider.clone();
    let provider_for_glass = glass_theme_provider.clone();
    let provider_for_superman = superman_theme_provider.clone();
    let connection_for_standard = connection.clone();
    let lightbox_for_standard = lightbox.clone();
    let style_manager_for_standard = style_manager.clone();
    let albums_for_standard = albums_home_refresh_slot.clone();
    standard_theme.connect_toggled(move |button| {
        if button.is_active() {
            gtk::style_context_remove_provider_for_display(
                &display_for_standard,
                &provider_for_teal,
            );
            gtk::style_context_remove_provider_for_display(
                &display_for_standard,
                &provider_for_blue,
            );
            gtk::style_context_remove_provider_for_display(
                &display_for_standard,
                &provider_for_glass,
            );
            gtk::style_context_remove_provider_for_display(
                &display_for_standard,
                &provider_for_superman,
            );
            gtk::style_context_add_provider_for_display(
                &display_for_standard,
                &provider_for_standard,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
            );
            style_manager_for_standard.set_color_scheme(adw::ColorScheme::Default);
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

    let display_for_teal = display.clone();
    let provider_for_standard = standard_theme_provider.clone();
    let provider_for_teal = teal_theme_provider.clone();
    let provider_for_blue = blue_theme_provider.clone();
    let provider_for_glass = glass_theme_provider.clone();
    let provider_for_superman = superman_theme_provider.clone();
    let connection_for_teal = connection.clone();
    let lightbox_for_teal = lightbox.clone();
    let albums_for_teal = albums_home_refresh_slot.clone();
    let style_manager_for_teal = style_manager.clone();
    teal_theme.connect_toggled(move |button| {
        if button.is_active() {
            gtk::style_context_remove_provider_for_display(
                &display_for_teal,
                &provider_for_standard,
            );
            gtk::style_context_remove_provider_for_display(
                &display_for_teal,
                &provider_for_blue,
            );
            gtk::style_context_remove_provider_for_display(
                &display_for_teal,
                &provider_for_glass,
            );
            gtk::style_context_remove_provider_for_display(
                &display_for_teal,
                &provider_for_superman,
            );
            gtk::style_context_add_provider_for_display(
                &display_for_teal,
                &provider_for_teal,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
            );
            style_manager_for_teal.set_color_scheme(adw::ColorScheme::Default);
            lightbox_for_teal.use_standard_backdrop(style_manager_for_teal.is_dark());
            if let Err(error) = db::set_setting(
                &connection_for_teal.borrow(),
                THEME_SETTING_KEY,
                "teal",
            ) {
                eprintln!("Could not save appearance theme: {error}");
            }
            let albums = db::albums(&connection_for_teal.borrow()).unwrap_or_default();
            if let Some(refresh) = albums_for_teal.borrow().as_ref() {
                refresh(&albums);
            }
        }
    });

    let display_for_blue = display.clone();
    let provider_for_standard = standard_theme_provider.clone();
    let provider_for_teal = teal_theme_provider.clone();
    let provider_for_blue = blue_theme_provider.clone();
    let provider_for_glass = glass_theme_provider.clone();
    let provider_for_superman = superman_theme_provider.clone();
    let connection_for_blue = connection.clone();
    let lightbox_for_blue = lightbox.clone();
    let albums_for_blue = albums_home_refresh_slot.clone();
    let style_manager_for_blue = style_manager.clone();
    blue_theme.connect_toggled(move |button| {
        if button.is_active() {
            gtk::style_context_remove_provider_for_display(
                &display_for_blue,
                &provider_for_standard,
            );
            gtk::style_context_remove_provider_for_display(
                &display_for_blue,
                &provider_for_teal,
            );
            gtk::style_context_remove_provider_for_display(
                &display_for_blue,
                &provider_for_glass,
            );
            gtk::style_context_remove_provider_for_display(
                &display_for_blue,
                &provider_for_superman,
            );
            gtk::style_context_add_provider_for_display(
                &display_for_blue,
                &provider_for_blue,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
            );
            style_manager_for_blue.set_color_scheme(adw::ColorScheme::Default);
            lightbox_for_blue.use_standard_backdrop(style_manager_for_blue.is_dark());
            if let Err(error) = db::set_setting(
                &connection_for_blue.borrow(),
                THEME_SETTING_KEY,
                "blue",
            ) {
                eprintln!("Could not save appearance theme: {error}");
            }
            let albums = db::albums(&connection_for_blue.borrow()).unwrap_or_default();
            if let Some(refresh) = albums_for_blue.borrow().as_ref() {
                refresh(&albums);
            }
        }
    });

    let display_for_glass = display.clone();
    let provider_for_standard = standard_theme_provider.clone();
    let provider_for_teal = teal_theme_provider.clone();
    let provider_for_blue = blue_theme_provider.clone();
    let provider_for_glass = glass_theme_provider.clone();
    let provider_for_superman = superman_theme_provider.clone();
    let connection_for_glass = connection.clone();
    let lightbox_for_glass = lightbox.clone();
    let albums_for_glass = albums_home_refresh_slot.clone();
    let style_manager_for_glass = style_manager.clone();
    glass_theme.connect_toggled(move |button| {
        if button.is_active() {
            gtk::style_context_remove_provider_for_display(
                &display_for_glass,
                &provider_for_standard,
            );
            gtk::style_context_remove_provider_for_display(
                &display_for_glass,
                &provider_for_teal,
            );
            gtk::style_context_remove_provider_for_display(
                &display_for_glass,
                &provider_for_blue,
            );
            gtk::style_context_remove_provider_for_display(
                &display_for_glass,
                &provider_for_superman,
            );
            gtk::style_context_add_provider_for_display(
                &display_for_glass,
                &provider_for_glass,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
            );
            style_manager_for_glass.set_color_scheme(adw::ColorScheme::Default);
            lightbox_for_glass.use_standard_backdrop(style_manager_for_glass.is_dark());
            if let Err(error) = db::set_setting(
                &connection_for_glass.borrow(),
                THEME_SETTING_KEY,
                "glass",
            ) {
                eprintln!("Could not save appearance theme: {error}");
            }
            let albums = db::albums(&connection_for_glass.borrow()).unwrap_or_default();
            if let Some(refresh) = albums_for_glass.borrow().as_ref() {
                refresh(&albums);
            }
        }
    });

    let display_for_superman = display.clone();
    let provider_for_standard = standard_theme_provider.clone();
    let provider_for_teal = teal_theme_provider.clone();
    let provider_for_blue = blue_theme_provider.clone();
    let provider_for_glass = glass_theme_provider.clone();
    let provider_for_superman = superman_theme_provider.clone();
    let connection_for_superman = connection.clone();
    let lightbox_for_superman = lightbox.clone();
    let albums_for_superman = albums_home_refresh_slot.clone();
    let style_manager_for_superman = style_manager.clone();
    superman_theme.connect_toggled(move |button| {
        if button.is_active() {
            gtk::style_context_remove_provider_for_display(
                &display_for_superman,
                &provider_for_standard,
            );
            gtk::style_context_remove_provider_for_display(
                &display_for_superman,
                &provider_for_teal,
            );
            gtk::style_context_remove_provider_for_display(
                &display_for_superman,
                &provider_for_blue,
            );
            gtk::style_context_remove_provider_for_display(
                &display_for_superman,
                &provider_for_glass,
            );
            gtk::style_context_add_provider_for_display(
                &display_for_superman,
                &provider_for_superman,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
            );
            style_manager_for_superman.set_color_scheme(adw::ColorScheme::Default);
            lightbox_for_superman.use_standard_backdrop(style_manager_for_superman.is_dark());
            if let Err(error) = db::set_setting(
                &connection_for_superman.borrow(),
                THEME_SETTING_KEY,
                "superman",
            ) {
                eprintln!("Could not save appearance theme: {error}");
            }
            let albums = db::albums(&connection_for_superman.borrow()).unwrap_or_default();
            if let Some(refresh) = albums_for_superman.borrow().as_ref() {
                refresh(&albums);
            }
        }
    });

    let display_for_iphone = display.clone();
    let provider_for_iphone_standard = standard_theme_provider.clone();
    let provider_for_iphone_teal = teal_theme_provider.clone();
    let provider_for_iphone_blue = blue_theme_provider.clone();
    let provider_for_iphone_glass = glass_theme_provider.clone();
    let provider_for_iphone_superman = superman_theme_provider.clone();
    let connection_for_iphone = connection.clone();
    let lightbox_for_iphone = lightbox.clone();
    let albums_for_iphone = albums_home_refresh_slot.clone();
    let style_manager_for_iphone = style_manager.clone();
    iphone_theme.connect_toggled(move |button| {
        if button.is_active() {
            gtk::style_context_remove_provider_for_display(
                &display_for_iphone,
                &provider_for_iphone_standard,
            );
            gtk::style_context_remove_provider_for_display(
                &display_for_iphone,
                &provider_for_iphone_teal,
            );
            gtk::style_context_remove_provider_for_display(
                &display_for_iphone,
                &provider_for_iphone_blue,
            );
            gtk::style_context_remove_provider_for_display(
                &display_for_iphone,
                &provider_for_iphone_glass,
            );
            gtk::style_context_remove_provider_for_display(
                &display_for_iphone,
                &provider_for_iphone_superman,
            );
            style_manager_for_iphone.set_color_scheme(adw::ColorScheme::PreferDark);
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
    settings_box.append(&teal_theme);
    settings_box.append(&blue_theme);
    settings_box.append(&glass_theme);
    settings_box.append(&superman_theme);
    settings_box.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    settings_box.append(&clear_thumbnails);
    settings_box.append(&clear_database);
    settings_box.append(&clear_all);
    settings_popover.set_child(Some(&settings_box));
    settings.set_popover(Some(&settings_popover));

    (
        settings,
        standard_theme_provider,
        teal_theme_provider,
        blue_theme_provider,
        glass_theme_provider,
        superman_theme_provider,
        display,
        saved_theme,
        clear_thumbnails,
        clear_database,
        clear_all,
    )
}
