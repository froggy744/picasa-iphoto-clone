pub fn database_path() -> Result<PathBuf> {
    let data_dir = dirs_path().context("could not determine the user's data directory")?;
    Ok(data_dir.join("picasa-rs").join("library.db"))
}

fn dirs_path() -> Option<PathBuf> {
    dirs::data_dir()
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
    migrate_folder_schema(&connection)?;
    migrate_album_schema(&connection)?;
    Ok(connection)
}

fn migrate_folder_schema(connection: &Connection) -> Result<()> {
    let columns = connection
        .prepare("PRAGMA table_info(folders)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let needs_inference = !columns.iter().any(|column| column == "parent_id")
        || !columns.iter().any(|column| column == "imported_root");
    if !columns.iter().any(|column| column == "parent_id") {
        connection.execute("ALTER TABLE folders ADD COLUMN parent_id INTEGER REFERENCES folders(id)", [])?;
    }
    if !columns.iter().any(|column| column == "imported_root") {
        connection.execute("ALTER TABLE folders ADD COLUMN imported_root BOOLEAN NOT NULL DEFAULT 0", [])?;
    }
    if !needs_inference {
        repair_folder_parent_links(connection)?;
        connection.execute_batch("CREATE INDEX IF NOT EXISTS idx_folders_parent ON folders(parent_id);")?;
        return Ok(());
    }
    let rows = connection
        .prepare("SELECT id, path FROM folders ORDER BY length(path) ASC")?
        .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (id, path) in &rows {
        let parent = rows
            .iter()
            .filter(|(candidate_id, candidate_path)| candidate_id != id && is_descendant_path(path, candidate_path))
            .min_by_key(|(_, candidate_path)| candidate_path.len())
            .map(|(candidate_id, _)| *candidate_id);
        connection.execute("UPDATE folders SET parent_id = ?1, imported_root = ?2 WHERE id = ?3", rusqlite::params![parent, parent.is_none(), id])?;
    }
    repair_folder_parent_links(connection)?;
    connection.execute_batch("CREATE INDEX IF NOT EXISTS idx_folders_parent ON folders(parent_id);")?;
    Ok(())
}

/// Keep persisted folder links consistent when a parent folder is registered
/// after one of its descendants. This also repairs databases created before
/// the hierarchy fields were introduced.
fn repair_folder_parent_links(connection: &Connection) -> Result<()> {
    let rows = connection
        .prepare("SELECT id, path, parent_id FROM folders")?
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<i64>>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    for (id, path, current_parent_id) in rows {
        let direct_parent_id = Path::new(&path)
            .parent()
            .and_then(|parent| parent.to_str())
            .and_then(|parent_path| {
                connection
                    .query_row(
                        "SELECT id FROM folders WHERE path = ?1 AND id != ?2",
                        rusqlite::params![parent_path, id],
                        |row| row.get::<_, i64>(0),
                    )
                    .optional()
                    .ok()
                    .flatten()
            });

        if direct_parent_id.is_some() && direct_parent_id != current_parent_id {
            connection.execute(
                "UPDATE folders SET parent_id = ?1, imported_root = 0 WHERE id = ?2",
                rusqlite::params![direct_parent_id, id],
            )?;
        }
    }
    Ok(())
}

fn is_descendant_path(candidate: &str, ancestor: &str) -> bool {
    let candidate = candidate.trim_end_matches('/');
    let ancestor = ancestor.trim_end_matches('/');
    candidate.starts_with(ancestor)
        && candidate.len() > ancestor.len()
        && candidate.as_bytes().get(ancestor.len()) == Some(&b'/')
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
