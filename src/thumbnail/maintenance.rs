// Safe thumbnail-cache garbage collection. The rules are deliberately
// conservative: a thumbnail is only removed when its exact cache key no longer
// matches any current library photo, directories are never touched, and an
// original that is merely offline can never make its thumbnail look stale.

use rusqlite::Connection;

/// Outcome of one cleanup pass over the thumbnail cache directory.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CacheCleanup {
    /// Cached `.jpg` files that still match a current photo cache key.
    pub valid: usize,
    /// Cached `.jpg` files deleted because no current photo references them.
    pub removed: usize,
    /// Bytes reclaimed by the pass, including removed stale `.failed` markers.
    pub bytes_freed: u64,
}

/// Snapshot of the cache directory for the Library statistics page.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CacheStats {
    /// `.jpg` thumbnail files present in the cache directory.
    pub cached: u64,
    /// Present `.jpg` files whose cache key matches a current library photo.
    pub referenced: u64,
    /// Cache keys expected from the current (non-trashed) library photos.
    pub required: u64,
    /// Bytes used by the top-level cache files (thumbnails, markers, other).
    pub bytes: u64,
}

impl CacheStats {
    /// Cached `.jpg` files that no current photo references.
    pub fn unused(&self) -> u64 {
        self.cached.saturating_sub(self.referenced)
    }
}

/// The cache paths every current (non-trashed) library photo expects in the
/// cache directory. Keys are derived purely from database fingerprints, so a
/// photo whose original is offline right now still owns its thumbnail and
/// survives a cleanup.
pub fn valid_cache_paths(connection: &Connection) -> Result<HashSet<PathBuf>> {
    valid_cache_paths_in(&cache_dir()?, connection)
}

fn valid_cache_paths_in(directory: &Path, connection: &Connection) -> Result<HashSet<PathBuf>> {
    let mut statement =
        connection.prepare("SELECT path, mtime, size_bytes FROM photos WHERE trashed = 0")?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<i64>>(1)?,
            row.get::<_, Option<i64>>(2)?,
        ))
    })?;
    Ok(rows
        .map(|row| {
            let (path, mtime, size_bytes) = row?;
            Ok(directory.join(cache_file_name(&path, mtime, size_bytes)))
        })
        .collect::<rusqlite::Result<HashSet<_>>>()?)
}

/// Delete thumbnails the current library no longer references.
///
/// Only `.jpg` files directly inside the cache directory are considered, and a
/// file is removed only when its cache key matches no current photo. Stale
/// `.failed` markers go with their keys; a marker whose key is still current
/// stays so known-bad sources keep being suppressed. Subdirectories (such as
/// the materialized remote RAW sources under `thumbs/source`), symlinks and
/// unrelated files are never touched, and neither are the originals.
pub fn cleanup_cache(valid: &HashSet<PathBuf>) -> Result<CacheCleanup> {
    // A thumbnail being generated right now may not be part of the key
    // snapshot yet. Both registries cover every `create()` destination that
    // can be mid-write, so reserve them instead of racing the workers.
    let reserved = currently_generating_cache_paths();
    cleanup_cache_in(&cache_dir()?, valid, &reserved)
}

fn cleanup_cache_in(
    directory: &Path,
    valid: &HashSet<PathBuf>,
    reserved: &HashSet<PathBuf>,
) -> Result<CacheCleanup> {
    let mut cleanup = CacheCleanup::default();
    for entry in read_cache_entries(directory)? {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        // Only regular files are candidates; this also skips directories that
        // merely carry a thumbnail-looking name.
        if !file_type.is_file() {
            continue;
        }
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name.ends_with(".jpg") {
            if valid.contains(&path) || reserved.contains(&path) {
                cleanup.valid += 1;
            } else {
                // Read the size before the file disappears; DirEntry::metadata
                // no longer resolves once the entry is deleted.
                let bytes = entry_len(&entry);
                if fs::remove_file(&path).is_ok() {
                    cleanup.removed += 1;
                    cleanup.bytes_freed += bytes;
                }
            }
        } else if name.ends_with(".failed") {
            let thumbnail = path.with_extension("jpg");
            if !valid.contains(&thumbnail) && !reserved.contains(&thumbnail) {
                let bytes = entry_len(&entry);
                if fs::remove_file(&path).is_ok() {
                    cleanup.bytes_freed += bytes;
                }
            }
        }
    }
    Ok(cleanup)
}

/// Measure the cache directory against the current photo cache keys.
pub fn cache_stats(valid: &HashSet<PathBuf>) -> Result<CacheStats> {
    cache_stats_in(&cache_dir()?, valid)
}

fn cache_stats_in(directory: &Path, valid: &HashSet<PathBuf>) -> Result<CacheStats> {
    let mut stats = CacheStats {
        required: valid.len() as u64,
        ..CacheStats::default()
    };
    for entry in read_cache_entries(directory)? {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_file() {
            continue;
        }
        let path = entry.path();
        stats.bytes += entry_len(&entry);
        let is_jpg = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(".jpg"));
        if is_jpg {
            stats.cached += 1;
            if valid.contains(&path) {
                stats.referenced += 1;
            }
        }
    }
    Ok(stats)
}

fn read_cache_entries(directory: &Path) -> Result<Vec<fs::DirEntry>> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        // A missing cache directory simply holds nothing to clean or count.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(anyhow::Error::new(error).context(format!(
                "could not read cache directory {}",
                directory.display()
            )))
        }
    };
    Ok(entries.flatten().collect())
}

fn entry_len(entry: &fs::DirEntry) -> u64 {
    entry
        .metadata()
        .map(|metadata| metadata.len())
        .unwrap_or_default()
}

/// Destinations a thumbnail worker may be writing right now: `create()`
/// claims its target in `IN_FLIGHT` before decoding, and foreground requests
/// stay in `PRIORITY_PENDING` until their worker finishes.
fn currently_generating_cache_paths() -> HashSet<PathBuf> {
    let mut paths = HashSet::new();
    if let Some(in_flight) = IN_FLIGHT.get() {
        if let Ok(in_flight) = in_flight.lock() {
            paths.extend(in_flight.iter().cloned());
        }
    }
    if let Some(pending) = PRIORITY_PENDING.get() {
        if let Ok(pending) = pending.lock() {
            paths.extend(pending.iter().cloned());
        }
    }
    paths
}

#[cfg(test)]
mod cleanup_tests {
    use super::*;

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    struct Fixture {
        directory: PathBuf,
        connection: Connection,
    }

    impl Fixture {
        fn new() -> Self {
            let directory = std::env::temp_dir().join(format!(
                "picasa-thumb-cleanup-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&directory).unwrap();
            let connection = Connection::open_in_memory().unwrap();
            connection
                .execute_batch(
                    "CREATE TABLE photos (
                       path TEXT UNIQUE NOT NULL,
                       mtime INTEGER,
                       size_bytes INTEGER,
                       trashed BOOLEAN DEFAULT 0
                     );",
                )
                .unwrap();
            Self {
                directory,
                connection,
            }
        }

        fn add_photo(&self, path: &str, mtime: Option<i64>, size_bytes: Option<i64>) {
            self.connection
                .execute(
                    "INSERT INTO photos(path, mtime, size_bytes) VALUES (?1, ?2, ?3)",
                    rusqlite::params![path, mtime, size_bytes],
                )
                .unwrap();
        }

        fn add_trashed_photo(&self, path: &str, mtime: Option<i64>, size_bytes: Option<i64>) {
            self.connection
                .execute(
                    "INSERT INTO photos(path, mtime, size_bytes, trashed) VALUES (?1, ?2, ?3, 1)",
                    rusqlite::params![path, mtime, size_bytes],
                )
                .unwrap();
        }

        fn thumbnail_name(&self, path: &str, mtime: Option<i64>, size_bytes: Option<i64>) -> String {
            cache_file_name(path, mtime, size_bytes)
        }

        /// The failure marker name exactly as `create()` derives it: the
        /// thumbnail path with its extension replaced by `failed`.
        fn failure_marker_name(
            &self,
            path: &str,
            mtime: Option<i64>,
            size_bytes: Option<i64>,
        ) -> String {
            Path::new(&self.thumbnail_name(path, mtime, size_bytes))
                .with_extension("failed")
                .to_string_lossy()
                .into_owned()
        }

        fn write(&self, name: &str, contents: &[u8]) -> PathBuf {
            let path = self.directory.join(name);
            fs::write(&path, contents).unwrap();
            path
        }

        fn valid_paths(&self) -> HashSet<PathBuf> {
            valid_cache_paths_in(&self.directory, &self.connection).unwrap()
        }

        fn clean(&self) -> CacheCleanup {
            cleanup_cache_in(&self.directory, &self.valid_paths(), &HashSet::new()).unwrap()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.directory);
        }
    }

    #[test]
    fn valid_thumbnail_is_preserved() {
        let fixture = Fixture::new();
        fixture.add_photo("/photos/vacation.jpg", Some(1000), Some(2000));
        let thumbnail = fixture.write(
            &fixture.thumbnail_name("/photos/vacation.jpg", Some(1000), Some(2000)),
            b"jpeg bytes",
        );

        let cleanup = fixture.clean();

        assert!(thumbnail.is_file());
        assert_eq!(cleanup, CacheCleanup { valid: 1, removed: 0, bytes_freed: 0 });
    }

    #[test]
    fn stale_thumbnail_is_removed() {
        let fixture = Fixture::new();
        // The original was edited after this thumbnail was written: its mtime
        // changed, so the cache key changed too.
        fixture.add_photo("/photos/edited.jpg", Some(2000), Some(3000));
        let stale = fixture.write(
            &fixture.thumbnail_name("/photos/edited.jpg", Some(1000), Some(3000)),
            b"stale bytes",
        );
        let expected_bytes = stale.metadata().unwrap().len();

        let cleanup = fixture.clean();

        assert!(!stale.exists());
        assert_eq!(cleanup.removed, 1);
        assert_eq!(cleanup.bytes_freed, expected_bytes);
    }

    #[test]
    fn stale_failure_marker_is_removed() {
        let fixture = Fixture::new();
        fixture.add_photo("/photos/retagged.png", Some(7), Some(9));
        // Marker left by an earlier fingerprint (older mtime) of the photo.
        let marker = fixture.write(&fixture.failure_marker_name("/photos/retagged.png", Some(6), Some(9)), DECODE_FAILURE_MARKER);
        let expected_bytes = marker.metadata().unwrap().len();

        let cleanup = fixture.clean();

        assert!(!marker.exists());
        assert_eq!(cleanup.bytes_freed, expected_bytes);
        // Markers are not thumbnails and must not inflate the removed count.
        assert_eq!(cleanup.removed, 0);
    }

    #[test]
    fn valid_failure_marker_is_preserved() {
        let fixture = Fixture::new();
        fixture.add_photo("/photos/broken.png", Some(11), Some(22));
        let marker = fixture.write(&fixture.failure_marker_name("/photos/broken.png", Some(11), Some(22)), DECODE_FAILURE_MARKER);

        fixture.clean();

        assert!(marker.is_file());
    }

    #[test]
    fn subdirectories_are_untouched() {
        let fixture = Fixture::new();
        // Materialized remote RAW sources live in a subdirectory of the cache.
        let sources = fixture.directory.join("source");
        fs::create_dir_all(&sources).unwrap();
        let raw = sources.join("abcdef.raw");
        fs::write(&raw, b"raw bytes").unwrap();
        // A directory that merely looks like a thumbnail is also left alone.
        let directory_named_jpg = fixture.directory.join("feedface.jpg");
        fs::create_dir_all(&directory_named_jpg).unwrap();

        fixture.clean();

        assert!(raw.is_file());
        assert!(directory_named_jpg.is_dir());
    }

    #[test]
    fn offline_photos_keep_their_valid_thumbnail() {
        let fixture = Fixture::new();
        // The original is not on disk right now (unmounted share); only the
        // database row and the cached thumbnail exist.
        fixture.add_photo("/mnt/offline-share/trip.raw", Some(42), Some(84));
        let thumbnail = fixture.write(
            &fixture.thumbnail_name("/mnt/offline-share/trip.raw", Some(42), Some(84)),
            b"jpeg bytes",
        );

        let cleanup = fixture.clean();

        assert!(thumbnail.is_file());
        assert_eq!(cleanup.valid, 1);
        assert_eq!(cleanup.removed, 0);
    }

    #[test]
    fn trashed_photos_do_not_hold_their_thumbnails() {
        let fixture = Fixture::new();
        fixture.add_trashed_photo("/photos/deleted.jpg", Some(5), Some(6));
        let thumbnail = fixture.write(
            &fixture.thumbnail_name("/photos/deleted.jpg", Some(5), Some(6)),
            b"jpeg bytes",
        );

        let cleanup = fixture.clean();

        assert!(!thumbnail.exists());
        assert_eq!(cleanup.removed, 1);
    }

    #[test]
    fn unrelated_files_are_ignored() {
        let fixture = Fixture::new();
        fixture.add_photo("/photos/keep.jpg", Some(1), Some(2));
        let thumbnail = fixture.write(
            &fixture.thumbnail_name("/photos/keep.jpg", Some(1), Some(2)),
            b"jpeg bytes",
        );
        let notes = fixture.write("notes.txt", b"keep me");
        let dotfile = fixture.write(".DS_Store", b"keep me");
        let extensionless = fixture.write("orphan", b"keep me");

        fixture.clean();

        for kept in [thumbnail, notes, dotfile, extensionless] {
            assert!(kept.is_file(), "{} was removed", kept.display());
        }
    }

    #[test]
    fn cache_stats_distinguish_cached_referenced_and_unused() {
        let fixture = Fixture::new();
        fixture.add_photo("/photos/a.jpg", Some(1), Some(2));
        fixture.add_photo("/photos/b.jpg", Some(3), Some(4));
        let valid = fixture.valid_paths();
        let referenced = fixture.write(
            &fixture.thumbnail_name("/photos/a.jpg", Some(1), Some(2)),
            b"a",
        );
        let unused_thumbnail = fixture.write("deadbeef.jpg", b"stale");
        let marker = fixture.write(&fixture.failure_marker_name("/photos/b.jpg", Some(3), Some(4)), DECODE_FAILURE_MARKER);
        let notes = fixture.write("notes.txt", b"x");
        let expected_bytes: u64 = [&referenced, &unused_thumbnail, &marker, &notes]
            .into_iter()
            .map(|path| path.metadata().unwrap().len())
            .sum();

        let stats = cache_stats_in(&fixture.directory, &valid).unwrap();

        assert_eq!(stats.cached, 2);
        assert_eq!(stats.referenced, 1);
        assert_eq!(stats.required, 2);
        assert_eq!(stats.unused(), 1);
        assert_eq!(stats.bytes, expected_bytes);
    }
}
