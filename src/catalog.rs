use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use walkdir::WalkDir;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS photos (
  id INTEGER PRIMARY KEY,
  path TEXT UNIQUE NOT NULL,
  folder_id INTEGER,
  taken_at TEXT,
  camera TEXT,
  width INTEGER,
  height INTEGER,
  size_bytes INTEGER,
  mtime INTEGER,
  added_at INTEGER NOT NULL DEFAULT 0,
  rotation INTEGER DEFAULT 0,
  edit_recipe TEXT NOT NULL DEFAULT '',
  favorite BOOLEAN DEFAULT 0,
  trashed BOOLEAN DEFAULT 0
);
CREATE TABLE IF NOT EXISTS folders (
  id INTEGER PRIMARY KEY,
  path TEXT UNIQUE NOT NULL,
  name TEXT,
  parent_id INTEGER REFERENCES folders(id),
  imported_root BOOLEAN NOT NULL DEFAULT 0,
  watched BOOLEAN NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS albums (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL COLLATE NOCASE UNIQUE,
  created_at INTEGER NOT NULL DEFAULT 0,
  cover_frame TEXT,
  cover_photo_id INTEGER REFERENCES photos(id) ON DELETE SET NULL
);
CREATE TABLE IF NOT EXISTS album_photos (
  album_id INTEGER NOT NULL REFERENCES albums(id) ON DELETE CASCADE,
  photo_id INTEGER NOT NULL REFERENCES photos(id) ON DELETE CASCADE,
  PRIMARY KEY(album_id, photo_id)
);
CREATE TABLE IF NOT EXISTS settings (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_photos_folder ON photos(folder_id);
CREATE INDEX IF NOT EXISTS idx_photos_taken_at ON photos(taken_at DESC);
CREATE INDEX IF NOT EXISTS idx_album_photos_photo ON album_photos(photo_id);
"#;

#[derive(Clone, Debug)]
pub struct PhotoRecord {
    pub id: i64,
    pub path: String,
    pub folder_path: String,
    pub favorite: bool,
    pub rotation: i32,
    pub taken_at: Option<String>,
    pub camera: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub size_bytes: Option<i64>,
}

#[derive(Clone, Debug, Default)]
pub struct ImportSummary {
    pub photos_seen: usize,
    pub folders_seen: usize,
}

pub fn database_path() -> Result<PathBuf> {
    let base = dirs::data_dir().context("could not determine the user's data directory")?;
    Ok(base.join("picasa-rs").join("library.db"))
}

pub fn open_default() -> Result<Connection> {
    let path = database_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let connection = Connection::open(path)?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    connection.pragma_update(None, "journal_mode", "WAL")?;
    connection.execute_batch(SCHEMA)?;
    ensure_column(&connection, "photos", "added_at", "INTEGER NOT NULL DEFAULT 0")?;
    ensure_column(&connection, "photos", "rotation", "INTEGER DEFAULT 0")?;
    ensure_column(&connection, "photos", "favorite", "BOOLEAN DEFAULT 0")?;
    ensure_column(&connection, "photos", "trashed", "BOOLEAN DEFAULT 0")?;
    ensure_column(&connection, "folders", "parent_id", "INTEGER")?;
    ensure_column(&connection, "folders", "imported_root", "BOOLEAN NOT NULL DEFAULT 0")?;
    ensure_column(&connection, "folders", "watched", "BOOLEAN NOT NULL DEFAULT 0")?;
    Ok(connection)
}

fn ensure_column(connection: &Connection, table: &str, column: &str, declaration: &str) -> Result<()> {
    let sql = format!("PRAGMA table_info({table})");
    let mut statement = connection.prepare(&sql)?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if !columns.iter().any(|name| name == column) {
        connection.execute_batch(&format!(
            "ALTER TABLE {table} ADD COLUMN {column} {declaration};"
        ))?;
    }
    Ok(())
}

pub fn roots() -> Result<Vec<PathBuf>> {
    let connection = open_default()?;
    let mut statement = connection.prepare(
        "SELECT path FROM folders WHERE imported_root = 1 ORDER BY path COLLATE NOCASE",
    )?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
    Ok(rows
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .map(PathBuf::from)
        .collect())
}

pub fn photos() -> Result<Vec<PhotoRecord>> {
    let connection = open_default()?;
    let mut statement = connection.prepare(
        "SELECT p.id, p.path, COALESCE(f.path, ''), p.favorite, p.rotation,
                p.taken_at, p.camera, p.width, p.height, p.size_bytes
         FROM photos p
         LEFT JOIN folders f ON f.id = p.folder_id
         WHERE p.trashed = 0
         ORDER BY COALESCE(f.path, ''), p.path COLLATE NOCASE",
    )?;
    let rows = statement.query_map([], |row| {
        Ok(PhotoRecord {
            id: row.get(0)?,
            path: row.get(1)?,
            folder_path: row.get(2)?,
            favorite: row.get(3)?,
            rotation: row.get(4)?,
            taken_at: row.get(5)?,
            camera: row.get(6)?,
            width: row.get(7)?,
            height: row.get(8)?,
            size_bytes: row.get(9)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn favorite_paths() -> Result<HashSet<String>> {
    let connection = open_default()?;
    let mut statement =
        connection.prepare("SELECT path FROM photos WHERE trashed = 0 AND favorite = 1")?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
    Ok(rows.collect::<rusqlite::Result<HashSet<_>>>()?)
}

pub fn set_favorite_by_path(path: &str, favorite: bool) -> Result<()> {
    let connection = open_default()?;
    connection.execute(
        "UPDATE photos SET favorite = ?1 WHERE path = ?2",
        params![favorite, path],
    )?;
    Ok(())
}

pub fn import_root(root: &Path) -> Result<ImportSummary> {
    let root = root
        .canonicalize()
        .with_context(|| format!("could not open {}", root.display()))?;
    let root_text = root.to_string_lossy().into_owned();
    let mut connection = open_default()?;
    let transaction = connection.transaction()?;

    transaction.execute(
        "INSERT INTO folders(path, name, parent_id, imported_root)
         VALUES (?1, ?2, NULL, 1)
         ON CONFLICT(path) DO UPDATE SET
           name = excluded.name,
           imported_root = 1",
        params![root_text, folder_name(&root)],
    )?;
    let root_id: i64 = transaction.query_row(
        "SELECT id FROM folders WHERE path = ?1",
        [&root_text],
        |row| row.get(0),
    )?;

    let mut folder_ids = HashMap::<PathBuf, i64>::new();
    folder_ids.insert(root.clone(), root_id);
    let added_at = now_epoch_seconds();
    let mut summary = ImportSummary::default();

    for entry in WalkDir::new(&root)
        .follow_links(false)
        .into_iter()
        .filter_map(|entry| entry.ok())
    {
        if entry.file_type().is_dir() {
            if entry.path() != root {
                let _ = ensure_folder(&transaction, &root, entry.path(), root_id, &mut folder_ids)?;
                summary.folders_seen += 1;
            }
            continue;
        }
        if !entry.file_type().is_file() || !is_displayable_photo(entry.path()) {
            continue;
        }

        let parent = entry.path().parent().unwrap_or(&root);
        let folder_id = ensure_folder(&transaction, &root, parent, root_id, &mut folder_ids)?;
        let metadata = entry.metadata().ok();
        let size_bytes = metadata.as_ref().map(|value| value.len() as i64);
        let mtime = metadata
            .and_then(|value| value.modified().ok())
            .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
            .map(|value| value.as_secs() as i64);
        let path = entry.path().to_string_lossy().into_owned();

        transaction.execute(
            "INSERT INTO photos(path, folder_id, size_bytes, mtime, added_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(path) DO UPDATE SET
               folder_id = excluded.folder_id,
               size_bytes = excluded.size_bytes,
               mtime = excluded.mtime,
               trashed = 0",
            params![path, folder_id, size_bytes, mtime, added_at],
        )?;
        summary.photos_seen += 1;
    }

    transaction.commit()?;
    Ok(summary)
}

fn ensure_folder(
    transaction: &Transaction<'_>,
    root: &Path,
    folder: &Path,
    root_id: i64,
    folder_ids: &mut HashMap<PathBuf, i64>,
) -> Result<i64> {
    if let Some(id) = folder_ids.get(folder).copied() {
        return Ok(id);
    }
    let Ok(relative) = folder.strip_prefix(root) else {
        return Ok(root_id);
    };

    let mut current = root.to_path_buf();
    let mut parent_id = root_id;
    for component in relative.components() {
        current.push(component.as_os_str());
        if let Some(id) = folder_ids.get(&current).copied() {
            parent_id = id;
            continue;
        }

        let path_text = current.to_string_lossy().into_owned();
        transaction.execute(
            "INSERT INTO folders(path, name, parent_id, imported_root)
             VALUES (?1, ?2, ?3, 0)
             ON CONFLICT(path) DO UPDATE SET
               name = excluded.name,
               parent_id = excluded.parent_id",
            params![path_text, folder_name(&current), parent_id],
        )?;
        let id: i64 = transaction.query_row(
            "SELECT id FROM folders WHERE path = ?1",
            [&path_text],
            |row| row.get(0),
        )?;
        folder_ids.insert(current.clone(), id);
        parent_id = id;
    }
    Ok(parent_id)
}

fn folder_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| path.to_str().unwrap_or("Photos"))
        .to_string()
}

fn now_epoch_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs() as i64)
        .unwrap_or_default()
}

pub fn is_displayable_photo(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
        return false;
    };
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "jpg" | "jpeg" | "png" | "webp" | "bmp" | "gif" | "tif" | "tiff"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_extensions_are_case_insensitive() {
        assert!(is_displayable_photo(Path::new("a.JPG")));
        assert!(is_displayable_photo(Path::new("a.tiff")));
        assert!(!is_displayable_photo(Path::new("a.txt")));
    }
}
