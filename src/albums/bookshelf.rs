#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BookshelfThemeKind {
    /// A folder-backed theme containing one image that represents exactly one shelf row.
    Row,
    /// Compatibility mode for the older page-sized *-bookshelf.png/jpg assets.
    Legacy,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct BookshelfTheme {
    id: String,
    image_path: PathBuf,
    kind: BookshelfThemeKind,
    row_height: i32,
    surface_y: i32,
}

#[derive(Clone)]
struct BookshelfRuntime {
    rows: gtk::Box,
    last_columns: Rc<Cell<usize>>,
    last_card_width: Rc<Cell<i32>>,
    selected_theme: Rc<RefCell<Option<BookshelfTheme>>>,
    standard_width: Rc<Cell<i32>>,
    last_standard_width: Rc<Cell<i32>>,
}

fn bookshelf_theme_image_in(directory: &Path) -> Option<PathBuf> {
    let preferred = [
        "row.png",
        "row.jpg",
        "bookshelf.png",
        "bookshelf.jpg",
        "single-row-bookshelf.png",
        "single-row-bookshelf.jpg",
    ];
    for name in preferred {
        let path = directory.join(name);
        if path.is_file() {
            return Some(path);
        }
    }

    let mut images = png_or_jpg_paths_matching(directory, |_| true);
    images.sort();
    images.into_iter().next()
}

fn bookshelf_theme_geometry(directory: &Path) -> (i32, i32) {
    let config = directory.join("theme.conf");
    let Ok(text) = std::fs::read_to_string(config) else {
        return (BOOKSHELF_ROW_HEIGHT, BOOKSHELF_SURFACE_Y);
    };
    let mut row_height = BOOKSHELF_ROW_HEIGHT;
    let mut surface_y = BOOKSHELF_SURFACE_Y;
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let Ok(value) = value.trim().parse::<i32>() else {
            continue;
        };
        match key.trim() {
            "row_height" if value > 0 => row_height = value,
            "surface_y" if value >= 0 => surface_y = value,
            _ => {}
        }
    }
    surface_y = surface_y.clamp(0, row_height);
    (row_height, surface_y)
}

fn bookshelf_themes_in(directory: &Path) -> Vec<BookshelfTheme> {
    let mut themes = Vec::new();
    if let Ok(entries) = std::fs::read_dir(directory) {
        for path in entries.filter_map(Result::ok).map(|entry| entry.path()) {
            if path.is_dir() {
                if let Some(image_path) = bookshelf_theme_image_in(&path) {
                    let id = path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("bookshelf")
                        .to_string();
                    let (row_height, surface_y) = bookshelf_theme_geometry(&path);
                    themes.push(BookshelfTheme {
                        id,
                        image_path,
                        kind: BookshelfThemeKind::Row,
                        row_height,
                        surface_y,
                    });
                }
            } else if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.ends_with("-bookshelf.png") || name.ends_with("-bookshelf.jpg")
                })
            {
                let id = path
                    .file_stem()
                    .and_then(|name| name.to_str())
                    .unwrap_or("bookshelf")
                    .to_string();
                themes.push(BookshelfTheme {
                    id,
                    image_path: path,
                    kind: BookshelfThemeKind::Legacy,
                    row_height: BOOKSHELF_ROW_HEIGHT,
                    surface_y: BOOKSHELF_SURFACE_Y,
                });
            }
        }
    }

    themes.sort_by_key(|theme| {
        (
            theme.kind != BookshelfThemeKind::Row,
            theme.id != "single",
            theme.id.to_ascii_lowercase(),
        )
    });
    themes
}

fn bookshelf_theme_for_appearance(appearance: AlbumAppearance) -> Option<BookshelfTheme> {
    if !appearance.bookshelf_enabled {
        return None;
    }
    let themes =
        bookshelf_themes_in(&crate::css::resolve_runtime_dir(Path::new(BOOKSHELF_THEME_DIRECTORY)));
    (!themes.is_empty()).then(|| themes[appearance.background_index % themes.len()].clone())
}

fn bookshelf_background_path(directory: &Path, index: usize) -> Option<PathBuf> {
    let themes = bookshelf_themes_in(directory);
    (!themes.is_empty()).then(|| themes[index % themes.len()].image_path.clone())
}

fn bookshelf_card_margin_top(surface_y: i32, cover_height: i32) -> i32 {
    (surface_y - cover_height).max(0)
}

fn bookshelf_background_rules(directory: &Path) -> String {
    bookshelf_themes_in(directory)
        .into_iter()
        .enumerate()
        .filter_map(|(index, theme)| {
            (theme.kind == BookshelfThemeKind::Legacy).then(|| {
                let uri = gio::File::for_path(theme.image_path).uri();
                format!(
                    ".albums-bookshelf-{index} {{ background-image: url(\"{uri}\"); \
                     background-size: 100% auto; background-repeat: repeat-y; \
                     background-position: center top; }}"
                )
            })
        })
        .collect()
}

fn bookshelf_background_css_class(appearance: AlbumAppearance) -> Option<String> {
    let themes =
        bookshelf_themes_in(&crate::css::resolve_runtime_dir(Path::new(BOOKSHELF_THEME_DIRECTORY)));
    if !appearance.bookshelf_enabled || themes.is_empty() {
        return None;
    }
    let index = appearance.background_index % themes.len();
    (themes[index].kind == BookshelfThemeKind::Legacy).then(|| format!("albums-bookshelf-{index}"))
}

fn bookshelf_columns_for_width(width: i32, _card_width: i32) -> usize {
    if width <= 0 {
        return BOOKSHELF_MAX_COLUMNS;
    }

    let usable_width = (width - BOOKSHELF_SIDE_PADDING * 2).max(BOOKSHELF_MIN_CARD_WIDTH);
    let per_card = BOOKSHELF_MIN_CARD_WIDTH + BOOKSHELF_COLUMN_GAP;
    ((usable_width + BOOKSHELF_COLUMN_GAP) / per_card).clamp(1, BOOKSHELF_MAX_COLUMNS as i32)
        as usize
}

fn bookshelf_target_card_width(width: i32, columns: usize) -> i32 {
    if width <= 0 || columns == 0 {
        return BOOKSHELF_MIN_CARD_WIDTH;
    }

    let usable_width = (width - BOOKSHELF_SIDE_PADDING * 2).max(BOOKSHELF_MIN_CARD_WIDTH);
    let slot_width = usable_width / columns as i32;
    ((slot_width as f64 * BOOKSHELF_CARD_FILL_RATIO).round() as i32)
        .clamp(BOOKSHELF_MIN_CARD_WIDTH, BOOKSHELF_MAX_CARD_WIDTH)
}

fn bookshelf_row(theme: &BookshelfTheme) -> gtk::Overlay {
    let row = gtk::Overlay::new();
    row.set_hexpand(true);
    row.set_vexpand(false);
    row.set_height_request(theme.row_height);
    row.set_size_request(-1, theme.row_height);
    row.set_overflow(gtk::Overflow::Hidden);
    row.add_css_class("albums-bookshelf-row");

    let background = gtk::Picture::new();
    background.set_filename(Some(&theme.image_path));
    background.set_content_fit(gtk::ContentFit::Fill);
    background.set_can_shrink(true);
    background.set_hexpand(true);
    background.set_vexpand(true);
    background.set_halign(gtk::Align::Fill);
    background.set_valign(gtk::Align::Fill);
    background.set_can_target(false);
    row.set_child(Some(&background));
    row
}

fn bookshelf_row_count(card_count: usize, columns: usize) -> usize {
    let columns = columns.max(1);
    let required = card_count.div_ceil(columns);
    required.max(BOOKSHELF_MIN_ROWS)
}

fn build_bookshelf_rows(
    rows: &gtk::Box,
    cards: &[gtk::Button],
    columns: usize,
    theme: &BookshelfTheme,
) {
    clear_box(rows);
    let columns = columns.clamp(1, BOOKSHELF_MAX_COLUMNS);
    let row_count = bookshelf_row_count(cards.len(), columns);

    for row_index in 0..row_count {
        let start = (row_index * columns).min(cards.len());
        let end = (start + columns).min(cards.len());
        let chunk = &cards[start..end];

        let row = bookshelf_row(theme);
        let slots = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        slots.set_homogeneous(true);
        slots.set_hexpand(true);
        slots.set_vexpand(false);
        slots.set_halign(gtk::Align::Fill);
        slots.set_valign(gtk::Align::Start);
        slots.set_height_request(theme.row_height);

        for index in 0..columns {
            let slot = gtk::Box::new(gtk::Orientation::Vertical, 0);
            slot.set_hexpand(true);
            slot.set_vexpand(false);
            slot.set_halign(gtk::Align::Fill);
            slot.set_valign(gtk::Align::Start);
            if let Some(card) = chunk.get(index) {
                slot.append(card);
            }
            slots.append(&slot);
        }

        row.add_overlay(&slots);
        rows.append(&row);
    }
}

fn collect_bookshelf_cards(root: &gtk::Widget, cards: &mut Vec<gtk::Button>) {
    if let Some(button) = root.downcast_ref::<gtk::Button>() {
        if button.has_css_class("album-card") {
            cards.push(button.clone());
            return;
        }
    }
    let mut child = root.first_child();
    while let Some(current) = child {
        collect_bookshelf_cards(&current, cards);
        child = current.next_sibling();
    }
}

fn resize_bookshelf_cards(rows: &gtk::Box, target_width: i32, theme: &BookshelfTheme) {
    let mut cards = Vec::new();
    collect_bookshelf_cards(rows.upcast_ref(), &mut cards);

    for card in cards {
        let Some(cover) = find_descendant_with_css_class(card.upcast_ref(), "album-cover")
            .and_then(|widget| widget.downcast::<gtk::Overlay>().ok())
        else {
            continue;
        };

        let current_width = cover.width_request();
        let current_height = cover.height_request();
        if current_width <= 0 || current_height <= 0 || current_width == target_width {
            continue;
        }

        let scale_x = target_width as f64 / current_width as f64;
        let target_height =
            ((current_height as f64 * scale_x).round() as i32).clamp(1, theme.surface_y.max(1));

        if let Some(picture) = find_descendant_with_css_class(cover.upcast_ref(), "thumbnail") {
            // Recompute from the original opening instead of rescaling rounded
            // margins, which can drift by a pixel after each resize.
            if let Some(opening) = unsafe { cover.data::<PhotoOpening>(FRAME_OPENING_KEY) }
                .map(|opening| unsafe { *opening.as_ref() })
            {
                let (left, top, right, bottom) =
                    photo_margins(opening, target_width, target_height);
                picture.set_margin_start(left);
                picture.set_margin_top(top);
                picture.set_margin_end(right);
                picture.set_margin_bottom(bottom);
            }
        }

        cover.set_width_request(target_width);
        cover.set_height_request(target_height);
        cover.set_size_request(target_width, target_height);

        card.set_width_request(target_width);
        card.set_margin_top(bookshelf_card_margin_top(theme.surface_y, target_height));

        if let Some(content) = card.child() {
            content.set_width_request(target_width);
            content.set_size_request(target_width, -1);
        }
    }
}

fn reflow_bookshelf_rows(runtime: &BookshelfRuntime, columns: usize, theme: &BookshelfTheme) {
    let mut cards = Vec::new();
    collect_bookshelf_cards(runtime.rows.upcast_ref(), &mut cards);
    if cards.is_empty() {
        runtime.last_columns.set(columns);
        return;
    }

    // Keep strong references while detaching cards from their old slot boxes.
    for card in &cards {
        if let Some(parent) = card.parent().and_downcast::<gtk::Box>() {
            parent.remove(card);
        }
    }
    build_bookshelf_rows(&runtime.rows, &cards, columns, theme);
    runtime.last_columns.set(columns);

    
}

fn insert_child(
    cards: &gtk::FlowBox,
    child: &impl IsA<gtk::Widget>,
    width: Option<i32>,
    height: Option<i32>,
) {
    cards.insert(child, -1);

    // GtkFlowBox automatically wraps every inserted widget in a
    // GtkFlowBoxChild. Prevent that wrapper from stretching the album card.
    if let Some(flow_child) = cards
        .last_child()
        .and_then(|child| child.downcast::<gtk::FlowBoxChild>().ok())
    {
        if let Some(width) = width {
            flow_child.set_width_request(width);
        }
        if let Some(height) = height {
            flow_child.set_height_request(height);
        }
        flow_child.set_hexpand(false);
        flow_child.set_vexpand(false);
        flow_child.set_halign(gtk::Align::Start);
        flow_child.set_valign(gtk::Align::Start);
    }
}
