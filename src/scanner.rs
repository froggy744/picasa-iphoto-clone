use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufReader, Cursor};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{Context, Result};
use chrono::{Local, TimeZone};
use exif::{In, Reader as ExifReader, Tag, Value};
use gio::prelude::*;

use crate::db::{self, PhotoMetadata};
use crate::thumbnail;

#[derive(Debug, Clone)]
pub enum ScanEvent {
    Started {
        root: PathBuf,
    },
    FolderStarted {
        folder: db::Folder,
    },
    PhotoIndexed {
        path: PathBuf,
        id: i64,
        photo: db::Photo,
        newly_discovered: bool,
    },
    IndexingFinished {
        imported: usize,
    },
    ThumbnailsStarted {
        total: usize,
    },
    ThumbnailsDeferred {
        total: usize,
    },
    ThumbnailCreated {
        path: PathBuf,
    },
    Failed {
        path: PathBuf,
        error: String,
    },
    Finished {
        imported: usize,
        failed: usize,
    },
    Cancelled {
        imported: usize,
    },
}

/// Cooperative cancellation handle for an import and its thumbnail pass.
#[derive(Clone, Default)]
pub struct ScanControl(Arc<AtomicBool>);

impl ScanControl {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

// Refresh may enqueue several stored folders at once.  Scans share the same
// SQLite database and must therefore run one at a time; otherwise concurrent
// transactions can make all but one scan fail before thumbnails are created.
static SCAN_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

pub fn supported(path: &Path) -> bool {
    crate::image_format::supported(path)
}

pub fn scan(root: &str, events: Option<&Sender<ScanEvent>>) -> Result<usize> {
    scan_with_control(root, events, &ScanControl::default())
}

fn scan_with_control(
    root: &str,
    events: Option<&Sender<ScanEvent>>,
    control: &ScanControl,
) -> Result<usize> {
    let normalized_root = crate::source::normalize_import_reference(root);
    let root = normalized_root.as_str();
    if !root_is_available(root) {
        anyhow::bail!("scan root is unavailable: {root}");
    }
    let connection = db::open_default()?;
    let folder_id = db::insert_folder(&connection, root)?;
    let folder = db::folders(&connection)?
        .into_iter()
        .find(|folder| folder.id == folder_id);
    if let Some(folder) = folder {
        send(events, ScanEvent::FolderStarted { folder });
    }
    let indexed = db::photo_fingerprints(&connection)?;
    let root_file = crate::source::file(root);
    let (files, discovered_folders) = if root.starts_with("smb://") {
        collect_smb_files(root, control)?
    } else if root.starts_with("nfs://") {
        collect_nfs_files(root, control)?
    } else {
        collect_files(&root_file, control)?
    };
    if control.is_cancelled() {
        send(events, ScanEvent::Cancelled { imported: 0 });
        return Ok(0);
    }

    // Only reconcile deletions after the complete tree was enumerated and the
    // root is still available. If a removable drive went offline, discovery
    // fails or this second check fails, so existing library records survive.
    if !root_is_available(root) {
        anyhow::bail!("scan root became unavailable: {root}");
    }
    let present_paths = files
        .iter()
        .map(|(file, _, _)| crate::source::reference(file))
        .collect::<HashSet<_>>();
    let removed = db::remove_missing_photos(&connection, folder_id, &present_paths)?;
    if removed > 0 {}

    let mut imported = 0;
    let mut failed = 0;
    let mut thumbnails = Vec::new();
    let mut indexed_events = Vec::new();
    let mut folder_ids = HashMap::from([(root.to_string(), folder_id)]);
    let mut transaction = Some(connection.unchecked_transaction()?);
    for (path, parent_path) in discovered_folders {
        if path == root {
            continue;
        }
        let parent_id = parent_path
            .as_ref()
            .and_then(|parent| folder_ids.get(parent))
            .copied()
            .unwrap_or(folder_id);
        let id = db::insert_discovered_folder(transaction.as_ref().unwrap(), &path, parent_id)?;

        folder_ids.insert(path, id);
    }
    for (file, info, folder_path) in files {
        if control.is_cancelled() {
            break;
        }
        let path = crate::source::reference(&file);
        let folder_id = if let Some(folder_id) = folder_ids.get(&folder_path) {
            *folder_id
        } else {
            let parent_path = crate::source::file(&folder_path)
                .parent()
                .map(|parent| crate::source::reference(&parent));
            let parent_id = parent_path
                .as_ref()
                .and_then(|parent| folder_ids.get(parent))
                .copied()
                .unwrap_or(folder_id);
            let folder_id = db::insert_discovered_folder(
                transaction.as_ref().unwrap(),
                &folder_path,
                parent_id,
            )?;
            folder_ids.insert(folder_path.clone(), folder_id);
            folder_id
        };
        let fingerprint = (
            info.modification_date_time().map(|time| time.to_unix()),
            Some(info.size()),
        );
        let existing = indexed.get(&path);
        let fingerprint_matches =
            existing.is_some_and(|(mtime, size, _, _)| (*mtime, *size) == fingerprint);
        let missing_raw_dimensions = is_raw(&path)
            && existing.is_some_and(|(_, _, width, height)| {
                width.unwrap_or_default() <= 0 || height.unwrap_or_default() <= 0
            });
        let missing_heif_thumbnail = is_heif(&path)
            && existing.is_some()
            && thumbnail::existing_cache_path(&path, fingerprint.0, fingerprint.1)
                .ok()
                .flatten()
                .is_none();
        let missing_raw_thumbnail = is_raw(&path)
            && existing.is_some()
            && thumbnail::existing_cache_path(&path, fingerprint.0, fingerprint.1)
                .ok()
                .flatten()
                .is_none();
        if fingerprint_matches
            && !missing_raw_dimensions
            && !missing_heif_thumbnail
            && !missing_raw_thumbnail
        {
            db::set_photo_folder(transaction.as_ref().unwrap(), &path, folder_id)?;
            continue;
        }
        let newly_discovered = existing.is_none();
        let result = read_metadata(&path, &info);
        match result {
            Ok(photo_metadata) => {
                let id = db::upsert_photo(
                    transaction.as_ref().unwrap(),
                    Path::new(&path),
                    Some(folder_id),
                    &photo_metadata,
                )?;
                let photo = db::photo(transaction.as_ref().unwrap(), id)?
                    .context("indexed photo disappeared")?;
                imported += 1;
                // A metadata-only repair does not invalidate the thumbnail.
                if !fingerprint_matches || missing_heif_thumbnail {
                    thumbnails.push((
                        path.clone(),
                        photo_metadata.mtime,
                        photo_metadata.size_bytes,
                    ));
                }
                indexed_events.push(ScanEvent::PhotoIndexed {
                    path: PathBuf::from(path),
                    id,
                    photo,
                    newly_discovered,
                });
            }
            Err(error) => {
                failed += 1;
                send(
                    events,
                    ScanEvent::Failed {
                        path: PathBuf::from(path),
                        error: error.to_string(),
                    },
                );
            }
        }
        if imported > 0 && imported % 64 == 0 {
            transaction.take().unwrap().commit()?;
            for event in indexed_events.drain(..) {
                send(events, event);
            }

            transaction = Some(connection.unchecked_transaction()?);
        }
    }
    if let Some(transaction) = transaction {
        transaction.commit()?;
    }
    for event in indexed_events.drain(..) {
        send(events, event);
    }
    if control.is_cancelled() {
        send(events, ScanEvent::Cancelled { imported });
        return Ok(imported);
    }
    send(events, ScanEvent::IndexingFinished { imported });
    send(
        events,
        ScanEvent::ThumbnailsStarted {
            total: thumbnails.len(),
        },
    );
    // Do not hold the import worker open for thumbnail generation. This worker
    // continues independently while indexing and browsing remain available.
    if let Some(sender) = events.cloned() {
        let control = control.clone();
        std::thread::spawn(move || {
            let progress_sender = sender.clone();
            let thumbnail_results = thumbnail::create_many_cancellable(
                &thumbnails,
                || control.is_cancelled(),
                |path| {
                    let _ = progress_sender.send(ScanEvent::ThumbnailCreated {
                        path: PathBuf::from(path),
                    });
                },
            );
            let mut thumbnail_failed = 0;
            for ((path, _mtime, _size_bytes), result) in
                thumbnails.into_iter().zip(thumbnail_results)
            {
                if let Some(Err(error)) = result {
                    thumbnail_failed += 1;
                    let _ = sender.send(ScanEvent::Failed {
                        path: PathBuf::from(path),
                        error: format!("thumbnail: {error}"),
                    });
                }
            }
            if control.is_cancelled() {
                let _ = sender.send(ScanEvent::Cancelled { imported });
            } else {
                let _ = sender.send(ScanEvent::Finished {
                    imported,
                    failed: failed + thumbnail_failed,
                });
            }
        });
    } else {
        send(events, ScanEvent::Finished { imported, failed });
    }
    Ok(imported)
}

fn root_is_available(root: &str) -> bool {
    crate::source::probe_source_available(root)
}

/// Enumerate an SMB tree through the direct transport. Children are wrapped
/// in synthetic gio::FileInfo objects so the downstream metadata pipeline
/// (fingerprints, source::read / materialize) works unchanged.
fn collect_smb_files(
    root_path: &str,
    control: &ScanControl,
) -> Result<(
    Vec<(gio::File, gio::FileInfo, String)>,
    Vec<(String, Option<String>)>,
)> {
    let mut pending = vec![(root_path.trim_end_matches('/').to_string(), None)];
    let mut files = Vec::new();
    let mut folders = Vec::new();
    let mut visited = 0usize;
    while let Some((directory, parent_path)) = pending.pop() {
        if control.is_cancelled() {
            break;
        }
        visited += 1;
        if visited > 20_000 {
            anyhow::bail!("SMB scan runaway: more than 20000 directories under {root_path}");
        }
        let dir_uri = format!("{directory}/");
        folders.push((dir_uri.clone(), parent_path));
        let entries = crate::smb_transport::list_dir(&dir_uri)
            .map_err(|error| anyhow::anyhow!("could not list {dir_uri}: {error}"))?;
        for entry in entries {
            if control.is_cancelled() {
                break;
            }
            let child_uri = format!(
                "{dir_uri}{}",
                crate::smb_transport::percent_encode_segment(&entry.name)
            );
            if entry.is_dir {
                // Same Lightroom-artifact skip as the gvfs walk.
                let name = entry.name.to_ascii_lowercase();
                if !name.ends_with(".lrdata") && name != "previews" && name != "cache" {
                    pending.push((child_uri, Some(dir_uri.clone())));
                }
                continue;
            }
            if !supported(Path::new(&entry.name)) {
                continue;
            }
            // Fingerprint attributes come from a stat (size + mtime).
            let meta =
                crate::smb_transport::stat_in_lane(&child_uri, crate::smb_transport::SmbLane::User)
                    .map_err(anyhow::Error::msg)
                    .with_context(|| format!("could not stat {child_uri}; scan is incomplete"))?;
            let info = gio::FileInfo::new();
            info.set_name(&entry.name);
            info.set_file_type(gio::FileType::Regular);
            info.set_size(meta.size as i64);
            if let Some(mtime) = meta.mtime {
                if let Ok(date_time) = glib::DateTime::from_unix_utc(mtime) {
                    info.set_modification_date_time(&date_time);
                }
            }
            files.push((gio::File::for_uri(&child_uri), info, dir_uri.clone()));
        }
    }
    Ok((files, folders))
}

fn collect_nfs_files(
    root_path: &str,
    control: &ScanControl,
) -> Result<(
    Vec<(gio::File, gio::FileInfo, String)>,
    Vec<(String, Option<String>)>,
)> {
    let mut pending = vec![(root_path.trim_end_matches('/').to_string(), None)];
    let mut files = Vec::new();
    let mut folders = Vec::new();
    let mut visited = 0usize;
    while let Some((directory, parent_path)) = pending.pop() {
        if control.is_cancelled() {
            break;
        }
        visited += 1;
        if visited > 20_000 {
            anyhow::bail!("NFS scan runaway: more than 20000 directories under {root_path}");
        }
        let dir_uri = format!("{directory}/");
        folders.push((dir_uri.clone(), parent_path));
        let entries = crate::nfs_transport::list_dir(&dir_uri)
            .map_err(|error| anyhow::anyhow!("could not list {dir_uri}: {error}"))?;
        for entry in entries {
            if control.is_cancelled() {
                break;
            }
            let child_uri = format!(
                "{dir_uri}{}",
                crate::smb_transport::percent_encode_segment(&entry.name)
            );
            if entry.is_dir {
                let name = entry.name.to_ascii_lowercase();
                if !name.ends_with(".lrdata") && name != "previews" && name != "cache" {
                    pending.push((child_uri, Some(dir_uri.clone())));
                }
                continue;
            }
            if !supported(Path::new(&entry.name)) {
                continue;
            }
            let meta = crate::nfs_transport::stat(&child_uri)
                .map_err(anyhow::Error::msg)
                .with_context(|| format!("could not stat {child_uri}; scan is incomplete"))?;
            let info = gio::FileInfo::new();
            info.set_name(&entry.name);
            info.set_file_type(gio::FileType::Regular);
            info.set_size(meta.size as i64);
            if let Ok(date_time) = glib::DateTime::from_unix_utc(meta.mtime) {
                info.set_modification_date_time(&date_time);
            }
            files.push((gio::File::for_uri(&child_uri), info, dir_uri.clone()));
        }
    }
    Ok((files, folders))
}

pub fn spawn_scan(root: String, events: Sender<ScanEvent>) -> ScanControl {
    let control = ScanControl::default();
    let worker_control = control.clone();
    std::thread::spawn(move || {
        let lock = SCAN_LOCK.get_or_init(|| Mutex::new(()));
        let _guard = lock.lock().expect("scan lock poisoned");
        send(
            Some(&events),
            ScanEvent::Started {
                root: PathBuf::from(&root),
            },
        );
        if let Err(error) = scan_with_control(&root, Some(&events), &worker_control) {
            send(
                Some(&events),
                ScanEvent::Failed {
                    path: PathBuf::from(root),
                    error: error.to_string(),
                },
            );
        }
    });
    control
}

fn send(events: Option<&Sender<ScanEvent>>, event: ScanEvent) {
    if let Some(events) = events {
        let _ = events.send(event);
    }
}

fn collect_files(
    root: &gio::File,
    control: &ScanControl,
) -> Result<(
    Vec<(gio::File, gio::FileInfo, String)>,
    Vec<(String, Option<String>)>,
)> {
    let root_path = crate::source::reference(root);
    let mut pending = vec![(root.clone(), root_path.clone(), None)];
    let mut files = Vec::new();
    let mut folders = Vec::new();
    while let Some((directory, folder_path, parent_path)) = pending.pop() {
        if control.is_cancelled() {
            break;
        }

        folders.push((folder_path.clone(), parent_path));
        let enumerator = directory
            .enumerate_children(
                "standard::name,standard::type,time::modified,standard::size",
                gio::FileQueryInfoFlags::NONE,
                gio::Cancellable::NONE,
            )
            .with_context(|| format!("could not list {}", directory.uri()))?;
        while let Some(info) = enumerator.next_file(gio::Cancellable::NONE)? {
            if control.is_cancelled() {
                break;
            }
            let child = enumerator.child(&info);
            match info.file_type() {
                gio::FileType::Directory => {
                    // Lightroom stores thousands of Smart Preview DNGs in
                    // *.lrdata folders. They are cache artifacts, not user
                    // photos; indexing them makes scrolling trigger a slow
                    // raw decode (several seconds per item).
                    let name = info.name().to_string_lossy().to_ascii_lowercase();
                    if !name.ends_with(".lrdata") && name != "previews" && name != "cache" {
                        pending.push((
                            child.clone(),
                            crate::source::reference(&child),
                            Some(folder_path.clone()),
                        ));
                    }
                }
                gio::FileType::Regular if supported(Path::new(&info.name())) => {
                    files.push((child, info, folder_path.clone()))
                }
                _ => {}
            }
        }
    }
    Ok((files, folders))
}

fn read_metadata(path: &str, attributes: &gio::FileInfo) -> Result<PhotoMetadata> {
    let mtime = attributes
        .modification_date_time()
        .map(|time| time.to_unix());
    let (width, height, exif) = if is_raw(path) {
        // Prefer the cheap EXIF dimensions, then ask the RAW decoder for its
        // metadata-only image geometry. PixelX/YDimension are missing from
        // many DNG and NEF containers.
        let local = crate::source::materialize(path)?;
        let exif = fs::File::open(local).ok().and_then(|file| {
            ExifReader::new()
                .read_from_container(&mut BufReader::new(file))
                .ok()
        });
        let exif_width = exif
            .as_ref()
            .and_then(|data| exif_u32(data, Tag::PixelXDimension));
        let exif_height = exif
            .as_ref()
            .and_then(|data| exif_u32(data, Tag::PixelYDimension));
        let raw_dimensions = if exif_width.is_none() || exif_height.is_none() {
            crate::thumbnail::dimensions(path, &[]).ok()
        } else {
            None
        };
        let width = exif_width.or_else(|| raw_dimensions.map(|(width, _)| width));
        let height = exif_height.or_else(|| raw_dimensions.map(|(_, height)| height));
        (width, height, exif)
    } else {
        let bytes = crate::source::read(path)?;
        // image-rs does not decode every HEIF variant. Keep the record when
        // dimensions are unavailable; thumbnail generation will report a
        // per-file failure without aborting the rest of the scan.
        let dimensions = crate::thumbnail::dimensions(path, &bytes).ok();
        let exif = ExifReader::new()
            .read_from_container(&mut Cursor::new(&bytes))
            .ok();
        (
            dimensions.map(|(width, _)| width),
            dimensions.map(|(_, height)| height),
            exif,
        )
    };
    let taken_at = exif.as_ref().and_then(exif_date).or_else(|| {
        mtime.and_then(|seconds| {
            Local
                .timestamp_opt(seconds, 0)
                .single()
                .map(|date| date.to_rfc3339())
        })
    });
    let camera = exif.as_ref().and_then(|data| {
        let make = data.get_field(Tag::Make, In::PRIMARY).and_then(field_text);
        let model = data.get_field(Tag::Model, In::PRIMARY).and_then(field_text);
        match (make, model) {
            (Some(make), Some(model)) if model.starts_with(&make) => Some(model),
            (Some(make), Some(model)) => Some(format!("{make} {model}")),
            (Some(make), None) => Some(make),
            (None, Some(model)) => Some(model),
            _ => None,
        }
    });
    Ok(PhotoMetadata {
        taken_at,
        camera,
        width: width.map(i64::from),
        height: height.map(i64::from),
        size_bytes: Some(attributes.size()),
        mtime,
    })
}

fn exif_date(exif: &exif::Exif) -> Option<String> {
    exif.get_field(Tag::DateTimeOriginal, In::PRIMARY)
        .and_then(field_text)
        .or_else(|| {
            exif.get_field(Tag::DateTime, In::PRIMARY)
                .and_then(field_text)
        })
        .map(|date| date.replace(':', "-").replacen('-', "-", 2))
}

fn field_text(field: &exif::Field) -> Option<String> {
    match &field.value {
        Value::Ascii(values) => values
            .first()
            .map(|value| String::from_utf8_lossy(value).trim().to_string())
            .filter(|value| !value.is_empty()),
        _ => Some(field.display_value().to_string()),
    }
}

fn exif_u32(exif: &exif::Exif, tag: Tag) -> Option<u32> {
    exif.get_field(tag, In::PRIMARY)
        .and_then(|field| match &field.value {
            Value::Long(values) => values.first().copied(),
            Value::Short(values) => values.first().copied().map(u32::from),
            _ => None,
        })
}

fn is_raw(path: &str) -> bool {
    crate::image_format::uses(path, crate::image_format::DecoderKind::Raw)
}

fn is_heif(path: &str) -> bool {
    crate::image_format::uses(path, crate::image_format::DecoderKind::Heif)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn recursive_collection_reaches_nested_photo_below_imported_root() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "picasa-rs-scanner-recursion-{}-{unique}",
            std::process::id()
        ));
        let nested = root.join("Marianne Lotter").join("FB-Marianne");
        fs::create_dir_all(&nested).unwrap();
        let photo = nested.join("photo.jpg");
        fs::write(&photo, []).unwrap();

        let (files, folders) =
            collect_files(&gio::File::for_path(&root), &ScanControl::default()).unwrap();
        assert!(files
            .iter()
            .any(|(_, _, folder)| folder.ends_with("FB-Marianne")));
        assert!(folders
            .iter()
            .any(|(folder, _)| folder.ends_with("FB-Marianne")));

        let _ = fs::remove_dir_all(&root);
    }
}
