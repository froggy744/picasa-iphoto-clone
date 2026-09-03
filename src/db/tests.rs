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
}
