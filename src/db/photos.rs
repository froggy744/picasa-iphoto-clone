pub fn insert_folder(connection: &Connection, path: &str) -> Result<i64> {
    let name = path
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(&path);
    connection.execute(
        "INSERT INTO folders(path, name) VALUES (?1, ?2)
         ON CONFLICT(path) DO UPDATE SET name = excluded.name",
        params![path, name],
    )?;
    Ok(
        connection.query_row("SELECT id FROM folders WHERE path = ?1", [path], |row| {
            row.get(0)
        })?,
    )
}

pub fn folders(connection: &Connection) -> Result<Vec<Folder>> {
    let mut statement = connection.prepare(
        "SELECT f.id, f.path, COALESCE(f.name, f.path),
                (SELECT COUNT(*) FROM photos p
                 JOIN folders pf ON pf.id = p.folder_id
                 WHERE p.trashed = 0
                   AND (pf.path = f.path OR pf.path LIKE f.path || '/%')),
                (SELECT COUNT(*) FROM folders child
                 WHERE child.path LIKE f.path || '/%'
                   AND child.path NOT LIKE f.path || '/%/%')
         FROM folders f ORDER BY f.path COLLATE NOCASE",
    )?;
    let rows = statement.query_map([], |row| {
        Ok(Folder {
            id: row.get(0)?,
            path: row.get(1)?,
            name: row.get(2)?,
            photo_count: row.get(3)?,
            subfolder_count: row.get(4)?,
            available: crate::source::cached_source_available(&row.get::<_, String>(1)?),
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
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
             (SELECT child.id FROM folders child JOIN folders selected ON selected.id = ?1
              WHERE child.path = selected.path OR child.path LIKE selected.path || '/%'))
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
