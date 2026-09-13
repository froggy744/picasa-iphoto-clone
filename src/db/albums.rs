pub fn albums(connection: &Connection) -> Result<Vec<Album>> {
    let mut statement = connection.prepare(
        "SELECT a.id, a.name, a.created_at, COUNT(p.id), a.cover_frame, a.cover_photo_id
         FROM albums a
         LEFT JOIN album_photos ap ON ap.album_id = a.id
         LEFT JOIN photos p ON p.id = ap.photo_id AND p.trashed = 0
         GROUP BY a.id
         ORDER BY a.name COLLATE NOCASE, a.id",
    )?;
    let rows = statement.query_map([], |row| {
        Ok(Album {
            id: row.get(0)?,
            name: row.get(1)?,
            created_at: row.get(2)?,
            photo_count: row.get(3)?,
            cover_frame: row.get(4)?,
            cover_photo_id: row.get(5)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn create_album(connection: &Connection, name: &str) -> Result<Album> {
    let name = name.trim();
    if name.is_empty() {
        anyhow::bail!("album name cannot be blank");
    }
    let duplicate = connection
        .query_row(
            "SELECT id FROM albums WHERE name = ?1 COLLATE NOCASE",
            [name],
            |row| row.get::<_, i64>(0),
        )
        .optional()?;
    if duplicate.is_some() {
        anyhow::bail!("an album named ‘{name}’ already exists");
    }
    connection.execute(
        "INSERT INTO albums(name, created_at)
         VALUES (?1, CAST(strftime('%s', 'now') AS INTEGER))",
        [name],
    )?;
    let id = connection.last_insert_rowid();
    Ok(Album {
        id,
        name: name.to_string(),
        created_at: connection.query_row(
            "SELECT created_at FROM albums WHERE id = ?1",
            [id],
            |row| row.get(0),
        )?,
        photo_count: 0,
        cover_frame: None,
        cover_photo_id: None,
    })
}

/// Rename an album while retaining the same validation as album creation.
pub fn rename_album(connection: &Connection, album_id: i64, name: &str) -> Result<()> {
    let name = name.trim();
    if name.is_empty() {
        anyhow::bail!("album name cannot be blank");
    }
    let updated = connection.execute(
        "UPDATE albums SET name = ?1 WHERE id = ?2",
        params![name, album_id],
    )?;
    if updated == 0 {
        anyhow::bail!("album no longer exists");
    }
    Ok(())
}

pub fn set_album_cover_frame(
    connection: &Connection,
    album_id: i64,
    cover_frame: &str,
) -> Result<()> {
    connection.execute(
        "UPDATE albums SET cover_frame = ?1 WHERE id = ?2",
        params![cover_frame, album_id],
    )?;
    Ok(())
}

/// Drop an album's own frame so it follows the page's automatic assignment
/// again.
pub fn clear_album_cover_frame(connection: &Connection, album_id: i64) -> Result<()> {
    connection.execute(
        "UPDATE albums SET cover_frame = NULL WHERE id = ?1",
        [album_id],
    )?;
    Ok(())
}

/// Drop every album's own frame, returning the number that had one.
pub fn clear_all_album_cover_frames(connection: &Connection) -> Result<usize> {
    Ok(connection.execute(
        "UPDATE albums SET cover_frame = NULL WHERE cover_frame IS NOT NULL",
        [],
    )?)
}

/// Choose the photo the album shows on its card.
pub fn set_album_cover_photo(connection: &Connection, album_id: i64, photo_id: i64) -> Result<()> {
    connection.execute(
        "UPDATE albums SET cover_photo_id = ?1 WHERE id = ?2",
        params![photo_id, album_id],
    )?;
    Ok(())
}

/// Drop one album's chosen cover photo so it goes back to the automatic pick.
pub fn clear_album_cover_photo(connection: &Connection, album_id: i64) -> Result<()> {
    connection.execute(
        "UPDATE albums SET cover_photo_id = NULL WHERE id = ?1",
        [album_id],
    )?;
    Ok(())
}

/// Drop every album's chosen cover photo, returning the number that had one.
pub fn clear_all_album_cover_photos(connection: &Connection) -> Result<usize> {
    Ok(connection.execute(
        "UPDATE albums SET cover_photo_id = NULL WHERE cover_photo_id IS NOT NULL",
        [],
    )?)
}

pub fn delete_album(connection: &Connection, album_id: i64) -> Result<()> {
    connection.execute("DELETE FROM albums WHERE id = ?1", [album_id])?;
    Ok(())
}

pub fn add_photos_to_album(
    connection: &Connection,
    album_id: i64,
    photo_ids: &[i64],
) -> Result<usize> {
    let transaction = connection.unchecked_transaction()?;
    let mut added = 0;
    for photo_id in photo_ids {
        added += transaction.execute(
            "INSERT OR IGNORE INTO album_photos(album_id, photo_id) VALUES (?1, ?2)",
            params![album_id, photo_id],
        )?;
    }
    transaction.commit()?;
    Ok(added)
}

pub fn remove_photos_from_album(
    connection: &Connection,
    album_id: i64,
    photo_ids: &[i64],
) -> Result<usize> {
    let transaction = connection.unchecked_transaction()?;
    let mut removed = 0;
    for photo_id in photo_ids {
        removed += transaction.execute(
            "DELETE FROM album_photos WHERE album_id = ?1 AND photo_id = ?2",
            params![album_id, photo_id],
        )?;
    }
    transaction.commit()?;
    Ok(removed)
}
