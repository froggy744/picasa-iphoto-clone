fn photo_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Photo> {
    Ok(Photo {
        id: row.get(0)?,
        path: row.get(1)?,
        folder_id: row.get(2)?,
        taken_at: row.get(3)?,
        camera: row.get(4)?,
        width: row.get(5)?,
        height: row.get(6)?,
        size_bytes: row.get(7)?,
        mtime: row.get(8)?,
        rotation: row.get(9)?,
        favorite: row.get(10)?,
        trashed: row.get(11)?,
        folder_path: row.get(12)?,
    })
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    #[test]
    fn rename_and_trash_updates_are_persisted() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(SCHEMA).unwrap();
        connection
            .execute("INSERT INTO photos(path) VALUES (?1)", ["/tmp/old.jpg"])
            .unwrap();
        let id = connection.last_insert_rowid();

        set_photo_path(&connection, id, "/tmp/new.jpg").unwrap();
        set_trashed(&connection, id, true).unwrap();

        let photo = photo(&connection, id).unwrap().unwrap();
        assert_eq!(photo.path, "/tmp/new.jpg");
        assert!(photo.trashed);
    }

    #[test]
    fn appearance_setting_is_replaced_and_persisted() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(SCHEMA).unwrap();

        assert_eq!(setting(&connection, "appearance-theme").unwrap(), None);
        set_setting(&connection, "appearance-theme", "iphone").unwrap();
        assert_eq!(
            setting(&connection, "appearance-theme").unwrap().as_deref(),
            Some("iphone")
        );
        set_setting(&connection, "appearance-theme", "standard").unwrap();
        assert_eq!(
            setting(&connection, "appearance-theme").unwrap().as_deref(),
            Some("standard")
        );
    }

    #[test]
    fn albums_keep_unique_memberships_and_do_not_delete_photos() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .unwrap();
        connection.execute_batch(SCHEMA).unwrap();
        connection
            .execute(
                "INSERT INTO photos(path) VALUES ('/tmp/a.jpg'), ('/tmp/b.jpg')",
                [],
            )
            .unwrap();
        let photo_ids = connection
            .prepare("SELECT id FROM photos ORDER BY id")
            .unwrap()
            .query_map([], |row| row.get::<_, i64>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();

        let album = create_album(&connection, "  Holiday  ").unwrap();
        assert_eq!(album.name, "Holiday");
        assert!(create_album(&connection, "holiday").is_err());
        assert_eq!(
            add_photos_to_album(&connection, album.id, &photo_ids).unwrap(),
            2
        );
        assert_eq!(
            add_photos_to_album(&connection, album.id, &photo_ids).unwrap(),
            0
        );
        assert_eq!(
            photos_in_album(&connection, album.id, None).unwrap().len(),
            2
        );

        assert_eq!(
            remove_photos_from_album(&connection, album.id, &[photo_ids[0]]).unwrap(),
            1
        );
        assert_eq!(
            photos_in_album(&connection, album.id, None).unwrap().len(),
            1
        );

        let second_album = create_album(&connection, "Portfolio").unwrap();
        assert_eq!(
            add_photos_to_album(&connection, second_album.id, &[photo_ids[1]]).unwrap(),
            1
        );
        assert_eq!(
            photos_in_album(&connection, second_album.id, None)
                .unwrap()
                .len(),
            1
        );

        delete_album(&connection, album.id).unwrap();
        assert_eq!(albums(&connection).unwrap().len(), 1);
        let photo_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM photos", [], |row| row.get(0))
            .unwrap();
        assert_eq!(photo_count, 2);
    }

    #[test]
    fn album_membership_survives_reopening_database() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "picasa-rs-album-test-{}-{unique}.db",
            std::process::id()
        ));

        {
            let connection = open(&path).unwrap();
            connection
                .execute(
                    "INSERT INTO photos(path) VALUES ('/tmp/persistent.jpg')",
                    [],
                )
                .unwrap();
            let photo_id = connection.last_insert_rowid();
            let album = create_album(&connection, "Persistent").unwrap();
            add_photos_to_album(&connection, album.id, &[photo_id]).unwrap();
        }

        {
            let connection = open(&path).unwrap();
            let album = albums(&connection).unwrap().remove(0);
            assert_eq!(album.name, "Persistent");
            assert_eq!(
                photos_in_album(&connection, album.id, None).unwrap().len(),
                1
            );
        }

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("db-shm"));
        let _ = std::fs::remove_file(path.with_extension("db-wal"));
    }

    #[test]
    fn existing_album_tables_gain_timestamps_and_cascades() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE photos (id INTEGER PRIMARY KEY);
                 CREATE TABLE albums (id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE);
                 CREATE TABLE album_photos (
                   album_id INTEGER NOT NULL,
                   photo_id INTEGER NOT NULL,
                   UNIQUE(album_id, photo_id)
                 );
                 INSERT INTO photos(id) VALUES (1);
                 INSERT INTO albums(id, name) VALUES (1, 'Existing');
                 INSERT INTO album_photos(album_id, photo_id) VALUES (1, 1);",
            )
            .unwrap();

        migrate_album_schema(&connection).unwrap();
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .unwrap();
        let created_at: i64 = connection
            .query_row("SELECT created_at FROM albums WHERE id = 1", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert!(created_at > 0);
        connection
            .execute("DELETE FROM albums WHERE id = 1", [])
            .unwrap();
        let membership_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM album_photos", [], |row| row.get(0))
            .unwrap();
        assert_eq!(membership_count, 0);
        let photo_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM photos", [], |row| row.get(0))
            .unwrap();
        assert_eq!(photo_count, 1);
    }

    #[test]
    fn folder_relationships_and_duplicate_roots_are_persisted() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(SCHEMA).unwrap();

        let root = insert_folder(&connection, "/mnt/steam/Wickus").unwrap();
        assert_eq!(insert_folder(&connection, "/mnt/steam/Wickus").unwrap(), root);
        let dcim = insert_discovered_folder(&connection, "/mnt/steam/Wickus/DCIM", root).unwrap();
        let leaf = insert_discovered_folder(
            &connection,
            "/mnt/steam/Wickus/DCIM/104NCZ_5",
            dcim,
        )
        .unwrap();

        let folders_by_path = folders(&connection)
            .unwrap()
            .into_iter()
            .map(|folder| (folder.path.clone(), folder))
            .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(folders_by_path.len(), 3);
        assert!(folders_by_path["/mnt/steam/Wickus"].imported_root);
        assert_eq!(folders_by_path["/mnt/steam/Wickus/DCIM"].parent_id, Some(root));
        assert_eq!(folders_by_path["/mnt/steam/Wickus/DCIM/104NCZ_5"].parent_id, Some(dcim));
        assert!(!folders_by_path["/mnt/steam/Wickus/DCIM/104NCZ_5"].imported_root);
        assert_eq!(leaf, folders_by_path["/mnt/steam/Wickus/DCIM/104NCZ_5"].id);
    }

    #[test]
    fn importing_a_folder_under_an_existing_root_does_not_create_another_root() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(SCHEMA).unwrap();

        let root = insert_folder(&connection, "/home/peet/Pictures").unwrap();
        let child = insert_folder(&connection, "/home/peet/Pictures/Screenshots").unwrap();
        let child_folder = folders(&connection)
            .unwrap()
            .into_iter()
            .find(|folder| folder.id == child)
            .unwrap();

        assert_eq!(child_folder.parent_id, Some(root));
        assert!(!child_folder.imported_root);
        assert_eq!(folders(&connection).unwrap().iter().filter(|folder| folder.imported_root).count(), 1);
    }

    #[test]
    fn importing_parent_after_child_reparents_the_existing_child_root() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(SCHEMA).unwrap();

        let child = insert_folder(&connection, "/mnt/steam/Wickus/DCIM/104NCZ_5").unwrap();
        let root = insert_folder(&connection, "/mnt/steam/Wickus").unwrap();
        let child_folder = folders(&connection)
            .unwrap()
            .into_iter()
            .find(|folder| folder.id == child)
            .unwrap();

        assert_eq!(child_folder.parent_id, Some(root));
        assert!(!child_folder.imported_root);
    }

    #[test]
    fn refreshing_a_discovered_folder_preserves_its_real_parent() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(SCHEMA).unwrap();

        let root = insert_folder(&connection, "/mnt/steam/Wickus").unwrap();
        let dcim = insert_discovered_folder(&connection, "/mnt/steam/Wickus/DCIM", root).unwrap();
        let leaf = insert_discovered_folder(
            &connection,
            "/mnt/steam/Wickus/DCIM/104NCZ_5",
            dcim,
        )
        .unwrap();

        assert_eq!(insert_folder(&connection, "/mnt/steam/Wickus/DCIM/104NCZ_5").unwrap(), leaf);
        let refreshed = folders(&connection)
            .unwrap()
            .into_iter()
            .find(|folder| folder.id == leaf)
            .unwrap();
        assert_eq!(refreshed.parent_id, Some(dcim));
        assert!(!refreshed.imported_root);
    }

    #[test]
    fn importing_siblings_registers_their_shared_parent() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(SCHEMA).unwrap();

        let camera = insert_folder(&connection, "/mnt/steam/Tatiana Pics/Camera").unwrap();
        let biology = insert_folder(&connection, "/mnt/steam/Tatiana Pics/Biology").unwrap();
        let folders_by_id = folders(&connection)
            .unwrap()
            .into_iter()
            .map(|folder| (folder.id, folder))
            .collect::<std::collections::HashMap<_, _>>();
        let parent = folders_by_id
            .values()
            .find(|folder| folder.path == "/mnt/steam/Tatiana Pics")
            .unwrap();

        assert_eq!(folders_by_id[&camera].parent_id, Some(parent.id));
        assert_eq!(folders_by_id[&biology].parent_id, Some(parent.id));
        assert!(parent.imported_root);
    }

    #[test]
    fn importing_an_empty_nested_folder_uses_its_direct_parent() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(SCHEMA).unwrap();

        let root = insert_folder(&connection, "/mnt/steam/Tatiana Pics/Camera").unwrap();
        let year = insert_discovered_folder(
            &connection,
            "/mnt/steam/Tatiana Pics/Camera/2026",
            root,
        )
        .unwrap();
        let empty = insert_folder(
            &connection,
            "/mnt/steam/Tatiana Pics/Camera/2026/2026-Test",
        )
        .unwrap();

        let folder = folders(&connection)
            .unwrap()
            .into_iter()
            .find(|folder| folder.id == empty)
            .unwrap();
        assert_eq!(folder.parent_id, Some(year));
        assert_eq!(folder.photo_count, 0);
    }

    #[test]
    fn parent_counts_and_filters_include_descendant_photos() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(SCHEMA).unwrap();
        let root = insert_folder(&connection, "/photos/root").unwrap();
        let child = insert_discovered_folder(&connection, "/photos/root/child", root).unwrap();
        let grandchild = insert_discovered_folder(
            &connection,
            "/photos/root/child/grandchild",
            child,
        )
        .unwrap();
        upsert_photo(&connection, Path::new("/photos/root/a.jpg"), Some(root), &PhotoMetadata::default()).unwrap();
        upsert_photo(&connection, Path::new("/photos/root/child/b.jpg"), Some(child), &PhotoMetadata::default()).unwrap();
        upsert_photo(&connection, Path::new("/photos/root/child/grandchild/c.jpg"), Some(grandchild), &PhotoMetadata::default()).unwrap();

        let folders_by_id = folders(&connection)
            .unwrap()
            .into_iter()
            .map(|folder| (folder.id, folder))
            .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(folders_by_id[&root].photo_count, 3);
        assert_eq!(folders_by_id[&child].photo_count, 2);
        assert_eq!(folders_by_id[&grandchild].photo_count, 1);
        assert_eq!(photos(&connection, Some(root), false, None).unwrap().len(), 3);
        assert_eq!(photos(&connection, Some(child), false, None).unwrap().len(), 2);
        assert_eq!(photos(&connection, Some(grandchild), false, None).unwrap().len(), 1);
    }
}
