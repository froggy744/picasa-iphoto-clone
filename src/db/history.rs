/// Record a committed operation inside the caller's transaction. A batch reuses
/// its event ID across chunks, including when an intervening chunk fails.
fn record_edit(
    connection: &Connection,
    photo_id: i64,
    action: &str,
    event_id: Option<i64>,
) -> Result<i64> {
    let now = chrono::Utc::now().timestamp_millis();
    let event_id = match event_id {
        Some(id) => {
            let stored: String = connection.query_row(
                "SELECT action FROM editing_events WHERE id = ?1",
                [id],
                |row| row.get(0),
            )?;
            anyhow::ensure!(stored == action, "History operation does not match batch");
            id
        }
        None => {
            connection.execute(
                "INSERT INTO editing_events(action,edited_at) VALUES (?1,?2)",
                params![action, now],
            )?;
            connection.last_insert_rowid()
        }
    };
    connection.execute(
        "INSERT OR IGNORE INTO editing_event_items(event_id,photo_id) VALUES (?1,?2)",
        params![event_id, photo_id],
    )?;
    connection.execute(
        "INSERT INTO recently_edited(photo_id,event_id,edited_at) VALUES (?1,?2,?3)
         ON CONFLICT(photo_id) DO UPDATE SET event_id=excluded.event_id,edited_at=excluded.edited_at",
        params![photo_id,event_id,now],
    )?;
    Ok(event_id)
}

pub fn commit_edit_recipes(
    connection: &Connection,
    updates: &[(i64, String)],
    action: &str,
    event_id: Option<i64>,
) -> Result<Option<i64>> {
    let transaction = connection.unchecked_transaction()?;
    let mut event = event_id;
    for (id, recipe) in updates {
        let changed = transaction.execute(
            "UPDATE photos SET edit_recipe=?1 WHERE id=?2 AND edit_recipe != ?1 AND trashed=0",
            params![recipe, id],
        )?;
        if changed > 0 {
            event = Some(record_edit(&transaction, *id, action, event)?);
        }
    }
    transaction.commit()?;
    Ok(event)
}

pub fn history_photos(connection: &Connection) -> Result<Vec<Photo>> {
    history_photos_limited(connection, -1)
}

pub fn history_photos_limited(connection: &Connection, limit: i64) -> Result<Vec<Photo>> {
    let mut statement = connection.prepare(
        "SELECT p.id,p.path,p.folder_id,p.taken_at,p.camera,p.width,p.height,p.size_bytes,p.mtime,p.added_at,p.rotation,p.edit_recipe,p.favorite,p.trashed,f.path,
                h.edited_at, c.photo_id IS NOT NULL
         FROM recently_edited h JOIN photos p ON p.id=h.photo_id
         LEFT JOIN folders f ON f.id=p.folder_id
         LEFT JOIN collage_projects c ON c.photo_id=p.id
         WHERE p.trashed=0 ORDER BY h.edited_at DESC,h.event_id DESC,p.id DESC LIMIT ?1",
    )?;
    let rows = statement.query_map([limit], |row| {
        let mut photo = photo_from_row(row)?;
        let timestamp: i64 = row.get(15)?;
        let kind = if row.get::<_, bool>(16)? {
            "Collage"
        } else {
            "Photo"
        };
        let date = chrono::DateTime::from_timestamp_millis(timestamp)
            .map(|date| {
                date.with_timezone(&chrono::Local)
                    .format("%Y-%m-%d %H:%M:%S")
                    .to_string()
            })
            .unwrap_or_default();
        photo.history_caption = Some(format!("{kind} · {date}"));
        Ok(photo)
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn collage_project(connection: &Connection, photo_id: i64) -> Result<Option<String>> {
    Ok(connection
        .query_row(
            "SELECT draft FROM collage_projects WHERE photo_id=?1",
            [photo_id],
            |row| row.get(0),
        )
        .optional()?)
}

/// An export is a library item, but its editable project is stored separately
/// from the rendered image. Re-saving keeps the same identity and History tile.
pub fn save_collage_project(
    connection: &Connection,
    photo_id: Option<i64>,
    path: &Path,
    metadata: &PhotoMetadata,
    draft: &str,
) -> Result<i64> {
    let mut saved_draft: serde_json::Value = serde_json::from_str(draft)?;
    anyhow::ensure!(saved_draft.is_object(), "Invalid collage project JSON");
    let transaction = connection.unchecked_transaction()?;
    let previous = photo_id
        .map(|id| collage_project(&transaction, id))
        .transpose()?
        .flatten();
    if photo_id.is_some() {
        anyhow::ensure!(
            previous.is_some(),
            "Saved collage no longer exists in the library"
        );
    }
    let id = if let Some(id) = photo_id {
        transaction.execute(
            "UPDATE photos SET path=?1,width=?2,height=?3,mtime=?4,size_bytes=?5 WHERE id=?6",
            params![
                path.to_string_lossy(),
                metadata.width,
                metadata.height,
                metadata.mtime,
                metadata.size_bytes,
                id
            ],
        )?;
        id
    } else {
        // Do not take over an unrelated library photo at an existing path.
        transaction.execute(
            "INSERT INTO photos(path,width,height,mtime,size_bytes,added_at) VALUES (?1,?2,?3,?4,?5,?6)",
            params![path.to_string_lossy(),metadata.width,metadata.height,metadata.mtime,metadata.size_bytes,chrono::Utc::now().timestamp()],
        )?;
        transaction.last_insert_rowid()
    };
    transaction.execute("INSERT INTO collage_projects(photo_id,draft) VALUES (?1,?2) ON CONFLICT(photo_id) DO UPDATE SET draft=excluded.draft", params![id,draft])?;
    if previous.as_deref() != Some(draft) {
        record_edit(
            &transaction,
            id,
            if previous.is_some() {
                "collage_saved"
            } else {
                "collage_created"
            },
            None,
        )?;
    }
    // Persist identity together with the project, even if the UI closes before
    // the worker's completion callback can update its in-memory draft.
    saved_draft["saved_photo_id"] = id.into();
    set_setting(
        &transaction,
        crate::collage::DRAFT_SETTING_KEY,
        &serde_json::to_string(&saved_draft)?,
    )?;
    transaction.commit()?;
    Ok(id)
}
