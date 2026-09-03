fn refresh_grid(
    connection: &Rc<RefCell<Connection>>,
    filter: sidebar::SidebarFilter,
    search: &str,
    sort: PhotoSort,
    gallery: &grid::Gallery,
) {
    let started = Instant::now();
    eprintln!(
        "SEARCH PERF refresh_start filter={:?} query={:?}",
        filter, search
    );
    // An active search is a library-wide view, regardless of the destination
    // that was selected before typing began.
    if !search.is_empty() {
        let db_started = Instant::now();
        if let Ok(mut photos) = db::photos(&connection.borrow(), None, false, Some(search)) {
            eprintln!(
                "SEARCH PERF global_db_done query={:?} rows={} elapsed_ms={}",
                search,
                photos.len(),
                db_started.elapsed().as_millis()
            );
            let filter_started = Instant::now();
            retain_enabled_formats(&connection.borrow(), &mut photos);
            eprintln!(
                "SEARCH PERF global_format_filter_done rows={} elapsed_ms={}",
                photos.len(),
                filter_started.elapsed().as_millis()
            );
            let sort_started = Instant::now();
            sort_photos(&mut photos, sort);
            eprintln!(
                "SEARCH PERF global_sort_done rows={} elapsed_ms={}",
                photos.len(),
                sort_started.elapsed().as_millis()
            );
            eprintln!(
                "VIEW TRACE refresh filter={:?} search={:?} photos={}",
                filter,
                search,
                photos.len()
            );
            let gallery_started = Instant::now();
            gallery.replace(&photos);
            eprintln!(
                "SEARCH PERF global_gallery_done rows={} elapsed_ms={} total_ms={}",
                photos.len(),
                gallery_started.elapsed().as_millis(),
                started.elapsed().as_millis()
            );
        }
        return;
    }

    if let sidebar::SidebarFilter::Album(album_id) = filter {
        if let Ok(mut photos) = db::photos_in_album(&connection.borrow(), album_id, None) {
            retain_enabled_formats(&connection.borrow(), &mut photos);
            sort_photos(&mut photos, sort);
            gallery.replace(&photos);
        }
        return;
    }
    let (folder_id, favorites) = match filter {
        sidebar::SidebarFilter::All | sidebar::SidebarFilter::RecentlyAdded => (None, false),
        sidebar::SidebarFilter::Favorites => (None, true),
        sidebar::SidebarFilter::Folder(id) => (Some(id), false),
        sidebar::SidebarFilter::Albums => unreachable!(),
        sidebar::SidebarFilter::Album(_) => unreachable!(),
    };

    if let Ok(mut photos) = db::photos(
        &connection.borrow(),
        folder_id,
        favorites,
        (!search.is_empty()).then_some(search),
    ) {
        retain_enabled_formats(&connection.borrow(), &mut photos);
        sort_photos(&mut photos, sort);
        eprintln!(
            "VIEW TRACE refresh filter={:?} search={:?} photos={}",
            filter,
            search,
            photos.len()
        );
        gallery.replace(&photos);
    } else {
        eprintln!(
            "VIEW TRACE refresh failed filter={:?} search={:?}",
            filter, search
        );
    }
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
            SortField::DateAdded => compare_optional(left.mtime, right.mtime, sort.direction),
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
    use super::{sort_photos, valid_file_name, PhotoSort, SortDirection, SortField};
    use crate::db::Photo;

    #[test]
    fn rename_rejects_paths_and_accepts_a_file_name() {
        assert!(valid_file_name("holiday photo.jpg"));
        assert!(!valid_file_name(""));
        assert!(!valid_file_name(".."));
        assert!(!valid_file_name("folder/photo.jpg"));
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
            rotation: 0,
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
