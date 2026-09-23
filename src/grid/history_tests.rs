use super::*;

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
