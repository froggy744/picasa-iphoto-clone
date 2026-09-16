#[derive(Clone)]
struct PreviewBase {
    path: String,
    rotation: i32,
    target_width: u32,
    target_height: u32,
    image: image::RgbaImage,
}

impl PreviewBase {
    fn matches(&self, path: &str, rotation: i32, target_width: u32, target_height: u32) -> bool {
        self.path == path
            && self.rotation == rotation
            && self.target_width == target_width
            && self.target_height == target_height
    }
}

const PREVIEW_BASE_CACHE_CAPACITY: usize = 2;

fn cached_preview_base(
    cache: &Arc<Mutex<Vec<PreviewBase>>>,
    path: &str,
    rotation: i32,
    target_width: u32,
    target_height: u32,
) -> Option<image::RgbaImage> {
    cache
        .lock()
        .ok()?
        .iter()
        .find(|entry| entry.matches(path, rotation, target_width, target_height))
        .map(|entry| entry.image.clone())
}

fn store_preview_base(cache: &Arc<Mutex<Vec<PreviewBase>>>, entry: PreviewBase) {
    let Ok(mut cache) = cache.lock() else {
        return;
    };
    cache.retain(|existing| {
        !existing.matches(
            &entry.path,
            entry.rotation,
            entry.target_width,
            entry.target_height,
        )
    });
    cache.push(entry);
    while cache.len() > PREVIEW_BASE_CACHE_CAPACITY {
        cache.remove(0);
    }
}

#[derive(Clone)]
struct PreviewGeometry {
    path: String,
    rotation: i32,
    target_width: u32,
    target_height: u32,
    crop: CropRect,
    straighten: f32,
    image: image::RgbaImage,
}

impl PreviewGeometry {
    fn matches(
        &self,
        path: &str,
        rotation: i32,
        target_width: u32,
        target_height: u32,
        crop: CropRect,
        straighten: f32,
    ) -> bool {
        self.path == path
            && self.rotation == rotation
            && self.target_width == target_width
            && self.target_height == target_height
            && self.crop == crop.normalized()
            && self.straighten == straighten
    }
}

struct PreviewJob {
    generation: u64,
    path: String,
    rotation: i32,
    target_width: u32,
    target_height: u32,
    recipe: EditRecipe,
    // True for rapid-fire frames rendered while a slider drag is in progress.
    // They skip the busy spinner/status churn; the settle render after the
    // drag ends shows them again.
    interactive: bool,
    result_sender: std::sync::mpsc::Sender<anyhow::Result<(u64, u32, u32, Vec<u8>)>>,
}

fn spawn_preview_worker(
    base_cache: Arc<Mutex<Vec<PreviewBase>>>,
) -> std::sync::mpsc::Sender<PreviewJob> {
    let (job_sender, job_receiver) = std::sync::mpsc::channel::<PreviewJob>();
    std::thread::spawn(move || {
        let mut geometry_cache: Option<PreviewGeometry> = None;
        let mut pending: Option<PreviewJob> = None;

        loop {
            let mut job = match pending.take() {
                Some(job) => job,
                None => match job_receiver.recv() {
                    Ok(job) => job,
                    Err(_) => break,
                },
            };

            let mut coalesced = 0usize;
            while let Ok(newer) = job_receiver.try_recv() {
                job = newer;
                coalesced += 1;
            }
            if coalesced > 0 && std::env::var_os("PICASA_TRACE").is_some() {
                eprintln!(
                    "EDIT PREVIEW WORKER coalesced={} generation={}",
                    coalesced, job.generation
                );
            }

            let result = render_preview_job(&job, &base_cache, &mut geometry_cache).map(|image| {
                (
                    job.generation,
                    image.width(),
                    image.height(),
                    image.into_raw(),
                )
            });

            // If newer slider requests arrived while this frame was rendering,
            // keep only the newest one. The just-completed frame is stale, but
            // its decoded/geometry cache work remains useful to the next job.
            let mut newest = None;
            let mut superseded = 0usize;
            while let Ok(next) = job_receiver.try_recv() {
                newest = Some(next);
                superseded += 1;
            }
            if let Some(next) = newest {
                if std::env::var_os("PICASA_TRACE").is_some() {
                    eprintln!(
                        "EDIT PREVIEW WORKER superseded={} finished_generation={} next_generation={}",
                        superseded, job.generation, next.generation
                    );
                }
                pending = Some(next);
                continue;
            }

            let _ = job.result_sender.send(result);
        }
    });
    job_sender
}

fn render_preview_job(
    job: &PreviewJob,
    base_cache: &Arc<Mutex<Vec<PreviewBase>>>,
    geometry_cache: &mut Option<PreviewGeometry>,
) -> anyhow::Result<image::RgbaImage> {
    let total_started = Instant::now();
    let crop = job.recipe.crop.normalized();
    let straighten = job.recipe.straighten;
    let geometry_needed = straighten.abs() >= 0.01 || !crop.is_full();

    let geometry_hit = geometry_needed
        && geometry_cache.as_ref().is_some_and(|entry| {
            entry.matches(
                &job.path,
                job.rotation,
                job.target_width,
                job.target_height,
                crop,
                straighten,
            )
        });

    let mut base_hit = false;
    let mut decode_ms = 0u128;
    let mut geometry_ms = 0u128;

    let geometry = if geometry_hit {
        geometry_cache
            .as_ref()
            .expect("geometry cache matched")
            .image
            .clone()
    } else {
        let base = if let Some(image) = cached_preview_base(
            base_cache,
            &job.path,
            job.rotation,
            job.target_width,
            job.target_height,
        ) {
            base_hit = true;
            image
        } else {
            let decode_started = Instant::now();
            let image = super::render::decode_base_for_viewer(
                &job.path,
                job.rotation,
                job.target_width,
                job.target_height,
            )?;
            decode_ms = decode_started.elapsed().as_millis();
            store_preview_base(
                base_cache,
                PreviewBase {
                    path: job.path.clone(),
                    rotation: job.rotation,
                    target_width: job.target_width,
                    target_height: job.target_height,
                    image: image.clone(),
                },
            );
            image
        };

        if geometry_needed {
            let geometry_started = Instant::now();
            let image = super::render::apply_geometry(base, &job.recipe);
            geometry_ms = geometry_started.elapsed().as_millis();
            geometry_cache.replace(PreviewGeometry {
                path: job.path.clone(),
                rotation: job.rotation,
                target_width: job.target_width,
                target_height: job.target_height,
                crop,
                straighten,
                image: image.clone(),
            });
            image
        } else {
            base
        }
    };

    let tone_started = Instant::now();
    let rendered = super::render::apply_tone(geometry, &job.recipe);
    let tone_ms = tone_started.elapsed().as_millis();

    if std::env::var_os("PICASA_TRACE").is_some() {
        eprintln!(
            "EDIT PREVIEW base_cache={} geometry_cache={} decode_ms={} geometry_ms={} tone_ms={} total_ms={} target={}x{} path={}",
            if geometry_hit {
                "skip"
            } else if base_hit {
                "hit"
            } else {
                "miss"
            },
            if geometry_hit {
                "hit"
            } else if geometry_needed {
                "miss"
            } else {
                "bypass"
            },
            decode_ms,
            geometry_ms,
            tone_ms,
            total_started.elapsed().as_millis(),
            job.target_width,
            job.target_height,
            job.path
        );
    }

    Ok(rendered)
}
