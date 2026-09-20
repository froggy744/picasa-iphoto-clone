// One-time rewrite of gvfs FUSE paths to stable network URIs.
//
// Folders imported through a gvfs mount were registered under
// `/run/user/N/gvfs/...`. Such rows index fine, but PIC displays them
// nowhere: the Folders tree excludes gvfs paths and Network Shares only
// lists scheme paths. This migration rewrites every affected folder and
// photo path to its canonical smb:// / nfs:// URI.
//
// Safety properties (deliberate, not incidental):
// - Non-destructive: a consistent snapshot of the library is written with
//   `VACUUM INTO` BEFORE anything changes, and an existing snapshot is
//   never overwritten.
// - Transactional: all path updates run in one transaction; any failure
//   rolls the whole rewrite back and leaves the library untouched.
// - Preserving: photo IDs, rotations, edit recipes, favourites, album
//   links and album covers are untouched (only `path`/`folder` path
//   columns change). Thumbnails and materialized sources keep their bytes:
//   their file names hash the source path, so each cache entry is renamed
//   to the new key after the commit. A miss would only regenerate lazily.
// - Idempotent: a settings flag records completion; reruns exit early.

const GVFS_PATH_MIGRATION_KEY: &str = "gvfs-path-migration-v1";
const GVFS_PATH_MIGRATION_BACKUP: &str = "library.pre-gvfs-migration.db";

struct PathRewrite {
    old: String,
    new: String,
}

pub fn migrate_gvfs_paths(connection: &Connection) -> Result<()> {
    if setting(connection, GVFS_PATH_MIGRATION_KEY)?.as_deref() == Some("done") {
        return Ok(());
    }

    let mut folder_rewrites: Vec<(i64, PathRewrite)> = Vec::new();
    {
        let mut statement = connection.prepare("SELECT id, path FROM folders")?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let id: i64 = row.get(0)?;
            let path: String = row.get(1)?;
            let new = crate::source::normalize_import_reference(&path);
            if new != path {
                folder_rewrites.push((id, PathRewrite { old: path, new }));
            }
        }
    }

    let mut photo_rewrites: Vec<(i64, PathRewrite, Option<i64>, Option<i64>)> = Vec::new();
    {
        let mut statement =
            connection.prepare("SELECT id, path, mtime, size_bytes FROM photos")?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let id: i64 = row.get(0)?;
            let path: String = row.get(1)?;
            let mtime: Option<i64> = row.get(2)?;
            let size_bytes: Option<i64> = row.get(3)?;
            let new = crate::source::normalize_import_reference(&path);
            if new != path {
                photo_rewrites.push((
                    id,
                    PathRewrite { old: path, new },
                    mtime,
                    size_bytes,
                ));
            }
        }
    }

    if folder_rewrites.is_empty() && photo_rewrites.is_empty() {
        set_setting(connection, GVFS_PATH_MIGRATION_KEY, "done")?;
        return Ok(());
    }
    eprintln!(
        "gvfs path migration: rewriting {} folder(s) and {} photo path(s)",
        folder_rewrites.len(),
        photo_rewrites.len()
    );

    // Snapshot BEFORE any change. A pre-existing snapshot is kept: the very
    // first pre-migration state is the one worth preserving.
    let backup_path = database_path()?
        .parent()
        .map(|directory| directory.join(GVFS_PATH_MIGRATION_BACKUP))
        .context("could not locate the library backup directory")?;
    if !backup_path.exists() {
        connection
            .prepare("VACUUM INTO ?1")?
            .execute([backup_path.to_string_lossy().as_ref()])?;
        eprintln!(
            "gvfs path migration: library backed up to {}",
            backup_path.display()
        );
    }

    let transaction = connection.unchecked_transaction()?;
    for (id, rewrite) in &folder_rewrites {
        transaction.execute(
            "UPDATE folders SET path = ?2 WHERE id = ?1",
            params![id, rewrite.new],
        )?;
    }
    for (id, rewrite, _, _) in &photo_rewrites {
        transaction.execute(
            "UPDATE photos SET path = ?2 WHERE id = ?1",
            params![id, rewrite.new],
        )?;
    }
    transaction.commit()?;
    set_setting(connection, GVFS_PATH_MIGRATION_KEY, "done")?;

    for (_, rewrite, mtime, size_bytes) in &photo_rewrites {
        crate::thumbnail::migrate_cache_entry(&rewrite.old, &rewrite.new, *mtime, *size_bytes);
        migrate_source_cache(&rewrite.old, &rewrite.new);
    }
    Ok(())
}

/// Move a materialized remote source to the cache key of the new path.
/// File names hash the reference (see `source::materialize`). Best-effort.
fn migrate_source_cache(old_reference: &str, new_reference: &str) {
    let hex = |reference: &str| {
        let mut hasher = blake3::Hasher::new();
        hasher.update(reference.as_bytes());
        hasher.finalize().to_hex().to_string()
    };
    let extension = std::path::Path::new(new_reference)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("raw");
    let old_name = format!("{}.{}", hex(old_reference), extension);
    let new_name = format!("{}.{}", hex(new_reference), extension);
    if old_name == new_name {
        return;
    }
    let Ok(sources_root) = crate::thumbnail::sources_dir() else {
        return;
    };
    let old_path = crate::thumbnail::shard_dir_for_sources(&sources_root, &old_name).join(&old_name);
    if !old_path.is_file() {
        // Pre-sharding flat location.
        let legacy = sources_root.join(&old_name);
        if !legacy.is_file() {
            return;
        }
        let new_path =
            crate::thumbnail::shard_dir_for_sources(&sources_root, &new_name).join(&new_name);
        if let Some(parent) = new_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::rename(&legacy, &new_path);
        return;
    }
    let new_path = crate::thumbnail::shard_dir_for_sources(&sources_root, &new_name).join(&new_name);
    if let Some(parent) = new_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::rename(&old_path, &new_path);
}
