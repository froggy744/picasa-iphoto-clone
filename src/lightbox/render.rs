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
    decode_cancel: Rc<RefCell<Option<Arc<AtomicBool>>>>,
    picture_viewport: &gtk::ScrolledWindow,
    native_texture: Rc<RefCell<Option<NativeTextureCache>>>,
    display_texture_cache: DisplayTextureCache,
    fit_geometry_fixed: bool,
    cache_hit: bool,
) {
    let started = std::time::Instant::now();
    let Some(photo) = photos.get(index) else {
        return;
    };

    // Decode a display-quality image off the GTK thread. The cached thumbnail
    // is shown only for the initial open. During navigation the previous full
    // image remains until this result is ready, avoiding a low-resolution
    // thumbnail flash between adjacent photos.
    let path = photo.path();
    if let Some(previous) = decode_cancel.borrow_mut().take() {
        previous.store(true, Ordering::Release);
    }
    if cache_hit {
        if std::env::var_os("PICASA_TRACE").is_some() {
            eprintln!("UI PERF lightbox_display_cache_reuse path={path}");
        }
        return;
    }
    let cancelled = Arc::new(AtomicBool::new(false));
    *decode_cancel.borrow_mut() = Some(cancelled.clone());
    let cancelled_for_thread = cancelled.clone();
    let result_slot = ResultSlot::new();
    let result_slot_for_worker = result_slot.clone();
    let decode_path = path.clone();
    let rotation = photo.rotation();
    let target_started = std::time::Instant::now();
    let (target_width, target_height, logical_width, logical_height, scale_factor, fallback) =
        viewer_decode_target(root, rotation, zoom.get() < 0.0);
    if std::env::var_os("PICASA_TRACE").is_some() {
        eprintln!(
            "UI PERF lightbox_target_calculation_ms={} allocation={}x{} logical_viewport={}x{} scale_factor={} decode_target={}x{} user_rotation={} fallback={}",
            target_started.elapsed().as_millis(),
            root.width(),
            root.height(),
            logical_width,
            logical_height,
            scale_factor,
            target_width,
            target_height,
            rotation,
            fallback
        );
    }
    if std::thread::Builder::new()
        .name("lightbox-decode".to_string())
        .spawn(move || {
            if cancelled_for_thread.load(Ordering::Acquire) {
                result_slot_for_worker.send(Err(anyhow::anyhow!("cancelled before decode")));
                return;
            }
            let gate = VIEWER_DECODE_GATE
                .get_or_init(|| DecodeSemaphore::new(MAX_CONCURRENT_VIEWER_DECODES));
            let gate_started = std::time::Instant::now();
            let Some(_permit) = gate.acquire_cancelled(&cancelled_for_thread) else {
                if std::env::var_os("PICASA_TRACE").is_some() {
                    eprintln!(
                        "UI TRACE lightbox_decode_cancelled stage=decode_gate wait_ms={}",
                        gate_started.elapsed().as_millis()
                    );
                }
                result_slot_for_worker.send(Err(anyhow::anyhow!("cancelled at decode gate")));
                return;
            };
            if std::env::var_os("PICASA_TRACE").is_some() {
                eprintln!(
                    "UI PERF lightbox_decode_gate_wait_ms={}",
                    gate_started.elapsed().as_millis()
                );
            }
            let worker_started = std::time::Instant::now();
            let result: anyhow::Result<(u32, u32, Vec<u8>, std::time::Instant)> = (|| {
                let image = crate::thumbnail::decode_for_viewer_with_cancel(
                    &decode_path,
                    target_width,
                    target_height,
                    || cancelled_for_thread.load(Ordering::Acquire),
                )?;
                if cancelled_for_thread.load(Ordering::Acquire) {
                    anyhow::bail!("cancelled");
                }
                let rotation_started = std::time::Instant::now();
                let image = rotate_image(image, rotation);
                if std::env::var_os("PICASA_TRACE").is_some() {
                    eprintln!(
                        "VIEW PERF user_orientation_ms={} rotation={} output={}x{}",
                        rotation_started.elapsed().as_millis(),
                        rotation,
                        image.width(),
                        image.height()
                    );
                    eprintln!(
                        "VIEW PERF lightbox_worker_total_ms={} output={}x{}",
                        worker_started.elapsed().as_millis(),
                        image.width(),
                        image.height()
                    );
                }
                Ok((
                    image.width(),
                    image.height(),
                    image.into_raw(),
                    std::time::Instant::now(),
                ))
            })();
            result_slot_for_worker.send(result);
        })
        .is_err() {
        result_slot.send(Err(anyhow::anyhow!("could not start lightbox decode thread")));
    }

    let picture = picture.clone();
    let root = root.clone();
    let picture_viewport = picture_viewport.clone();
    let cache_path = path.clone();
    let photo = photo.clone();
    let display_texture_cache_for_result = display_texture_cache.clone();
    glib::MainContext::default().spawn_local(async move {
        let result = ResultSlot::wait(result_slot).await;
        let delivery_started = std::time::Instant::now();

        if generation.get() != expected_generation || cancelled.load(Ordering::Acquire) {
            if std::env::var_os("PICASA_TRACE").is_some() {
                eprintln!("UI TRACE lightbox_stale_result ignored expected_generation={} current_generation={}", expected_generation, generation.get());
            }
            return;
        }

        match result {
            Ok((width, height, pixels, worker_finished)) => {
                let channel_wait_ms = worker_finished.elapsed().as_millis();
                let texture_started = std::time::Instant::now();
                let bytes = glib::Bytes::from_owned(pixels);
                let texture = gtk::gdk::MemoryTexture::new(
                    width as i32,
                    height as i32,
                    gtk::gdk::MemoryFormat::R8g8b8a8,
                    &bytes,
                    width as usize * 4,
                );
                let texture_ms = texture_started.elapsed().as_millis();
                let gtk_started = std::time::Instant::now();

                // Never overwrite PhotoObject's source dimensions with the
                // dimensions of a display-sized viewer decode. The original
                // dimensions are what 1:1 mode needs to request native pixels.
                if zoom.get() < 0.0 {
                    *native_texture.borrow_mut() = Some(NativeTextureCache {
                        path: cache_path.clone(),
                        rotation,
                        texture: texture.clone(),
                    });
                }

                picture.set_paintable(Some(&texture));
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
                if std::env::var_os("PICASA_TRACE").is_some() {
                    eprintln!(
                        "UI PERF lightbox_texture_creation_ms={} size={}x{}",
                        texture_ms, width, height
                    );
                    eprintln!(
                        "UI PERF lightbox_main_thread_delivery_ms={} worker_finished_to_gtk_ms={} channel_wait_ms={} gtk_update_ms={}",
                        delivery_started.elapsed().as_millis(),
                        channel_wait_ms,
                        channel_wait_ms,
                        gtk_started.elapsed().as_millis()
                    );
                    eprintln!(
                        "UI PERF lightbox_full_texture_visible_ms={} size={}x{}",
                        started.elapsed().as_millis(),
                        width,
                        height
                    );
                }
            }
            Err(error) => {
                if std::env::var_os("PICASA_TRACE").is_some() {
                    eprintln!("UI TRACE lightbox_full_decode_error={}", error);
                }
            }
        }
    });

    if std::env::var_os("PICASA_TRACE").is_some() {
        eprintln!(
            "UI PERF lightbox_decode_queued_ms={} generation={} path={}",
            started.elapsed().as_millis(),
            expected_generation,
            path
        );
    }
}

fn display_texture_cache_lookup(
    cache: &DisplayTextureCache,
    path: &str,
    rotation: i32,
    target_width: u32,
    target_height: u32,
) -> Option<gtk::gdk::MemoryTexture> {
    let mut cache = cache.borrow_mut();
    let position = cache.iter().position(|entry| {
        entry.path == path
            && entry.rotation == rotation
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
    target_width: u32,
    target_height: u32,
    texture: gtk::gdk::MemoryTexture,
) {
    let mut cache = cache.borrow_mut();
    cache.retain(|entry| {
        !(entry.path == path
            && entry.rotation == rotation
            && entry.target_width == target_width
            && entry.target_height == target_height)
    });
    cache.push_front(DisplayTextureCacheEntry {
        path,
        rotation,
        target_width,
        target_height,
        texture,
    });
    cache.truncate(DISPLAY_TEXTURE_CACHE_CAPACITY);
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
    let navigation_started = std::time::Instant::now();
    if std::env::var_os("PICASA_TRACE").is_some() {
        eprintln!("UI TRACE lightbox_navigation_request path={}", photo.path());
    }
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
        if std::env::var_os("PICASA_TRACE").is_some() {
            eprintln!(
                "UI PERF lightbox_display_cache_hit path={} target={}x{}",
                path, target_width, target_height
            );
            eprintln!("UI PERF lightbox_preview_visible_ms=0 source=display_cache");
        }
        return (true, true);
    }
    if std::env::var_os("PICASA_TRACE").is_some() {
        eprintln!(
            "UI PERF lightbox_display_cache_miss path={} target={}x{} request_ms={}",
            path,
            target_width,
            target_height,
            navigation_started.elapsed().as_millis()
        );
    }

    // Nikon RAW/NEF cached thumbnails can have a different presentation
    // path/aspect from the embedded display preview. Leave the previous full
    // image in place for uncached RAW navigation so it cannot flash a second
    // image or trigger a transient black-bar allocation.
    if crate::image_format::uses(&path, crate::image_format::DecoderKind::Raw) {
        if std::env::var_os("PICASA_TRACE").is_some() {
            eprintln!("UI PERF lightbox_preview_skipped source=raw_previous_full");
        }
        return (false, false);
    }

    let Some(thumbnail) = photo
        .cached_thumbnail_path()
        .filter(|thumbnail| std::path::Path::new(thumbnail).is_file())
    else {
        return (false, false);
    };
    let Ok((mut width, mut height)) = image::image_dimensions(&thumbnail) else {
        return (false, false);
    };
    if matches!(photo.rotation().rem_euclid(360), 90 | 270) {
        std::mem::swap(&mut width, &mut height);
    }

    let started = std::time::Instant::now();
    set_fit_geometry_from_intrinsic(
        picture,
        photo,
        root,
        zoom,
        width as i32,
        height as i32,
    );
    if photo.rotation().rem_euclid(360) == 0 {
        picture.set_filename(Some(thumbnail));
    } else if let Some(rotated) = crate::photo_texture::rotated_thumbnail(&thumbnail, photo.rotation())
    {
        picture.set_paintable(Some(&rotated));
    } else {
        return (false, false);
    }
    if std::env::var_os("PICASA_TRACE").is_some() {
        eprintln!(
            "UI PERF lightbox_preview_visible_ms={} source=thumbnail geometry={}x{}",
            started.elapsed().as_millis(),
            picture.width_request(),
            picture.height_request()
        );
    }
    (true, false)
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
    let (width, height) = (photo.width(), photo.height());
    if matches!(photo.rotation().rem_euclid(360), 90 | 270) {
        (height, width)
    } else {
        (width, height)
    }
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

        if std::env::var_os("PICASA_TRACE").is_some() {
            eprintln!(
                "UI TRACE lightbox_pan_center h={} h_upper={} h_page={} v={} v_upper={} v_page={}",
                centered_h,
                horizontal.upper(),
                horizontal.page_size(),
                centered_v,
                vertical.upper(),
                vertical.page_size()
            );
        }
    });
}

fn show_cached_preview(picture: &gtk::Picture, photo: &PhotoObject) {
    let started = std::time::Instant::now();
    if let Some(thumbnail) = photo
        .cached_thumbnail_path()
        .filter(|path| std::path::Path::new(path).is_file())
    {
        if let Some(rotated) = crate::photo_texture::rotated_thumbnail(&thumbnail, photo.rotation())
        {
            picture.set_paintable(Some(&rotated));
        } else {
            picture.set_filename(Some(thumbnail));
        }
        if std::env::var_os("PICASA_TRACE").is_some() {
            eprintln!(
                "UI PERF lightbox_preview_ms={} path={}",
                started.elapsed().as_millis(),
                photo.path()
            );
        }
    }
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
    let started = std::time::Instant::now();
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
    let (fitted_width, fitted_height) = fitted_picture_dimensions(
        photo.width(),
        photo.height(),
        intrinsic_width,
        intrinsic_height,
        viewport_width,
        viewport_height,
        zoom,
    );

    picture.set_size_request(fitted_width, fitted_height);
    if std::env::var_os("PICASA_TRACE").is_some() {
        eprintln!(
            "UI PERF fit_ms={} zoom={} size={}x{}",
            started.elapsed().as_millis(),
            zoom,
            fitted_width,
            fitted_height
        );
    }
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
