// Editor fragment included by editor.rs. Provides the interactive overlay
// canvas layer and the Overlays panel tab. Pure geometry helpers live at the
// top so they can be unit-tested without a GTK display.

const OVERLAY_HANDLE_RADIUS: f64 = 9.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OverlayDragMode {
    None,
    Move,
    Resize,
    Pan,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LayerSelection {
    Image(usize),
    Text(usize),
}

#[derive(Clone, Copy, Debug)]
struct OverlayDragState {
    mode: OverlayDragMode,
    /// Pointer position at drag begin, in normalized photo coordinates.
    start_norm: (f64, f64),
    /// Layer rectangle at drag begin, in normalized photo coordinates.
    start_rect: super::model::NormRect,
    layer: LayerSelection,
    /// Text layer font size (photo-height fraction) at drag begin.
    start_size: f32,
    anchor: super::model::OverlayAnchor,
    pan_origin_h: f64,
    pan_origin_v: f64,
}

impl Default for OverlayDragState {
    fn default() -> Self {
        Self {
            mode: OverlayDragMode::None,
            start_norm: (0.0, 0.0),
            start_rect: super::model::NormRect {
                left: 0.0,
                top: 0.0,
                width: 0.0,
                height: 0.0,
            },
            layer: LayerSelection::Image(0),
            start_size: 0.0,
            anchor: super::model::OverlayAnchor::Center,
            pan_origin_h: 0.0,
            pan_origin_v: 0.0,
        }
    }
}

/// Where the current photo sits inside the overlay DrawingArea, in the same
/// logical coordinates the draw func and gesture handlers receive.
///
/// Fit mode is a plain letterbox. Zoomed and native 1:1 modes place the
/// scaled content centred in the viewport, then shift it by the scrolled
/// adjustment values so dragging the photo underneath keeps handles glued to
/// the image.
fn overlay_display_rect(
    viewport_w: f64,
    viewport_h: f64,
    photo_w: f64,
    photo_h: f64,
    zoom: f64,
    native: bool,
    content_w: f64,
    content_h: f64,
    h_scroll: f64,
    v_scroll: f64,
) -> (f64, f64, f64, f64) {
    let (photo_w, photo_h) = (photo_w.max(1.0), photo_h.max(1.0));
    if zoom <= 0.0 && !native {
        return contained_rect(viewport_w, viewport_h, photo_w, photo_h);
    }
    let content_w = content_w.max(1.0);
    let content_h = content_h.max(1.0);
    let pad_x = ((viewport_w - content_w) * 0.5).max(0.0);
    let pad_y = ((viewport_h - content_h) * 0.5).max(0.0);
    (pad_x - h_scroll, pad_y - v_scroll, content_w, content_h)
}

fn screen_to_normalized(display: (f64, f64, f64, f64), px: f64, py: f64) -> (f64, f64) {
    let (x, y, w, h) = display;
    if w <= 0.0 || h <= 0.0 {
        return (0.0, 0.0);
    }
    (
        ((px - x) / w).clamp(0.0, 1.0),
        ((py - y) / h).clamp(0.0, 1.0),
    )
}

fn resize_handle_point(rect: super::model::NormRect, anchor: OverlayAnchor) -> (f32, f32) {
    match anchor {
        OverlayAnchor::TopLeft => (rect.right(), rect.bottom()),
        OverlayAnchor::TopRight => (rect.left, rect.bottom()),
        OverlayAnchor::BottomLeft => (rect.right(), rect.top),
        OverlayAnchor::BottomRight => (rect.left, rect.top),
        OverlayAnchor::Center => (rect.right(), rect.bottom()),
    }
}

/// True when the screen point sits within `radius` of the overlay's resize
/// handle (the only handle; resize is aspect-locked around the anchor).
fn hits_resize_handle(
    display: (f64, f64, f64, f64),
    rect: super::model::NormRect,
    px: f64,
    py: f64,
    radius: f64,
    anchor: OverlayAnchor,
) -> bool {
    let (x, y, w, h) = display;
    let (handle_norm_x, handle_norm_y) = resize_handle_point(rect, anchor);
    let handle_x = x + handle_norm_x as f64 * w;
    let handle_y = y + handle_norm_y as f64 * h;
    let dx = px - handle_x;
    let dy = py - handle_y;
    dx * dx + dy * dy <= radius * radius
}

/// Aspect-locked resize anchored at the rectangle's pinned corner: width
/// follows the pointer, height is derived from the asset's pixel aspect and
/// the photo's aspect, then both are clamped so the overlay stays inside the
/// photo without shifting the fixed corner.
fn resize_rect_from_anchor(
    start: super::model::NormRect,
    pointer_norm: (f64, f64),
    photo_w: f32,
    photo_h: f32,
    asset_aspect: f32,
    anchor: OverlayAnchor,
) -> super::model::NormRect {
    let photo_w = photo_w.max(1.0e-6);
    let photo_h = photo_h.max(1.0e-6);
    let aspect = if asset_aspect.is_finite() && asset_aspect > 1.0e-6 {
        asset_aspect
    } else {
        1.0
    };
    let px = pointer_norm.0 as f32;
    let center_x = start.left + start.width * 0.5;
    let center_y = start.top + start.height * 0.5;
    let max_width = match anchor {
        OverlayAnchor::TopLeft | OverlayAnchor::BottomLeft => 1.0 - start.left,
        OverlayAnchor::TopRight | OverlayAnchor::BottomRight => start.right(),
        OverlayAnchor::Center => center_x.min(1.0 - center_x) * 2.0,
    }
    .max(OverlaySpec::MIN_WIDTH);
    let desired_width = match anchor {
        OverlayAnchor::TopLeft | OverlayAnchor::BottomLeft => px - start.left,
        OverlayAnchor::TopRight | OverlayAnchor::BottomRight => start.right() - px,
        OverlayAnchor::Center => (px - center_x).abs() * 2.0,
    }
    .clamp(OverlaySpec::MIN_WIDTH, max_width);
    // Height in normalized units for a photo-width fraction `desired_width`.
    let height_for_width = desired_width * photo_w / photo_h / aspect;
    let max_height = match anchor {
        OverlayAnchor::TopLeft | OverlayAnchor::TopRight => 1.0 - start.top,
        OverlayAnchor::BottomLeft | OverlayAnchor::BottomRight => start.bottom(),
        OverlayAnchor::Center => center_y.min(1.0 - center_y) * 2.0,
    };
    let height = height_for_width.min(max_height).max(OverlaySpec::MIN_WIDTH);
    // Re-derive width so a height-clamped size keeps the same pixel aspect.
    let width = (height * photo_h * aspect / photo_w).clamp(OverlaySpec::MIN_WIDTH, max_width);
    let (left, top) = match anchor {
        OverlayAnchor::TopLeft => (start.left, start.top),
        OverlayAnchor::TopRight => (start.right() - width, start.top),
        OverlayAnchor::BottomLeft => (start.left, start.bottom() - height),
        OverlayAnchor::BottomRight => (start.right() - width, start.bottom() - height),
        OverlayAnchor::Center => (center_x - width * 0.5, center_y - height * 0.5),
    };
    super::model::NormRect {
        left,
        top,
        width,
        height,
    }
}

/// Translate `start` by the pointer delta captured at drag begin.
fn move_rect(
    start: super::model::NormRect,
    start_norm: (f64, f64),
    pointer_norm: (f64, f64),
) -> super::model::NormRect {
    let dx = (pointer_norm.0 - start_norm.0) as f32;
    let dy = (pointer_norm.1 - start_norm.1) as f32;
    super::model::NormRect {
        left: start.left + dx,
        top: start.top + dy,
        width: start.width,
        height: start.height,
    }
    .clamp_inside()
}

/// Blit an RGBA overlay onto a cairo context, scaled to the destination box
/// with bilinear filtering and multiplied by `opacity`. Cairo's ARGB32 format
/// is native-endian premultiplied BGRA, so the conversion happens here.
fn blit_rgba_onto(
    context: &gtk::cairo::Context,
    image: &image::RgbaImage,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    opacity: f64,
) {
    if w < 0.5 || h < 0.5 || opacity <= 0.01 || image.width() == 0 || image.height() == 0 {
        return;
    }
    let target_w = w.round().max(1.0) as u32;
    let target_h = h.round().max(1.0) as u32;
    let scaled = if image.width() == target_w && image.height() == target_h {
        image.clone()
    } else {
        image::imageops::resize(
            image,
            target_w,
            target_h,
            image::imageops::FilterType::Triangle,
        )
    };
    let Ok(mut surface) = gtk::cairo::ImageSurface::create(
        gtk::cairo::Format::ARgb32,
        target_w as i32,
        target_h as i32,
    ) else {
        return;
    };
    {
        let Ok(mut data) = surface.data() else {
            return;
        };
        for (source, destination) in scaled.pixels().zip(data.chunks_mut(4)) {
            let alpha = source[3];
            let inverse = alpha as f64 / 255.0;
            destination[0] = (source[2] as f64 * inverse).round() as u8;
            destination[1] = (source[1] as f64 * inverse).round() as u8;
            destination[2] = (source[0] as f64 * inverse).round() as u8;
            destination[3] = alpha;
        }
    }
    if context.save().is_err() {
        return;
    }
    let _ = context.translate(x, y);
    let _ = context.scale(w / target_w as f64, h / target_h as f64);
    let _ = context.set_source_surface(&surface, 0.0, 0.0);
    let _ = context.paint_with_alpha(opacity.clamp(0.0, 1.0));
    let _ = context.restore();
}

fn asset_aspect(hash: &str) -> Option<f32> {
    let image = super::overlays::load_asset(hash)?;
    if image.height() == 0 {
        return None;
    }
    Some(image.width() as f32 / image.height() as f32)
}

fn overlay_screen_rect(
    display: (f64, f64, f64, f64),
    photo_w: f32,
    photo_h: f32,
    overlay: &OverlaySpec,
) -> Option<(f64, f64, f64, f64)> {
    let aspect = asset_aspect(&overlay.asset)?;
    let rect = overlay.rect(photo_w, photo_h, aspect);
    let (x, y, w, h) = display;
    Some((
        x + rect.left as f64 * w,
        y + rect.top as f64 * h,
        rect.width as f64 * w,
        rect.height as f64 * h,
    ))
}

fn overlay_norm_rect(
    overlay: &OverlaySpec,
    photo_w: f32,
    photo_h: f32,
) -> Option<super::model::NormRect> {
    let aspect = asset_aspect(&overlay.asset)?;
    Some(overlay.rect(photo_w, photo_h, aspect))
}

fn text_norm_rect(
    layer: &super::model::TextLayerSpec,
    photo_w: f32,
    photo_h: f32,
) -> Option<super::model::NormRect> {
    super::text_render::text_rect(layer, photo_w as f64, photo_h as f64)
}

fn text_screen_rect(
    display: (f64, f64, f64, f64),
    photo_w: f32,
    photo_h: f32,
    layer: &super::model::TextLayerSpec,
) -> Option<(f64, f64, f64, f64)> {
    let rect = text_norm_rect(layer, photo_w, photo_h)?;
    let (x, y, w, h) = display;
    Some((
        x + rect.left as f64 * w,
        y + rect.top as f64 * h,
        rect.width as f64 * w,
        rect.height as f64 * h,
    ))
}

fn draw_selection_chrome(
    context: &gtk::cairo::Context,
    display: (f64, f64, f64, f64),
    rect: super::model::NormRect,
    anchor: OverlayAnchor,
) {
    let (dx, dy, dw, dh) = display;
    let x = dx + rect.left as f64 * dw;
    let y = dy + rect.top as f64 * dh;
    let w = rect.width as f64 * dw;
    let h = rect.height as f64 * dh;
    if context.save().is_err() {
        return;
    }
    let _ = context.set_source_rgba(0.25, 0.62, 1.0, 0.95);
    let _ = context.set_line_width(2.0);
    let _ = context.rectangle(x, y, w, h);
    let _ = context.stroke();
    let handle = OVERLAY_HANDLE_RADIUS;
    let (handle_norm_x, handle_norm_y) = resize_handle_point(rect, anchor);
    let handle_x = dx + handle_norm_x as f64 * dw - handle;
    let handle_y = dy + handle_norm_y as f64 * dh - handle;
    let _ = context.set_source_rgba(1.0, 1.0, 1.0, 0.95);
    let _ = context.rectangle(handle_x, handle_y, handle * 2.0, handle * 2.0);
    let _ = context.fill();
    let _ = context.set_source_rgba(0.25, 0.62, 1.0, 1.0);
    let _ = context.set_line_width(1.5);
    let _ = context.rectangle(handle_x, handle_y, handle * 2.0, handle * 2.0);
    let _ = context.stroke();
    let _ = context.restore();
}

fn build_position_row(
    parent: &gtk::Box,
) -> (Vec<(OverlayAnchor, gtk::ToggleButton)>, gtk::ToggleButton) {
    let anchor_row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    anchor_row.add_css_class("linked");
    anchor_row.add_css_class("overlay-anchor-row");
    let mut anchor_buttons = Vec::new();
    let first_anchor = gtk::ToggleButton::with_label(OverlayAnchor::TopLeft.label());
    first_anchor.set_active(true);
    first_anchor.set_hexpand(true);
    anchor_row.append(&first_anchor);
    anchor_buttons.push((OverlayAnchor::TopLeft, first_anchor.clone()));
    for anchor in [
        OverlayAnchor::TopRight,
        OverlayAnchor::Center,
        OverlayAnchor::BottomLeft,
        OverlayAnchor::BottomRight,
    ] {
        let button = gtk::ToggleButton::with_label(anchor.label());
        button.set_group(Some(&first_anchor));
        button.set_hexpand(true);
        anchor_row.append(&button);
        anchor_buttons.push((anchor, button));
    }
    parent.append(&anchor_row);
    (anchor_buttons, first_anchor)
}

/// Wire up drawing, hit-testing and gestures on the overlay layer. The layer
/// only becomes event-targetable when the Overlays tab is active and the
/// photo actually has overlays; otherwise it draws (or idles) and pointer
/// events fall through to the scrollable photo beneath.
#[allow(clippy::too_many_arguments)]
fn configure_overlay_canvas(
    layer: &gtk::DrawingArea,
    session: Rc<RefCell<EditSession>>,
    selected: Rc<Cell<Option<usize>>>,
    selected_text: Rc<Cell<Option<usize>>>,
    preview_dimensions: Rc<Cell<(i32, i32)>>,
    canvas_zoom: Rc<Cell<f64>>,
    native_one_to_one: Rc<Cell<bool>>,
    picture_scroll: gtk::ScrolledWindow,
    overlays_toggle: gtk::ToggleButton,
    text_toggle: gtk::ToggleButton,
    zoom_in_action: Rc<dyn Fn()>,
    zoom_out_action: Rc<dyn Fn()>,
    update_history_buttons: Rc<dyn Fn()>,
    sync_overlays_panel: Rc<dyn Fn()>,
    sync_text_panel: Rc<dyn Fn()>,
    update_canvas_input: Rc<dyn Fn()>,
) {
    layer.set_hexpand(true);
    layer.set_vexpand(true);
    layer.set_can_target(false);

    let display_rect_from_params = {
        let preview_dimensions = preview_dimensions.clone();
        let canvas_zoom = canvas_zoom.clone();
        let native_one_to_one = native_one_to_one.clone();
        let picture_scroll = picture_scroll.clone();
        move |viewport_w: f64,
              viewport_h: f64,
              h_scroll: f64,
              v_scroll: f64|
              -> (f64, f64, f64, f64) {
            let (photo_w, photo_h) = preview_dimensions.get();
            let zoom = canvas_zoom.get();
            let native = native_one_to_one.get();
            let (content_w, content_h) = if native {
                (photo_w.max(1) as f64, photo_h.max(1) as f64)
            } else if zoom > 0.0 {
                canvas_size_for_zoom(&picture_scroll, (photo_w, photo_h), zoom)
            } else {
                (0.0, 0.0)
            };
            overlay_display_rect(
                viewport_w,
                viewport_h,
                photo_w.max(1) as f64,
                photo_h.max(1) as f64,
                zoom,
                native,
                content_w,
                content_h,
                h_scroll,
                v_scroll,
            )
        }
    };

    {
        let session = session.clone();
        let selected = selected.clone();
        let selected_text = selected_text.clone();
        let preview_dimensions = preview_dimensions.clone();
        let overlays_toggle = overlays_toggle.clone();
        let text_toggle = text_toggle.clone();
        let picture_scroll = picture_scroll.clone();
        let display_rect_from_params = display_rect_from_params.clone();
        layer.set_draw_func(move |_, context, width, height| {
            let hadj = picture_scroll.hadjustment().value();
            let vadj = picture_scroll.vadjustment().value();
            let display =
                display_rect_from_params(width.max(1) as f64, height.max(1) as f64, hadj, vadj);
            let (photo_w_f, photo_h_f) = preview_dimensions.get();
            let (photo_w, photo_h) = (photo_w_f.max(1) as f32, photo_h_f.max(1) as f32);
            let (dx, dy, dw, dh) = display;
            if dw <= 0.0 || dh <= 0.0 {
                return;
            }
            // Keep bitmap paints inside the photo's letterbox; the selection
            // chrome is drawn after the clip is released so handles remain
            // visible when the overlay hugs an edge.
            if context.save().is_err() {
                return;
            }
            let _ = context.rectangle(dx, dy, dw, dh);
            let _ = context.clip();
            {
                let recipe = session.borrow();
                for overlay in &recipe.recipe.overlays {
                    if !overlay.visible {
                        continue;
                    }
                    let Some(aspect) = asset_aspect(&overlay.asset) else {
                        continue;
                    };
                    let Some(image) = super::overlays::load_asset(&overlay.asset) else {
                        continue;
                    };
                    let rect = overlay.rect(photo_w, photo_h, aspect);
                    let x = dx + rect.left as f64 * dw;
                    let y = dy + rect.top as f64 * dh;
                    let w = rect.width as f64 * dw;
                    let h = rect.height as f64 * dh;
                    blit_rgba_onto(context, &image, x, y, w, h, overlay.opacity as f64);
                }
                for text_layer in &recipe.recipe.text_layers {
                    if !text_layer.visible {
                        continue;
                    }
                    let Some(raster) = super::text_render::render_text_rgba(
                        text_layer,
                        f64::from(photo_w),
                        f64::from(photo_h),
                    ) else {
                        continue;
                    };
                    let rect = text_layer.rect_with_size(
                        raster.width() as f32 / photo_w,
                        raster.height() as f32 / photo_h,
                    );
                    let x = dx + rect.left as f64 * dw;
                    let y = dy + rect.top as f64 * dh;
                    let w = rect.width as f64 * dw;
                    let h = rect.height as f64 * dh;
                    blit_rgba_onto(context, &raster, x, y, w, h, text_layer.opacity as f64);
                }
            }
            let _ = context.restore();

            if overlays_toggle.is_active() {
                let recipe = session.borrow();
                let Some(rect) = selected
                    .get()
                    .and_then(|index| recipe.recipe.overlays.get(index))
                    .filter(|overlay| overlay.visible)
                    .and_then(|overlay| {
                        overlay_norm_rect(overlay, photo_w, photo_h)
                            .map(|rect| (rect, overlay.anchor))
                    })
                else {
                    return;
                };
                draw_selection_chrome(context, display, rect.0, rect.1);
            } else if text_toggle.is_active() {
                let recipe = session.borrow();
                let Some(rect) = selected_text
                    .get()
                    .and_then(|index| recipe.recipe.text_layers.get(index))
                    .filter(|layer| layer.visible)
                    .and_then(|layer| {
                        text_norm_rect(layer, photo_w, photo_h).map(|rect| (rect, layer.anchor))
                    })
                else {
                    return;
                };
                draw_selection_chrome(context, display, rect.0, rect.1);
            }
        });
    }

    // Redraw whenever the scroll position or zoom changes so handles stay
    // glued to the photo while panning underneath the layer.
    {
        let layer = layer.clone();
        let hadj = picture_scroll.hadjustment();
        hadj.connect_value_changed(move |_| layer.queue_draw());
    }
    {
        let layer = layer.clone();
        let vadj = picture_scroll.vadjustment();
        vadj.connect_value_changed(move |_| layer.queue_draw());
    }

    let drag_state: Rc<RefCell<OverlayDragState>> =
        Rc::new(RefCell::new(OverlayDragState::default()));
    let drag = gtk::GestureDrag::new();
    drag.set_button(1);

    {
        let session = session.clone();
        let selected = selected.clone();
        let selected_text = selected_text.clone();
        let preview_dimensions = preview_dimensions.clone();
        let picture_scroll = picture_scroll.clone();
        let drag_state = drag_state.clone();
        let display_rect_from_params = display_rect_from_params.clone();
        let sync_overlays_panel = sync_overlays_panel.clone();
        let sync_text_panel = sync_text_panel.clone();
        let update_canvas_input = update_canvas_input.clone();
        let overlays_toggle = overlays_toggle.clone();
        let text_toggle = text_toggle.clone();
        let layer = layer.clone();
        drag.connect_drag_begin(move |_, start_x, start_y| {
            let widget_width = layer.width().max(1) as f64;
            let widget_height = layer.height().max(1) as f64;
            let hadj = picture_scroll.hadjustment().value();
            let vadj = picture_scroll.vadjustment().value();
            let display = display_rect_from_params(widget_width, widget_height, hadj, vadj);
            let (photo_w_f, photo_h_f) = preview_dimensions.get();
            let (photo_w, photo_h) = (photo_w_f.max(1) as f32, photo_h_f.max(1) as f32);
            let start_norm = screen_to_normalized(display, start_x, start_y);

            let recipe = session.borrow();
            let overlays = &recipe.recipe.overlays;
            let texts = &recipe.recipe.text_layers;
            let selected_index = selected.get();
            let selected_text_index = selected_text.get();

            // Prefer the resize handle of the selected text layer while the
            // Text tab owns the canvas.
            if text_toggle.is_active() {
                if let Some(index) = selected_text_index {
                    if let Some(text) = texts.get(index).filter(|text| text.visible) {
                        if let Some(rect) = text_norm_rect(text, photo_w, photo_h) {
                            if hits_resize_handle(
                                display,
                                rect,
                                start_x,
                                start_y,
                                OVERLAY_HANDLE_RADIUS,
                                text.anchor,
                            ) {
                                let anchor = text.anchor;
                                let start_size = text.size;
                                drop(recipe);
                                drag_state.replace(OverlayDragState {
                                    mode: OverlayDragMode::Resize,
                                    start_norm,
                                    start_rect: rect,
                                    layer: LayerSelection::Text(index),
                                    start_size,
                                    anchor,
                                    pan_origin_h: 0.0,
                                    pan_origin_v: 0.0,
                                });
                                session.borrow_mut().begin_action();
                                return;
                            }
                        }
                    }
                }
            }

            // Next: the resize handle of the selected image overlay while the
            // Overlays tab owns the canvas.
            if overlays_toggle.is_active() {
                if let Some(index) = selected_index {
                    if let Some(overlay) = overlays.get(index).filter(|overlay| overlay.visible) {
                        if let Some(rect) = overlay_norm_rect(overlay, photo_w, photo_h) {
                            if hits_resize_handle(
                                display,
                                rect,
                                start_x,
                                start_y,
                                OVERLAY_HANDLE_RADIUS,
                                overlay.anchor,
                            ) {
                                let anchor = overlay.anchor;
                                drop(recipe);
                                drag_state.replace(OverlayDragState {
                                    mode: OverlayDragMode::Resize,
                                    start_norm,
                                    start_rect: rect,
                                    layer: LayerSelection::Image(index),
                                    start_size: 0.0,
                                    anchor,
                                    pan_origin_h: 0.0,
                                    pan_origin_v: 0.0,
                                });
                                session.borrow_mut().begin_action();
                                return;
                            }
                        }
                    }
                }
            }

            // Hit-test topmost text first, then topmost image overlay.
            let mut text_hit: Option<usize> = None;
            for (index, text) in texts.iter().enumerate().rev() {
                if !text.visible {
                    continue;
                }
                let Some((x, y, w, h)) = text_screen_rect(display, photo_w, photo_h, text) else {
                    continue;
                };
                if start_x >= x && start_x <= x + w && start_y >= y && start_y <= y + h {
                    text_hit = Some(index);
                    break;
                }
            }
            let mut hit: Option<usize> = None;
            for (index, overlay) in overlays.iter().enumerate().rev() {
                if !overlay.visible {
                    continue;
                }
                let Some((x, y, w, h)) = overlay_screen_rect(display, photo_w, photo_h, overlay)
                else {
                    continue;
                };
                if start_x >= x && start_x <= x + w && start_y >= y && start_y <= y + h {
                    hit = Some(index);
                    break;
                }
            }
            drop(recipe);

            match text_hit {
                Some(index) => {
                    selected_text.set(Some(index));
                    text_toggle.set_active(true);
                    let (start_rect, anchor, start_size) = {
                        let recipe = session.borrow();
                        let text = recipe.recipe.text_layers.get(index);
                        let rect = text
                            .and_then(|text| text_norm_rect(text, photo_w, photo_h))
                            .unwrap_or_default_for_test();
                        (
                            rect,
                            text.map(|text| text.anchor)
                                .unwrap_or(OverlayAnchor::Center),
                            text.map(|text| text.size).unwrap_or(0.0),
                        )
                    };
                    drag_state.replace(OverlayDragState {
                        mode: OverlayDragMode::Move,
                        start_norm,
                        start_rect,
                        layer: LayerSelection::Text(index),
                        start_size,
                        anchor,
                        pan_origin_h: 0.0,
                        pan_origin_v: 0.0,
                    });
                    session.borrow_mut().begin_action();
                    sync_text_panel();
                    update_canvas_input();
                    layer.queue_draw();
                    return;
                }
                None => {}
            }

            match hit {
                Some(index) => {
                    selected.set(Some(index));
                    overlays_toggle.set_active(true);
                    let (start_rect, anchor) = {
                        let recipe = session.borrow();
                        let overlay = recipe.recipe.overlays.get(index);
                        let rect = overlay
                            .and_then(|overlay| overlay_norm_rect(overlay, photo_w, photo_h))
                            .unwrap_or_default_for_test();
                        (
                            rect,
                            overlay
                                .map(|overlay| overlay.anchor)
                                .unwrap_or(OverlayAnchor::Center),
                        )
                    };
                    drag_state.replace(OverlayDragState {
                        mode: OverlayDragMode::Move,
                        start_norm,
                        start_rect,
                        layer: LayerSelection::Image(index),
                        start_size: 0.0,
                        anchor,
                        pan_origin_h: 0.0,
                        pan_origin_v: 0.0,
                    });
                    session.borrow_mut().begin_action();
                    sync_overlays_panel();
                    update_canvas_input();
                    layer.queue_draw();
                }
                None => {
                    drag_state.replace(OverlayDragState {
                        mode: OverlayDragMode::Pan,
                        start_norm: (0.0, 0.0),
                        start_rect: super::model::NormRect {
                            left: 0.0,
                            top: 0.0,
                            width: 0.0,
                            height: 0.0,
                        },
                        layer: LayerSelection::Image(0),
                        start_size: 0.0,
                        anchor: OverlayAnchor::Center,
                        pan_origin_h: hadj,
                        pan_origin_v: vadj,
                    });
                }
            }
        });
    }

    {
        let session = session.clone();
        let preview_dimensions = preview_dimensions.clone();
        let picture_scroll = picture_scroll.clone();
        let drag_state = drag_state.clone();
        let display_rect_from_params = display_rect_from_params.clone();
        let layer = layer.clone();
        let layer_for_update = layer.clone();
        drag.connect_drag_update(move |_, dx, dy| {
            let state = *drag_state.borrow();
            match state.mode {
                OverlayDragMode::None => {}
                OverlayDragMode::Pan => {
                    let hadj = picture_scroll.hadjustment();
                    let vadj = picture_scroll.vadjustment();
                    let max_h = (hadj.upper() - hadj.page_size()).max(hadj.lower());
                    let max_v = (vadj.upper() - vadj.page_size()).max(vadj.lower());
                    hadj.set_value((state.pan_origin_h - dx).clamp(hadj.lower(), max_h));
                    vadj.set_value((state.pan_origin_v - dy).clamp(vadj.lower(), max_v));
                }
                OverlayDragMode::Move | OverlayDragMode::Resize => {
                    let widget_width = layer_for_update.width().max(1) as f64;
                    let widget_height = layer_for_update.height().max(1) as f64;
                    let hadj = picture_scroll.hadjustment().value();
                    let vadj = picture_scroll.vadjustment().value();
                    let display = display_rect_from_params(widget_width, widget_height, hadj, vadj);
                    // Gesture offsets are relative to drag begin; rebuild the
                    // absolute pointer from the recorded origin.
                    let pointer_screen = (
                        display.0 + state.start_norm.0 * display.2 + dx,
                        display.1 + state.start_norm.1 * display.3 + dy,
                    );
                    let pointer_norm =
                        screen_to_normalized(display, pointer_screen.0, pointer_screen.1);
                    let (photo_w_f, photo_h_f) = preview_dimensions.get();
                    let (photo_w, photo_h) = (photo_w_f.max(1) as f32, photo_h_f.max(1) as f32);
                    let mut session = session.borrow_mut();
                    match state.layer {
                        LayerSelection::Image(index) => {
                            let aspect = session
                                .recipe
                                .overlays
                                .get(index)
                                .map(|overlay| asset_aspect(&overlay.asset).unwrap_or(1.0))
                                .unwrap_or(1.0);
                            session.mutate_active(|recipe| {
                                let Some(overlay) = recipe.overlays.get_mut(index) else {
                                    return;
                                };
                                let rect = if state.mode == OverlayDragMode::Resize {
                                    resize_rect_from_anchor(
                                        state.start_rect,
                                        pointer_norm,
                                        photo_w,
                                        photo_h,
                                        aspect,
                                        state.anchor,
                                    )
                                } else {
                                    move_rect(state.start_rect, state.start_norm, pointer_norm)
                                };
                                overlay.set_rect(rect, photo_w, photo_h, aspect);
                            });
                        }
                        LayerSelection::Text(index) => {
                            session.mutate_active(|recipe| {
                                let Some(text) = recipe.text_layers.get_mut(index) else {
                                    return;
                                };
                                if state.mode == OverlayDragMode::Resize {
                                    let aspect = if state.start_rect.height > 1.0e-6 {
                                        state.start_rect.width * photo_w
                                            / (state.start_rect.height * photo_h)
                                    } else {
                                        1.0
                                    };
                                    let resized = resize_rect_from_anchor(
                                        state.start_rect,
                                        pointer_norm,
                                        photo_w,
                                        photo_h,
                                        aspect,
                                        state.anchor,
                                    );
                                    let scale = if state.start_rect.width > 1.0e-6 {
                                        f64::from(resized.width / state.start_rect.width)
                                    } else {
                                        1.0
                                    };
                                    text.size = (f64::from(state.start_size) * scale) as f32;
                                    text.size = text.size.clamp(
                                        super::model::TextLayerSpec::MIN_SIZE,
                                        super::model::TextLayerSpec::MAX_SIZE,
                                    );
                                    text.set_position_from_rect(resized);
                                } else {
                                    let moved =
                                        move_rect(state.start_rect, state.start_norm, pointer_norm);
                                    text.set_position_from_rect(moved);
                                }
                            });
                        }
                    }
                    drop(session);
                    layer.queue_draw();
                }
            }
        });
    }

    {
        let session = session.clone();
        let drag_state = drag_state.clone();
        let update_history_buttons = update_history_buttons.clone();
        let sync_overlays_panel = sync_overlays_panel.clone();
        let sync_text_panel = sync_text_panel.clone();
        let layer = layer.clone();
        drag.connect_drag_end(move |_, _, _| {
            let state = *drag_state.borrow();
            if matches!(state.mode, OverlayDragMode::Move | OverlayDragMode::Resize) {
                session.borrow_mut().end_action();
                update_history_buttons();
                match state.layer {
                    LayerSelection::Image(_) => sync_overlays_panel(),
                    LayerSelection::Text(_) => sync_text_panel(),
                }
                layer.queue_draw();
            }
            drag_state.replace(OverlayDragState::default());
        });
    }
    layer.add_controller(drag);

    // Wheel: Ctrl+wheel zooms through the same actions as the toolbar; plain
    // wheel pans the photo (the layer eats events only while targeted).
    let wheel = gtk::EventControllerScroll::new(
        gtk::EventControllerScrollFlags::VERTICAL | gtk::EventControllerScrollFlags::DISCRETE,
    );
    {
        let zoom_in_action = zoom_in_action.clone();
        let zoom_out_action = zoom_out_action.clone();
        let picture_scroll = picture_scroll.clone();
        wheel.connect_scroll(move |controller, _, dy| {
            if dy == 0.0 {
                return glib::Propagation::Proceed;
            }
            if controller
                .current_event_state()
                .contains(gtk::gdk::ModifierType::CONTROL_MASK)
            {
                if dy < 0.0 {
                    zoom_in_action();
                } else {
                    zoom_out_action();
                }
                return glib::Propagation::Stop;
            }
            let hadj = picture_scroll.hadjustment();
            let vadj = picture_scroll.vadjustment();
            let step = vadj.step_increment().max(24.0);
            let max_h = (hadj.upper() - hadj.page_size()).max(hadj.lower());
            let max_v = (vadj.upper() - vadj.page_size()).max(vadj.lower());
            hadj.set_value(hadj.value().clamp(hadj.lower(), max_h));
            vadj.set_value((vadj.value() + dy.signum() * step * 2.0).clamp(vadj.lower(), max_v));
            glib::Propagation::Stop
        });
    }
    layer.add_controller(wheel);
}

trait OptionalNormRectExt {
    fn unwrap_or_default_for_test(self) -> super::model::NormRect;
}

impl OptionalNormRectExt for Option<super::model::NormRect> {
    fn unwrap_or_default_for_test(self) -> super::model::NormRect {
        self.unwrap_or(super::model::NormRect {
            left: 0.0,
            top: 0.0,
            width: 0.0,
            height: 0.0,
        })
    }
}

/// Build the Overlays panel contents inside `parent` and return a sync
/// closure that refreshes the list, selection and controls from the session.
#[allow(clippy::too_many_arguments)]
fn build_overlays_panel(
    parent: &gtk::Box,
    session: Rc<RefCell<EditSession>>,
    connection: Rc<RefCell<Connection>>,
    parent_window: gtk::Window,
    selected: Rc<Cell<Option<usize>>>,
    syncing: Rc<Cell<bool>>,
    preview_dimensions: Rc<Cell<(i32, i32)>>,
    update_history_buttons: Rc<dyn Fn()>,
    redraw_canvas: Rc<dyn Fn()>,
    update_canvas_input: Rc<dyn Fn()>,
    overlays_toggle: gtk::ToggleButton,
) -> Rc<dyn Fn()> {
    let import = gtk::Button::with_label("Import Overlay Image…");
    import.set_hexpand(true);
    import.set_tooltip_text(Some(
        "Add a PNG or JPG logo, badge or banner on top of this photo",
    ));
    import.add_css_class("crop-apply-button");
    parent.append(&import);

    let empty_hint = gtk::Label::new(Some(
        "No overlays yet.\nImport a PNG or JPG, then drag it into place on the photo.",
    ));
    empty_hint.set_xalign(0.0);
    empty_hint.set_justify(gtk::Justification::Left);
    empty_hint.add_css_class("dim-label");
    empty_hint.set_margin_top(8);
    parent.append(&empty_hint);

    add_section_label(parent, "OVERLAYS");
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::Single);
    list.add_css_class("overlay-list");
    list.set_margin_bottom(6);
    parent.append(&list);

    let selected_section = gtk::Box::new(gtk::Orientation::Vertical, 0);
    selected_section.add_css_class("overlay-selected-section");
    parent.append(&selected_section);

    add_section_label(&selected_section, "SELECTED OVERLAY");
    let opacity_row = add_slider(&selected_section, "Opacity", 0.0, 1.0, 0.01, 2);

    add_section_label(&selected_section, "POSITION");
    let (anchor_buttons, first_anchor) = build_position_row(&selected_section);

    let fit_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    fit_row.set_margin_top(6);
    let fit_width = gtk::Button::with_label("Fit to Width");
    fit_width.set_hexpand(true);
    fit_width.set_tooltip_text(Some("Stretch this overlay across the full photo width"));
    fit_width.add_css_class("crop-reset-button");
    let fit_screen = gtk::Button::with_label("Fit to Screen");
    fit_screen.set_hexpand(true);
    fit_screen.set_tooltip_text(Some(
        "Scale this overlay down until it fits entirely inside the photo",
    ));
    fit_screen.add_css_class("crop-reset-button");
    fit_row.append(&fit_width);
    fit_row.append(&fit_screen);
    selected_section.append(&fit_row);

    let action_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    action_row.set_margin_top(6);
    let reset_placement = gtk::Button::with_label("Reset Placement");
    reset_placement.set_hexpand(true);
    reset_placement.set_tooltip_text(Some("Centre this overlay and restore full opacity"));
    reset_placement.add_css_class("crop-reset-button");
    let remove = gtk::Button::with_label("Remove Overlay");
    remove.set_hexpand(true);
    remove.set_tooltip_text(Some("Delete this overlay from the photo"));
    remove.add_css_class("crop-reset-button");
    action_row.append(&reset_placement);
    action_row.append(&remove);
    selected_section.append(&action_row);

    // Populate the list and control states from the recipe.
    let sync: Rc<dyn Fn()> = {
        let session = session.clone();
        let connection = connection.clone();
        let selected = selected.clone();
        let syncing = syncing.clone();
        let list = list.clone();
        let empty_hint = empty_hint.clone();
        let selected_section = selected_section.clone();
        let opacity_row = opacity_row.clone();
        let anchor_buttons = anchor_buttons.clone();
        let first_anchor = first_anchor.clone();
        let redraw_canvas = redraw_canvas.clone();
        let update_canvas_input = update_canvas_input.clone();
        Rc::new(move || {
            syncing.set(true);
            let overlays = session.borrow().recipe.overlays.clone();
            if selected.get().is_some_and(|index| index >= overlays.len()) {
                selected.set(if overlays.is_empty() { None } else { Some(0) });
            }
            if selected.get().is_none() && !overlays.is_empty() {
                selected.set(Some(0));
            }
            let active = selected.get();
            empty_hint.set_visible(overlays.is_empty());
            selected_section.set_visible(!overlays.is_empty());
            list.set_visible(!overlays.is_empty());

            while let Some(child) = list.first_child() {
                list.remove(&child);
            }
            let connection = connection.borrow();
            for (index, overlay) in overlays.iter().enumerate() {
                let row = gtk::ListBoxRow::new();
                let label = gtk::Label::new(None);
                label.set_xalign(0.0);
                label.set_hexpand(true);
                let name = crate::db::overlay_asset(&connection, &overlay.asset)
                    .ok()
                    .flatten()
                    .map(|asset| asset.original_name)
                    .unwrap_or_else(|| {
                        let short = overlay.asset.chars().take(8).collect::<String>();
                        format!("Overlay {short}")
                    });
                let visibility = if overlay.visible { "" } else { " · hidden" };
                label.set_text(&format!("{name}{visibility}"));
                row.set_child(Some(&label));
                list.append(&row);
                if Some(index) == active {
                    list.select_row(Some(&row));
                }
            }

            if let Some(overlay) = active.and_then(|index| overlays.get(index)) {
                opacity_row.set_value(overlay.opacity as f64);
                for (anchor, button) in &anchor_buttons {
                    button.set_active(*anchor == overlay.anchor);
                }
            } else {
                opacity_row.set_value(1.0);
                first_anchor.set_active(true);
            }
            syncing.set(false);
            redraw_canvas();
            update_canvas_input();
        })
    };

    // Row clicks change the selection.
    {
        let selected = selected.clone();
        let syncing = syncing.clone();
        let sync = sync.clone();
        list.connect_row_selected(move |_, row| {
            if syncing.get() {
                return;
            }
            let index = row.map(|row| row.index() as usize);
            if selected.get() == index {
                return;
            }
            selected.set(index);
            sync();
        });
    }

    // Opacity: coalesce continuous drags into one undo step; no preview
    // re-render is needed because the canvas paints opacity live.
    {
        let session = session.clone();
        let selected = selected.clone();
        let syncing = syncing.clone();
        let update_history_buttons = update_history_buttons.clone();
        let redraw_canvas = redraw_canvas.clone();
        let dragging = Rc::new(Cell::new(false));
        let press = gtk::GestureClick::new();
        press.set_button(1);
        press.set_propagation_phase(gtk::PropagationPhase::Capture);
        {
            let session = session.clone();
            let selected = selected.clone();
            let dragging = dragging.clone();
            press.connect_pressed(move |_, _, _, _| {
                if selected.get().is_some() {
                    dragging.set(true);
                    session.borrow_mut().begin_action();
                }
            });
        }
        {
            let session = session.clone();
            let dragging = dragging.clone();
            let update_history_buttons = update_history_buttons.clone();
            let redraw_canvas = redraw_canvas.clone();
            press.connect_released(move |_, _, _, _| {
                if dragging.replace(false) {
                    session.borrow_mut().end_action();
                    update_history_buttons();
                    redraw_canvas();
                }
            });
        }
        opacity_row.add_controller(press);
        opacity_row.connect_value_changed(move |scale| {
            if syncing.get() {
                return;
            }
            let Some(index) = selected.get() else {
                return;
            };
            let value = (scale.value() as f32).clamp(0.0, 1.0);
            let was_dragging = dragging.get();
            {
                let mut session = session.borrow_mut();
                if was_dragging {
                    session.mutate_active(move |recipe| {
                        if let Some(overlay) = recipe.overlays.get_mut(index) {
                            overlay.opacity = value;
                        }
                    });
                } else {
                    session.mutate(move |recipe| {
                        if let Some(overlay) = recipe.overlays.get_mut(index) {
                            overlay.opacity = value;
                        }
                    });
                }
            }
            if !was_dragging {
                update_history_buttons();
            }
            redraw_canvas();
        });
    }

    // Position toggles move the overlay to the named spot on the photo,
    // keeping its size and opacity.
    for (anchor, button) in &anchor_buttons {
        let anchor = *anchor;
        let session = session.clone();
        let selected = selected.clone();
        let syncing = syncing.clone();
        let update_history_buttons = update_history_buttons.clone();
        let redraw_canvas = redraw_canvas.clone();
        button.connect_toggled(move |button| {
            if syncing.get() || !button.is_active() {
                return;
            }
            let Some(index) = selected.get() else {
                return;
            };
            session.borrow_mut().mutate(move |recipe| {
                let Some(overlay) = recipe.overlays.get_mut(index) else {
                    return;
                };
                overlay.position_at(anchor);
            });
            update_history_buttons();
            redraw_canvas();
        });
    }

    {
        let session = session.clone();
        let selected = selected.clone();
        let update_history_buttons = update_history_buttons.clone();
        let sync = sync.clone();
        let preview_dimensions = preview_dimensions.clone();
        reset_placement.connect_clicked(move |_| {
            let Some(index) = selected.get() else {
                return;
            };
            let (photo_w, photo_h) = preview_dimensions.get();
            session.borrow_mut().mutate(move |recipe| {
                if let Some(overlay) = recipe.overlays.get_mut(index) {
                    overlay.reset_placement();
                    let aspect = asset_aspect(&overlay.asset).unwrap_or(1.0);
                    overlay.width = overlay.width.min(OverlaySpec::max_fitting_width(
                        photo_w.max(1) as f32,
                        photo_h.max(1) as f32,
                        aspect,
                    ));
                }
            });
            update_history_buttons();
            sync();
        });
    }
    {
        let session = session.clone();
        let selected = selected.clone();
        let update_history_buttons = update_history_buttons.clone();
        let sync = sync.clone();
        let preview_dimensions = preview_dimensions.clone();
        fit_width.connect_clicked(move |_| {
            let Some(index) = selected.get() else {
                return;
            };
            let (photo_w, photo_h) = preview_dimensions.get();
            session.borrow_mut().mutate(move |recipe| {
                if let Some(overlay) = recipe.overlays.get_mut(index) {
                    let aspect = asset_aspect(&overlay.asset).unwrap_or(1.0);
                    overlay.fit_to_width(photo_w.max(1) as f32, photo_h.max(1) as f32, aspect);
                }
            });
            update_history_buttons();
            sync();
        });
    }
    {
        let session = session.clone();
        let selected = selected.clone();
        let update_history_buttons = update_history_buttons.clone();
        let sync = sync.clone();
        let preview_dimensions = preview_dimensions.clone();
        fit_screen.connect_clicked(move |_| {
            let Some(index) = selected.get() else {
                return;
            };
            let (photo_w, photo_h) = preview_dimensions.get();
            session.borrow_mut().mutate(move |recipe| {
                if let Some(overlay) = recipe.overlays.get_mut(index) {
                    let aspect = asset_aspect(&overlay.asset).unwrap_or(1.0);
                    overlay.fit_to_screen(photo_w.max(1) as f32, photo_h.max(1) as f32, aspect);
                }
            });
            update_history_buttons();
            sync();
        });
    }
    {
        let session = session.clone();
        let selected = selected.clone();
        let update_history_buttons = update_history_buttons.clone();
        let sync = sync.clone();
        remove.connect_clicked(move |_| {
            let Some(index) = selected.get() else {
                return;
            };
            session.borrow_mut().mutate(move |recipe| {
                if index < recipe.overlays.len() {
                    recipe.overlays.remove(index);
                }
            });
            selected.set(None);
            update_history_buttons();
            sync();
        });
    }

    // Import: decode and hash off-thread, then store bytes + DB row and push
    // a centred overlay onto the recipe on the main thread.
    {
        let session = session.clone();
        let connection = connection.clone();
        let parent_window = parent_window.clone();
        let selected = selected.clone();
        let update_history_buttons = update_history_buttons.clone();
        let sync = sync.clone();
        let overlays_toggle = overlays_toggle.clone();
        let preview_dimensions = preview_dimensions.clone();
        import.connect_clicked(move |button| {
            let dialog = gtk::FileChooserNative::new(
                Some("Import Overlay Image"),
                Some(&parent_window),
                gtk::FileChooserAction::Open,
                Some("Import"),
                Some("Cancel"),
            );
            let filter = gtk::FileFilter::new();
            filter.set_name(Some("PNG and JPG images"));
            filter.add_pattern("*.png");
            filter.add_pattern("*.jpg");
            filter.add_pattern("*.jpeg");
            filter.add_pattern("*.PNG");
            filter.add_pattern("*.JPG");
            filter.add_pattern("*.JPEG");
            dialog.add_filter(&filter);

            let session = session.clone();
            let connection = connection.clone();
            let selected = selected.clone();
            let update_history_buttons = update_history_buttons.clone();
            let sync = sync.clone();
            let overlays_toggle = overlays_toggle.clone();
            let preview_dimensions = preview_dimensions.clone();
            let button = button.clone();
            dialog.connect_response(move |dialog, response| {
                if response != gtk::ResponseType::Accept {
                    dialog.destroy();
                    return;
                }
                let Some(path) = dialog.file().and_then(|file| file.path()) else {
                    dialog.destroy();
                    return;
                };
                dialog.destroy();
                let session = session.clone();
                let connection = connection.clone();
                let selected = selected.clone();
                let update_history_buttons = update_history_buttons.clone();
                let sync = sync.clone();
                let overlays_toggle = overlays_toggle.clone();
                let preview_dimensions = preview_dimensions.clone();
                let button = button.clone();
                // Decode + hash on a worker thread; only the PathBuf crosses
                // the thread boundary. GTK/session state stays on the main
                // thread and is resumed when the channel delivers.
                let (result_tx, result_rx) = std::sync::mpsc::channel();
                std::thread::spawn(move || {
                    let _ = result_tx.send(super::overlays::prepare_overlay_asset(&path));
                });
                glib::timeout_add_local(Duration::from_millis(20), move || {
                    let prepared = match result_rx.try_recv() {
                        Ok(prepared) => prepared,
                        Err(std::sync::mpsc::TryRecvError::Empty) => {
                            return glib::ControlFlow::Continue;
                        }
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                            show_overlay_error(
                                &button,
                                "Could not import overlay",
                                "The import worker stopped unexpectedly",
                            );
                            return glib::ControlFlow::Break;
                        }
                    };
                    let prepared = match prepared {
                        Ok(prepared) => prepared,
                        Err(error) => {
                            show_overlay_error(
                                &button,
                                "Could not import overlay",
                                &error.to_string(),
                            );
                            return glib::ControlFlow::Break;
                        }
                    };
                    {
                        let connection = connection.borrow();
                        if let Err(error) =
                            super::overlays::store_overlay_asset(&connection, &prepared)
                        {
                            show_overlay_error(
                                &button,
                                "Could not store overlay",
                                &error.to_string(),
                            );
                            return glib::ControlFlow::Break;
                        }
                    }
                    let hash = prepared.hash.clone();
                    let aspect = if prepared.height > 0 {
                        prepared.width as f32 / prepared.height as f32
                    } else {
                        1.0
                    };
                    let (photo_w, photo_h) = preview_dimensions.get();
                    session.borrow_mut().mutate(|recipe| {
                        let mut overlay = OverlaySpec::new_centered(hash.clone());
                        overlay.width = overlay.width.min(OverlaySpec::max_fitting_width(
                            photo_w.max(1) as f32,
                            photo_h.max(1) as f32,
                            aspect,
                        ));
                        recipe.overlays.push(overlay);
                        selected.set(Some(recipe.overlays.len() - 1));
                    });
                    update_history_buttons();
                    overlays_toggle.set_active(true);
                    sync();
                    glib::ControlFlow::Break
                });
            });
            dialog.show();
        });
    }

    // Initial paint.
    sync();
    sync
}

fn show_overlay_error(button: &gtk::Button, heading: &str, body: &str) {
    let dialog = libadwaita::AlertDialog::builder()
        .heading(heading)
        .body(body.to_string())
        .close_response("close")
        .build();
    dialog.add_response("close", "Close");
    if let Some(window) = button
        .root()
        .and_then(|root| root.downcast::<gtk::Window>().ok())
    {
        dialog.present(Some(&window));
    }
}

#[cfg(test)]
mod overlay_geometry_tests {
    use super::super::model::NormRect;
    use super::*;

    fn rect(left: f32, top: f32, width: f32, height: f32) -> NormRect {
        NormRect {
            left,
            top,
            width,
            height,
        }
    }

    #[test]
    fn fit_mode_letterboxes_the_photo_inside_the_viewport() {
        let display =
            overlay_display_rect(800.0, 600.0, 400.0, 400.0, 0.0, false, 0.0, 0.0, 0.0, 0.0);
        assert!((display.0 - 100.0).abs() < 1e-6);
        assert!((display.1 - 0.0).abs() < 1e-6);
        assert!((display.2 - 600.0).abs() < 1e-6);
        assert!((display.3 - 600.0).abs() < 1e-6);
    }

    #[test]
    fn zoomed_mode_subtracts_scroll_from_the_centred_origin() {
        let display = overlay_display_rect(
            500.0, 400.0, 1000.0, 800.0, 2.0, false, 1000.0, 800.0, 300.0, 200.0,
        );
        assert!((display.0 + 300.0).abs() < 1e-6);
        assert!((display.1 + 200.0).abs() < 1e-6);
        assert!((display.2 - 1000.0).abs() < 1e-6);
        assert!((display.3 - 800.0).abs() < 1e-6);
    }

    #[test]
    fn native_mode_uses_the_photo_pixels_as_content_size() {
        let display = overlay_display_rect(
            1200.0, 900.0, 800.0, 600.0, 0.0, true, 800.0, 600.0, 0.0, 0.0,
        );
        assert!((display.0 - 200.0).abs() < 1e-6);
        assert!((display.1 - 150.0).abs() < 1e-6);
        assert!((display.2 - 800.0).abs() < 1e-6);
    }

    #[test]
    fn screen_points_map_into_normalized_photo_coordinates() {
        let display = (100.0, 50.0, 400.0, 200.0);
        let (nx, ny) = screen_to_normalized(display, 300.0, 150.0);
        assert!((nx - 0.5).abs() < 1e-9);
        assert!((ny - 0.5).abs() < 1e-9);
        let (nx, ny) = screen_to_normalized(display, 0.0, 9999.0);
        assert_eq!(nx, 0.0);
        assert_eq!(ny, 1.0);
    }

    #[test]
    fn resize_handle_hit_test_uses_a_screen_space_radius() {
        let display = (0.0, 0.0, 400.0, 400.0);
        let overlay_rect = rect(0.5, 0.5, 0.25, 0.25);
        assert!(hits_resize_handle(
            display,
            overlay_rect,
            305.0,
            296.0,
            9.0,
            OverlayAnchor::TopLeft
        ));
        assert!(!hits_resize_handle(
            display,
            overlay_rect,
            320.0,
            300.0,
            9.0,
            OverlayAnchor::TopLeft
        ));
        assert!(hits_resize_handle(
            display,
            overlay_rect,
            204.0,
            197.0,
            9.0,
            OverlayAnchor::BottomRight
        ));
    }

    #[test]
    fn resize_keeps_the_top_left_corner_and_asset_aspect() {
        let start = rect(0.25, 0.25, 0.2, 0.1);
        let resized = resize_rect_from_anchor(
            start,
            (0.65, 0.9),
            800.0,
            600.0,
            1.0,
            OverlayAnchor::TopLeft,
        );
        assert!((resized.left - 0.25).abs() < 1e-6);
        assert!((resized.top - 0.25).abs() < 1e-6);
        let expected_height = resized.width * 800.0 / 600.0;
        assert!(
            (resized.height - expected_height).abs() < 1e-5,
            "height {} expected {}",
            resized.height,
            expected_height
        );
        assert!(resized.right() <= 1.0 + 1e-5);
        assert!(resized.bottom() <= 1.0 + 1e-5);
    }

    #[test]
    fn resize_clamps_to_the_photo_edge_without_moving_the_fixed_corner() {
        let start = rect(0.8, 0.8, 0.05, 0.05);
        let resized = resize_rect_from_anchor(
            start,
            (5.0, 5.0),
            1000.0,
            500.0,
            2.0,
            OverlayAnchor::TopLeft,
        );
        assert!((resized.left - 0.8).abs() < 1e-6);
        assert!((resized.top - 0.8).abs() < 1e-6);
        assert!(resized.right() <= 1.0 + 1e-5);
        assert!(resized.bottom() <= 1.0 + 1e-5);
        assert!(resized.width >= OverlaySpec::MIN_WIDTH - 1e-6);
    }

    #[test]
    fn resize_with_bottom_right_anchor_keeps_that_corner_fixed() {
        let start = rect(0.25, 0.25, 0.2, 0.2);
        let resized = resize_rect_from_anchor(
            start,
            (0.05, 0.1),
            800.0,
            600.0,
            1.0,
            OverlayAnchor::BottomRight,
        );
        assert!((resized.right() - start.right()).abs() < 1e-6);
        assert!((resized.bottom() - start.bottom()).abs() < 1e-6);
        let expected_height = resized.width * 800.0 / 600.0;
        assert!((resized.height - expected_height).abs() < 1e-5);
        assert!(resized.left >= -1e-6);
        assert!(resized.top >= -1e-6);
    }

    #[test]
    fn resize_with_center_anchor_keeps_the_centre_fixed() {
        let start = rect(0.4, 0.4, 0.2, 0.2);
        let resized = resize_rect_from_anchor(
            start,
            (0.9, 0.5),
            1000.0,
            1000.0,
            1.0,
            OverlayAnchor::Center,
        );
        let before = (
            start.left + start.width * 0.5,
            start.top + start.height * 0.5,
        );
        let after = (
            resized.left + resized.width * 0.5,
            resized.top + resized.height * 0.5,
        );
        assert!((after.0 - before.0).abs() < 1e-6);
        assert!((after.1 - before.1).abs() < 1e-6);
    }

    #[test]
    fn move_applies_the_pointer_delta_and_clamps_inside_the_photo() {
        let start = rect(0.5, 0.5, 0.3, 0.3);
        let moved = move_rect(start, (0.5, 0.5), (0.7, 0.6));
        assert!((moved.left - 0.7).abs() < 1e-6);
        assert!((moved.top - 0.6).abs() < 1e-6);
        assert!((moved.width - 0.3).abs() < 1e-6);

        let clamped = move_rect(start, (0.5, 0.5), (1.4, 0.5));
        assert!((clamped.right() - 1.0).abs() < 1e-6);
        assert!((clamped.width - 0.3).abs() < 1e-6);
    }

    #[test]
    fn drag_mode_defaults_are_inert() {
        let state = OverlayDragState::default();
        assert_eq!(state.mode, OverlayDragMode::None);
        assert_eq!(state.layer, LayerSelection::Image(0));
        assert_eq!(state.start_size, 0.0);
    }
}
