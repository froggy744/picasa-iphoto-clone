use std::sync::mpsc::TryRecvError;

static REFRESH_GENERATION: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

fn refresh_grid(
    connection: &Rc<RefCell<Connection>>,
    filter: sidebar::SidebarFilter,
    search: &str,
    sort: PhotoSort,
    gallery: &Rc<grid::Gallery>,
) {
    let folder_target = if search.is_empty() {
        if let sidebar::SidebarFilter::Folder(folder_id) = filter {
            db::folders(&connection.borrow())
                .ok()
                .and_then(|folders| folders.into_iter().find(|folder| folder.id == folder_id))
                .map(|folder| (folder.id, folder.path))
        } else {
            None
        }
    } else {
        None
    };
    refresh_grid_inner(connection, filter, search, sort, gallery, folder_target);
}

fn refresh_grid_to_folder(
    connection: &Rc<RefCell<Connection>>,
    filter: sidebar::SidebarFilter,
    search: &str,
    sort: PhotoSort,
    gallery: &Rc<grid::Gallery>,
    folder_id: i64,
    folder_path: String,
) {
    refresh_grid_inner(
        connection,
        filter,
        search,
        sort,
        gallery,
        Some((folder_id, folder_path)),
    );
}

fn refresh_grid_inner(
    connection: &Rc<RefCell<Connection>>,
    filter: sidebar::SidebarFilter,
    search: &str,
    sort: PhotoSort,
    gallery: &Rc<grid::Gallery>,
    folder_target: Option<(i64, String)>,
) {
    let _ = connection;
    if filter == sidebar::SidebarFilter::Albums {
        return;
    }

    let generation = REFRESH_GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    let search = search.to_owned();
    let (sender, receiver) = std::sync::mpsc::channel();

    std::thread::spawn(move || {
        let Ok(connection) = db::open_default() else {
            let _ = sender.send(None);
            return;
        };

        let folder_stream = search.is_empty()
            && matches!(filter, sidebar::SidebarFilter::Folder(_));
        let mut photos = if !search.is_empty() {
            // An active search is a library-wide view, regardless of the
            // destination that was selected before typing began.
            db::photos(&connection, None, false, Some(&search)).unwrap_or_default()
        } else if let sidebar::SidebarFilter::Album(album_id) = filter {
            db::photos_in_album(&connection, album_id, None).unwrap_or_default()
        } else {
            let (folder_id, favorites) = match filter {
                sidebar::SidebarFilter::All | sidebar::SidebarFilter::RecentlyAdded => {
                    (None, false)
                }
                sidebar::SidebarFilter::Favorites => (None, true),
                // Picasa-style Folder mode is a single continuous stream.
                // The selected folder is a scroll destination, not a query
                // boundary, so load every indexed folder exactly once.
                sidebar::SidebarFilter::Folder(_) => (None, false),
                sidebar::SidebarFilter::Albums => return,
                sidebar::SidebarFilter::Album(_) => unreachable!(),
            };
            db::photos(&connection, folder_id, favorites, None).unwrap_or_default()
        };

        retain_enabled_formats(&connection, &mut photos);
        limit_recently_added(&connection, filter, &mut photos);
        if folder_stream {
            let folders = db::folders(&connection).unwrap_or_default();
            let display_mode = sidebar::FolderDisplayMode::from_setting(
                db::setting(&connection, sidebar::FOLDER_DISPLAY_MODE_SETTING_KEY)
                    .ok()
                    .flatten()
                    .as_deref(),
            );
            sort_folder_stream(&mut photos, &folders, sort, display_mode);
        } else {
            sort_photos(&mut photos, sort);
        }
        let _ = sender.send(Some(photos));
    });

    let gallery = gallery.clone();
    glib::timeout_add_local(std::time::Duration::from_millis(25), move || {
        match receiver.try_recv() {
            Ok(Some(photos)) => {
                if REFRESH_GENERATION.load(std::sync::atomic::Ordering::Relaxed) == generation {
                    gallery.replace(&photos);
                    if let Some((folder_id, folder_path)) = folder_target.clone() {
                        let gallery = gallery.clone();
                        // replace() may schedule a progressive model build.
                        // Start the scroll helper on the next main-loop turn so
                        // it never succeeds against the previous grid model.
                        glib::idle_add_local_once(move || {
                            scroll_gallery_to_folder_when_ready(
                                gallery,
                                folder_id,
                                folder_path,
                            );
                        });
                    }
                }
                glib::ControlFlow::Break
            }
            Ok(None) | Err(TryRecvError::Disconnected) => glib::ControlFlow::Break,
            Err(TryRecvError::Empty) => glib::ControlFlow::Continue,
        }
    });
}

/// Scroll a sidebar folder selection to its section once a potentially
/// progressive Folder-stream replacement has loaded far enough to contain it.
fn scroll_gallery_to_folder_when_ready(
    gallery: Rc<grid::Gallery>,
    folder_id: i64,
    folder_path: String,
) {
    let attempts = Rc::new(Cell::new(0u32));
    let attempts_for_timer = attempts.clone();
    glib::timeout_add_local(Duration::from_millis(25), move || {
        attempts_for_timer.set(attempts_for_timer.get() + 1);
        if gallery.scroll_to_folder(folder_id, &folder_path) {
            glib::ControlFlow::Break
        } else if attempts_for_timer.get() >= 240 {
            // Six seconds is intentionally generous for a very large library
            // using progressive ListStore replacement. A missing/empty folder
            // simply leaves the current scroll position unchanged.
            glib::ControlFlow::Break
        } else {
            glib::ControlFlow::Continue
        }
    });
}

fn limit_recently_added(
    connection: &Connection,
    filter: sidebar::SidebarFilter,
    photos: &mut Vec<db::Photo>,
) {
    if filter != sidebar::SidebarFilter::RecentlyAdded {
        return;
    }

    // Select the newest files first, then restore the user's chosen display
    // ordering in refresh_grid().
    sort_photos(
        photos,
        PhotoSort {
            field: SortField::DateAdded,
            direction: SortDirection::Descending,
        },
    );
    photos.truncate(db::recently_added_limit(connection));
}

fn retain_enabled_formats(connection: &Connection, photos: &mut Vec<db::Photo>) {
    let enabled = crate::image_format::enabled_ids(connection).unwrap_or_else(|_| {
        crate::image_format::all()
            .iter()
            .map(|format| format.id)
            .collect()
    });
    photos.retain(|photo| crate::image_format::path_is_enabled_in(&enabled, &photo.path));
}

fn sort_photos(photos: &mut [db::Photo], sort: PhotoSort) {
    photos.sort_by(|left, right| photo_ordering(left, right, sort));
}

fn photo_ordering(left: &db::Photo, right: &db::Photo, sort: PhotoSort) -> Ordering {
    let ordering = match sort.field {
        SortField::DateTaken => compare_optional(
            left.taken_at.as_deref(),
            right.taken_at.as_deref(),
            sort.direction,
        ),
        SortField::Name => directed_ordering(
            crate::source::filename(&left.path)
                .to_lowercase()
                .cmp(&crate::source::filename(&right.path).to_lowercase()),
            sort.direction,
        ),
        SortField::FileSize => compare_optional(left.size_bytes, right.size_bytes, sort.direction),
        SortField::Dimensions => compare_optional(
            pixel_count(left.width, left.height),
            pixel_count(right.width, right.height),
            sort.direction,
        ),
        SortField::DateAdded => directed_ordering(
            left.added_at
                .cmp(&right.added_at)
                .then(left.id.cmp(&right.id)),
            sort.direction,
        ),
    };

    ordering.then_with(|| left.path.to_lowercase().cmp(&right.path.to_lowercase()))
}

/// Order the Folder view as one stream of direct-folder sections. Photos are
/// sorted normally inside each section, while folder sections follow the same
/// hierarchy users see in the sidebar. Imported-only mode keeps imported roots
/// together and in the same alphabetical root order as that sidebar mode.
fn sort_folder_stream(
    photos: &mut [db::Photo],
    folders: &[db::Folder],
    sort: PhotoSort,
    display_mode: sidebar::FolderDisplayMode,
) {
    let order = folder_tree_order(folders);
    let tree_rank = order
        .iter()
        .enumerate()
        .map(|(index, folder_id)| (*folder_id, index))
        .collect::<std::collections::HashMap<_, _>>();
    let by_id = folders
        .iter()
        .map(|folder| (folder.id, folder))
        .collect::<std::collections::HashMap<_, _>>();

    let imported_rank = if display_mode == sidebar::FolderDisplayMode::ImportedOnly {
        let mut roots = folders
            .iter()
            .filter(|folder| folder.imported_root)
            .collect::<Vec<_>>();
        roots.sort_by(|left, right| {
            left.name
                .to_lowercase()
                .cmp(&right.name.to_lowercase())
                .then_with(|| left.path.to_lowercase().cmp(&right.path.to_lowercase()))
        });
        roots
            .into_iter()
            .enumerate()
            .map(|(index, folder)| (folder.id, index))
            .collect::<std::collections::HashMap<_, _>>()
    } else {
        std::collections::HashMap::new()
    };

    let root_rank_by_folder = folders
        .iter()
        .map(|folder| {
            let rank = nearest_imported_root(folder.id, &by_id)
                .and_then(|root_id| imported_rank.get(&root_id).copied())
                .unwrap_or(usize::MAX);
            (folder.id, rank)
        })
        .collect::<std::collections::HashMap<_, _>>();

    photos.sort_by(|left, right| {
        let left_id = left.folder_id.unwrap_or_default();
        let right_id = right.folder_id.unwrap_or_default();
        let left_tree = tree_rank.get(&left_id).copied().unwrap_or(usize::MAX);
        let right_tree = tree_rank.get(&right_id).copied().unwrap_or(usize::MAX);

        let section_order = if display_mode == sidebar::FolderDisplayMode::ImportedOnly {
            root_rank_by_folder
                .get(&left_id)
                .copied()
                .unwrap_or(usize::MAX)
                .cmp(
                    &root_rank_by_folder
                        .get(&right_id)
                        .copied()
                        .unwrap_or(usize::MAX),
                )
                .then_with(|| left_tree.cmp(&right_tree))
        } else {
            left_tree.cmp(&right_tree)
        };

        section_order.then_with(|| photo_ordering(left, right, sort))
    });
}

fn folder_tree_order(folders: &[db::Folder]) -> Vec<i64> {
    let by_id = folders
        .iter()
        .map(|folder| (folder.id, folder))
        .collect::<std::collections::HashMap<_, _>>();
    let mut children = std::collections::HashMap::<Option<i64>, Vec<i64>>::new();
    for folder in folders {
        children.entry(folder.parent_id).or_default().push(folder.id);
    }
    for ids in children.values_mut() {
        ids.sort_by(|left, right| {
            let left = by_id.get(left).expect("folder id exists");
            let right = by_id.get(right).expect("folder id exists");
            left.name
                .to_lowercase()
                .cmp(&right.name.to_lowercase())
                .then_with(|| left.path.to_lowercase().cmp(&right.path.to_lowercase()))
        });
    }

    fn visit(
        folder_id: i64,
        children: &std::collections::HashMap<Option<i64>, Vec<i64>>,
        seen: &mut std::collections::HashSet<i64>,
        order: &mut Vec<i64>,
    ) {
        if !seen.insert(folder_id) {
            return;
        }
        order.push(folder_id);
        if let Some(ids) = children.get(&Some(folder_id)) {
            for child_id in ids {
                visit(*child_id, children, seen, order);
            }
        }
    }

    let mut order = Vec::with_capacity(folders.len());
    let mut seen = std::collections::HashSet::new();
    if let Some(roots) = children.get(&None) {
        for root_id in roots {
            visit(*root_id, &children, &mut seen, &mut order);
        }
    }
    // Corrupt/legacy parent links must not make a folder disappear from the
    // stream. Append any orphaned rows deterministically at the end.
    let mut remaining = folders
        .iter()
        .filter(|folder| !seen.contains(&folder.id))
        .collect::<Vec<_>>();
    remaining.sort_by(|left, right| left.path.to_lowercase().cmp(&right.path.to_lowercase()));
    for folder in remaining {
        visit(folder.id, &children, &mut seen, &mut order);
    }
    order
}

fn nearest_imported_root<'a>(
    folder_id: i64,
    by_id: &std::collections::HashMap<i64, &'a db::Folder>,
) -> Option<i64> {
    let mut current = Some(folder_id);
    let mut seen = std::collections::HashSet::new();
    while let Some(id) = current {
        if !seen.insert(id) {
            break;
        }
        let folder = by_id.get(&id)?;
        if folder.imported_root {
            return Some(id);
        }
        current = folder.parent_id;
    }
    None
}

fn compare_optional<T: Ord>(
    left: Option<T>,
    right: Option<T>,
    direction: SortDirection,
) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => directed_ordering(left.cmp(&right), direction),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn directed_ordering(ordering: Ordering, direction: SortDirection) -> Ordering {
    match direction {
        SortDirection::Ascending => ordering,
        SortDirection::Descending => ordering.reverse(),
    }
}

fn pixel_count(width: Option<i64>, height: Option<i64>) -> Option<i128> {
    match (width, height) {
        (Some(width), Some(height)) if width > 0 && height > 0 => {
            Some(i128::from(width) * i128::from(height))
        }
        _ => None,
    }
}

fn confirm_action(
    parent: &adw::ApplicationWindow,
    title: &str,
    message: &str,
    action: impl Fn() + 'static,
) {
    let dialog = gtk::MessageDialog::builder()
        .transient_for(parent)
        .modal(true)
        .message_type(gtk::MessageType::Warning)
        .buttons(gtk::ButtonsType::Cancel)
        .text(title)
        .secondary_text(message)
        .build();
    dialog.add_button("Continue", gtk::ResponseType::Accept);

    dialog.connect_response(move |dialog, response| {
        if response == gtk::ResponseType::Accept {
            action();
        }
        dialog.close();
    });
    dialog.present();
}

#[cfg(test)]
mod photo_action_tests {
    use super::{
        sort_folder_stream, sort_photos, valid_file_name, wallpaper_layout, PhotoSort,
        SortDirection, SortField, WallpaperLayout,
    };
    use crate::db::{Folder, Photo};

    #[test]
    fn rename_rejects_paths_and_accepts_a_file_name() {
        assert!(valid_file_name("holiday photo.jpg"));
        assert!(!valid_file_name(""));
        assert!(!valid_file_name(".."));
        assert!(!valid_file_name("folder/photo.jpg"));
    }

    #[test]
    fn wallpaper_layout_preserves_portraits_and_panorama_width() {
        assert_eq!(
            wallpaper_layout(3000, 4500, 1920, 1080),
            WallpaperLayout::PortraitBlur
        );
        assert_eq!(
            wallpaper_layout(6000, 4000, 1920, 1080),
            WallpaperLayout::Cover
        );
        assert_eq!(
            wallpaper_layout(8000, 2000, 1920, 1080),
            WallpaperLayout::PanoramaBlur
        );
    }

    fn photo(
        path: &str,
        taken_at: Option<&str>,
        size_bytes: Option<i64>,
        dimensions: Option<(i64, i64)>,
        mtime: Option<i64>,
    ) -> Photo {
        Photo {
            id: 0,
            path: path.to_string(),
            folder_id: None,
            folder_path: None,
            taken_at: taken_at.map(str::to_string),
            camera: None,
            width: dimensions.map(|value| value.0),
            height: dimensions.map(|value| value.1),
            size_bytes,
            mtime,
            added_at: mtime.unwrap_or_default(),
            rotation: 0,
            edit_recipe: String::new(),
            favorite: false,
            trashed: false,
        }
    }

    fn folder(
        id: i64,
        path: &str,
        name: &str,
        parent_id: Option<i64>,
        imported_root: bool,
    ) -> Folder {
        Folder {
            id,
            path: path.to_string(),
            name: name.to_string(),
            parent_id,
            imported_root,
            watched: false,
            photo_count: 1,
            subfolder_count: 0,
            available: true,
        }
    }

    fn photo_in_folder(path: &str, folder_id: i64, folder_path: &str) -> Photo {
        let mut value = photo(path, Some("2024-01-01 12:00:00"), None, None, None);
        value.folder_id = Some(folder_id);
        value.folder_path = Some(folder_path.to_string());
        value
    }

    #[test]
    fn folder_stream_uses_tree_section_order_not_global_photo_sort() {
        let folders = vec![
            folder(1, "/root", "root", None, true),
            folder(2, "/root/B", "B", Some(1), false),
            folder(3, "/root/A", "A", Some(1), false),
        ];
        let mut photos = vec![
            photo_in_folder("/root/B/b.jpg", 2, "/root/B"),
            photo_in_folder("/root/A/z.jpg", 3, "/root/A"),
            photo_in_folder("/root/A/a.jpg", 3, "/root/A"),
        ];

        sort_folder_stream(
            &mut photos,
            &folders,
            PhotoSort {
                field: SortField::Name,
                direction: SortDirection::Ascending,
            },
            crate::sidebar::FolderDisplayMode::Tree,
        );

        assert_eq!(
            photos
                .iter()
                .map(|photo| (photo.folder_id, crate::source::filename(&photo.path)))
                .collect::<Vec<_>>(),
            vec![
                (Some(3), "a.jpg".to_string()),
                (Some(3), "z.jpg".to_string()),
                (Some(2), "b.jpg".to_string()),
            ]
        );
    }

    #[test]
    fn imported_only_stream_follows_imported_root_order() {
        let folders = vec![
            folder(10, "/Pictures", "Pictures", None, true),
            folder(11, "/Pictures/Drone", "Drone", Some(10), false),
            folder(20, "/Data", "Data", None, true),
            folder(21, "/Data/Trips", "Trips", Some(20), false),
        ];
        let mut photos = vec![
            photo_in_folder("/Pictures/Drone/p.jpg", 11, "/Pictures/Drone"),
            photo_in_folder("/Data/Trips/d.jpg", 21, "/Data/Trips"),
        ];

        sort_folder_stream(
            &mut photos,
            &folders,
            PhotoSort {
                field: SortField::Name,
                direction: SortDirection::Ascending,
            },
            crate::sidebar::FolderDisplayMode::ImportedOnly,
        );

        assert_eq!(
            photos.iter().map(|photo| photo.folder_id).collect::<Vec<_>>(),
            vec![Some(21), Some(11)]
        );
    }

    #[test]
    fn photo_sort_supports_names_dates_sizes_and_dimensions() {
        let source = vec![
            photo("/photos/z.jpg", Some("2022"), Some(20), None, Some(2)),
            photo(
                "/photos/A.jpg",
                Some("2024"),
                Some(10),
                Some((6000, 4000)),
                Some(3),
            ),
            photo("/photos/m.jpg", None, Some(30), Some((3000, 2000)), Some(1)),
        ];

        let sorted_paths = |field, direction| {
            let mut photos = source.clone();
            sort_photos(&mut photos, PhotoSort { field, direction });
            photos
                .into_iter()
                .map(|photo| photo.path)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            sorted_paths(SortField::Name, SortDirection::Ascending),
            ["/photos/A.jpg", "/photos/m.jpg", "/photos/z.jpg"]
        );
        assert_eq!(
            sorted_paths(SortField::DateTaken, SortDirection::Descending),
            ["/photos/A.jpg", "/photos/z.jpg", "/photos/m.jpg"]
        );
        assert_eq!(
            sorted_paths(SortField::FileSize, SortDirection::Descending),
            ["/photos/m.jpg", "/photos/z.jpg", "/photos/A.jpg"]
        );
        assert_eq!(
            sorted_paths(SortField::Dimensions, SortDirection::Descending),
            ["/photos/A.jpg", "/photos/m.jpg", "/photos/z.jpg"]
        );
        assert_eq!(
            sorted_paths(SortField::DateAdded, SortDirection::Ascending),
            ["/photos/m.jpg", "/photos/z.jpg", "/photos/A.jpg"]
        );
    }
}
