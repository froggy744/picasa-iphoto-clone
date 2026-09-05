pub fn insert_folder(connection: &Connection, path: &str) -> Result<i64> {
    let name = path
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(&path);
    let mut imported_parent: Option<i64> = connection
        .query_row(
            "SELECT id FROM folders
             WHERE imported_root = 1 AND path != ?1 AND ?1 LIKE path || '/%'
             ORDER BY length(path) DESC LIMIT 1",
            [path],
            |row| row.get(0),
        )
        .optional()?;
    let existing: Option<(i64, Option<i64>, bool)> = connection
        .query_row(
            "SELECT id, parent_id, imported_root FROM folders WHERE path = ?1",
            [path],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let direct_parent: Option<i64> = Path::new(path)
        .parent()
        .and_then(|parent| parent.to_str())
        .and_then(|parent| {
            connection
                .query_row("SELECT id FROM folders WHERE path = ?1", [parent], |row| {
                    row.get(0)
                })
                .optional()
                .ok()
                .flatten()
        });
    if imported_parent.is_none() && existing.as_ref().is_none_or(|(_, _, root)| *root) {
        let selected_parent = Path::new(path).parent().and_then(|parent| parent.to_str());
        let sibling_parent = if let Some(selected_parent) = selected_parent {
            let paths = connection
                .prepare("SELECT path FROM folders WHERE imported_root = 1 AND path != ?1")?
                .query_map([path], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            paths.into_iter().find_map(|candidate| {
                (Path::new(&candidate).parent().and_then(|parent| parent.to_str())
                    == Some(selected_parent))
                .then_some(selected_parent.to_string())
            })
        } else {
            None
        };
        if let Some(sibling_parent) = sibling_parent {
            let parent_id = insert_folder(connection, &sibling_parent)?;
            imported_parent = Some(parent_id);
            if std::env::var_os("PICASA_TRACE").is_some() {
                eprintln!("FOLDER TRACE inferred_shared_parent path={path} parent_path={sibling_parent} parent_id={parent_id}");
            }
        }
    }
    let (parent_id, imported_root) = match existing {
        // A refresh or re-import of a discovered folder must keep its real
        // containing folder. Only a previously imported root may be demoted
        // under an already-imported ancestor.
        Some((_, parent_id, true)) if imported_parent.is_none() => {
            let parent_id = direct_parent.or(parent_id);
            (parent_id, parent_id.is_none())
        }
        Some((_, parent_id, false)) => (direct_parent.or(parent_id), false),
        _ => {
            let parent_id = direct_parent.or(imported_parent);
            (parent_id, parent_id.is_none())
        }
    };
    if std::env::var_os("PICASA_TRACE").is_some() {
        eprintln!("FOLDER TRACE insert_import path={path} imported_parent={imported_parent:?} existing={existing:?} target_parent={parent_id:?} target_root={imported_root}");
    }
    connection.execute(
        "INSERT INTO folders(path, name, parent_id, imported_root) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(path) DO UPDATE SET name = excluded.name,
           parent_id = excluded.parent_id, imported_root = excluded.imported_root",
        params![path, name, parent_id, imported_root],
    )?;
    let id = connection.query_row("SELECT id FROM folders WHERE path = ?1", [path], |row| {
        row.get(0)
    })?;
    let reparented = if imported_root {
        connection.execute(
            "UPDATE folders
             SET parent_id = ?1, imported_root = 0
             WHERE id != ?1 AND imported_root = 1 AND path LIKE ?2 || '/%'",
            params![id, path],
        )?
    } else {
        0
    };
    if std::env::var_os("PICASA_TRACE").is_some() {
        eprintln!("FOLDER TRACE import_result id={id} path={path} parent_id={parent_id:?} imported_root={imported_root} reparented_descendant_roots={reparented}");
    }
    Ok(id)
}

pub fn insert_discovered_folder(connection: &Connection, path: &str, parent_id: i64) -> Result<i64> {
    let name = path.trim_end_matches('/').rsplit('/').next().filter(|name| !name.is_empty()).unwrap_or(path);
    let existing: Option<(i64, Option<i64>, bool)> = connection
        .query_row("SELECT id, parent_id, imported_root FROM folders WHERE path = ?1", [path], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .optional()?;
    connection.execute(
        "INSERT INTO folders(path, name, parent_id, imported_root) VALUES (?1, ?2, ?3, 0)
         ON CONFLICT(path) DO UPDATE SET name = excluded.name,
           parent_id = CASE WHEN folders.imported_root = 1 THEN folders.parent_id ELSE excluded.parent_id END",
        params![path, name, parent_id],
    )?;
    let id = connection.query_row("SELECT id FROM folders WHERE path = ?1", [path], |row| row.get(0))?;
    if std::env::var_os("PICASA_TRACE").is_some() {
        eprintln!("FOLDER TRACE discover path={path} requested_parent={parent_id} existing={existing:?} result_id={id}");
    }
    Ok(id)
}

pub fn folders(connection: &Connection) -> Result<Vec<Folder>> {
    let mut statement = connection.prepare(
        "SELECT f.id, f.path, COALESCE(f.name, f.path), f.parent_id, f.imported_root,
                (WITH RECURSIVE descendants(id) AS (
                   SELECT id FROM folders WHERE id = f.id
                   UNION ALL
                   SELECT child.id FROM folders child JOIN descendants ON child.parent_id = descendants.id
                 )
                 SELECT COUNT(*) FROM photos p
                 WHERE p.trashed = 0 AND p.folder_id IN (SELECT id FROM descendants)),
                (SELECT COUNT(*) FROM folders child WHERE child.parent_id = f.id)
         FROM folders f ORDER BY f.path COLLATE NOCASE",
    )?;
    let rows = statement.query_map([], |row| {
        Ok(Folder {
            id: row.get(0)?,
            path: row.get(1)?,
            name: row.get(2)?,
            parent_id: row.get(3)?,
            imported_root: row.get(4)?,
            photo_count: row.get(5)?,
            subfolder_count: row.get(6)?,
            available: crate::source::cached_source_available(&row.get::<_, String>(1)?),
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Remove a folder and its indexed descendants from the application database.
/// This only deletes database rows; it never touches the filesystem.
pub fn remove_folder(connection: &Connection, folder_id: i64) -> Result<()> {
    let transaction = connection.unchecked_transaction()?;
    transaction.execute(
        "DELETE FROM photos
         WHERE folder_id IN (
           WITH RECURSIVE descendants(id) AS (
             SELECT id FROM folders WHERE id = ?1
             UNION ALL
             SELECT child.id FROM folders child
             JOIN descendants ON child.parent_id = descendants.id
           )
           SELECT id FROM descendants
         )",
        [folder_id],
    )?;
    transaction.execute(
        "DELETE FROM folders
         WHERE id IN (
           WITH RECURSIVE descendants(id) AS (
             SELECT id FROM folders WHERE id = ?1
             UNION ALL
             SELECT child.id FROM folders child
             JOIN descendants ON child.parent_id = descendants.id
           )
           SELECT id FROM descendants
         )",
        [folder_id],
    )?;
    transaction.commit()?;
    Ok(())
}

pub fn folder_exists(connection: &Connection, folder_id: i64) -> Result<bool> {
    Ok(connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM folders WHERE id = ?1)",
        [folder_id],
        |row| row.get(0),
    )?)
}

pub fn upsert_photo(
    connection: &Connection,
    path: &Path,
    folder_id: Option<i64>,
    metadata: &PhotoMetadata,
) -> Result<i64> {
    let path = path.to_string_lossy();
    connection.execute(
        "INSERT INTO photos(path, folder_id, taken_at, camera, width, height, size_bytes, mtime)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(path) DO UPDATE SET folder_id=excluded.folder_id,
           taken_at=excluded.taken_at, camera=excluded.camera, width=excluded.width,
           height=excluded.height, size_bytes=excluded.size_bytes, mtime=excluded.mtime",
        params![
            path.as_ref(),
            folder_id,
            metadata.taken_at,
            metadata.camera,
            metadata.width,
            metadata.height,
            metadata.size_bytes,
            metadata.mtime
        ],
    )?;
    Ok(
        connection.query_row("SELECT id FROM photos WHERE path = ?1", [&path], |row| {
            row.get(0)
        })?,
    )
}

pub fn set_photo_folder(connection: &Connection, path: &str, folder_id: i64) -> Result<()> {
    connection.execute(
        "UPDATE photos SET folder_id = ?2 WHERE path = ?1",
        params![path, folder_id],
    )?;
    Ok(())
}

pub fn photo_fingerprints(
    connection: &Connection,
) -> Result<std::collections::HashMap<String, (Option<i64>, Option<i64>, Option<i64>, Option<i64>)>>
{
    let mut statement =
        connection.prepare("SELECT path, mtime, size_bytes, width, height FROM photos")?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            (row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?),
        ))
    })?;
    Ok(rows.collect::<rusqlite::Result<std::collections::HashMap<_, _>>>()?)
}

pub fn photo(connection: &Connection, id: i64) -> Result<Option<Photo>> {
    Ok(connection
        .query_row(
            "SELECT p.id,p.path,p.folder_id,p.taken_at,p.camera,p.width,p.height,p.size_bytes,p.mtime,p.rotation,p.favorite,p.trashed,f.path
             FROM photos p LEFT JOIN folders f ON f.id = p.folder_id WHERE p.id = ?1",
            [id],
            photo_from_row,
        )
        .optional()?)
}

pub fn photos(
    connection: &Connection,
    folder_id: Option<i64>,
    favorites_only: bool,
    search: Option<&str>,
) -> Result<Vec<Photo>> {
    let search = search.map(|value| format!("%{}%", value.replace('%', "\\%").replace('_', "\\_")));
    let mut statement = connection.prepare(
        "SELECT p.id,p.path,p.folder_id,p.taken_at,p.camera,p.width,p.height,p.size_bytes,p.mtime,p.rotation,p.favorite,p.trashed,f.path
         FROM photos p LEFT JOIN folders f ON f.id = p.folder_id
         WHERE p.trashed = 0 AND (?1 IS NULL OR p.folder_id IN
             (WITH RECURSIVE descendants(id) AS (
                SELECT id FROM folders WHERE id = ?1
                UNION ALL
                SELECT child.id FROM folders child JOIN descendants ON child.parent_id = descendants.id
              ) SELECT id FROM descendants))
           AND (?2 = 0 OR p.favorite = 1) AND (?3 IS NULL OR p.path LIKE ?3 ESCAPE '\\')
         ORDER BY p.taken_at IS NULL, p.taken_at DESC, p.path COLLATE NOCASE",
    )?;
    let rows = statement.query_map(
        params![folder_id, favorites_only as i32, search],
        photo_from_row,
    )?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn photos_in_album(
    connection: &Connection,
    album_id: i64,
    search: Option<&str>,
) -> Result<Vec<Photo>> {
    let search = search.map(|value| format!("%{}%", value.replace('%', "\\%").replace('_', "\\_")));
    let mut statement = connection.prepare(
        "SELECT p.id,p.path,p.folder_id,p.taken_at,p.camera,p.width,p.height,p.size_bytes,p.mtime,p.rotation,p.favorite,p.trashed,f.path
         FROM album_photos ap
         JOIN photos p ON p.id = ap.photo_id
         LEFT JOIN folders f ON f.id = p.folder_id
         WHERE ap.album_id = ?1 AND p.trashed = 0
           AND (?2 IS NULL OR p.path LIKE ?2 ESCAPE '\\')
         ORDER BY p.taken_at IS NULL, p.taken_at DESC, p.path COLLATE NOCASE",
    )?;
    let rows = statement.query_map(params![album_id, search], photo_from_row)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn set_favorite(connection: &Connection, id: i64, favorite: bool) -> Result<()> {
    connection.execute(
        "UPDATE photos SET favorite = ?1 WHERE id = ?2",
        params![favorite, id],
    )?;
    Ok(())
}

pub fn set_rotation(connection: &Connection, id: i64, rotation: i32) -> Result<()> {
    let normalized = rotation.rem_euclid(360);
    if ![0, 90, 180, 270].contains(&normalized) {
        anyhow::bail!("invalid rotation: {rotation}");
    }
    connection.execute(
        "UPDATE photos SET rotation = ?1 WHERE id = ?2",
        params![normalized, id],
    )?;
    Ok(())
}
