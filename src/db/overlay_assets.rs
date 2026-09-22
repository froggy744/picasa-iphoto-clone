/// Metadata for one content-addressed overlay asset stored in the managed
/// overlay directory. The image bytes themselves live on disk, keyed by hash;
/// this table only records what the library knows about each asset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayAsset {
    pub hash: String,
    pub width: i64,
    pub height: i64,
    pub format: String,
    pub original_name: String,
    pub added_at: i64,
}

/// Insert or refresh an overlay asset record. Re-importing identical content
/// (same hash) updates the original filename but keeps a single row.
pub fn upsert_overlay_asset(
    connection: &Connection,
    hash: &str,
    width: i64,
    height: i64,
    format: &str,
    original_name: &str,
) -> Result<()> {
    connection.execute(
        "INSERT INTO overlay_assets(hash, width, height, format, original_name, added_at)
         VALUES (?1, ?2, ?3, ?4, ?5, CAST(strftime('%s', 'now') AS INTEGER))
         ON CONFLICT(hash) DO UPDATE SET
           width = excluded.width,
           height = excluded.height,
           format = excluded.format,
           original_name = excluded.original_name",
        params![hash, width, height, format, original_name],
    )?;
    Ok(())
}

pub fn overlay_asset(connection: &Connection, hash: &str) -> Result<Option<OverlayAsset>> {
    Ok(connection
        .query_row(
            "SELECT hash, width, height, format, original_name, added_at
             FROM overlay_assets WHERE hash = ?1",
            [hash],
            overlay_asset_from_row,
        )
        .optional()?)
}

/// Every recorded overlay asset, newest additions first.
#[cfg(test)]
pub fn overlay_assets(connection: &Connection) -> Result<Vec<OverlayAsset>> {
    let mut statement = connection.prepare(
        "SELECT hash, width, height, format, original_name, added_at
         FROM overlay_assets ORDER BY added_at DESC, hash",
    )?;
    let rows = statement.query_map([], overlay_asset_from_row)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn overlay_asset_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<OverlayAsset> {
    Ok(OverlayAsset {
        hash: row.get(0)?,
        width: row.get(1)?,
        height: row.get(2)?,
        format: row.get(3)?,
        original_name: row.get(4)?,
        added_at: row.get(5)?,
    })
}
