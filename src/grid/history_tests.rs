use super::*;
use chrono::{Datelike, Local, TimeZone};

#[test]
fn history_group_labels_bucket_by_edit_age_not_import_date() {
    let today = Local::now().date_naive();
    let noon = |days_before: i64| -> i64 {
        let date = today
            .checked_sub_days(chrono::Days::new(days_before as u64))
            .unwrap_or(today);
        Local
            .from_local_datetime(&date.and_hms_opt(12, 0, 0).unwrap())
            .single()
            .unwrap()
            .timestamp_millis()
    };

    assert_eq!(history_group_label(noon(0)), "Today");
    assert_eq!(history_group_label(noon(1)), "Yesterday");
    assert_eq!(history_group_label(noon(3)), "Earlier This Week");
    assert_eq!(history_group_label(noon(7)), "Earlier This Week");

    // Fixed timestamps that cannot fall into the rolling week/month buckets.
    assert_eq!(history_group_label(noon(400)), "Aug 2025");
    let last_year = Local
        .from_local_datetime(
            &chrono::NaiveDate::from_ymd_opt(2020, 6, 15)
                .unwrap()
                .and_hms_opt(12, 0, 0)
                .unwrap(),
        )
        .single()
        .unwrap()
        .timestamp_millis();
    assert_eq!(history_group_label(last_year), "Jun 2020");
    assert_eq!(history_group_label(0), "Unknown Date");

    // Same calendar month but older than the rolling 7-day week window.
    if today.day() > 10 {
        let same_month = Local
            .from_local_datetime(&today.with_day(1).unwrap().and_hms_opt(12, 0, 0).unwrap())
            .single()
            .unwrap()
            .timestamp_millis();
        let same_month_date = chrono::DateTime::from_timestamp_millis(same_month)
            .unwrap()
            .with_timezone(&Local);
        let age = (today - same_month_date.date_naive()).num_days();
        if age > 7 {
            assert_eq!(history_group_label(same_month), "Earlier This Month");
        }
    }
}

#[test]
fn history_photos_expose_edited_at_for_grouping() {
    let connection = rusqlite::Connection::open_in_memory().unwrap();
    connection.execute_batch(crate::db::SCHEMA).unwrap();
    connection
        .execute(
            "INSERT INTO photos(id,path) VALUES (1,'/history-edit-time.jpg')",
            [],
        )
        .unwrap();
    crate::db::set_edit_recipe(&connection, 1, "edited").unwrap();
    let history = crate::db::history_photos(&connection).unwrap();
    assert_eq!(history.len(), 1);
    assert!(history[0].edited_at > 0);
    let ordinary = crate::db::photo(&connection, 1).unwrap().unwrap();
    assert_eq!(ordinary.edited_at, 0);
}

#[test]
#[ignore = "requires a GTK display; run with --ignored --test-threads=1"]
fn history_group_ranges_merge_contiguous_edit_buckets() {
    gtk::init().unwrap();
    let gallery = Gallery::new(
        &[],
        180,
        |_| {},
        |_, _| {},
        |_, _, _, _| {},
        |_, _| {},
        |_| {},
    );
    let today = Local::now().date_naive();
    let photo_with_edit = |id: i64, days_before: i64| {
        let date = today
            .checked_sub_days(chrono::Days::new(days_before as u64))
            .unwrap_or(today);
        let millis = Local
            .from_local_datetime(&date.and_hms_opt(12, 0, 0).unwrap())
            .single()
            .unwrap()
            .timestamp_millis();
        let mut photo = crate::db::Photo {
            id,
            path: format!("/edit-{id}.jpg"),
            folder_id: None,
            folder_path: None,
            taken_at: None,
            camera: None,
            width: None,
            height: None,
            size_bytes: None,
            mtime: None,
            added_at: 0,
            rotation: 0,
            edit_recipe: "edited".into(),
            favorite: false,
            trashed: false,
            history_caption: None,
            edited_at: millis,
        };
        photo.history_caption = Some(format!("Photo · {millis}"));
        photo
    };
    let photos = vec![
        photo_with_edit(1, 0),
        photo_with_edit(2, 0),
        photo_with_edit(3, 1),
        photo_with_edit(4, 3),
        photo_with_edit(5, 40),
    ];
    gallery.replace(&photos);
    gallery.set_grouping(GroupMode::History, GroupDate::Taken);
    let ranges = gallery.group_ranges.borrow().clone();
    let labels: Vec<_> = ranges.iter().map(|range| range.label.clone()).collect();
    assert_eq!(
        labels,
        vec![
            "Today".to_string(),
            "Yesterday".to_string(),
            "Earlier This Week".to_string(),
            history_group_label(photos[4].edited_at),
        ]
    );
    assert_eq!((ranges[0].start, ranges[0].end), (0, 2));
    assert_eq!((ranges[1].start, ranges[1].end), (2, 3));
    assert_eq!((ranges[2].start, ranges[2].end), (3, 4));
    assert_eq!((ranges[3].start, ranges[3].end), (4, 5));
    gallery.set_grouping(GroupMode::None, GroupDate::Taken);
}

#[test]
#[ignore = "requires a GTK display; run with --ignored --test-threads=1"]
fn history_grid_reuses_items_without_leaking_captions_into_other_views() {
    gtk::init().unwrap();
    let connection = rusqlite::Connection::open_in_memory().unwrap();
    connection.execute_batch(crate::db::SCHEMA).unwrap();
    connection
        .execute(
            "INSERT INTO photos(id,path) VALUES (1,'/history-test.jpg')",
            [],
        )
        .unwrap();
    crate::db::set_edit_recipe(&connection, 1, "edited").unwrap();
    let history = crate::db::history_photos(&connection).unwrap();
    let gallery = Gallery::new(
        &[],
        180,
        |_| {},
        |_, _| {},
        |_, _, _, _| {},
        |_, _| {},
        |_| {},
    );
    gallery.replace(&history);
    assert!(gallery.current_photos.borrow()[0]
        .history_caption()
        .unwrap()
        .starts_with("Photo · "));

    let ordinary = vec![crate::db::photo(&connection, 1).unwrap().unwrap()];
    gallery.replace(&ordinary);
    assert_eq!(gallery.current_photos.borrow()[0].history_caption(), None);
    gallery.replace(&history);
    assert!(gallery.current_photos.borrow()[0]
        .history_caption()
        .is_some());

    let frame = gtk::Overlay::new();
    let tile = SquareTile::new(180, 120, &frame);
    let plain = PhotoObject::from_photo(&ordinary[0]);
    tile.bind_photo(&plain);
    tile.imp()
        .applied_visual_key
        .replace(photo_presentation_key(&plain));
    tile.imp().visual_loaded.set(true);
    tile.bind_photo(&gallery.current_photos.borrow()[0]);
    let caption = find_overlay_child(&frame, "history-caption").unwrap();
    assert!(caption.is_visible());
    tile.bind_photo(&plain);
    assert!(!caption.is_visible());
    plain.set_rotation(90);
    tile.bind_photo(&plain);
    assert!(!tile.imp().visual_loaded.get());
}
