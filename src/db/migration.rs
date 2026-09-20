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
    let backup_dir = database_path()?
        .parent()
        .map(|directory| directory.to_path_buf())
        .context("could not locate the library directory")?;
    migrate_gvfs_paths_in(connection, &backup_dir)
}

/// `migrate_gvfs_paths` with an explicit backup directory, so tests can run
/// the full rewrite hermetically against an in-memory library.
pub fn migrate_gvfs_paths_in(connection: &Connection, backup_dir: &Path) -> Result<()> {
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
    let backup_path = backup_dir.join(GVFS_PATH_MIGRATION_BACKUP);
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

#[cfg(test)]
mod migration_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn unique_temp_dir(tag: &str) -> PathBuf {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let directory = std::env::temp_dir().join(format!(
            "pic-migration-{}-{}-{}",
            tag,
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    fn gvfs_library() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(SCHEMA).unwrap();
        // The accidental-import shape: a folder and its photos registered
        // under the gvfs FUSE root, plus untouched local rows for contrast.
        connection
            .execute(
                "INSERT INTO folders (id, path, name, imported_root) VALUES (1,
                 '/run/user/1000/gvfs/smb-share:server=dietpi.local,share=4tbs/pics-sport/local choice',
                 'local choice', 1)",
                [],
            )
            .unwrap();
        connection
            .execute("INSERT INTO folders (id, path, name) VALUES (2, '/home/peet/Pictures', 'Pictures')", [])
            .unwrap();
        for (id, suffix, mtime, size) in [
            (10, "DSC_001.jpg", 1_000, 5_000),
            (11, "DSC_002.jpg", 2_000, 6_000),
        ] {
            connection
                .execute(
                    "INSERT INTO photos (id, path, folder_id, mtime, size_bytes, rotation,
                         edit_recipe, favorite, added_at)
                     VALUES (?1, '/run/user/1000/gvfs/smb-share:server=dietpi.local,share=4tbs/pics-sport/local choice/'
                         || ?2, 1, ?3, ?4, 90, 'v=1|crop=0.1,0.1,0.8,0.8', 1, 42)",
                    rusqlite::params![id, suffix, mtime, size],
                )
                .unwrap();
        }
        connection
            .execute(
                "INSERT INTO photos (id, path, folder_id, added_at) VALUES (12, '/home/peet/Pictures/x.png', 2, 7)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO albums (id, name, cover_photo_id) VALUES (1, 'Trip', 11)",
                [],
            )
            .unwrap();
        connection
            .execute("INSERT INTO album_photos (album_id, photo_id) VALUES (1, 11)", [])
            .unwrap();
        connection
    }

    fn photo_column(connection: &Connection, id: i64, column: &str) -> String {
        connection
            .query_row(
                &format!("SELECT CAST({column} AS TEXT) FROM photos WHERE id = ?1"),
                [id],
                |row| row.get(0),
            )
            .unwrap()
    }

    #[test]
    fn rewrites_gvfs_paths_once_without_duplication_or_loss() {
        let backup_dir = unique_temp_dir("backup");
        let connection = gvfs_library();

        migrate_gvfs_paths_in(&connection, &backup_dir).unwrap();

        // Folder: stable URI, same row (ID preserved), local folder untouched.
        let folder_path: String = connection
            .query_row("SELECT path FROM folders WHERE id = 1", [], |row| row.get(0))
            .unwrap();
        assert_eq!(folder_path, "smb://dietpi.local/4tbs/pics-sport/local%20choice");
        let local_folder: String = connection
            .query_row("SELECT path FROM folders WHERE id = 2", [], |row| row.get(0))
            .unwrap();
        assert_eq!(local_folder, "/home/peet/Pictures");

        // Photos: same count, same IDs, rewritten paths, no gvfs leftovers.
        let (total, gvfs_left): (i64, i64) = connection
            .query_row(
                "SELECT COUNT(*), SUM(path LIKE '/run/user/%') FROM photos",
                [],
                |row| Ok((row.get(0)?, row.get::<_, Option<i64>>(1)?.unwrap_or(0))),
            )
            .unwrap();
        assert_eq!(total, 3);
        assert_eq!(gvfs_left, 0);
        assert_eq!(
            photo_column(&connection, 10, "path"),
            "smb://dietpi.local/4tbs/pics-sport/local%20choice/DSC_001.jpg"
        );
        assert_eq!(
            photo_column(&connection, 11, "path"),
            "smb://dietpi.local/4tbs/pics-sport/local%20choice/DSC_002.jpg"
        );

        // Metadata preserved exactly.
        assert_eq!(photo_column(&connection, 10, "rotation"), "90");
        assert_eq!(
            photo_column(&connection, 10, "edit_recipe"),
            "v=1|crop=0.1,0.1,0.8,0.8"
        );
        let favorite: i64 = connection
            .query_row("SELECT favorite FROM photos WHERE id = 10", [], |row| row
                .get(0))
            .unwrap();
        assert_eq!(favorite, 1);

        // Albums untouched, cover still points at the same photo.
        let (album_links, cover): (i64, Option<i64>) = connection
            .query_row(
                "SELECT (SELECT COUNT(*) FROM album_photos),
                        (SELECT cover_photo_id FROM albums WHERE id = 1)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(album_links, 1);
        assert_eq!(cover, Some(11));

        // Backup captured the pre-migration state.
        let backup = backup_dir.join(GVFS_PATH_MIGRATION_BACKUP);
        let backup = Connection::open(&backup).unwrap();
        let gvfs_in_backup: i64 = backup
            .query_row(
                "SELECT COUNT(*) FROM photos WHERE path LIKE '/run/user/%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(gvfs_in_backup, 2);

        // Idempotent: rerun is a no-op (flag set, nothing matches anyway).
        migrate_gvfs_paths_in(&connection, &backup_dir).unwrap();
        let total_after: i64 = connection
            .query_row("SELECT COUNT(*) FROM photos", [], |row| row.get(0))
            .unwrap();
        assert_eq!(total_after, 3);

        let _ = std::fs::remove_dir_all(&backup_dir);
    }

    #[test]
    fn local_only_libraries_are_flagged_without_a_backup() {
        let backup_dir = unique_temp_dir("local");
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(SCHEMA).unwrap();
        connection
            .execute(
                "INSERT INTO folders (id, path, name) VALUES (1, '/home/peet/Pictures', 'Pictures')",
                [],
            )
            .unwrap();
        migrate_gvfs_paths_in(&connection, &backup_dir).unwrap();
        assert_eq!(
            setting(&connection, GVFS_PATH_MIGRATION_KEY).unwrap().as_deref(),
            Some("done")
        );
        assert!(!backup_dir.join(GVFS_PATH_MIGRATION_BACKUP).exists());
        let _ = std::fs::remove_dir_all(&backup_dir);
    }
}
