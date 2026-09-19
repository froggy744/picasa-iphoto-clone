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
    /// `.jpg` thumbnail files present in the cache shards.
    pub cached: u64,
    /// Present `.jpg` files whose cache key matches a current library photo.
    pub referenced: u64,
    /// Cache keys expected from the current (non-trashed) library photos.
    pub required: u64,
    /// Bytes used by the cached thumbnails, markers and other files.
    pub bytes: u64,
    /// Bytes used by materialized remote sources (`../source`).
    pub source_bytes: u64,
}

impl CacheStats {
    /// Cached `.jpg` files that no current photo references.
    pub fn unused(&self) -> u64 {
        self.cached.saturating_sub(self.referenced)
    }
}

/// The cache file names every current (non-trashed) library photo expects.
/// Validity is keyed on the file NAME, not its full path: cache keys are
/// globally unique, so a thumbnail remains valid no matter which shard layout
/// generation (or legacy flat directory) it currently sits in.
pub fn valid_cache_names(connection: &Connection) -> Result<HashSet<String>> {
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
            Ok(cache_file_name(&path, mtime, size_bytes))
        })
        .collect::<rusqlite::Result<HashSet<_>>>()?)
}

/// The materialized remote source file names the current library still needs,
/// keyed exactly like `source::materialize` derives them: blake3 over the
/// normalized reference, with the reference's extension. Files whose names do
/// not decode to that key shape (foreign files) are never pruned.
pub fn valid_source_names(connection: &Connection) -> Result<HashSet<String>> {
    let mut statement = connection.prepare("SELECT path FROM photos WHERE trashed = 0")?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
    let mut names = HashSet::new();
    for row in rows {
        let reference = crate::smb_transport::normalize_smb_reference(&row?);
        let mut hasher = blake3::Hasher::new();
        hasher.update(reference.as_bytes());
        let stem = hasher.finalize().to_hex();
        let extension = Path::new(&reference)
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("raw");
        names.insert(format!("{stem}.{extension}"));
    }
    Ok(names)
}

/// Delete thumbnails the current library no longer references.
///
/// Both the legacy flat directory and the cache shards are swept; a `.jpg`
/// file is removed only when its cache-key name matches no current photo, so
/// valid thumbnails are safe in any layout. Stale `.failed` markers go with
/// their keys; a marker whose key is still current stays so known-bad sources
/// keep being suppressed. Empty shard directories are pruned afterwards.
/// Unrelated files and the originals are never touched.
pub fn cleanup_cache(valid: &HashSet<String>) -> Result<CacheCleanup> {
    // A thumbnail being generated right now may not be part of the key
    // snapshot yet. Both registries cover every `create()` destination that
    // can be mid-write, so reserve them instead of racing the workers.
    let reserved: HashSet<String> = currently_generating_cache_paths()
        .iter()
        .filter_map(|path| path.file_name().and_then(|name| name.to_str()))
        .map(str::to_owned)
        .collect();
    cleanup_cache_in(&cache_dir()?, valid, &reserved)
}

fn cleanup_cache_in(
    directory: &Path,
    valid: &HashSet<String>,
    reserved: &HashSet<String>,
) -> Result<CacheCleanup> {
    let mut cleanup = CacheCleanup::default();
    // Legacy flat-layout files first: any pre-sharding thumbnail still sits
    // directly in the cache root.
    sweep_cache_files(directory, valid, reserved, &mut cleanup)?;
    let files_root = directory.join("files");
    for shard in read_dir_entries(&files_root)? {
        if !shard.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
            continue;
        }
        let shard_path = shard.path();
        sweep_cache_files(&shard_path, valid, reserved, &mut cleanup)?;
        // Drop shards this pass emptied so the bucket count reflects use.
        let _ = fs::remove_dir(&shard_path);
    }
    Ok(cleanup)
}

/// One directory of candidate thumbnail/marker files, shard or flat.
fn sweep_cache_files(
    directory: &Path,
    valid: &HashSet<String>,
    reserved: &HashSet<String>,
    cleanup: &mut CacheCleanup,
) -> Result<()> {
    for entry in read_dir_entries(directory)? {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        // Only regular files are candidates; nested directories that merely
        // carry a thumbnail-looking name are left alone.
        if !file_type.is_file() {
            continue;
        }
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name.ends_with(".jpg") {
            if valid.contains(name) || reserved.contains(name) {
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
            let thumbnail = Path::new(name)
                .with_extension("jpg")
                .to_string_lossy()
                .into_owned();
            if !valid.contains(thumbnail.as_str()) && !reserved.contains(thumbnail.as_str()) {
                let bytes = entry_len(&entry);
                if fs::remove_file(&path).is_ok() {
                    cleanup.bytes_freed += bytes;
                }
            }
        }
    }
    Ok(())
}

/// Measure the cache directory against the current photo cache keys.
pub fn cache_stats(valid: &HashSet<String>) -> Result<CacheStats> {
    cache_stats_in(&cache_dir()?, valid)
}

fn cache_stats_in(directory: &Path, valid: &HashSet<String>) -> Result<CacheStats> {
    let mut stats = CacheStats {
        required: valid.len() as u64,
        ..CacheStats::default()
    };
    let mut measure = |subdirectory: &Path| -> Result<()> {
        for entry in read_dir_entries(subdirectory)? {
            if !entry.file_type().map(|kind| kind.is_file()).unwrap_or(false) {
                continue;
            }
            stats.bytes += entry_len(&entry);
            let is_jpg = entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.ends_with(".jpg"));
            if is_jpg {
                stats.cached += 1;
                if entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| valid.contains(name))
                {
                    stats.referenced += 1;
                }
            }
        }
        Ok(())
    };
    // Legacy flat-layout files migrate on first access; anything left there
    // still counts so the numbers do not silently shrink mid-migration.
    measure(directory)?;
    let files_root = directory.join("files");
    for shard in read_dir_entries(&files_root)? {
        if shard.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
            measure(&shard.path())?;
        }
    }
    if let Ok(source_bytes) = source_dir_bytes(directory) {
        stats.source_bytes = source_bytes;
    }
    Ok(stats)
}

/// Bytes used by the materialized remote sources (`../source`), reported
/// separately because they dwarf the thumbnails themselves.
fn source_dir_bytes(directory: &Path) -> Result<u64> {
    let sources_root = directory
        .parent()
        .map(|parent| parent.join("source"))
        .unwrap_or_else(|| directory.join("source"));
    let mut total = 0;
    let mut measure = |subdirectory: &Path| -> Result<()> {
        for entry in read_dir_entries(subdirectory)? {
            if entry
                .file_type()
                .map(|kind| kind.is_file())
                .unwrap_or(false)
            {
                total += entry_len(&entry);
            }
        }
        Ok(())
    };
    measure(&sources_root)?;
    for shard in read_dir_entries(&sources_root)? {
        if shard.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
            measure(&shard.path())?;
        }
    }
    Ok(total)
}

/// Delete materialized remote sources the current library no longer
/// references. Aggressive by design: a source can be re-materialized from the
/// original share at any time, and these multi-megabyte files dominate the
/// cache. Foreign/unknown files are never touched. Validity is keyed on the
/// file name, so a referenced source survives in any layout.
pub fn cleanup_sources(valid: &HashSet<String>) -> Result<CacheCleanup> {
    cleanup_source_cache_in(&sources_dir()?, valid)
}

fn cleanup_source_cache_in(sources_root: &Path, valid: &HashSet<String>) -> Result<CacheCleanup> {
    let mut cleanup = CacheCleanup::default();
    // Legacy flat-layout files (pre-sharding): a referenced one is kept for
    // lazy migration on next use, an unreferenced one is garbage.
    sweep_source_files(sources_root, valid, &mut cleanup)?;
    for shard in read_dir_entries(sources_root)? {
        if !shard.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
            continue;
        }
        let shard_path = shard.path();
        sweep_source_files(&shard_path, valid, &mut cleanup)?;
        let _ = fs::remove_dir(&shard_path);
    }
    Ok(cleanup)
}

/// One directory of candidate source files, shard or flat.
fn sweep_source_files(
    directory: &Path,
    valid: &HashSet<String>,
    cleanup: &mut CacheCleanup,
) -> Result<()> {
    for entry in read_dir_entries(directory)? {
        if !entry
            .file_type()
            .map(|kind| kind.is_file())
            .unwrap_or(false)
        {
            continue;
        }
        let path = entry.path();
        // Only exact cache-key shapes are prunable; unknown names stay.
        let Some(key) = source_key(entry.file_name().to_str()) else {
            continue;
        };
        if valid.contains(key.as_str()) {
            continue;
        }
        let bytes = entry_len(&entry);
        if fs::remove_file(&path).is_ok() {
            cleanup.removed += 1;
            cleanup.bytes_freed += bytes;
        }
    }
    Ok(())
}

/// Reconstruct the deterministic part of a source cache file name: 64 hex
/// characters plus an extension. Anything else (temp files, foreign data) is
/// not a prunable key.
fn source_key(file_name: Option<&str>) -> Option<String> {
    let file_name = file_name?;
    let (stem, extension) = file_name.split_once('.')?;
    if stem.len() != 64 || !stem.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    if extension.is_empty() || extension.contains('/') {
        return None;
    }
    Some(file_name.to_string())
}

fn read_dir_entries(directory: &Path) -> Result<Vec<fs::DirEntry>> {
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

        fn thumbnail_name(
            &self,
            path: &str,
            mtime: Option<i64>,
            size_bytes: Option<i64>,
        ) -> String {
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
            let path = shard_dir_in(&self.directory.join("files"), name).join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, contents).unwrap();
            path
        }

        /// Write a file in the legacy flat layout (pre-sharding location).
        fn write_legacy(&self, name: &str, contents: &[u8]) -> PathBuf {
            let path = self.directory.join(name);
            fs::write(&path, contents).unwrap();
            path
        }

        fn valid_names(&self) -> HashSet<String> {
            valid_cache_names(&self.connection).unwrap()
        }

        fn clean(&self) -> CacheCleanup {
            cleanup_cache_in(&self.directory, &self.valid_names(), &HashSet::new()).unwrap()
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
        assert_eq!(
            cleanup,
            CacheCleanup {
                valid: 1,
                removed: 0,
                bytes_freed: 0
            }
        );
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
        let marker = fixture.write(
            &fixture.failure_marker_name("/photos/retagged.png", Some(6), Some(9)),
            DECODE_FAILURE_MARKER,
        );
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
        let marker = fixture.write(
            &fixture.failure_marker_name("/photos/broken.png", Some(11), Some(22)),
            DECODE_FAILURE_MARKER,
        );

        fixture.clean();

        assert!(marker.is_file());
    }

    #[test]
    fn directories_outside_the_shards_are_untouched() {
        let fixture = Fixture::new();
        // Legacy-layout leftovers and unrelated top-level directories are
        // never part of the sharded cleanup sweep.
        let legacy_sources = fixture.directory.join("source");
        fs::create_dir_all(&legacy_sources).unwrap();
        let raw = legacy_sources.join("abcdef.raw");
        fs::write(&raw, b"raw bytes").unwrap();
        // A directory that merely looks like a thumbnail is also left alone.
        let directory_named_jpg = fixture.directory.join("feedface.jpg");
        fs::create_dir_all(&directory_named_jpg).unwrap();

        fixture.clean();

        assert!(raw.is_file());
        assert!(directory_named_jpg.is_dir());
    }

    #[test]
    fn legacy_flat_thumbnail_migrates_into_its_shard() {
        let fixture = Fixture::new();
        fixture.add_photo("/photos/old.jpg", Some(10), Some(20));
        let name = fixture.thumbnail_name("/photos/old.jpg", Some(10), Some(20));
        let legacy = fixture.write_legacy(&name, b"jpeg bytes");
        let marker = fixture.write_legacy(
            Path::new(&name)
                .with_extension("failed")
                .to_string_lossy()
                .as_ref(),
            DECODE_FAILURE_MARKER,
        );

        let resolved =
            resolve_cache_path_in(&fixture.directory, "/photos/old.jpg", Some(10), Some(20))
                .unwrap();

        assert!(resolved.is_file());
        assert!(resolved.starts_with(fixture.directory.join("files")));
        assert_eq!(resolved.file_name().unwrap(), name.as_str());
        assert!(!legacy.exists(), "legacy copy must move, not duplicate");
        assert!(!marker.exists(), "stale marker goes with the migration");
    }

    #[test]
    fn cleanup_prunes_shard_directories_it_emptied() {
        let fixture = Fixture::new();
        fixture.add_photo("/photos/keep.jpg", Some(1), Some(2));
        // Only a stale thumbnail exists; its shard must vanish with it.
        fixture.write(
            &fixture.thumbnail_name("/photos/gone.jpg", Some(3), Some(4)),
            b"stale",
        );

        let files_root = fixture.directory.join("files");
        let before = fs::read_dir(&files_root).unwrap().count();
        assert!(before > 0);
        fixture.clean();
        let after = fs::read_dir(&files_root).unwrap().count();

        assert_eq!(after, 0, "empty shards must be removed");
    }

    #[test]
    fn source_cache_keeps_referenced_and_prunes_stale() {
        let fixture = Fixture::new();
        fixture.add_photo("smb://host/share/keep.NEF", Some(1), Some(2));
        fixture.add_photo("smb://host/share/stale.NEF", Some(3), Some(4));
        // Simulate a share that no longer holds 'stale' (removed from library
        // paths but its source file lingers): drop it from the DB instead.
        fixture
            .connection
            .execute(
                "DELETE FROM photos WHERE path = 'smb://host/share/stale.NEF'",
                [],
            )
            .unwrap();

        let sources_root = fixture.directory.join("source");
        let valid = valid_source_names(&fixture.connection).unwrap();
        assert_eq!(valid.len(), 1);
        let referenced_name = valid.iter().next().unwrap().clone();
        let stale_stem = {
            let mut hasher = blake3::Hasher::new();
            hasher.update("smb://host/share/stale.NEF".as_bytes());
            hasher.finalize().to_hex()
        };
        let stale_name = format!("{stale_stem}.NEF");
        let foreign_name = "172e0c6c-not-a-key.txt";

        for name in [referenced_name.as_str(), stale_name.as_str(), foreign_name] {
            let path = shard_dir_in(&sources_root, name).join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, b"source bytes").unwrap();
        }

        let cleanup = cleanup_source_cache_in(&sources_root, &valid).unwrap();

        let referenced = shard_dir_in(&sources_root, &referenced_name).join(&referenced_name);
        assert!(referenced.is_file(), "referenced source must survive");
        assert_eq!(cleanup.removed, 1, "only the stale source is pruned");
        let stale_path = shard_dir_in(&sources_root, &stale_name).join(&stale_name);
        assert!(!stale_path.exists());
        let foreign_path = shard_dir_in(&sources_root, foreign_name).join(foreign_name);
        assert!(foreign_path.exists(), "foreign files are never pruned");
        assert!(cleanup.bytes_freed > 0);
    }

    #[test]
    fn shard_folders_are_single_alphanumeric_characters() {
        // Deterministic mapping into the 0-9 / a-z alphabet.
        let name = "3f9a1c2d00000000000000000000000000000000000000000000000000000000.jpg";
        let folder = cache_shard_char(name).unwrap();
        assert!(folder.is_ascii_alphanumeric());
        assert_eq!(folder, cache_shard_char(name).unwrap());

        // Every bucket the hash space can produce is a valid folder name, and
        // a synthetic spread of keys covers essentially all of them.
        let mut buckets = HashSet::new();
        for index in 0..2000u64 {
            // Vary the leading four hex chars (the sampled prefix) across the
            // whole u16 range, with a scattered tail for realism.
            let name = format!(
                "{:04x}{:060x}.jpg",
                index & 0xffff,
                index.wrapping_mul(0x9e37_79b9_7f4a_7c15)
            );
            let folder = cache_shard_char(&name).expect("hex key yields a folder");
            assert!(folder.is_ascii_alphanumeric(), "{folder} not 0-9a-z");
            buckets.insert(folder);
        }
        assert_eq!(buckets.len(), 36, "spread covered only {buckets:?}");

        // Foreign names have no hex prefix and land in 'misc'.
        assert_eq!(cache_shard_char("notes.txt"), None);
    }

    #[test]
    fn shard_folders_are_derived_from_the_key_not_the_directory() {
        let fixture = Fixture::new();
        let name = fixture.thumbnail_name("/photos/a.jpg", Some(1), Some(2));
        let shard = shard_dir_in(&fixture.directory.join("files"), &name);
        let shard_name = shard.file_name().and_then(|name| name.to_str()).unwrap();
        assert_eq!(shard_name.len(), 1);
        assert!(shard_name.chars().next().unwrap().is_ascii_alphanumeric());
        // A single-character shard is also distinct from a hash prefix: 'misc'
        // stays reserved for foreign names.
        assert_ne!(shard_name, "misc");
    }

    #[test]
    fn relocated_aux_roots_leave_the_thumbnail_directory() {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "picasa-aux-relocate-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let thumbs = root.join("thumbs");
        let source = thumbs.join("source");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("abc.raw"), b"raw").unwrap();

        relocate_legacy_aux_root(&thumbs, "source");

        assert!(root.join("source").join("abc.raw").is_file());
        assert!(!source.exists(), "legacy aux directory must be gone");
        let _ = fs::remove_dir_all(&root);
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
        let valid = fixture.valid_names();
        let referenced = fixture.write(
            &fixture.thumbnail_name("/photos/a.jpg", Some(1), Some(2)),
            b"a",
        );
        let unused_thumbnail = fixture.write("deadbeef.jpg", b"stale");
        let marker = fixture.write(
            &fixture.failure_marker_name("/photos/b.jpg", Some(3), Some(4)),
            DECODE_FAILURE_MARKER,
        );
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
