pub fn build(
    albums: &[Album],
    connection: Rc<RefCell<Connection>>,
    thumbnail_width: i32,
    on_album: Rc<dyn Fn(i64)>,
    on_appearance_changed: Rc<dyn Fn()>,
    open_theme_settings: Rc<dyn Fn()>,
) -> gtk::ScrolledWindow {
    let scrolled = gtk::ScrolledWindow::new();
    scrolled.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scrolled.set_hexpand(true);
    scrolled.set_vexpand(true);

    let background = gtk::CssProvider::new();
    let background_rules = bookshelf_background_rules(&crate::css::resolve_runtime_dir(Path::new(
        BOOKSHELF_THEME_DIRECTORY,
    )));
    background.load_from_string(&background_rules);
    scrolled
        .style_context()
        .add_provider(&background, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 2);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 18);
    content.set_margin_start(32);
    content.set_margin_end(32);
    content.set_margin_top(28);
    content.set_margin_bottom(32);

    let style = gtk::CssProvider::new();
    style.load_from_string(crate::css::ALBUMS);
    gtk::style_context_add_provider_for_display(
        &scrolled.display(),
        &style,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 3,
    );

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let heading = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    heading.set_hexpand(true);

    let title = gtk::Label::new(Some("Albums"));
    title.set_xalign(0.0);
    title.add_css_class("albums-index-title");
    heading.append(&title);

    let count = gtk::Label::new(None);
    count.set_xalign(0.0);
    count.add_css_class("dim-label");
    count.add_css_class("albums-home-count");
    heading.append(&count);
    header.append(&heading);

    let controls = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let sort =
        gtk::DropDown::from_strings(&["Name A–Z", "Name Z–A", "Newest first", "Most photos"]);
    sort.add_css_class("albums-sort");
    sort.set_tooltip_text(Some("Sort albums"));
    sort.set_selected(index_setting(&connection.borrow(), ALBUM_SORT_KEY, 0, 3));
    let sort_label = gtk::Label::new(Some("Sort"));
    sort_label.set_mnemonic_widget(Some(&sort));
    controls.append(&sort_label);
    controls.append(&sort);
    let size = gtk::DropDown::from_strings(&["Compact", "Comfortable", "Large"]);
    size.add_css_class("albums-size");
    size.set_tooltip_text(Some("Album cover size"));
    size.set_selected(index_setting(&connection.borrow(), ALBUM_SIZE_KEY, 1, 2));
    let size_label = gtk::Label::new(Some("Cover size"));
    size_label.add_css_class("albums-size-label");
    size_label.set_mnemonic_widget(Some(&size));
    controls.append(&size_label);
    controls.append(&size);
    header.append(&controls);

    let create = gtk::Button::with_label("New Album");
    create.add_css_class("suggested-action");
    create.add_css_class("albums-create");
    header.append(&create);
    content.append(&header);

    let cards = gtk::FlowBox::new();
    cards.set_selection_mode(gtk::SelectionMode::None);
    cards.set_min_children_per_line(1);
    cards.set_max_children_per_line(6);
    cards.set_row_spacing(20);
    cards.set_column_spacing(20);
    cards.set_homogeneous(false);
    cards.set_hexpand(true);
    cards.set_vexpand(false);
    cards.set_valign(gtk::Align::Start);
    cards.add_css_class("albums-home-grid");
    cards
        .style_context()
        .add_provider(&background, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 2);

    let bookshelf_rows = gtk::Box::new(gtk::Orientation::Vertical, 0);
    bookshelf_rows.set_hexpand(true);
    bookshelf_rows.set_vexpand(false);
    bookshelf_rows.set_valign(gtk::Align::Start);
    bookshelf_rows.set_visible(false);
    bookshelf_rows.add_css_class("albums-bookshelf-rows");

    let bookshelf_runtime = BookshelfRuntime {
        rows: bookshelf_rows.clone(),
        last_columns: Rc::new(Cell::new(0)),
        last_card_width: Rc::new(Cell::new(0)),
        selected_theme: Rc::new(RefCell::new(None)),
        standard_width: Rc::new(Cell::new(0)),
        last_standard_width: Rc::new(Cell::new(0)),
    };
    unsafe {
        scrolled.set_data(BOOKSHELF_RUNTIME_KEY, bookshelf_runtime.clone());
    }

    content.append(&cards);
    content.append(&bookshelf_rows);
    scrolled.set_child(Some(&content));
    for (control, key) in [(sort, ALBUM_SORT_KEY), (size, ALBUM_SIZE_KEY)] {
        let weak_view = scrolled.downgrade();
        let connection = connection.clone();
        let on_album = on_album.clone();
        let changed = on_appearance_changed.clone();
        control.connect_selected_notify(move |control| {
            if let Err(error) =
                db::set_setting(&connection.borrow(), key, &control.selected().to_string())
            {
                eprintln!("Could not save album presentation: {error}");
                return;
            }
            if let Some(view) = weak_view.upgrade() {
                let albums = db::albums(&connection.borrow()).unwrap_or_default();
                refresh(
                    &view,
                    &albums,
                    connection.clone(),
                    thumbnail_width,
                    on_album.clone(),
                    changed.clone(),
                );
            }
        });
    }
    refresh_presentation(&scrolled, &connection.borrow());

    populate(
        &cards,
        &bookshelf_runtime,
        &count,
        albums,
        connection.clone(),
        thumbnail_width,
        on_album.clone(),
        on_appearance_changed.clone(),
    );

    let runtime_for_resize = bookshelf_runtime.clone();
    let standard_cards = cards.clone();
    scrolled.add_tick_callback(move |_, _| {
        let preferred = runtime_for_resize.standard_width.get();
        if preferred > 0 && standard_cards.width() > 0 {
            let width = standard_cards.width();
            if width != runtime_for_resize.last_standard_width.get() {
                resize_standard_cards(&standard_cards, width, preferred);
                runtime_for_resize.last_standard_width.set(width);
            }
        }
        if !runtime_for_resize.rows.is_visible() {
            return glib::ControlFlow::Continue;
        }
        let Some(theme) = runtime_for_resize.selected_theme.borrow().clone() else {
            return glib::ControlFlow::Continue;
        };
        let width = runtime_for_resize.rows.width();
        if width <= 0 {
            return glib::ControlFlow::Continue;
        }

        let columns = bookshelf_columns_for_width(width, BOOKSHELF_MIN_CARD_WIDTH);
        if columns != runtime_for_resize.last_columns.get() {
            reflow_bookshelf_rows(&runtime_for_resize, columns, &theme);
        }

        let target_card_width = bookshelf_target_card_width(width, columns);
        if target_card_width != runtime_for_resize.last_card_width.get() {
            resize_bookshelf_cards(&runtime_for_resize.rows, target_card_width, &theme);
            runtime_for_resize.last_card_width.set(target_card_width);

            
        }

        glib::ControlFlow::Continue
    });

    install_context_menu(
        &scrolled,
        connection,
        on_appearance_changed,
        open_theme_settings,
    );

    scrolled
}

pub fn connect_create_album(scrolled: &gtk::ScrolledWindow, create_album: Rc<dyn Fn()>) {
    if let Some(button) = find_descendant_with_css_class(scrolled.upcast_ref(), "albums-create")
        .and_then(|widget| widget.downcast::<gtk::Button>().ok())
    {
        button.connect_clicked(move |_| create_album());
    }
}

fn index_setting(connection: &Connection, key: &str, default: u32, max: u32) -> u32 {
    db::setting(connection, key)
        .ok()
        .flatten()
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|value| *value <= max)
        .unwrap_or(default)
}

fn resize_standard_cards(grid: &gtk::FlowBox, available: i32, preferred: i32) {
    let gap = if preferred == 180 { 8 } else { 24 };
    let columns = ((available + gap) / (preferred + gap)).max(1);
    let width = ((available - (columns - 1) * gap) / columns)
        .min(preferred + 24)
        .max(1);
    grid.set_max_children_per_line(columns as u32);
    let mut cards = Vec::new();
    collect_bookshelf_cards(grid.upcast_ref(), &mut cards);
    for card in cards {
        card.set_width_request(width);
        if let Some(content) = card.child() {
            content.set_width_request(width);
        }
        if let Some(cover) = find_descendant_with_css_class(card.upcast_ref(), "album-cover") {
            cover.set_size_request(width, width * 2 / 3);
        }
        // The overlay and FlowBox wrapper must shrink along with the cover.
        if let Some(tile) = card.parent() {
            tile.set_width_request(width);
            if let Some(slot) = tile.parent() {
                slot.set_width_request(width);
            }
        }
    }
}

pub fn refresh(
    scrolled: &gtk::ScrolledWindow,
    albums: &[Album],
    connection: Rc<RefCell<Connection>>,
    thumbnail_width: i32,
    on_album: Rc<dyn Fn(i64)>,
    on_appearance_changed: Rc<dyn Fn()>,
) {
    refresh_presentation(scrolled, &connection.borrow());
    // The ScrolledWindow may expose a GTK viewport around its content. Find
    // the existing widgets by their local CSS markers instead of assuming
    // that the content box is the direct child.
    let root = scrolled.clone().upcast::<gtk::Widget>();
    let Some(count) = find_descendant_with_css_class(&root, "albums-home-count")
        .and_then(|widget| widget.downcast::<gtk::Label>().ok())
    else {
        return;
    };
    let Some(cards) = find_descendant_with_css_class(&root, "albums-home-grid")
        .and_then(|widget| widget.downcast::<gtk::FlowBox>().ok())
    else {
        return;
    };
    let Some(runtime) = (unsafe { scrolled.data::<BookshelfRuntime>(BOOKSHELF_RUNTIME_KEY) })
        .map(|runtime| unsafe { runtime.as_ref() }.clone())
    else {
        return;
    };

    populate(
        &cards,
        &runtime,
        &count,
        albums,
        connection,
        thumbnail_width,
        on_album,
        on_appearance_changed,
    );
}

pub fn refresh_presentation(scrolled: &gtk::ScrolledWindow, connection: &Connection) {
    let appearance = settings::album_appearance(connection);
    for class in ["albums-size", "albums-size-label"] {
        if let Some(widget) = find_descendant_with_css_class(scrolled.upcast_ref(), class) {
            widget.set_visible(!appearance.bookshelf_enabled && !appearance.covers_enabled);
        }
    }
    remove_bookshelf_background_classes(scrolled.upcast_ref());
    if let Some(cards) = find_descendant_with_css_class(scrolled.upcast_ref(), "albums-home-grid") {
        remove_bookshelf_background_classes(&cards);
    }

    let has_theme = appearance.bookshelf_enabled && bookshelf_background_count() > 0;
    if has_theme {
        scrolled.add_css_class("albums-bookshelf");
    } else {
        scrolled.remove_css_class("albums-bookshelf");
    }

    // Folder-backed row themes are rendered by real ShelfRow widgets. Only
    // older root-level bookshelf images remain page-sized CSS backgrounds.
    if let Some(background_class) = bookshelf_background_css_class(appearance) {
        scrolled.add_css_class(&background_class);
    }
}

fn remove_bookshelf_background_classes(widget: &gtk::Widget) {
    for class_name in widget
        .css_classes()
        .into_iter()
        .filter(|name| name.starts_with("albums-bookshelf-"))
    {
        widget.remove_css_class(&class_name);
    }
}

fn find_descendant_with_css_class(root: &gtk::Widget, class_name: &str) -> Option<gtk::Widget> {
    if root.has_css_class(class_name) {
        return Some(root.clone());
    }

    let mut child = root.first_child();
    while let Some(candidate) = child {
        let next = candidate.next_sibling();
        if let Some(found) = find_descendant_with_css_class(&candidate, class_name) {
            return Some(found);
        }
        child = next;
    }

    None
}

fn populate(
    cards: &gtk::FlowBox,
    bookshelf_runtime: &BookshelfRuntime,
    count: &gtk::Label,
    albums: &[Album],
    connection: Rc<RefCell<Connection>>,
    thumbnail_width: i32,
    on_album: Rc<dyn Fn(i64)>,
    on_appearance_changed: Rc<dyn Fn()>,
) {
    while let Some(child) = cards.first_child() {
        cards.remove(&child);
    }
    clear_box(&bookshelf_runtime.rows);
    bookshelf_runtime.last_columns.set(0);
    bookshelf_runtime.last_card_width.set(0);
    bookshelf_runtime.selected_theme.replace(None);

    count.set_text(&format!(
        "{} album{}",
        albums.len(),
        if albums.len() == 1 { "" } else { "s" }
    ));

    

    let appearance = settings::album_appearance(&connection.borrow());
    let bookshelf_theme = bookshelf_theme_for_appearance(appearance);
    let row_theme = bookshelf_theme
        .as_ref()
        .filter(|theme| theme.kind == BookshelfThemeKind::Row)
        .cloned();
    let responsive_bookshelf = row_theme.is_some();
    let standard = !appearance.bookshelf_enabled && !appearance.covers_enabled;
    let preferred_width =
        [180, 240, 280][index_setting(&connection.borrow(), ALBUM_SIZE_KEY, 1, 2) as usize];
    bookshelf_runtime
        .standard_width
        .set(if standard { preferred_width } else { 0 });
    bookshelf_runtime.last_standard_width.set(0);
    cards.set_homogeneous(standard);
    cards.set_max_children_per_line(if standard { 20 } else { 6 });
    if standard {
        cards.add_css_class("albums-standard");
    } else {
        cards.remove_css_class("albums-standard");
    }
    let mut albums = albums.to_vec();
    let sort = index_setting(&connection.borrow(), ALBUM_SORT_KEY, 0, 3);
    albums.sort_by(|a, b| {
        let names = || {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then(a.id.cmp(&b.id))
        };
        match sort {
            1 => names().reverse(),
            2 => b.created_at.cmp(&a.created_at).then(b.id.cmp(&a.id)),
            3 => b.photo_count.cmp(&a.photo_count).then_with(names),
            _ => names(),
        }
    });

    cards.set_visible(!responsive_bookshelf);
    cards.set_vexpand(false);
    cards.set_valign(gtk::Align::Start);
    bookshelf_runtime.rows.set_visible(responsive_bookshelf);
    bookshelf_runtime.selected_theme.replace(row_theme.clone());

    let frames: Vec<FrameAsset> = album_frame_paths_for_appearance(
        &crate::css::resolve_runtime_dir(Path::new(ALBUM_COVER_THEME_DIRECTORY)),
        appearance,
    )
            .into_iter()
            .filter_map(|path| {
                gtk::gdk::Texture::from_filename(&path).ok().map(|texture| {
                    let opening = frame_photo_opening(&path);
                    FrameAsset {
                        path,
                        texture,
                        opening,
                    }
                })
            })
            .collect();
    let framed = !frames.is_empty();
    cards.set_row_spacing(if standard {
        32
    } else if framed {
        28
    } else {
        20
    });
    cards.set_column_spacing(if standard {
        if preferred_width == 180 {
            8
        } else {
            24
        }
    } else if framed {
        24
    } else {
        20
    });

    if albums.is_empty() {
        let empty = gtk::Label::new(Some(
            "Your albums start here\nChoose New Album to organise your photos into a collection.",
        ));
        empty.set_xalign(0.0);
        empty.add_css_class("dim-label");
        if let Some(theme) = row_theme {
            let row = bookshelf_row(&theme);
            let holder = gtk::Box::new(gtk::Orientation::Vertical, 0);
            holder.set_margin_top(24);
            holder.set_margin_start(24);
            holder.append(&empty);
            row.add_overlay(&holder);
            bookshelf_runtime.rows.append(&row);
        } else {
            insert_child(cards, &empty, None, None);
        }
        return;
    }

    let thumbnail_width = if standard {
        preferred_width
    } else if framed {
        thumbnail_width.clamp(220, 300)
    } else {
        thumbnail_width
    };
    let frame_height = (thumbnail_width as f64 * 500.0 / 805.0).round() as i32;
    let card_width = frames
        .iter()
        .map(|frame| {
            (frame_height as f64 * frame.texture.width() as f64 / frame.texture.height() as f64)
                .round() as i32
        })
        .max()
        .unwrap_or_else(|| cover_width(thumbnail_width));
    let frame_paths: Vec<_> = frames.iter().map(|frame| frame.path.clone()).collect();
    let mut bookshelf_cards = Vec::with_capacity(albums.len());

    for album in &albums {
        let frame = selected_frame_index(
            album,
            &crate::css::resolve_runtime_dir(Path::new(ALBUM_COVER_THEME_DIRECTORY)),
            &frame_paths,
            appearance.cover_index,
        )
        .map(|index| &frames[index]);
        let actual_cover_height = if frame.is_some() {
            frame_height
        } else {
            cover_height(thumbnail_width)
        };
        let card = album_card(
            album,
            &connection.borrow(),
            thumbnail_width,
            on_album.clone(),
            frame,
            responsive_bookshelf,
            connection.clone(),
            on_appearance_changed.clone(),
        );
        if framed {
            card.set_width_request(card_width);
        }

        if let Some(theme) = row_theme.as_ref() {
            card.set_margin_top(bookshelf_card_margin_top(
                theme.surface_y,
                actual_cover_height,
            ));
            card.set_halign(gtk::Align::Center);
            bookshelf_cards.push(card);
        } else if standard {
            if let Some(cover) = find_descendant_with_css_class(card.upcast_ref(), "album-cover") {
                cover.set_height_request(thumbnail_width * 2 / 3);
            }
            let tile = gtk::Overlay::new();
            tile.set_child(Some(&card));
            insert_child(cards, &tile, Some(card_width), None);
        } else {
            insert_child(cards, &card, Some(card_width), None);
        }
    }

    if let Some(theme) = row_theme {
        let width = bookshelf_runtime.rows.width();
        let columns = bookshelf_columns_for_width(width, card_width);
        build_bookshelf_rows(&bookshelf_runtime.rows, &bookshelf_cards, columns, &theme);
        bookshelf_runtime.last_columns.set(columns);

        if width > 0 {
            let target_card_width = bookshelf_target_card_width(width, columns);
            resize_bookshelf_cards(&bookshelf_runtime.rows, target_card_width, &theme);
            bookshelf_runtime.last_card_width.set(target_card_width);
        }
    }
}

fn clear_box(container: &gtk::Box) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}
