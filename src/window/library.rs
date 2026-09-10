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
                sidebar::SidebarFilter::Folder(id) => (Some(id), false),
                sidebar::SidebarFilter::Albums => return,
                sidebar::SidebarFilter::Album(_) => unreachable!(),
            };
            db::photos(&connection, folder_id, favorites, None).unwrap_or_default()
        };

        retain_enabled_formats(&connection, &mut photos);
        limit_recently_added(&connection, filter, &mut photos);
        sort_photos(&mut photos, sort);
        let _ = sender.send(Some(photos));
    });

    let gallery = gallery.clone();
    glib::timeout_add_local(std::time::Duration::from_millis(25), move || {
        match receiver.try_recv() {
            Ok(Some(photos)) => {
                if REFRESH_GENERATION.load(std::sync::atomic::Ordering::Relaxed) == generation {
                    gallery.replace(&photos);
                }
                glib::ControlFlow::Break
            }
            Ok(None) | Err(TryRecvError::Disconnected) => glib::ControlFlow::Break,
            Err(TryRecvError::Empty) => glib::ControlFlow::Continue,
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
    photos.sort_by(|left, right| {
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
            SortField::FileSize => {
                compare_optional(left.size_bytes, right.size_bytes, sort.direction)
            }
            SortField::Dimensions => compare_optional(
                pixel_count(left.width, left.height),
                pixel_count(right.width, right.height),
                sort.direction,
            ),
            SortField::DateAdded => {
                directed_ordering(
                    left.added_at
                        .cmp(&right.added_at)
                        .then(left.id.cmp(&right.id)),
                    sort.direction,
                )
            }
        };

        ordering.then_with(|| left.path.to_lowercase().cmp(&right.path.to_lowercase()))
    });
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
        sort_photos, valid_file_name, wallpaper_layout, PhotoSort, SortDirection, SortField,
        WallpaperLayout,
    };
    use crate::db::Photo;

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
