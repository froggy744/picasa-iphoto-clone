fn notify_photo_changed(handler: &PhotoChangedHandler, photos: &[PhotoObject], index: usize) {
    let Some(photo) = photos.get(index) else {
        return;
    };
    if let Some(handler) = handler.borrow().as_ref() {
        handler(photo.clone());
    }
}

fn show_photo(
    picture: &gtk::Picture,
    photos: &[PhotoObject],
    index: usize,
    root: &gtk::Overlay,
    zoom: Rc<Cell<f64>>,
    generation: Rc<Cell<u64>>,
    expected_generation: u64,
    decode_cancel: Rc<RefCell<Option<Arc<ViewerRequestLease>>>>,
    picture_viewport: &gtk::ScrolledWindow,
    native_texture: Rc<RefCell<Option<NativeTextureCache>>>,
    display_texture_cache: DisplayTextureCache,
    fit_geometry_fixed: bool,
    cache_hit: bool,
    navigation_ready: Option<Rc<Cell<bool>>>,
    navigation_settled: Option<Rc<dyn Fn()>>,
) {
    let navigation_started = std::time::Instant::now();
    let Some(photo) = photos.get(index) else {
        return;
    };

    // Decode a display-quality image off the GTK thread. Never display a
    // thumbnail in the lightbox; keep the previous full-size image during
    // navigation and show a neutral backdrop on initial open.
    let path = photo.path();
    viewer_trace(format!(
        "show_photo_request generation={} index={} uri={}",
        expected_generation,
        index,
        viewer_trace_uri(&path),
    ));
    if let Some(previous) = decode_cancel.borrow_mut().take() {
        previous.cancel();
    }
    if cache_hit {
        viewer_trace(format!(
            "cache_hit lane=foreground uri={}",
            viewer_trace_uri(&path)
        ));
        viewer_trace(format!(
            "display_done lane=foreground source=texture_cache navigation_ms={} uri={}",
            navigation_started.elapsed().as_millis(),
            viewer_trace_uri(&path)
        ));
        cancel_lightbox_prefetch_except(None);
        VIEWER_FOREGROUND_GENERATION.store(0, Ordering::Release);
        if let Some(navigation_ready) = navigation_ready {
            navigation_ready.set(true);
        }
        if let Some(navigation_settled) = navigation_settled {
            navigation_settled();
        }
        return;
    }
    let rotation = photo.rotation();
    let edit_recipe_text = photo.edit_recipe();
    let (target_width, target_height, _, _, _, _) =
        viewer_decode_target(root, rotation, zoom.get() < 0.0);
    let key = ViewerRequestKey {
        path: path.clone(),
        mtime: photo.mtime(),
        size_bytes: photo.size_bytes(),
        rotation,
        edit_recipe: edit_recipe_text.clone(),
        target_width,
        target_height,
    };
    // Keep a matching prefetch alive: the foreground lease below promotes it.
    // Other speculative requests lose their leases before they can occupy a
    // decode slot ahead of the selected image.
    cancel_lightbox_prefetch_except(Some(&key));
    VIEWER_FOREGROUND_GENERATION.store(expected_generation, Ordering::Release);
    let (request, lease, claim) = claim_viewer_request(&key, true);
    viewer_trace(format!(
        "request lane=foreground action={} uri={} variant={}x{}",
        match claim {
            ViewerRequestClaim::New => "new",
            ViewerRequestClaim::JoinedForeground => "join",
            ViewerRequestClaim::PromotedPrefetch => "promote",
        },
        viewer_trace_uri(&path),
        target_width,
        target_height,
    ));
    if matches!(claim, ViewerRequestClaim::New) {
        start_viewer_request(key, request.clone());
    }
    *decode_cancel.borrow_mut() = Some(lease.clone());

    let picture = picture.clone();
    let root = root.clone();
    let picture_viewport = picture_viewport.clone();
    let cache_path = path.clone();
    let photo = photo.clone();
    let display_texture_cache_for_result = display_texture_cache.clone();
    let navigation_ready_for_result = navigation_ready.clone();
    let navigation_settled_for_result = navigation_settled.clone();
    glib::MainContext::default().spawn_local(async move {
        let result = ViewerResultSlot::wait(request.result.clone()).await;

        let stale =
            !viewer_generation_current(generation.get(), expected_generation) || lease.cancelled();
        if stale {
            viewer_trace(format!(
                "display_discard lane=foreground uri={} reason=stale",
                viewer_trace_uri(&cache_path),
            ));
            lease.release();
            let _ = VIEWER_FOREGROUND_GENERATION.compare_exchange(
                expected_generation,
                0,
                Ordering::AcqRel,
                Ordering::Acquire,
            );
            if viewer_generation_current(generation.get(), expected_generation) {
                if let Some(navigation_ready) = navigation_ready_for_result {
                    navigation_ready.set(true);
                }
                if let Some(navigation_settled) = navigation_settled_for_result {
                    navigation_settled();
                }
            }
            return;
        }
        let _ = VIEWER_FOREGROUND_GENERATION.compare_exchange(
            expected_generation,
            0,
            Ordering::AcqRel,
            Ordering::Acquire,
        );

        match result {
            Ok(result) => {
                let display_started = std::time::Instant::now();
                let bytes = glib::Bytes::from_owned(result.pixels.clone());
                let texture = gtk::gdk::MemoryTexture::new(
                    result.width as i32,
                    result.height as i32,
                    gtk::gdk::MemoryFormat::R8g8b8a8,
                    &bytes,
                    result.width as usize * 4,
                );

                // Never overwrite PhotoObject's source dimensions with the
                // dimensions of a display-sized viewer decode. The original
                // dimensions are what 1:1 mode needs to request native pixels.
                if zoom.get() < 0.0 {
                    *native_texture.borrow_mut() = Some(NativeTextureCache {
                        path: cache_path.clone(),
                        rotation,
                        edit_recipe: edit_recipe_text.clone(),
                        texture: texture.clone(),
                    });
                }

                picture.set_paintable(Some(&texture));
                viewer_trace(format!(
                    "display_apply elapsed_ms={} uri={}",
                    display_started.elapsed().as_millis(),
                    viewer_trace_uri(&cache_path),
                ));
                viewer_trace(format!(
                    "display_done lane=foreground source=decode navigation_ms={} uri={}",
                    navigation_started.elapsed().as_millis(),
                    viewer_trace_uri(&cache_path),
                ));
                if zoom.get() >= 0.0 {
                    if !fit_geometry_fixed {
                        fit_picture(
                            &picture,
                            std::slice::from_ref(&photo),
                            0,
                            root.width(),
                            root.height(),
                            zoom.get(),
                        );
                    }
                    display_texture_cache_insert(
                        &display_texture_cache_for_result,
                        cache_path.clone(),
                        rotation,
                        edit_recipe_text.clone(),
                        target_width,
                        target_height,
                        texture.clone(),
                    );
                }
                if zoom.get() < 0.0 {
                    picture.queue_resize();
                    picture_viewport.queue_resize();
                    center_viewport_soon(&picture_viewport);
                }
            }
            Err(_) => {
                // A failed decode must not leave the previous photo visible.
                // This is especially important when navigating from a valid
                // image to a corrupt source: retaining the old paintable makes
                // the viewer appear to open the wrong photo.
                picture.set_paintable(gtk::gdk::Paintable::NONE);
                picture.set_filename(Option::<&str>::None);
                picture.set_size_request(1, 1);
            }
        }
        if let Some(navigation_ready) = navigation_ready_for_result {
            navigation_ready.set(true);
        }
        if let Some(navigation_settled) = navigation_settled_for_result {
            navigation_settled();
        }
        lease.release();
    });
}

fn viewer_trace(message: impl std::fmt::Display) {
    if std::env::var_os("PICASA_TRACE").is_some() {
        static TRACE_STARTED: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
        let elapsed = TRACE_STARTED.get_or_init(std::time::Instant::now).elapsed();
        eprintln!("PIC_VIEWER t_ms={} tid={:?} {message}", elapsed.as_millis(), std::thread::current().id());
    }
}

fn viewer_trace_uri(uri: &str) -> String {
    let Some((scheme, rest)) = uri.split_once("://") else {
        return uri.to_owned();
    };
    // Stored share URIs normally contain no authority credentials, but trace
    // output must remain safe if a URI was supplied with userinfo.
    let safe_rest = rest
        .rsplit_once('@')
        .map(|(_, value)| value)
        .unwrap_or(rest);
    format!("{scheme}://{safe_rest}")
}

fn viewer_generation_current(current: u64, expected: u64) -> bool {
    current == expected
}

fn start_viewer_request(key: ViewerRequestKey, request: Arc<ViewerRequest>) {
    let request_for_worker = request.clone();
    let worker_key = key.clone();
    if std::thread::Builder::new()
        .name("lightbox-decode".to_string())
        .spawn(move || {
            let queued = std::time::Instant::now();
            let gate = VIEWER_DECODE_GATE
                .get_or_init(|| DecodeSemaphore::new(MAX_CONCURRENT_VIEWER_DECODES));
            let Some(_permit) = gate.acquire_while(|| request_for_worker.has_consumers()) else {
                finish_viewer_request(
                    &worker_key,
                    &request_for_worker,
                    Err(Arc::from("cancelled at decode gate")),
                );
                return;
            };
            if !request_for_worker.has_consumers()
                || (VIEWER_FOREGROUND_GENERATION.load(Ordering::Acquire) != 0
                    && !request_for_worker.foreground.load(Ordering::Acquire))
            {
                finish_viewer_request(
                    &worker_key,
                    &request_for_worker,
                    Err(Arc::from("cancelled before decode")),
                );
                return;
            }
            viewer_trace(format!(
                "decode_start uri={} variant={}x{} queue_ms={}",
                viewer_trace_uri(&worker_key.path),
                worker_key.target_width,
                worker_key.target_height,
                queued.elapsed().as_millis(),
            ));
            let started = std::time::Instant::now();
            let recipe = crate::edit::EditRecipe::decode(&worker_key.edit_recipe);
            let lane_request = request_for_worker.clone();
            let cancelled_request = request_for_worker.clone();
            let read_context = crate::source::ViewerReadContext::new(
                move || {
                    if lane_request.foreground.load(Ordering::Acquire) {
                        crate::source::ViewerReadLane::Foreground
                    } else {
                        crate::source::ViewerReadLane::Prefetch
                    }
                },
                move || !cancelled_request.has_consumers(),
                Some(crate::source::ViewerSourceFingerprint {
                    mtime: worker_key.mtime,
                    size_bytes: worker_key.size_bytes,
                }),
            );
            let result = crate::thumbnail::decode_for_viewer_with_cancel(
                &worker_key.path,
                worker_key.target_width,
                worker_key.target_height,
                Some(&read_context),
                || !request_for_worker.has_consumers(),
            )
            .map(|image| {
                let image = crate::edit::render::apply_recipe(
                    rotate_image(image, worker_key.rotation),
                    &recipe,
                );
                Arc::new(ViewerDecodeResult {
                    width: image.width(),
                    height: image.height(),
                    pixels: image.into_raw(),
                })
            })
            .map_err(|error| Arc::from(error.to_string()));
            viewer_trace(format!(
                "decode_done uri={} variant={}x{} elapsed_ms={} outcome={}",
                viewer_trace_uri(&worker_key.path),
                worker_key.target_width,
                worker_key.target_height,
                started.elapsed().as_millis(),
                if result.is_ok() { "ok" } else { "error" },
            ));
            finish_viewer_request(&worker_key, &request_for_worker, result);
        })
        .is_err()
    {
        finish_viewer_request(
            &key,
            &request,
            Err(Arc::from("could not start lightbox decode thread")),
        );
    }
}

fn display_texture_cache_lookup(
    cache: &DisplayTextureCache,
    path: &str,
    rotation: i32,
    edit_recipe: &str,
    target_width: u32,
    target_height: u32,
) -> Option<gtk::gdk::MemoryTexture> {
    let mut cache = cache.borrow_mut();
    let position = cache.iter().position(|entry| {
        entry.path == path
            && entry.rotation == rotation
            && entry.edit_recipe == edit_recipe
            && entry.target_width == target_width
            && entry.target_height == target_height
    })?;
    let entry = cache.remove(position)?;
    let texture = entry.texture.clone();
    cache.push_front(entry);
    Some(texture)
}

fn display_texture_cache_insert(
    cache: &DisplayTextureCache,
    path: String,
    rotation: i32,
    edit_recipe: String,
    target_width: u32,
    target_height: u32,
    texture: gtk::gdk::MemoryTexture,
) {
    // RGBA8 footprint of the texture. Used by the byte budget so a large or
    // zoomed viewport cannot cache an unbounded amount of pixel memory.
    let bytes = (texture.width().max(0) as usize)
        .saturating_mul(texture.height().max(0) as usize)
        .saturating_mul(4);
    let mut cache = cache.borrow_mut();
    cache.retain(|entry| {
        !(entry.path == path
            && entry.rotation == rotation
            && entry.edit_recipe == edit_recipe
            && entry.target_width == target_width
            && entry.target_height == target_height)
    });
    cache.push_front(DisplayTextureCacheEntry {
        path,
        rotation,
        edit_recipe,
        target_width,
        target_height,
        texture,
        bytes,
    });
    // Evict least-recently-used entries past either the count or the byte
    // budget. Always keep at least one entry so a single oversized texture can
    // still be shown without the cache immediately dropping everything.
    let mut total: usize = cache.iter().map(|entry| entry.bytes).sum();
    while cache.len() > DISPLAY_TEXTURE_CACHE_CAPACITY
        || (total > DISPLAY_TEXTURE_CACHE_BYTE_BUDGET && cache.len() > 1)
    {
        let Some(removed) = cache.pop_back() else {
            break;
        };
        total = total.saturating_sub(removed.bytes);
        viewer_trace(format!(
            "cache_evict uri={} bytes={} remaining_entries={} remaining_bytes={}",
            viewer_trace_uri(&removed.path),
            removed.bytes,
            cache.len(),
            total,
        ));
    }
}

thread_local! {
    // Only one lightbox exists, so the pending prefetch timer and cancel token
    // live in thread-local state rather than on every navigation closure.
    static PREFETCH_SOURCE: RefCell<Option<glib::SourceId>> = const { RefCell::new(None) };
    static PREFETCH_CANCEL: RefCell<Vec<Arc<ViewerRequestLease>>> = const { RefCell::new(Vec::new()) };
}

/// Cancel the pending prefetch timer and any in-flight prefetch decode.
/// Called on every navigation (so a stale prefetch never competes with the
/// photo the user actually moved to) and when the lightbox closes.
fn cancel_lightbox_prefetch() {
    cancel_lightbox_prefetch_except(None);
}

/// Cancel speculative leases except for the exact request that the foreground
/// is about to claim. Retaining that lease makes foreground promotion safe:
/// cancelling the old prefetch cannot discard the selected image's result.
fn cancel_lightbox_prefetch_except(keep: Option<&ViewerRequestKey>) {
    PREFETCH_SOURCE.with(|slot| {
        if let Some(source) = slot.borrow_mut().take() {
            source.remove();
        }
    });
    PREFETCH_CANCEL.with(|slot| {
        let mut leases = slot.borrow_mut();
        leases.retain(|lease| {
            let retain = keep.is_some_and(|key| {
                // The request itself is the canonical key; compare through
                // the registry's pointer only after the foreground claim.
                // Before that, an exact key match is represented by the
                // matching result request held by this lease.
                let requests = VIEWER_REQUESTS.get_or_init(|| Mutex::new(HashMap::new()));
                requests
                    .lock()
                    .unwrap()
                    .get(key)
                    .is_some_and(|request| Arc::ptr_eq(request, &lease.request))
            });
            if !retain {
                lease.cancel();
            }
            retain
        });
        if keep.is_none() {
            leases.clear();
        }
    });
}

/// After the user settles on a photo, warm both immediate neighbors in the
/// display-texture RAM cache, prioritizing the direction of movement. Delayed
/// so rapid stepping does not schedule a decode per keypress, and cancelled on
/// the next navigation or when the viewer closes.
fn schedule_lightbox_prefetch(
    photos: Rc<RefCell<Vec<PhotoObject>>>,
    current: usize,
    direction: i32,
    root: gtk::Overlay,
    zoom: Rc<Cell<f64>>,
    cache: DisplayTextureCache,
    generation: Rc<Cell<u64>>,
) {
    cancel_lightbox_prefetch();
    if direction == 0 || !root.is_visible() {
        return;
    }
    let expected_generation = generation.get();
    let source = glib::timeout_add_local(Duration::from_millis(250), move || {
        if !root.is_visible() || generation.get() != expected_generation {
            PREFETCH_SOURCE.with(|slot| {
                slot.borrow_mut().take();
            });
            return glib::ControlFlow::Break;
        }
        // Do not launch either neighbor until the requested photo has
        // completed its foreground decode. This timer is NOT a navigation
        // delay: show_photo() started the selected decode immediately.
        if VIEWER_FOREGROUND_GENERATION.load(Ordering::Acquire) != 0 {
            return glib::ControlFlow::Continue;
        }
        PREFETCH_SOURCE.with(|slot| {
            slot.borrow_mut().take();
        });
        let len = photos.borrow().len();
        let previous = current.checked_sub(1);
        let next = current.checked_add(1).filter(|&index| index < len);
        // Prefer the direction of travel, but warm *both* immediate neighbors.
        // No folder-wide prefetch: at most two speculative viewer decodes.
        let targets = if direction < 0 {
            [previous, next]
        } else {
            [next, previous]
        };
        for target in targets.into_iter().flatten() {
            prefetch_display_texture(&photos.borrow(), target, &root, zoom.get(), cache.clone());
        }
        glib::ControlFlow::Break
    });
    PREFETCH_SOURCE.with(|slot| {
        slot.borrow_mut().replace(source);
    });
}

/// Decode a neighbor photo into the display cache without touching the visible
/// picture. Uses the shared decode gate so a burst of prefetches cannot spawn
/// unbounded full-resolution RAW decodes, and bails out as soon as its cancel
/// token is set.
fn prefetch_display_texture(
    photos: &[PhotoObject],
    index: usize,
    root: &gtk::Overlay,
    zoom: f64,
    cache: DisplayTextureCache,
) {
    let Some(photo) = photos.get(index) else {
        return;
    };
    if zoom < 0.0 {
        return;
    }
    let (target_width, target_height, _, _, _, _) =
        viewer_decode_target(root, photo.rotation(), false);
    let path = photo.path();
    if display_texture_cache_lookup(
        &cache,
        &path,
        photo.rotation(),
        &photo.edit_recipe(),
        target_width,
        target_height,
    )
    .is_some()
    {
        viewer_trace(format!(
            "cache_hit lane=prefetch uri={}",
            viewer_trace_uri(&path)
        ));
        return;
    }

    let rotation = photo.rotation();
    let edit_recipe_text = photo.edit_recipe();
    let key = ViewerRequestKey {
        path: path.clone(),
        mtime: photo.mtime(),
        size_bytes: photo.size_bytes(),
        rotation,
        edit_recipe: edit_recipe_text.clone(),
        target_width,
        target_height,
    };
    let (request, lease, claim) = claim_viewer_request(&key, false);
    viewer_trace(format!(
        "request lane=prefetch action={} uri={} variant={}x{}",
        match claim {
            ViewerRequestClaim::New => "new",
            ViewerRequestClaim::JoinedForeground => "join",
            ViewerRequestClaim::PromotedPrefetch => "promote",
        },
        viewer_trace_uri(&path),
        target_width,
        target_height,
    ));
    PREFETCH_CANCEL.with(|slot| slot.borrow_mut().push(lease.clone()));
    if matches!(claim, ViewerRequestClaim::New) {
        start_viewer_request(key, request.clone());
    }
    let cache_path = path.clone();
    let cache_for_result = cache.clone();

    glib::MainContext::default().spawn_local(async move {
        let result = ViewerResultSlot::wait(request.result.clone()).await;
        if lease.cancelled() {
            viewer_trace(format!(
                "display_discard lane=prefetch uri={} reason=cancelled",
                viewer_trace_uri(&cache_path),
            ));
            lease.release();
            return;
        }
        let Ok(result) = result else {
            lease.release();
            return;
        };
        let bytes = glib::Bytes::from_owned(result.pixels.clone());
        let texture = gtk::gdk::MemoryTexture::new(
            result.width as i32,
            result.height as i32,
            gtk::gdk::MemoryFormat::R8g8b8a8,
            &bytes,
            result.width as usize * 4,
        );
        display_texture_cache_insert(
            &cache_for_result,
            cache_path,
            rotation,
            edit_recipe_text,
            target_width,
            target_height,
            texture,
        );
        lease.release();
    });
}

fn prepare_navigation_photo(
    picture: &gtk::Picture,
    photo: Option<&PhotoObject>,
    root: &gtk::Overlay,
    zoom: f64,
    display_cache: &DisplayTextureCache,
) -> (bool, bool) {
    let Some(photo) = photo else {
        return (false, false);
    };

    if zoom < 0.0 {
        return (false, false);
    }

    let (target_width, target_height, _, _, _, _) =
        viewer_decode_target(root, photo.rotation(), false);
    let path = photo.path();
    if let Some(texture) = display_texture_cache_lookup(
        display_cache,
        &path,
        photo.rotation(),
        &photo.edit_recipe(),
        target_width,
        target_height,
    ) {
        set_fit_geometry_from_intrinsic(
            picture,
            photo,
            root,
            zoom,
            texture.width(),
            texture.height(),
        );
        picture.set_paintable(Some(&texture));

        return (true, true);
    }

    // No blurry grid thumbnail in the full-size viewer. On a RAM cache miss,
    // retain the previous full-resolution paintable until show_photo() has
    // decoded the requested image. This applies to JPEG and RAW alike.
    // The generation check in show_photo() prevents stale results appearing.
    (false, false)
}

fn set_fit_geometry_from_intrinsic(
    picture: &gtk::Picture,
    photo: &PhotoObject,
    root: &gtk::Overlay,
    zoom: f64,
    intrinsic_width: i32,
    intrinsic_height: i32,
) {
    let (native_width, native_height) = presentation_native_dimensions(photo);
    let (width, height) = fitted_picture_dimensions(
        native_width,
        native_height,
        intrinsic_width,
        intrinsic_height,
        root.width(),
        root.height(),
        zoom,
    );
    picture.set_size_request(width, height);
}

fn presentation_native_dimensions(photo: &PhotoObject) -> (i64, i64) {
    let (mut width, mut height) = (photo.width().max(1) as u32, photo.height().max(1) as u32);
    if matches!(photo.rotation().rem_euclid(360), 90 | 270) {
        std::mem::swap(&mut width, &mut height);
    }
    let recipe = crate::edit::EditRecipe::decode(&photo.edit_recipe());
    let (width, height) = crate::edit::render::estimated_output_dimensions(width, height, &recipe);
    (i64::from(width), i64::from(height))
}

fn reset_viewport(viewport: &gtk::ScrolledWindow) {
    let horizontal = viewport.hadjustment();
    let vertical = viewport.vadjustment();
    horizontal.set_value(horizontal.lower());
    vertical.set_value(vertical.lower());
}

fn center_viewport_soon(viewport: &gtk::ScrolledWindow) {
    let viewport = viewport.clone();

    // Wait until GTK has applied the native-size child allocation and the
    // ScrolledWindow adjustments expose their real upper/page_size values.
    // A plain idle callback can run too early, leaving the 1:1 view at 0,0.
    glib::timeout_add_local_once(Duration::from_millis(16), move || {
        let horizontal = viewport.hadjustment();
        let vertical = viewport.vadjustment();

        let max_h = (horizontal.upper() - horizontal.page_size()).max(horizontal.lower());
        let max_v = (vertical.upper() - vertical.page_size()).max(vertical.lower());

        let centered_h = horizontal.lower() + (max_h - horizontal.lower()) / 2.0;
        let centered_v = vertical.lower() + (max_v - vertical.lower()) / 2.0;

        horizontal.set_value(centered_h);
        vertical.set_value(centered_v);
    });
}

fn viewer_decode_target(
    root: &gtk::Overlay,
    rotation: i32,
    one_to_one: bool,
) -> (u32, u32, i32, i32, i32, bool) {
    let allocated_width = root.width() - VIEWER_PADDING;
    let allocated_height = root.height() - VIEWER_PADDING;
    let fallback = allocated_width <= 0 || allocated_height <= 0;
    let (logical_width, logical_height) = if fallback {
        // A hidden overlay normally retains its window allocation. This only
        // applies on the very first frame before GTK has allocated the window.
        (1024, 768)
    } else {
        (allocated_width, allocated_height)
    };
    let scale_factor = root.scale_factor().max(1);
    // RAW catalog dimensions are frequently absent and can describe the
    // sensor rather than its embedded display preview. Use an unbounded 1:1
    // request and let decode_for_viewer clamp it to the actual source it
    // discovers. Falling back to the viewport made 1:1 indistinguishable
    // from fit-to-window for DNG and many NEF files.
    let mut target_width = if one_to_one {
        u32::MAX
    } else {
        (logical_width as u32).saturating_mul(scale_factor as u32)
    };
    let mut target_height = if one_to_one {
        u32::MAX
    } else {
        (logical_height as u32).saturating_mul(scale_factor as u32)
    };
    // decode_for_viewer applies EXIF orientation; user rotation happens in
    // this module afterward, so swap its input bounds for a quarter-turn.
    if matches!(rotation.rem_euclid(360), 90 | 270) {
        std::mem::swap(&mut target_width, &mut target_height);
    }
    (
        target_width.max(1),
        target_height.max(1),
        logical_width,
        logical_height,
        scale_factor,
        fallback,
    )
}

fn rotate_image(image: image::RgbaImage, rotation: i32) -> image::RgbaImage {
    match rotation.rem_euclid(360) {
        90 => image::imageops::rotate90(&image),
        180 => image::imageops::rotate180(&image),
        270 => image::imageops::rotate270(&image),
        _ => image,
    }
}

fn fit_picture(
    picture: &gtk::Picture,
    photos: &[PhotoObject],
    index: usize,
    viewport_width: i32,
    viewport_height: i32,
    zoom: f64,
) {
    let Some(photo) = photos.get(index) else {
        return;
    };

    if viewport_width <= 0 || viewport_height <= 0 {
        return;
    }

    let paintable = picture.paintable();
    let intrinsic_width = paintable
        .as_ref()
        .map(gtk::gdk::Paintable::intrinsic_width)
        .unwrap_or(0);
    let intrinsic_height = paintable
        .as_ref()
        .map(gtk::gdk::Paintable::intrinsic_height)
        .unwrap_or(0);
    let (native_width, native_height) = presentation_native_dimensions(photo);
    let (fitted_width, fitted_height) = fitted_picture_dimensions(
        native_width,
        native_height,
        intrinsic_width,
        intrinsic_height,
        viewport_width,
        viewport_height,
        zoom,
    );

    picture.set_size_request(fitted_width, fitted_height);
}

fn fitted_picture_dimensions(
    native_width: i64,
    native_height: i64,
    intrinsic_width: i32,
    intrinsic_height: i32,
    viewport_width: i32,
    viewport_height: i32,
    zoom: f64,
) -> (i32, i32) {
    let intrinsic_valid = intrinsic_width > 0 && intrinsic_height > 0;
    let native_valid = native_width > 0 && native_height > 0;

    // At 1:1, use the pixels actually present in the decoded texture. A RAW
    // embedded preview may differ from the sensor dimensions in the catalog.
    let (mut source_width, mut source_height) = if zoom < 0.0 && intrinsic_valid {
        (f64::from(intrinsic_width), f64::from(intrinsic_height))
    } else if native_valid {
        (native_width as f64, native_height as f64)
    } else if intrinsic_valid {
        (f64::from(intrinsic_width), f64::from(intrinsic_height))
    } else {
        (1.0, 1.0)
    };

    // Cached thumbnails are already EXIF-oriented, while database dimensions
    // generally describe the encoded source. Use the thumbnail only to detect
    // an axis swap; keep the native dimensions as the scaling limit. This
    // presents a large photo's thumbnail at its final fitted size without
    // treating a genuinely small source as a large image.
    if native_valid
        && intrinsic_valid
        && (source_width > source_height) != (intrinsic_width > intrinsic_height)
    {
        std::mem::swap(&mut source_width, &mut source_height);
    }

    let available_width = (viewport_width - VIEWER_PADDING).max(1) as f64;
    let available_height = (viewport_height - VIEWER_PADDING).max(1) as f64;
    let fit_scale = (available_width / source_width).min(available_height / source_height);
    // Known native dimensions remain the hard cap, so small source images are
    // never enlarged. If metadata is unavailable (common for RAW), this is a
    // cached opening preview rather than the decoded source; present it at the
    // viewer's fitted size while the correctly sized full decode is pending.
    let fit_scale = if native_valid {
        fit_scale.min(1.0)
    } else {
        fit_scale
    };
    let scale = if zoom < 0.0 {
        1.0
    } else if zoom == 0.0 {
        fit_scale
    } else {
        fit_scale * zoom
    };

    (
        (source_width * scale).round().max(1.0) as i32,
        (source_height * scale).round().max(1.0) as i32,
    )
}
