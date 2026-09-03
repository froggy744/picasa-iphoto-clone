pub fn database_path() -> Result<PathBuf> {
    let data_dir = dirs_path().context("could not determine the user's data directory")?;
    Ok(data_dir.join("picasa-rs").join("library.db"))
}

fn dirs_path() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
}

pub fn open_default() -> Result<Connection> {
    let path = database_path()?;
    open(&path)
}

pub fn open(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("could not create database directory {}", parent.display()))?;
    }
    let connection = Connection::open(path)
        .with_context(|| format!("could not open database {}", path.display()))?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    connection.pragma_update(None, "journal_mode", "WAL")?;
    connection.execute_batch(SCHEMA)?;
    migrate_album_schema(&connection)?;
    Ok(connection)
}

fn migrate_album_schema(connection: &Connection) -> Result<()> {
    let has_created_at = {
        let mut statement = connection.prepare("PRAGMA table_info(albums)")?;
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        columns.iter().any(|column| column == "created_at")
    };
    if !has_created_at {
        connection.execute_batch(
            "ALTER TABLE albums ADD COLUMN created_at INTEGER NOT NULL DEFAULT 0;
             UPDATE albums SET created_at = CAST(strftime('%s', 'now') AS INTEGER)
             WHERE created_at = 0;",
        )?;
    }

    let foreign_key_count: i64 = connection.query_row(
        "SELECT COUNT(*) FROM pragma_foreign_key_list('album_photos')",
        [],
        |row| row.get(0),
    )?;
    if foreign_key_count == 0 {
        connection.execute_batch(
            "PRAGMA foreign_keys = OFF;
             ALTER TABLE album_photos RENAME TO album_photos_legacy;
             CREATE TABLE album_photos (
               album_id INTEGER NOT NULL REFERENCES albums(id) ON DELETE CASCADE,
               photo_id INTEGER NOT NULL REFERENCES photos(id) ON DELETE CASCADE,
               PRIMARY KEY(album_id, photo_id)
             );
             INSERT OR IGNORE INTO album_photos(album_id, photo_id)
               SELECT old.album_id, old.photo_id FROM album_photos_legacy old
               JOIN albums a ON a.id = old.album_id
               JOIN photos p ON p.id = old.photo_id;
             DROP TABLE album_photos_legacy;
             CREATE INDEX IF NOT EXISTS idx_album_photos_photo ON album_photos(photo_id);
             PRAGMA foreign_keys = ON;",
        )?;
    }
    Ok(())
}

