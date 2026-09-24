const OPENING_ALPHA_THRESHOLD: u8 = 32;
// An opening smaller than this fraction of either axis is treated as noise.
const MIN_OPENING_FRACTION: f64 = 0.05;

/// The rectangular window a frame leaves transparent for the cover photo,
/// normalized against the frame's own size.
#[derive(Clone, Copy, Debug, PartialEq)]
struct PhotoOpening {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl PhotoOpening {
    /// Used when a frame's alpha channel has no readable opening (for example a
    /// JPEG without transparency). Matches the original supplied skins.
    const FALLBACK: Self = Self {
        x: 0.18,
        y: 53.0 / 500.0,
        width: 0.64,
        height: 273.0 / 500.0,
    };
}

/// A discovered cover frame with its decoded art and the opening that art
/// exposes, so every design keeps its own photo geometry.
struct FrameAsset {
    path: PathBuf,
    texture: gtk::gdk::Texture,
    opening: PhotoOpening,
}

thread_local! {
    static OPENING_CACHE: RefCell<HashMap<(PathBuf, Option<SystemTime>), PhotoOpening>> =
        RefCell::new(HashMap::new());
}

/// Find the transparent opening enclosed by the frame's opaque pixels.
///
/// Scanning out from the centre avoids the transparent margins around the
/// artwork, which also reach the image border.
fn opening_from_alpha(
    alpha: &[u8],
    width: usize,
    height: usize,
    threshold: u8,
) -> Option<PhotoOpening> {
    if width == 0 || height == 0 || alpha.len() < width * height {
        return None;
    }
    let at = |x: usize, y: usize| alpha[y * width + x];
    for (fx, fy) in [
        (0.5, 0.5),
        (0.5, 0.35),
        (0.5, 0.65),
        (0.35, 0.5),
        (0.65, 0.5),
    ] {
        let cx = ((width as f64 * fx) as usize).min(width - 1);
        let cy = ((height as f64 * fy) as usize).min(height - 1);
        if at(cx, cy) >= threshold {
            continue;
        }
        let mut top = cy;
        while top > 0 && at(cx, top - 1) < threshold {
            top -= 1;
        }
        let mut bottom = cy;
        while bottom + 1 < height && at(cx, bottom + 1) < threshold {
            bottom += 1;
        }
        let row = (top + bottom) / 2;
        let mut left = cx;
        while left > 0 && at(left - 1, row) < threshold {
            left -= 1;
        }
        let mut right = cx;
        while right + 1 < width && at(right + 1, row) < threshold {
            right += 1;
        }
        let opening = PhotoOpening {
            x: left as f64 / width as f64,
            y: top as f64 / height as f64,
            width: (right - left + 1) as f64 / width as f64,
            height: (bottom - top + 1) as f64 / height as f64,
        };
        let enclosed = opening.x > 0.0
            && opening.y > 0.0
            && opening.x + opening.width < 1.0
            && opening.y + opening.height < 1.0;
        if enclosed
            && opening.width >= MIN_OPENING_FRACTION
            && opening.height >= MIN_OPENING_FRACTION
        {
            return Some(opening);
        }
    }
    None
}

fn detect_photo_opening(path: &Path) -> Option<PhotoOpening> {
    let decoded = image::open(path).ok()?.to_rgba8();
    let (width, height) = decoded.dimensions();
    let alpha: Vec<u8> = decoded.pixels().map(|pixel| pixel.0[3]).collect();
    opening_from_alpha(
        &alpha,
        width as usize,
        height as usize,
        OPENING_ALPHA_THRESHOLD,
    )
}

/// Decoding every frame is only worth doing once per file revision, because
/// the album view repopulates whenever the library or the theme changes.
fn frame_photo_opening(path: &Path) -> PhotoOpening {
    let modified = std::fs::metadata(path)
        .ok()
        .and_then(|metadata| metadata.modified().ok());
    let key = (path.to_path_buf(), modified);
    if let Some(cached) = OPENING_CACHE.with(|cache| cache.borrow().get(&key).copied()) {
        return cached;
    }
    let opening = detect_photo_opening(path).unwrap_or(PhotoOpening::FALLBACK);
    OPENING_CACHE.with(|cache| {
        cache.borrow_mut().insert(key, opening);
    });
    opening
}

fn album_frame_paths_for_appearance(directory: &Path, appearance: AlbumAppearance) -> Vec<PathBuf> {
    if !appearance.covers_enabled {
        Vec::new()
    } else {
        album_frame_paths_in(directory)
    }
}

fn album_frame_paths_in(directory: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(entries) = std::fs::read_dir(directory) {
        for path in entries.filter_map(Result::ok).map(|entry| entry.path()) {
            if path.is_dir() {
                collect_matching_files(&path, &mut paths, &|name| name.ends_with("-frame.png"));
            }
        }
    }
    paths.sort();
    paths
}

pub(crate) fn album_cover_theme_count() -> usize {
    let root = crate::css::resolve_runtime_dir(Path::new(ALBUM_COVER_THEME_DIRECTORY));
    cover_themes(&root, &album_frame_paths_in(&root)).len()
}

fn collect_matching_files(
    directory: &Path,
    paths: &mut Vec<PathBuf>,
    matches: &impl Fn(&str) -> bool,
) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            collect_matching_files(&path, paths, matches);
        } else if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(matches)
        {
            paths.push(path);
        }
    }
}

pub(crate) fn bookshelf_background_count() -> usize {
    bookshelf_themes_in(&crate::css::resolve_runtime_dir(Path::new(BOOKSHELF_THEME_DIRECTORY))).len()
}

fn png_or_jpg_paths_matching(directory: &Path, matches: impl Fn(&str) -> bool) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut paths: Vec<_> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(&matches)
        })
        .collect();
    paths.sort();
    paths
}

/// Cover frames grouped by theme folder: the top-level directory under
/// `images/theme/album-covers`. A theme is one design family and may ship
/// several colour variants, so `Next Album Cover` walks folders, not files.
struct CoverTheme {
    name: String,
    frames: Vec<PathBuf>,
}

impl CoverTheme {
    /// The frame this album uses inside the theme.
    ///
    /// A theme with a single file always shows it; a theme with several variants
    /// gives the album a stable one chosen by hashing its id and name, so the
    /// page still draws only this theme.
    fn frame_for(&self, album: &Album) -> Option<&PathBuf> {
        self.frames
            .get(hash_slot(album, b"variant", self.frames.len()))
    }
}

fn cover_theme_name(root: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?;
    let component = relative.components().next()?;
    Some(component.as_os_str().to_string_lossy().into_owned())
}

/// `frames` arrives path-sorted, so themes come out alphabetically and every
/// theme's variants stay sorted too.
fn cover_themes(root: &Path, frames: &[PathBuf]) -> Vec<CoverTheme> {
    let mut themes: Vec<CoverTheme> = Vec::new();
    for path in frames {
        let Some(name) = cover_theme_name(root, path) else {
            continue;
        };
        match themes.iter_mut().find(|theme| theme.name == name) {
            Some(theme) => theme.frames.push(path.clone()),
            None => themes.push(CoverTheme {
                name,
                frames: vec![path.clone()],
            }),
        }
    }
    themes
}

/// A defined hash rather than a process-seeded or unspecified hasher, so an
/// album's design is stable across runs.
fn album_hash(album: &Album, salt: &[u8]) -> u64 {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&album.id.to_le_bytes());
    hasher.update(album.name.as_bytes());
    hasher.update(salt);
    let hash = hasher.finalize();
    u64::from_le_bytes(hash.as_bytes()[..8].try_into().unwrap())
}

fn hash_slot(album: &Album, salt: &[u8], count: usize) -> usize {
    debug_assert!(count > 0);
    (album_hash(album, salt) % count as u64) as usize
}

/// Albums that never picked a frame of their own follow the page's single
/// selected theme, so one page never mixes two themes. Only the variant inside
/// that theme differs between albums, and it stays stable across refreshes.
fn automatic_frame<'a>(
    album: &Album,
    themes: &'a [CoverTheme],
    cover_index: usize,
) -> Option<&'a PathBuf> {
    if themes.is_empty() {
        return None;
    }
    themes.get(cover_index % themes.len())?.frame_for(album)
}

fn frame_identifier(root: &Path, path: &Path) -> Option<String> {
    path.strip_prefix(root)
        .ok()
        .map(|relative| relative.to_string_lossy().into_owned())
}

/// The frame an album shows: its own choice while that file still exists,
/// otherwise the automatic theme assignment.
fn selected_frame_path(
    album: &Album,
    root: &Path,
    themes: &[CoverTheme],
    cover_index: usize,
) -> Option<PathBuf> {
    if let Some(selected) = album.cover_frame.as_deref() {
        let chosen = themes
            .iter()
            .flat_map(|theme| theme.frames.iter())
            .find(|path| frame_identifier(root, path).as_deref() == Some(selected));
        if let Some(path) = chosen {
            return Some(path.clone());
        }
    }
    automatic_frame(album, themes, cover_index).cloned()
}

fn selected_frame_index(
    album: &Album,
    root: &Path,
    frames: &[PathBuf],
    cover_index: usize,
) -> Option<usize> {
    if frames.is_empty() {
        return None;
    }
    let themes = cover_themes(root, frames);
    let selected = selected_frame_path(album, root, &themes, cover_index)?;
    frames.iter().position(|path| path == &selected)
}

/// `Next Album Cover` steps to the next theme folder while keeping the
/// album's stable variant inside it, so five themes cycle in five steps.
fn next_theme_identifier(
    album: &Album,
    root: &Path,
    frames: &[PathBuf],
    cover_index: usize,
) -> Option<String> {
    let themes = cover_themes(root, frames);
    let current = selected_frame_path(album, root, &themes, cover_index)?;
    let current_theme = themes
        .iter()
        .position(|theme| theme.frames.contains(&current))?;
    let next_theme = themes.get((current_theme + 1) % themes.len())?;
    frame_identifier(root, next_theme.frame_for(album)?)
}

fn change_to_next_album_cover(
    connection: &Connection,
    album: &Album,
    root: &Path,
    frames: &[PathBuf],
    cover_index: usize,
) -> anyhow::Result<Option<String>> {
    let Some(next) = next_theme_identifier(album, root, frames, cover_index) else {
        return Ok(None);
    };
    db::set_album_cover_frame(connection, album.id, &next)?;
    Ok(Some(next))
}

fn cover_width(thumbnail_width: i32) -> i32 {
    thumbnail_width.clamp(100, 300)
}

fn cover_height(thumbnail_width: i32) -> i32 {
    ((cover_width(thumbnail_width) as f64 * DEFAULT_THUMBNAIL_HEIGHT as f64
        / DEFAULT_THUMBNAIL_WIDTH as f64)
        .round()
        .max(1.0)) as i32
}

fn plain_cover_height(width: i32, photo_dimensions: Option<(i32, i32)>) -> i32 {
    let Some((photo_width, photo_height)) = photo_dimensions.filter(|(w, h)| *w > 0 && *h > 0)
    else {
        return cover_height(width);
    };
    // Keep unusually wide or tall photos within a usable card while the
    // picture's Contain fit still shows every pixel without distortion.
    let ratio = (photo_height as f64 / photo_width as f64).clamp(0.5, 1.5);
    (width as f64 * ratio).round().max(1.0) as i32
}

fn picture_dimensions(picture: &gtk::Picture) -> Option<(i32, i32)> {
    picture
        .paintable()
        .map(|paintable| (paintable.intrinsic_width(), paintable.intrinsic_height()))
}

/// The photo an album draws on its card. A cover chosen from the thumbnail
/// menu wins even when that photo is not a member of the album; otherwise the
/// album keeps its automatic first-thumbnail pick.
fn album_cover_photo(
    connection: &Connection,
    album: &Album,
    photos: &[db::Photo],
) -> Option<db::Photo> {
    if let Some(id) = album.cover_photo_id {
        let chosen = photos
            .iter()
            .find(|photo| photo.id == id)
            .cloned()
            .or_else(|| db::photo(connection, id).ok().flatten());
        if chosen.is_some() {
            return chosen;
        }
    }
    photos
        .iter()
        .find(|photo| {
            crate::thumbnail::existing_cache_path(&photo.path, photo.mtime, photo.size_bytes)
                .ok()
                .flatten()
                .is_some()
        })
        .cloned()
}

fn album_card(
    album: &Album,
    connection: &Connection,
    thumbnail_width: i32,
    on_album: Rc<dyn Fn(i64)>,
    frame: Option<&FrameAsset>,
    responsive_bookshelf: bool,
    fit_whole_photo: bool,
    menu_connection: Rc<RefCell<Connection>>,
    on_appearance_changed: Rc<dyn Fn()>,
) -> gtk::Button {
    let card = gtk::Button::new();
    card.set_has_frame(false);
    card.set_hexpand(false);
    card.set_vexpand(false);
    card.set_halign(gtk::Align::Start);
    card.set_valign(gtk::Align::Start);
    card.add_css_class("album-card");

    let height = if frame.is_some() {
        (thumbnail_width as f64 * 500.0 / 805.0).round() as i32
    } else {
        cover_height(thumbnail_width)
    };
    let width = frame
        .map(|frame| {
            (height as f64 * frame.texture.width() as f64 / frame.texture.height() as f64).round()
                as i32
        })
        .unwrap_or_else(|| cover_width(thumbnail_width));
    card.set_width_request(width);
    let content = gtk::Box::new(gtk::Orientation::Vertical, 7);
    content.set_size_request(width, -1);
    content.set_width_request(width);
    content.set_overflow(gtk::Overflow::Hidden);
    content.set_hexpand(false);
    content.set_vexpand(false);
    content.set_halign(gtk::Align::Start);
    if frame.is_some() {
        content.set_halign(gtk::Align::Center);
        card.add_css_class("framed-album-card");
    }

    // Match the normal photo-grid thumbnail allocation.
    let cover = gtk::Overlay::new();
    cover.set_width_request(width);
    cover.set_height_request(height);
    cover.set_size_request(width, height);
    cover.set_hexpand(false);
    cover.set_vexpand(false);
    cover.set_halign(gtk::Align::Start);
    cover.set_valign(gtk::Align::Start);
    cover.set_overflow(gtk::Overflow::Hidden);
    cover.add_css_class("album-cover");
    if frame.is_none() {
        cover.add_css_class("photo-frame");
        cover.add_css_class("photo-tile");
        cover.add_css_class("plain-album-cover");
    }

    let picture = gtk::Picture::new();

    picture.set_content_fit(if fit_whole_photo {
        gtk::ContentFit::Contain
    } else {
        gtk::ContentFit::Cover
    });

    picture.set_can_shrink(true);
    picture.set_size_request(1, 1);
    picture.set_hexpand(true);
    picture.set_vexpand(true);
    picture.set_halign(gtk::Align::Fill);
    picture.set_valign(gtk::Align::Fill);
    picture.set_overflow(gtk::Overflow::Hidden);
    picture.add_css_class("thumbnail");

    let photos = db::photos_in_album(connection, album.id, None).unwrap_or_default();
    // The chosen cover wins over the automatic pick, even before its cached
    // thumbnail exists.
    let cover_photo = album_cover_photo(connection, album, &photos).map(|photo| {
        let cached =
            crate::thumbnail::existing_cache_path(&photo.path, photo.mtime, photo.size_bytes)
                .ok()
                .flatten();
        (photo, cached)
    });

    

    if let Some((photo, cached)) = cover_photo.as_ref() {
        // A cover without a cached thumbnail yet still draws from its original
        // file, so a fresh choice is visible straight away.
        let source = cached
            .as_deref()
            .unwrap_or_else(|| Path::new(photo.path.as_str()));
        let path_string = source.to_string_lossy();

        // Use the same RAW/NEF crop as the normal gallery. Some embedded RAW
        // previews contain side bars that GTK's Cover mode cannot remove.
        let photo_object = crate::photo_object::PhotoObject::from_photo(photo);
        if let Some(cropped) = crate::grid::raw_cached_thumbnail(&photo_object, &path_string) {
            picture.set_paintable(Some(&cropped));
        // Respect user rotation without regenerating the cached thumbnail.
        } else if let Some(rotated) =
            crate::photo_texture::edited_thumbnail(&path_string, photo.rotation, &photo.edit_recipe)
        {
            picture.set_paintable(Some(&rotated));
        } else {
            picture.set_filename(Some(path_string.as_ref()));
        }
    } else {
        picture.set_paintable(gtk::gdk::Paintable::NONE);
    }

    if frame.is_none() && !responsive_bookshelf && fit_whole_photo {
        let height = plain_cover_height(width, picture_dimensions(&picture));
        cover.set_height_request(height);
        cover.set_size_request(width, height);
    }

    if frame.is_none() {
        // The picture's natural portrait height must not enlarge the cover's
        // fixed landscape box when cropping is selected. Overlay children do
        // not contribute to the cover's preferred size.
        cover.set_child(Some(&gtk::Box::new(gtk::Orientation::Vertical, 0)));
        cover.add_overlay(&picture);
    } else {
        cover.set_child(Some(&picture));
    }

    let placeholder = gtk::Image::from_icon_name("folder-pictures-symbolic");
    placeholder.set_pixel_size(48);
    placeholder.set_halign(gtk::Align::Center);
    placeholder.set_valign(gtk::Align::Center);
    placeholder.add_css_class("dim-label");
    placeholder.set_visible(cover_photo.is_none());
    cover.add_overlay(&placeholder);

    if frame.is_none() && !responsive_bookshelf && cover_photo.is_none() {
        let empty_label = gtk::Label::new(Some(if photos.is_empty() {
            "No photos yet"
        } else {
            "Preview unavailable"
        }));
        empty_label.add_css_class("dim-label");
        empty_label.set_halign(gtk::Align::Center);
        empty_label.set_valign(gtk::Align::Center);
        empty_label.set_margin_top(80);
        cover.add_overlay(&empty_label);
    }

    if let Some(frame) = frame {
        // The cached photo stays below the transparent opening; the PNG is the top layer.
        unsafe {
            cover.set_data(FRAME_OPENING_KEY, frame.opening);
        }
        let (left, top, right, bottom) = photo_margins(frame.opening, width, height);
        for widget in [
            picture.upcast_ref::<gtk::Widget>(),
            placeholder.upcast_ref(),
        ] {
            widget.set_margin_start(left);
            widget.set_margin_top(top);
            widget.set_margin_end(right);
            widget.set_margin_bottom(bottom);
        }
        placeholder.set_halign(gtk::Align::Fill);
        placeholder.set_valign(gtk::Align::Fill);
        placeholder.set_pixel_size(24);
        let skin = gtk::Picture::for_paintable(&frame.texture);
        skin.set_content_fit(gtk::ContentFit::Contain);
        skin.set_can_shrink(true);
        skin.set_size_request(1, 1);
        skin.set_halign(gtk::Align::Fill);
        skin.set_valign(gtk::Align::Fill);
        skin.set_can_target(false);
        skin.add_css_class("album-skin");
        cover.add_overlay(&skin);
    }

    // Album covers are rectangular and need to grow with the bookshelf.
    // Keep the cover itself in normal GTK layout instead of wrapping it in the
    // photo-grid tile widget, whose fixed allocation prevents responsive growth.
    content.append(&cover);

    // Album name below cover.
    let name = gtk::Label::new(Some(&album.name));
    name.add_css_class("album-name");
    name.set_xalign(0.0);
    name.set_width_request(width);
    name.set_size_request(width, -1);
    name.set_hexpand(false);
    name.set_overflow(gtk::Overflow::Hidden);
    name.set_max_width_chars(20);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    name.set_tooltip_text(Some(&album.name));
    if frame.is_some() {
        name.set_xalign(0.5);
    }
    content.append(&name);

    // Photo count below album name.
    let count_text = if album.photo_count == 1 {
        "1 photo".to_string()
    } else {
        format!("{} photos", album.photo_count)
    };

    let photo_count = gtk::Label::new(Some(&count_text));
    photo_count.add_css_class("album-photo-count");
    photo_count.set_xalign(0.0);
    photo_count.add_css_class("dim-label");
    if responsive_bookshelf {
        photo_count.add_css_class("albums-bookshelf-photo-count");
    }
    if frame.is_some() {
        photo_count.set_xalign(0.5);
    }
    content.append(&photo_count);

    card.set_child(Some(&content));
    install_album_context_menu(&card, menu_connection, album.clone(), on_appearance_changed);

    

    let album_id = album.id;
    card.connect_clicked(move |_| {
        on_album(album_id);
    });

    card
}

fn photo_margins(opening: PhotoOpening, width: i32, height: i32) -> (i32, i32, i32, i32) {
    // Round outward and add a pixel of underlap, so a frame's antialiased
    // opening edge cannot show a seam around the photo.
    (
        ((width as f64 * opening.x).floor() as i32 - 1).max(0),
        ((height as f64 * opening.y).floor() as i32 - 1).max(0),
        ((width as f64 * (1.0 - opening.x - opening.width)).floor() as i32 - 1).max(0),
        ((height as f64 * (1.0 - opening.y - opening.height)).floor() as i32 - 1).max(0),
    )
}
