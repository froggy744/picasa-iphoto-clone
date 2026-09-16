#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CropAspectPreset {
    Free,
    Original,
    Square,
    FourThree,
    ThreeTwo,
    SixteenNine,
    A4,
    UsLetter,
}

impl CropAspectPreset {
    fn label(self) -> &'static str {
        match self {
            Self::Free => "Free",
            Self::Original => "Original",
            Self::Square => "1:1",
            Self::FourThree => "4:3",
            Self::ThreeTwo => "3:2",
            Self::SixteenNine => "16:9",
            Self::A4 => "A4",
            Self::UsLetter => "US Letter",
        }
    }

    fn ratio(self, image_width: i32, image_height: i32, portrait: bool) -> Option<f64> {
        let ratio = match self {
            Self::Free => return None,
            Self::Original => {
                if image_width <= 0 || image_height <= 0 {
                    return None;
                }
                image_width as f64 / image_height as f64
            }
            Self::Square => 1.0,
            Self::FourThree => 4.0 / 3.0,
            Self::ThreeTwo => 3.0 / 2.0,
            Self::SixteenNine => 16.0 / 9.0,
            Self::A4 => 297.0 / 210.0,
            Self::UsLetter => 11.0 / 8.5,
        };
        Some(if portrait && ratio != 1.0 {
            1.0 / ratio
        } else {
            ratio
        })
    }
}

fn centered_crop_for_aspect(image_width: i32, image_height: i32, ratio: f64) -> CropRect {
    if image_width <= 0 || image_height <= 0 || !ratio.is_finite() || ratio <= 0.0 {
        return CropRect::default();
    }

    let source_ratio = image_width as f64 / image_height as f64;
    if (source_ratio - ratio).abs() < 0.0001 {
        return CropRect::default();
    }

    if source_ratio > ratio {
        let normalized_width = (ratio / source_ratio).clamp(0.0, 1.0);
        let margin = (1.0 - normalized_width) * 0.5;
        CropRect {
            left: margin as f32,
            top: 0.0,
            right: (1.0 - margin) as f32,
            bottom: 1.0,
        }
    } else {
        let normalized_height = (source_ratio / ratio).clamp(0.0, 1.0);
        let margin = (1.0 - normalized_height) * 0.5;
        CropRect {
            left: 0.0,
            top: margin as f32,
            right: 1.0,
            bottom: (1.0 - margin) as f32,
        }
    }
}

fn configure_crop_overlay(
    overlay: &gtk::DrawingArea,
    pending: Rc<RefCell<CropRect>>,
    preview_dimensions: Rc<Cell<(i32, i32)>>,
    active_aspect_ratio: Rc<Cell<Option<f64>>>,
) {
    let pending_for_draw = pending.clone();
    let preview_for_draw = preview_dimensions.clone();
    overlay.set_draw_func(move |_, context, width, height| {
        let (image_width, image_height) = preview_for_draw.get();
        let (x, y, w, h) = contained_rect(
            width as f64,
            height as f64,
            image_width as f64,
            image_height as f64,
        );
        let crop = pending_for_draw.borrow().normalized();
        let sx = x + crop.left as f64 * w;
        let sy = y + crop.top as f64 * h;
        let sw = (crop.right - crop.left) as f64 * w;
        let sh = (crop.bottom - crop.top) as f64 * h;

        context.set_source_rgba(0.0, 0.0, 0.0, 0.52);
        context.rectangle(x, y, w, (sy - y).max(0.0));
        context.rectangle(x, sy + sh, w, (y + h - sy - sh).max(0.0));
        context.rectangle(x, sy, (sx - x).max(0.0), sh);
        context.rectangle(sx + sw, sy, (x + w - sx - sw).max(0.0), sh);
        let _ = context.fill();
        context.set_source_rgba(1.0, 1.0, 1.0, 0.95);
        context.set_line_width(2.0);
        context.rectangle(sx, sy, sw, sh);
        let _ = context.stroke();
        context.set_line_width(1.0);
        context.set_source_rgba(1.0, 1.0, 1.0, 0.55);
        for third in [1.0 / 3.0, 2.0 / 3.0] {
            context.move_to(sx + sw * third, sy);
            context.line_to(sx + sw * third, sy + sh);
            context.move_to(sx, sy + sh * third);
            context.line_to(sx + sw, sy + sh * third);
        }
        let _ = context.stroke();
    });

    let drag = gtk::GestureDrag::new();
    drag.set_button(1);
    let origin = Rc::new(Cell::new((0.0f64, 0.0f64)));
    {
        let origin = origin.clone();
        drag.connect_drag_begin(move |_, x, y| origin.set((x, y)));
    }
    {
        let overlay = overlay.clone();
        let pending = pending.clone();
        let preview_dimensions = preview_dimensions.clone();
        let origin = origin.clone();
        drag.connect_drag_update(move |_, dx, dy| {
            let (ox, oy) = origin.get();
            let start = point_to_normalized(&overlay, preview_dimensions.get(), ox, oy);
            let current = point_to_normalized(&overlay, preview_dimensions.get(), ox + dx, oy + dy);
            let (Some((sx, sy)), Some((cx, cy))) = (start, current) else {
                return;
            };
            let (mut cx, mut cy) = (cx, cy);
            if let Some(target_ratio) = active_aspect_ratio.get() {
                let (image_width, image_height) = preview_dimensions.get();
                if image_width > 0 && image_height > 0 && target_ratio > 0.0 {
                    // In normalized coordinates the required width/height
                    // ratio must account for the displayed image aspect.
                    let normalized_ratio = target_ratio * image_height as f64 / image_width as f64;
                    let sign_x = if cx >= sx { 1.0 } else { -1.0 };
                    let sign_y = if cy >= sy { 1.0 } else { -1.0 };
                    let mut width = (cx - sx).abs();
                    let mut height = (cy - sy).abs();
                    if width > 0.0 && height > 0.0 && normalized_ratio > 0.0 {
                        if width / height > normalized_ratio {
                            width = height * normalized_ratio;
                        } else {
                            height = width / normalized_ratio;
                        }
                        cx = (sx + sign_x * width).clamp(0.0, 1.0);
                        cy = (sy + sign_y * height).clamp(0.0, 1.0);
                    }
                }
            }
            pending.replace(
                CropRect {
                    left: sx.min(cx) as f32,
                    top: sy.min(cy) as f32,
                    right: sx.max(cx) as f32,
                    bottom: sy.max(cy) as f32,
                }
                .normalized(),
            );
            overlay.queue_draw();
        });
    }
    overlay.add_controller(drag);
}

fn point_to_normalized(
    overlay: &gtk::DrawingArea,
    preview: (i32, i32),
    px: f64,
    py: f64,
) -> Option<(f64, f64)> {
    let (x, y, width, height) = contained_rect(
        overlay.width() as f64,
        overlay.height() as f64,
        preview.0 as f64,
        preview.1 as f64,
    );
    if width <= 0.0 || height <= 0.0 {
        return None;
    }
    Some((
        ((px - x) / width).clamp(0.0, 1.0),
        ((py - y) / height).clamp(0.0, 1.0),
    ))
}

fn contained_rect(
    container_w: f64,
    container_h: f64,
    image_w: f64,
    image_h: f64,
) -> (f64, f64, f64, f64) {
    if container_w <= 0.0 || container_h <= 0.0 || image_w <= 0.0 || image_h <= 0.0 {
        return (0.0, 0.0, container_w.max(0.0), container_h.max(0.0));
    }
    let scale = (container_w / image_w).min(container_h / image_h);
    let width = image_w * scale;
    let height = image_h * scale;
    (
        (container_w - width) * 0.5,
        (container_h - height) * 0.5,
        width,
        height,
    )
}
