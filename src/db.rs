use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

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
  rotation INTEGER DEFAULT 0,
  favorite BOOLEAN DEFAULT 0,
  trashed BOOLEAN DEFAULT 0
);
CREATE TABLE IF NOT EXISTS folders (
  id INTEGER PRIMARY KEY,
  path TEXT UNIQUE NOT NULL,
  name TEXT,
  parent_id INTEGER REFERENCES folders(id),
  imported_root BOOLEAN NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS albums (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL COLLATE NOCASE UNIQUE,
  created_at INTEGER NOT NULL DEFAULT 0
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
CREATE INDEX IF NOT EXISTS idx_photos_taken_at ON photos(taken_at DESC);
CREATE INDEX IF NOT EXISTS idx_photos_folder ON photos(folder_id);
CREATE INDEX IF NOT EXISTS idx_album_photos_photo ON album_photos(photo_id);
"#;

#[derive(Debug, Clone)]
pub struct Folder {
    pub id: i64,
    pub path: String,
    pub name: String,
    pub parent_id: Option<i64>,
    pub imported_root: bool,
    pub photo_count: i64,
    pub subfolder_count: i64,
    pub available: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Album {
    pub id: i64,
    pub name: String,
    pub created_at: i64,
    pub photo_count: i64,
}

#[derive(Debug, Clone)]
pub struct Photo {
    pub id: i64,
    pub path: String,
    pub folder_id: Option<i64>,
    pub folder_path: Option<String>,
    pub taken_at: Option<String>,
    pub camera: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub size_bytes: Option<i64>,
    pub mtime: Option<i64>,
    pub rotation: i32,
    pub favorite: bool,
    pub trashed: bool,
}

#[derive(Debug, Clone, Default)]
pub struct PhotoMetadata {
    pub taken_at: Option<String>,
    pub camera: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub size_bytes: Option<i64>,
    pub mtime: Option<i64>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LibraryCounts {
    pub photos: i64,
    pub albums: i64,
    pub folders: i64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SidebarCounts {
    pub photos: i64,
    pub favorites: i64,
    pub recently_added: i64,
}

include!("db/core.rs");
include!("db/albums.rs");
include!("db/photos.rs");
include!("db/settings.rs");
include!("db/tests.rs");
