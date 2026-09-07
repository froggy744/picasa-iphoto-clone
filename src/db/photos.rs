use std::collections::HashSet;

fn ensure_parent_folder(connection: &Connection, path: &str) -> Result<Option<i64>> {
    let Some(parent) = Path::new(path).parent().and_then(|parent| parent.to_str()) else {
        return Ok(None);
    };
    // Do not expose generic filesystem containers such as /mnt or /run in
    // the sidebar. A mounted share (for example /mnt/4TBP) is a useful
    // persisted grouping parent for imported folders below it.
    if matches!(parent, "/mnt" | "/run" | "/run/media") {
        return Ok(None);
    }
    if let Some(id) = connection
        .query_row("SELECT id FROM folders WHERE path = ?1", [parent], |row| row.get(0))
        .optional()?
    {
        return Ok(Some(id));
    }
    let parent_id = ensure_parent_folder(connection, parent)?;
    let name = parent.rsplit('/').next().filter(|name| !name.is_empty()).unwrap_or(parent);
    connection.execute(
        "INSERT INTO folders(path, name, parent_id, imported_root) VALUES (?1, ?2, ?3, 0)",
        params![parent, name, parent_id],
    )?;
    Ok(Some(connection.query_row("SELECT id FROM folders WHERE path = ?1", [parent], |row| row.get(0))?))
}

fn repair_existing_folder_parents(connection: &Connection) -> Result<()> {
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
        let Some(parent_path) = Path::new(&path).parent().and_then(|parent| parent.to_str())
        else {
            continue;
        };
        let parent_id: Option<i64> = connection
            .query_row(
                "SELECT id FROM folders WHERE path = ?1 AND id != ?2",
                params![parent_path, id],
                |row| row.get(0),
            )
            .optional()?;
        if parent_id != current_parent_id {
            connection.execute(
                "UPDATE folders SET parent_id = ?1 WHERE id = ?2",
                params![parent_id, id],
            )?;
        }
    }
    Ok(())
}

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
    let direct_parent = match direct_parent {
        Some(parent_id) => Some(parent_id),
        None => ensure_parent_folder(connection, path)?,
    };
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
            }
        }
    }
    let (parent_id, imported_root) = match existing {
        // A refresh or re-import of a discovered folder must keep its real
        // containing folder. Only a previously imported root may be demoted
        // under an already-imported ancestor.
        Some((_, parent_id, true)) => {
            let parent_id = direct_parent.or(parent_id);
            (parent_id, true)
        }
        Some((_, parent_id, false)) => (direct_parent.or(parent_id), false),
        _ => {
            let parent_id = direct_parent.or(imported_parent);
            (parent_id, parent_id.is_none())
        }
    };
    if std::env::var_os("PICASA_TRACE").is_some() {
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
    }
    repair_existing_folder_parents(connection)?;
    Ok(id)
}

/// Mark a user-selected folder as an independent library root. Ancestor roots
/// are no longer refreshed because the user has narrowed the imported scope to
/// this folder (and any other explicitly imported descendants).
pub fn mark_import_root(connection: &Connection, path: &str) -> Result<i64> {
    let id = insert_folder(connection, path)?;
    let transaction = connection.unchecked_transaction()?;
    transaction.execute(
        "UPDATE folders SET imported_root = 0
         WHERE imported_root = 1 AND path != ?1 AND ?1 LIKE path || '/%'",
        [path],
    )?;
    transaction.execute("UPDATE folders SET imported_root = 1 WHERE id = ?1", [id])?;
    transaction.commit()?;
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
    }
    Ok(id)
}

pub fn folders(connection: &Connection) -> Result<Vec<Folder>> {
    let mut statement = connection.prepare(
        "SELECT f.id, f.path, COALESCE(f.name, f.path), f.parent_id, f.imported_root, f.watched,
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
            watched: row.get(5)?,
            photo_count: row.get(6)?,
            subfolder_count: row.get(7)?,
            available: crate::source::cached_source_available(&row.get::<_, String>(1)?),
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn set_folder_watched(connection: &Connection, folder_id: i64, watched: bool) -> Result<()> {
    connection.execute(
        "UPDATE folders SET watched = ?1 WHERE id = ?2",
        params![watched, folder_id],
    )?;
    Ok(())
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

/// Remove indexed photos below a folder when a completed scan confirms they
/// are no longer present. The caller must verify that the storage location is
/// available before calling this function; an empty offline mount must never
/// be treated as an intentional deletion.
pub fn remove_missing_photos(
    connection: &Connection,
    folder_id: i64,
    present_paths: &HashSet<String>,
) -> Result<usize> {
    let stale_ids = {
        let mut statement = connection.prepare(
            "SELECT id, path FROM photos
             WHERE folder_id IN (
               WITH RECURSIVE descendants(id) AS (
                 SELECT id FROM folders WHERE id = ?1
                 UNION ALL
                 SELECT child.id FROM folders child
                 JOIN descendants ON child.parent_id = descendants.id
               )
               SELECT id FROM descendants
             )",
        )?;
        let rows = statement
            .query_map([folder_id], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
            .into_iter()
            .filter(|(_, path)| !present_paths.contains(path))
            .map(|(id, _)| id)
            .collect::<Vec<_>>()
    };
    if stale_ids.is_empty() {
        return Ok(0);
    }
    let transaction = connection.unchecked_transaction()?;
    for id in &stale_ids {
        transaction.execute("DELETE FROM photos WHERE id = ?1", [id])?;
    }
    transaction.commit()?;
    Ok(stale_ids.len())
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
        "INSERT INTO photos(path, folder_id, taken_at, camera, width, height, size_bytes, mtime, added_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, CAST(strftime('%s', 'now') AS INTEGER))
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
            "SELECT p.id,p.path,p.folder_id,p.taken_at,p.camera,p.width,p.height,p.size_bytes,p.mtime,p.added_at,p.rotation,p.favorite,p.trashed,f.path
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
        "SELECT p.id,p.path,p.folder_id,p.taken_at,p.camera,p.width,p.height,p.size_bytes,p.mtime,p.added_at,p.rotation,p.favorite,p.trashed,f.path
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

pub fn photo_availability_page(
    connection: &Connection,
    limit: usize,
    offset: usize,
) -> Result<Vec<(String, Option<String>)>> {
    let mut statement = connection.prepare(
        "SELECT p.path, f.path
         FROM photos p LEFT JOIN folders f ON f.id = p.folder_id
         WHERE p.trashed = 0
         ORDER BY p.id
         LIMIT ?1 OFFSET ?2",
    )?;
    let rows = statement.query_map(params![limit as i64, offset as i64], |row| {
        Ok((row.get(0)?, row.get(1)?))
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn photos_in_album(
    connection: &Connection,
    album_id: i64,
    search: Option<&str>,
) -> Result<Vec<Photo>> {
    let search = search.map(|value| format!("%{}%", value.replace('%', "\\%").replace('_', "\\_")));
    let mut statement = connection.prepare(
        "SELECT p.id,p.path,p.folder_id,p.taken_at,p.camera,p.width,p.height,p.size_bytes,p.mtime,p.added_at,p.rotation,p.favorite,p.trashed,f.path
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

pub fn set_favorite_for_folder(
    connection: &Connection,
    folder_id: i64,
    favorite: bool,
) -> Result<usize> {
    let changed = connection.execute(
        "UPDATE photos
         SET favorite = ?1
         WHERE trashed = 0 AND folder_id IN (
             WITH RECURSIVE descendants(id) AS (
                 SELECT id FROM folders WHERE id = ?2
                 UNION ALL
                 SELECT child.id FROM folders child
                 JOIN descendants ON child.parent_id = descendants.id
             )
             SELECT id FROM descendants
         )",
        params![favorite, folder_id],
    )?;
    Ok(changed)
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
