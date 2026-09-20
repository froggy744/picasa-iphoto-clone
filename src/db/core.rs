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

/// Open the initialized database without running migrations on grid refresh.
/// A short busy timeout also allows WAL readers to wait out brief contention.
pub fn open_default_read_only() -> Result<Connection> {
    let path = database_path()?;
    let connection = Connection::open_with_flags(
        &path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .with_context(|| format!("could not open read-only database {}", path.display()))?;
    connection.busy_timeout(std::time::Duration::from_secs(3))?;
    Ok(connection)
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
    migrate_photo_schema(&connection)?;
    migrate_folder_schema(&connection)?;
    migrate_album_schema(&connection)?;
    // Best-effort: a failed gvfs-path rewrite (e.g. a folder/path collision
    // with an existing canonical row) must never keep the library from
    // opening. The migration is transactional, so a failure leaves every
    // path exactly as it was; the legacy paths keep working.
    if let Err(error) = migrate_gvfs_paths(&connection) {
        eprintln!("gvfs path migration skipped: {error:#}");
    }
    Ok(connection)
}

fn migrate_photo_schema(connection: &Connection) -> Result<()> {
    let columns = connection
        .prepare("PRAGMA table_info(photos)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if !columns.iter().any(|column| column == "added_at") {
        connection.execute(
            "ALTER TABLE photos ADD COLUMN added_at INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    if !columns.iter().any(|column| column == "edit_recipe") {
        connection.execute(
            "ALTER TABLE photos ADD COLUMN edit_recipe TEXT NOT NULL DEFAULT ''",
            [],
        )?;
    }
    // Existing records have no import timestamp. Their stable row IDs retain
    // the database's historical insertion order until they are refreshed.
    connection.execute("UPDATE photos SET added_at = id WHERE added_at = 0", [])?;
    connection.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_photos_added_at ON photos(added_at DESC);",
    )?;
    Ok(())
}

fn migrate_folder_schema(connection: &Connection) -> Result<()> {
    let columns = connection
        .prepare("PRAGMA table_info(folders)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let had_parent_id = columns.iter().any(|column| column == "parent_id");
    let had_imported_root = columns.iter().any(|column| column == "imported_root");
    let needs_inference = !had_parent_id || !had_imported_root;
    if !had_parent_id {
        connection.execute("ALTER TABLE folders ADD COLUMN parent_id INTEGER REFERENCES folders(id)", [])?;
    }
    if !had_imported_root {
        connection.execute("ALTER TABLE folders ADD COLUMN imported_root BOOLEAN NOT NULL DEFAULT 0", [])?;
    }
    // Keep the per-root watch preference when upgrading older libraries.
    if !columns.iter().any(|column| column == "watched") {
        connection.execute("ALTER TABLE folders ADD COLUMN watched BOOLEAN NOT NULL DEFAULT 0", [])?;
    }
    if !needs_inference {
        repair_folder_parent_links(connection)?;
        repair_legacy_root_state(connection)?;
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
        connection.execute(
            "UPDATE folders SET parent_id = ?1, imported_root = ?2 WHERE id = ?3",
            rusqlite::params![parent, parent.is_none(), id],
        )?;
    }
    repair_folder_parent_links(connection)?;
    connection.execute(
        "INSERT OR REPLACE INTO settings(key, value) VALUES ('folder-root-legacy-migration-v1', 'complete')",
        [],
    )?;
    connection.execute_batch("CREATE INDEX IF NOT EXISTS idx_folders_parent ON folders(parent_id);")?;
    Ok(())
}

/// Recover the one legacy shape that can be identified after the root flag was
/// introduced: a parent grouping row was created after a nested root and was
/// therefore left unmarked. This runs once, before new explicit imports can
/// establish a different intentional nested-root scope.
fn repair_legacy_root_state(connection: &Connection) -> Result<()> {
    let already_repaired: Option<String> = connection
        .query_row(
            "SELECT value FROM settings WHERE key = 'folder-root-legacy-migration-v1'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if already_repaired.is_some() {
        return Ok(());
    }

    let candidates = connection
        .prepare(
            "SELECT parent.id, parent.path
             FROM folders parent
             WHERE parent.parent_id IS NULL
               AND parent.imported_root = 0
               AND EXISTS (
                 SELECT 1 FROM folders child
                 WHERE child.imported_root = 1
                   AND child.path LIKE parent.path || '/%'
                   AND child.id < parent.id
               )
               AND (
                 SELECT COUNT(*) FROM folders child
                 WHERE child.imported_root = 1
                   AND child.path LIKE parent.path || '/%'
               ) = 1
             ORDER BY length(parent.path), parent.path",
        )?
        .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    for (id, path) in candidates {
        connection.execute(
            "UPDATE folders SET imported_root = 0
             WHERE imported_root = 1 AND path LIKE ?1 || '/%'",
            [&path],
        )?;
        connection.execute(
            "UPDATE folders SET imported_root = 1 WHERE id = ?1",
            [id],
        )?;
    }

    connection.execute(
        "INSERT OR REPLACE INTO settings(key, value) VALUES ('folder-root-legacy-migration-v1', 'complete')",
        [],
    )?;
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
                "UPDATE folders SET parent_id = ?1 WHERE id = ?2",
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
    let columns = {
        let mut statement = connection.prepare("PRAGMA table_info(albums)")?;
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        columns
    };
    if !columns.iter().any(|column| column == "created_at") {
        connection.execute_batch(
            "ALTER TABLE albums ADD COLUMN created_at INTEGER NOT NULL DEFAULT 0;
             UPDATE albums SET created_at = CAST(strftime('%s', 'now') AS INTEGER)
             WHERE created_at = 0;",
        )?;
    }
    if !columns.iter().any(|column| column == "cover_frame") {
        connection.execute("ALTER TABLE albums ADD COLUMN cover_frame TEXT", [])?;
    }
    if !columns.iter().any(|column| column == "cover_photo_id") {
        connection.execute(
            "ALTER TABLE albums ADD COLUMN cover_photo_id INTEGER
             REFERENCES photos(id) ON DELETE SET NULL",
            [],
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
