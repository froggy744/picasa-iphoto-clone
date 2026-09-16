#[derive(Clone, Copy)]
enum OneToOneAnchor {
    Cursor {
        normalized_x: f64,
        normalized_y: f64,
        pointer_x: f64,
        pointer_y: f64,
    },
    Center,
}

fn fit_zoom_for_canvas(scroll: &gtk::ScrolledWindow, dimensions: (i32, i32)) -> f64 {
    let source_w = dimensions.0.max(1) as f64;
    let source_h = dimensions.1.max(1) as f64;
    // Leave a small breathing margin so the first + click visibly enlarges
    // the photo instead of jumping straight from Fit to native 100%.
    let viewport_w = (scroll.width() - 24).max(1) as f64;
    let viewport_h = (scroll.height() - 24).max(1) as f64;
    (viewport_w / source_w)
        .min(viewport_h / source_h)
        .min(1.0)
        .max(0.01)
}

fn canvas_size_for_zoom(
    scroll: &gtk::ScrolledWindow,
    dimensions: (i32, i32),
    zoom: f64,
) -> (f64, f64) {
    let fit = fit_zoom_for_canvas(scroll, dimensions);
    let effective_zoom = (fit * zoom).clamp(0.01, 8.0);
    (
        (dimensions.0.max(1) as f64 * effective_zoom).max(1.0),
        (dimensions.1.max(1) as f64 * effective_zoom).max(1.0),
    )
}

fn anchored_scroll_target(
    current_scroll: f64,
    pointer: f64,
    viewport: f64,
    old_content: f64,
    new_content: f64,
) -> f64 {
    let old_content = old_content.max(1.0);
    let new_content = new_content.max(1.0);
    let old_padding = ((viewport - old_content) * 0.5).max(0.0);
    let new_padding = ((viewport - new_content) * 0.5).max(0.0);
    let image_position = (current_scroll + pointer - old_padding).clamp(0.0, old_content);
    let normalized = image_position / old_content;
    normalized * new_content + new_padding - pointer
}

fn pointer_over_content(pointer: f64, viewport: f64, content: f64) -> bool {
    if !pointer.is_finite() || !viewport.is_finite() || !content.is_finite() {
        return false;
    }
    let padding = ((viewport - content) * 0.5).max(0.0);
    let visible_end = if content >= viewport {
        viewport
    } else {
        padding + content
    };
    pointer >= padding && pointer <= visible_end
}

fn fit_photo_rect(
    container_w: f64,
    container_h: f64,
    image_w: f64,
    image_h: f64,
) -> (f64, f64, f64, f64) {
    contained_rect(container_w, container_h, image_w, image_h)
}

fn normalized_image_point(
    pointer_x: f64,
    pointer_y: f64,
    photo_x: f64,
    photo_y: f64,
    photo_width: f64,
    photo_height: f64,
    h_scroll: f64,
    v_scroll: f64,
) -> Option<(f64, f64)> {
    if !pointer_x.is_finite()
        || !pointer_y.is_finite()
        || photo_width <= 0.0
        || photo_height <= 0.0
        || pointer_x < photo_x
        || pointer_y < photo_y
        || pointer_x > photo_x + photo_width
        || pointer_y > photo_y + photo_height
    {
        return None;
    }
    Some((
        ((h_scroll + pointer_x - photo_x) / photo_width).clamp(0.0, 1.0),
        ((v_scroll + pointer_y - photo_y) / photo_height).clamp(0.0, 1.0),
    ))
}

fn normalized_scroll_target(
    normalized: f64,
    pointer: f64,
    viewport: f64,
    native_content: f64,
) -> f64 {
    let padding = ((viewport - native_content) * 0.5).max(0.0);
    normalized.clamp(0.0, 1.0) * native_content + padding - pointer
}

fn one_to_one_scroll_target(
    current_scroll: f64,
    pointer: f64,
    viewport: f64,
    old_content: f64,
    native_content: f64,
    anchor_cursor: bool,
) -> f64 {
    if anchor_cursor {
        anchored_scroll_target(
            current_scroll,
            pointer,
            viewport,
            old_content,
            native_content,
        )
    } else {
        ((native_content - viewport) * 0.5).max(0.0)
    }
}

fn apply_canvas_zoom(
    picture: &gtk::Picture,
    scroll: &gtk::ScrolledWindow,
    dimensions: (i32, i32),
    zoom: f64,
) {
    if zoom <= 0.0 {
        picture.set_can_shrink(true);
        picture.set_hexpand(true);
        picture.set_vexpand(true);
        picture.set_halign(gtk::Align::Fill);
        picture.set_valign(gtk::Align::Fill);
        picture.set_size_request(1, 1);
        scroll.set_cursor_from_name(None);
        scroll.hadjustment().set_value(0.0);
        scroll.vadjustment().set_value(0.0);
        return;
    }
    // Positive zoom values are multipliers of the current Fit scale.
    let (width, height) = canvas_size_for_zoom(scroll, dimensions, zoom);
    let width = width.round() as i32;
    let height = height.round() as i32;
    // Keep shrinking enabled: disabling it makes GtkPicture insist on the
    // paintable's intrinsic size and defeats incremental size requests.
    picture.set_can_shrink(true);
    picture.set_hexpand(false);
    picture.set_vexpand(false);
    picture.set_halign(gtk::Align::Center);
    picture.set_valign(gtk::Align::Center);
    picture.set_size_request(width, height);
    scroll.set_cursor_from_name(if zoom > 1.0 { Some("grab") } else { None });
    picture.queue_resize();
}

fn apply_canvas_one_to_one(
    picture: &gtk::Picture,
    scroll: &gtk::ScrolledWindow,
    dimensions: (i32, i32),
) {
    picture.set_can_shrink(true);
    picture.set_hexpand(false);
    picture.set_vexpand(false);
    picture.set_halign(gtk::Align::Center);
    picture.set_valign(gtk::Align::Center);
    picture.set_size_request(dimensions.0.max(1), dimensions.1.max(1));
    scroll.set_cursor_from_name(Some("grab"));
    picture.queue_resize();
}
