use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use image::RgbaImage;

use super::model::EditRecipe;

pub const EDIT_SUFFIX: &str = "_edit";
const DEFAULT_EXPORT_QUALITY: u8 = 92;

pub fn recipe_is_edited(recipe: &str) -> bool {
    !EditRecipe::decode(recipe).is_default()
}

/// Build the on-disk file name for an export.
///
/// Edited photos get `_edit` before the extension (extension case preserved).
/// An existing `_edit` stem is never doubled. Unedited photos keep their
/// normal name.
pub fn export_file_name(original_name: &str, edited: bool) -> String {
    let path = Path::new(original_name);
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_string);
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("export");
    let base = if edited && !stem.to_ascii_lowercase().ends_with(EDIT_SUFFIX) {
        format!("{stem}{EDIT_SUFFIX}")
    } else {
        stem.to_string()
    };
    match extension {
        Some(extension) => format!("{base}.{extension}"),
        None => base,
    }
}

/// Pick a collision-free file name inside one batch (and against the disk).
///
/// `DSC_1001_edit.jpg` → `DSC_1001_edit_2.jpg` → `DSC_1001_edit_3.jpg`.
pub fn unique_file_name<F>(
    file_name: &str,
    reserved: &mut HashSet<String>,
    mut exists: F,
) -> String
where
    F: FnMut(&str) -> bool,
{
    if !reserved.contains(file_name) && !exists(file_name) {
        reserved.insert(file_name.to_string());
        return file_name.to_string();
    }
    let path = Path::new(file_name);
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_string);
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("export")
        .to_string();
    for counter in 2u32..10_000 {
        let candidate = match &extension {
            Some(extension) => format!("{stem}_{counter}.{extension}"),
            None => format!("{stem}_{counter}"),
        };
        if !reserved.contains(&candidate) && !exists(&candidate) {
            reserved.insert(candidate.clone());
            return candidate;
        }
    }
    let fallback = format!("{stem}-export");
    reserved.insert(fallback.clone());
    fallback
}

pub fn unique_export_path(folder: &Path, file_name: &str, reserved: &mut HashSet<String>) -> PathBuf {
    let candidate = unique_file_name(file_name, reserved, |name| folder.join(name).exists());
    folder.join(candidate)
}

/// Shrink so the long edge is at most `max_edge`, preserving aspect ratio.
/// Never upscales. `max_edge == 0` leaves the image untouched.
pub fn fit_max_edge(image: RgbaImage, max_edge: u32) -> RgbaImage {
    if max_edge == 0 {
        return image;
    }
    let (width, height) = (image.width(), image.height());
    let long_edge = width.max(height);
    if long_edge <= max_edge {
        return image;
    }
    let scale = max_edge as f32 / long_edge as f32;
    let new_width = ((width as f32 * scale).round() as u32).max(1);
    let new_height = ((height as f32 * scale).round() as u32).max(1);
    image::imageops::resize(&image, new_width, new_height, image::imageops::FilterType::Lanczos3)
}

#[derive(Clone, Debug)]
pub struct ExportJob {
    pub source_path: String,
    pub rotation: i32,
    pub edit_recipe: String,
    pub source_width: i64,
    pub source_height: i64,
    pub file_name: String,
}

impl ExportJob {
    pub fn from_record(
        path: String,
        rotation: i32,
        edit_recipe: String,
        source_width: i64,
        source_height: i64,
        original_name: &str,
    ) -> Self {
        let edited = recipe_is_edited(&edit_recipe);
        Self {
            source_path: path,
            rotation,
            edit_recipe,
            source_width,
            source_height,
            file_name: export_file_name(original_name, edited),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BatchExportOutcome {
    pub exported: usize,
    pub skipped: usize,
    pub failed: usize,
    pub errors: Vec<String>,
}

pub fn render_job(job: &ExportJob) -> Result<RgbaImage> {
    super::render::render_for_export(
        &job.source_path,
        job.rotation,
        &job.edit_recipe,
        job.source_width,
        job.source_height,
    )
}

/// Render and save one job to an exact destination path (caller resolves it).
pub fn export_job_to<F>(
    job: &ExportJob,
    destination: &Path,
    max_edge: u32,
    quality: u8,
    render: &F,
) -> Result<()>
where
    F: Fn(&ExportJob) -> Result<RgbaImage>,
{
    let image = fit_max_edge(render(job)?, max_edge);
    super::render::save_jpeg(&image, destination, quality)
}

/// Export every job into `folder`, skipping missing sources, continuing after
/// failures, and uniquifying names so nothing is silently overwritten.
///
/// `on_progress(done, total, file_name, failed)` runs after each item so the
/// UI can show a live `n / total` count. When `cancel` is set, the batch stops
/// before the next job; already-written files stay on disk.
pub fn run_batch_export<F, P>(
    jobs: &[ExportJob],
    folder: &Path,
    max_edge: u32,
    quality: u8,
    render: F,
    mut on_progress: P,
    cancel: Option<&AtomicBool>,
) -> BatchExportOutcome
where
    F: Fn(&ExportJob) -> Result<RgbaImage>,
    P: FnMut(usize, usize, &str, usize),
{
    let total = jobs.len();
    let mut outcome = BatchExportOutcome::default();
    let mut reserved = HashSet::new();
    for (index, job) in jobs.iter().enumerate() {
        if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            let done = index;
            outcome
                .errors
                .push(format!("cancelled after {done} / {total}"));
            on_progress(done, total, "", outcome.failed);
            break;
        }
        let done = index + 1;
        if !Path::new(&job.source_path).is_file() {
            outcome.skipped += 1;
            outcome
                .errors
                .push(format!("skipped (missing): {}", job.source_path));
            on_progress(done, total, &job.file_name, outcome.failed);
            continue;
        }
        let destination = unique_export_path(folder, &job.file_name, &mut reserved);
        match export_job_to(job, &destination, max_edge, quality, &render) {
            Ok(()) => outcome.exported += 1,
            Err(error) => {
                outcome.failed += 1;
                outcome
                    .errors
                    .push(format!("{}: {error:#}", job.source_path));
            }
        }
        on_progress(done, total, &job.file_name, outcome.failed);
    }
    outcome
}

pub fn default_export_quality() -> u8 {
    DEFAULT_EXPORT_QUALITY
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::model::CropRect;

    #[test]
    fn edited_name_gets_edit_suffix() {
        assert_eq!(export_file_name("DSC_1001.jpg", true), "DSC_1001_edit.jpg");
        assert_eq!(export_file_name("IMG_4221.JPG", true), "IMG_4221_edit.JPG");
    }

    #[test]
    fn unedited_name_is_preserved() {
        assert_eq!(export_file_name("DSC_1001.jpg", false), "DSC_1001.jpg");
        assert_eq!(export_file_name("IMG_4221.JPG", false), "IMG_4221.JPG");
    }

    #[test]
    fn edit_suffix_is_never_doubled() {
        assert_eq!(
            export_file_name("DSC_1001_edit.jpg", true),
            "DSC_1001_edit.jpg"
        );
        assert_eq!(
            export_file_name("IMG_4221_edit.JPG", true),
            "IMG_4221_edit.JPG"
        );
        assert_eq!(
            export_file_name("photo_Edit.jpeg", true),
            "photo_Edit.jpeg"
        );
    }

    #[test]
    fn missing_extension_still_gets_suffix() {
        assert_eq!(export_file_name("noextension", true), "noextension_edit");
        assert_eq!(export_file_name("noextension", false), "noextension");
    }

    #[test]
    fn recipe_detection_uses_edit_recipe_defaults() {
        assert!(!recipe_is_edited(""));
        assert!(!recipe_is_edited(&EditRecipe::default().encode()));
        let mut recipe = EditRecipe::default();
        recipe.exposure = 0.5;
        assert!(recipe_is_edited(&recipe.encode()));
        let mut recipe = EditRecipe::default();
        recipe.crop = CropRect {
            left: 0.1,
            top: 0.1,
            right: 0.9,
            bottom: 0.9,
        };
        assert!(recipe_is_edited(&recipe.encode()));
        let mut recipe = EditRecipe::default();
        recipe
            .text_layers
            .push(crate::edit::model::TextLayerSpec::new_default());
        assert!(recipe_is_edited(&recipe.encode()));
        let mut recipe = EditRecipe::default();
        recipe
            .overlays
            .push(crate::edit::model::OverlaySpec::new_centered("asset"));
        assert!(recipe_is_edited(&recipe.encode()));
    }

    #[test]
    fn job_uses_recipe_detection_for_file_name() {
        let job = ExportJob::from_record(
            "/photos/DSC_1001.jpg".into(),
            0,
            String::new(),
            100,
            80,
            "DSC_1001.jpg",
        );
        assert_eq!(job.file_name, "DSC_1001.jpg");
        let mut recipe = EditRecipe::default();
        recipe.contrast = 0.3;
        let job = ExportJob::from_record(
            "/photos/DSC_1001.jpg".into(),
            0,
            recipe.encode(),
            100,
            80,
            "DSC_1001.jpg",
        );
        assert_eq!(job.file_name, "DSC_1001_edit.jpg");
    }

    #[test]
    fn collisions_are_numbered() {
        let mut reserved = HashSet::new();
        let no = |_name: &str| false;
        assert_eq!(
            unique_file_name("DSC_1001_edit.jpg", &mut reserved, no),
            "DSC_1001_edit.jpg"
        );
        assert_eq!(
            unique_file_name("DSC_1001_edit.jpg", &mut reserved, no),
            "DSC_1001_edit_2.jpg"
        );
        assert_eq!(
            unique_file_name("DSC_1001_edit.jpg", &mut reserved, no),
            "DSC_1001_edit_3.jpg"
        );
        let mut reserved = HashSet::new();
        assert_eq!(
            unique_file_name("a.jpg", &mut reserved, |name| name == "a.jpg"),
            "a_2.jpg"
        );
    }

    #[test]
    fn fit_max_edge_preserves_portrait_and_landscape() {
        let portrait = RgbaImage::new(3000, 4000);
        let fitted = fit_max_edge(portrait, 1000);
        assert_eq!((fitted.width(), fitted.height()), (750, 1000));

        let landscape = RgbaImage::new(4000, 3000);
        let fitted = fit_max_edge(landscape, 1000);
        assert_eq!((fitted.width(), fitted.height()), (1000, 750));

        let square = RgbaImage::new(2000, 2000);
        let fitted = fit_max_edge(square, 500);
        assert_eq!((fitted.width(), fitted.height()), (500, 500));
    }

    #[test]
    fn fit_max_edge_never_upscales_or_touches_disabled() {
        let small = RgbaImage::new(100, 50);
        let fitted = fit_max_edge(small, 1000);
        assert_eq!((fitted.width(), fitted.height()), (100, 50));

        let image = RgbaImage::new(640, 480);
        let fitted = fit_max_edge(image, 0);
        assert_eq!((fitted.width(), fitted.height()), (640, 480));
    }

    #[test]
    fn batch_continues_after_failures_and_counts_them() {
        let dir = std::env::temp_dir().join(format!(
            "pic-export-batch-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let jobs: Vec<ExportJob> = (0..5)
            .map(|index| ExportJob {
                source_path: dir.join(format!("src-{index}.jpg")).display().to_string(),
                rotation: 0,
                edit_recipe: String::new(),
                source_width: 4,
                source_height: 4,
                file_name: export_file_name(&format!("photo-{index}.jpg"), index % 2 == 0),
            })
            .collect();
        for index in [0, 1, 3, 4] {
            std::fs::write(dir.join(format!("src-{index}.jpg")), b"x").unwrap();
        }
        let mut progress = Vec::new();
        let outcome = run_batch_export(
            &jobs,
            &dir,
            0,
            92,
            |job| {
                if job.source_path.ends_with("src-3.jpg") {
                    anyhow::bail!("simulated render failure");
                }
                Ok(RgbaImage::new(4, 4))
            },
            |done, total, name, failed| progress.push((done, total, name.to_string(), failed)),
            None,
        );
        assert_eq!(outcome.exported, 3);
        assert_eq!(outcome.skipped, 1);
        assert_eq!(outcome.failed, 1);
        assert_eq!(progress.len(), 5);
        assert_eq!(progress.last().unwrap().0, 5);
        assert!(dir.join("photo-0_edit.jpg").is_file());
        assert!(dir.join("photo-1.jpg").is_file());
        assert!(dir.join("photo-4_edit.jpg").is_file());
        assert!(!dir.join("photo-2_edit.jpg").exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn batch_stops_when_cancel_flag_is_set() {
        let dir = std::env::temp_dir().join(format!(
            "pic-export-cancel-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("src.jpg"), b"x").unwrap();
        let jobs: Vec<ExportJob> = (0..4)
            .map(|index| ExportJob {
                source_path: dir.join("src.jpg").display().to_string(),
                rotation: 0,
                edit_recipe: String::new(),
                source_width: 4,
                source_height: 4,
                file_name: format!("photo-{index}.jpg"),
            })
            .collect();
        let cancel = AtomicBool::new(true);
        let mut progress = Vec::new();
        let outcome = run_batch_export(
            &jobs,
            &dir,
            0,
            92,
            |_| Ok(RgbaImage::new(4, 4)),
            |done, total, _, failed| progress.push((done, total, failed)),
            Some(&cancel),
        );
        assert_eq!(outcome.exported, 0);
        assert_eq!(progress.len(), 1);
        assert_eq!(progress[0], (0, 4, 0));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn batch_output_names_never_collide_on_disk() {
        let dir = std::env::temp_dir().join(format!(
            "pic-export-collide-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("same_edit.jpg"), b"pre-existing").unwrap();
        let jobs: Vec<ExportJob> = (0..2)
            .map(|_| ExportJob {
                source_path: dir.join("src.jpg").display().to_string(),
                rotation: 0,
                edit_recipe: String::new(),
                source_width: 4,
                source_height: 4,
                file_name: "same_edit.jpg".into(),
            })
            .collect();
        std::fs::write(dir.join("src.jpg"), b"x").unwrap();
        let outcome = run_batch_export(
            &jobs,
            &dir,
            0,
            92,
            |_| Ok(RgbaImage::new(4, 4)),
            |_, _, _, _| {},
            None,
        );
        assert_eq!(outcome.exported, 2);
        assert_eq!(
            std::fs::read(dir.join("same_edit.jpg")).unwrap(),
            b"pre-existing"
        );
        assert!(dir.join("same_edit_2.jpg").is_file());
        assert!(dir.join("same_edit_3.jpg").is_file());
        std::fs::remove_dir_all(&dir).ok();
    }
}
