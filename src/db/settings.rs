pub fn setting(connection: &Connection, key: &str) -> Result<Option<String>> {
    Ok(connection
        .query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
            row.get(0)
        })
        .optional()?)
}

pub fn set_setting(connection: &Connection, key: &str, value: &str) -> Result<()> {
    connection.execute(
        "INSERT INTO settings(key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

pub fn library_counts(connection: &Connection) -> Result<LibraryCounts> {
    Ok(LibraryCounts {
        photos: connection.query_row(
            "SELECT COUNT(*) FROM photos WHERE trashed = 0",
            [],
            |row| row.get(0),
        )?,
        albums: connection.query_row("SELECT COUNT(*) FROM albums", [], |row| row.get(0))?,
        folders: connection.query_row("SELECT COUNT(*) FROM folders", [], |row| row.get(0))?,
    })
}

/// Counts shown beside the top-level Library destinations in the sidebar.
/// `RecentlyAdded` currently uses the same photo set as All Photos, so keep
/// its count aligned with what that destination actually displays.
pub fn sidebar_counts(connection: &Connection) -> Result<SidebarCounts> {
    let photos: i64 = connection.query_row(
        "SELECT COUNT(*) FROM photos WHERE trashed = 0",
        [],
        |row| row.get(0),
    )?;
    let favorites: i64 = connection.query_row(
        "SELECT COUNT(*) FROM photos WHERE trashed = 0 AND favorite = 1",
        [],
        |row| row.get(0),
    )?;
    Ok(SidebarCounts {
        photos,
        favorites,
        recently_added: photos,
    })
}

pub fn database_size(connection: &Connection) -> Result<u64> {
    let path: String = connection.query_row(
        "SELECT file FROM pragma_database_list WHERE name = 'main'",
        [],
        |row| row.get(0),
    )?;
    if path.is_empty() {
        return Ok(0);
    }
    Ok(std::fs::metadata(path)?.len())
}

pub fn set_photo_path(connection: &Connection, id: i64, path: &str) -> Result<()> {
    connection.execute(
        "UPDATE photos SET path = ?1 WHERE id = ?2",
        params![path, id],
    )?;
    Ok(())
}

pub fn set_trashed(connection: &Connection, id: i64, trashed: bool) -> Result<()> {
    connection.execute(
        "UPDATE photos SET trashed = ?1 WHERE id = ?2",
        params![trashed, id],
    )?;
    Ok(())
}

pub fn clear_photos(connection: &Connection) -> Result<()> {
    connection.execute_batch("DELETE FROM album_photos; DELETE FROM photos;")?;
    Ok(())
}

pub fn clear_all(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "DELETE FROM album_photos; DELETE FROM albums; DELETE FROM photos; DELETE FROM folders;",
    )?;
    Ok(())
}

