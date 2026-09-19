#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FilterTileEffect {
    Preset(FilterPreset),
    BlackWhite,
    Sepia,
}

impl FilterTileEffect {
    fn all() -> Vec<Self> {
        std::iter::once(Self::Preset(FilterPreset::None))
            .chain(FilterPreset::ALL.map(Self::Preset))
            .chain([Self::BlackWhite, Self::Sepia])
            .collect()
    }

    fn label(self) -> &'static str {
        match self {
            Self::Preset(FilterPreset::None) => "Original",
            Self::Preset(preset) => preset.label(),
            Self::BlackWhite => "B&W",
            Self::Sepia => "Sepia",
        }
    }

    fn recipe(self, base: &EditRecipe) -> EditRecipe {
        let mut recipe = base.clone();
        recipe.filter = FilterPreset::None;
        recipe.black_white = false;
        recipe.sepia = false;
        match self {
            Self::Preset(preset) => recipe.filter = preset,
            Self::BlackWhite => recipe.black_white = true,
            Self::Sepia => recipe.sepia = true,
        }
        recipe
    }
}

type FilterThumbnailPixels = (u32, u32, Vec<u8>);

fn render_filter_thumbnails(
    path: &str,
    rotation: i32,
    recipe: &EditRecipe,
) -> anyhow::Result<Vec<FilterThumbnailPixels>> {
    // Decode at twice the on-screen tile preview size. The widget's fixed
    // size_request (FILTER_TILE_PREVIEW_*) still owns the tile's natural size,
    // so the larger decode cannot inflate the grid; ContentFit::Cover then
    // minifies the texture ~2:1 with the GPU's mipmapped filtering, which is
    // far crisper than displaying a 1x texture that the FlowBox cell (163px)
    // would otherwise slightly upscale.
    let base = super::render::decode_base_for_viewer(
        path,
        rotation,
        (FILTER_TILE_PREVIEW_WIDTH.max(1) as u32) * FILTER_TILE_SUPERSAMPLE,
        (FILTER_TILE_PREVIEW_HEIGHT.max(1) as u32) * FILTER_TILE_SUPERSAMPLE,
    )?;
    Ok(FilterTileEffect::all()
        .into_iter()
        .map(|effect| {
            let image = super::render::apply_recipe(base.clone(), &effect.recipe(recipe));
            let image = cover_crop_to_tile_size(image);
            (image.width(), image.height(), image.into_raw())
        })
        .collect())
}

// Center-crop to the tile preview's aspect ratio and downscale to its exact
// size — the same aspect-fill result ContentFit::Cover produces on the GPU,
// but resampled in one averaging step from a supersampled source instead of
// being displayed from a 1x texture.
fn cover_crop_to_tile_size(image: image::RgbaImage) -> image::RgbaImage {
    use image::imageops::FilterType;

    let target_ratio = FILTER_TILE_PREVIEW_WIDTH as f64 / FILTER_TILE_PREVIEW_HEIGHT as f64;
    let ratio = image.width() as f64 / image.height() as f64;
    let cropped = if ratio > target_ratio {
        let new_width =
            ((image.height() as f64 * target_ratio).round() as u32).clamp(1, image.width());
        let x = (image.width() - new_width) / 2;
        image::imageops::crop_imm(&image, x, 0, new_width, image.height()).to_image()
    } else {
        let new_height =
            ((image.width() as f64 / target_ratio).round() as u32).clamp(1, image.height());
        let y = (image.height() - new_height) / 2;
        image::imageops::crop_imm(&image, 0, y, image.width(), new_height).to_image()
    };
    image::imageops::resize(
        &cropped,
        FILTER_TILE_PREVIEW_WIDTH.max(1) as u32,
        FILTER_TILE_PREVIEW_HEIGHT.max(1) as u32,
        FilterType::Triangle,
    )
}

#[derive(Clone)]
struct Controls {
    straighten: gtk::Scale,
    exposure: gtk::Scale,
    contrast: gtk::Scale,
    fill_light: gtk::Scale,
    highlights: gtk::Scale,
    shadows: gtk::Scale,
    temperature: gtk::Scale,
    saturation: gtk::Scale,
    sharpen: gtk::Scale,
    auto_contrast: gtk::ToggleButton,
    auto_color: gtk::ToggleButton,
    black_white: gtk::ToggleButton,
    sepia: gtk::ToggleButton,
    filters: Vec<(FilterPreset, gtk::ToggleButton)>,
}

impl Controls {
    fn sync(&self, recipe: &EditRecipe) {
        self.straighten.set_value(recipe.straighten as f64);
        self.exposure.set_value(recipe.exposure as f64);
        self.contrast.set_value(recipe.contrast as f64);
        self.fill_light.set_value(recipe.fill_light as f64);
        self.highlights.set_value(recipe.highlights as f64);
        self.shadows.set_value(recipe.shadows as f64);
        self.temperature.set_value(recipe.temperature as f64);
        self.saturation.set_value(recipe.saturation as f64);
        self.sharpen.set_value(recipe.sharpen as f64);
        self.auto_contrast.set_active(recipe.auto_contrast);
        self.auto_color.set_active(recipe.auto_color);
        self.black_white.set_active(recipe.black_white);
        self.sepia.set_active(recipe.sepia);
        for (preset, button) in &self.filters {
            button.set_active(recipe.filter == *preset);
        }
    }
}

fn add_section_label(parent: &gtk::Box, text: &str) {
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.set_margin_top(8);
    label.add_css_class("heading");
    label.add_css_class("edit-section-label");
    parent.append(&label);
}

const FILTER_TILE_WIDTH: i32 = 160;
// The preview frame paints a 2px border on each side, so the picture inside
// must minimum-fit the remaining width at the default 4:3 thumbnail ratio.
// Without a real minimum height the AspectFrame collapses to a 1px strip and
// the thumbnails appear missing until the async paintable happens to arrive.
const FILTER_TILE_FRAME_BORDER: i32 = 4;
const FILTER_TILE_PREVIEW_WIDTH: i32 = FILTER_TILE_WIDTH - FILTER_TILE_FRAME_BORDER;
const FILTER_TILE_PREVIEW_HEIGHT: i32 = FILTER_TILE_PREVIEW_WIDTH * 3 / 4;
// Tile previews decode at this multiple of their on-screen size and are then
// CPU-downscaled to the exact tile box; see render_filter_thumbnails.
const FILTER_TILE_SUPERSAMPLE: u32 = 2;

fn filter_tile(effect: FilterTileEffect) -> (gtk::ToggleButton, gtk::Picture) {
    let button = gtk::ToggleButton::new();
    button.add_css_class("filter-tile");
    button.set_tooltip_text(Some(effect.label()));
    // A fixed compact minimum width is intentional. It drives how many
    // homogeneous columns FlowBox can fit, but unlike the previous resize
    // callback this request never grows when the pane grows, so it cannot make
    // the inspector sticky after a resize.
    button.set_size_request(FILTER_TILE_WIDTH, -1);
    button.set_halign(gtk::Align::Fill);
    button.set_valign(gtk::Align::Start);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 5);
    content.set_hexpand(false);
    content.set_halign(gtk::Align::Fill);

    // The picture owns the 4:3 tile geometry directly. Wrapping it in an
    // AspectFrame would inflate the tile's minimum width (the frame enforces
    // its ratio on the minimum measurement), collapsing the grid back to a
    // single column at the default pane width. ContentFit::Cover already
    // letterbox-crops any thumbnail into the 4:3 preview box.
    let picture = gtk::Picture::new();
    picture.set_content_fit(gtk::ContentFit::Cover);
    picture.set_can_shrink(true);
    picture.set_hexpand(true);
    picture.set_vexpand(true);
    picture.set_halign(gtk::Align::Fill);
    picture.set_valign(gtk::Align::Fill);
    picture.set_size_request(FILTER_TILE_PREVIEW_WIDTH, FILTER_TILE_PREVIEW_HEIGHT);
    picture.add_css_class("filter-tile-preview");
    picture.set_overflow(gtk::Overflow::Hidden);

    let label = gtk::Label::new(Some(effect.label()));
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label.set_justify(gtk::Justification::Center);
    label.set_max_width_chars(18);
    label.add_css_class("filter-tile-label");
    content.append(&picture);
    content.append(&label);
    button.set_child(Some(&content));
    (button, picture)
}

fn add_slider(
    parent: &gtk::Box,
    text: &str,
    min: f64,
    max: f64,
    step: f64,
    digits: i32,
) -> gtk::Scale {
    let row = gtk::Box::new(gtk::Orientation::Vertical, 2);
    row.add_css_class("edit-adjustment-row");

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.set_hexpand(true);
    let value = gtk::Label::new(None);
    value.set_xalign(1.0);
    value.add_css_class("dim-label");
    value.add_css_class("edit-adjustment-value");
    header.append(&label);
    header.append(&value);

    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, min, max, step);
    scale.set_digits(digits);
    scale.set_draw_value(false);
    scale.set_hexpand(true);
    scale.add_css_class("edit-adjustment-scale");

    let precision = digits.max(0) as usize;
    value.set_text(&format!("{:.*}", precision, scale.value()));
    {
        let value = value.clone();
        scale.connect_value_changed(move |scale| {
            value.set_text(&format!("{:.*}", precision, scale.value()));
        });
    }

    row.append(&header);
    row.append(&scale);
    parent.append(&row);
    scale
}

fn connect_scale(
    scale: &gtk::Scale,
    session: Rc<RefCell<EditSession>>,
    syncing: Rc<Cell<bool>>,
    queue_preview: Rc<dyn Fn()>,
    update_history: Rc<dyn Fn()>,
    assign: impl Fn(&mut EditRecipe, f32) + 'static,
) {
    let dragging = Rc::new(Cell::new(false));
    let press = gtk::GestureClick::new();
    press.set_button(1);
    press.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let session = session.clone();
        let dragging = dragging.clone();
        press.connect_pressed(move |_, _, _, _| {
            dragging.set(true);
            session.borrow_mut().begin_action();
        });
    }
    {
        let session = session.clone();
        let dragging = dragging.clone();
        let update_history = update_history.clone();
        let queue_preview = queue_preview.clone();
        press.connect_released(move |_, _, _, _| {
            if dragging.replace(false) {
                let before = session.borrow().recipe.clone();
                session.borrow_mut().end_action();
                update_history();
                // The drag rendered draft frames; queue one full-resolution
                // settle frame, but only if the value actually changed.
                if session.borrow().recipe != before {
                    queue_preview();
                }
            }
        });
    }
    scale.add_controller(press);

    scale.connect_value_changed(move |scale| {
        if syncing.get() {
            return;
        }
        let value = scale.value() as f32;
        if dragging.get() {
            session
                .borrow_mut()
                .mutate_active(|recipe| assign(recipe, value));
        } else {
            // Keyboard steps and deliberate wheel steps remain normal single
            // undoable actions; only a continuous mouse drag is coalesced.
            session.borrow_mut().mutate(|recipe| assign(recipe, value));
            update_history();
        }
        queue_preview();
    });
}

fn configure_scale_scroll(scale: &gtk::Scale, tools_scroll: &gtk::ScrolledWindow) {
    // One mouse-wheel click over the slider track changes the value by this
    // much (wheel up increases, wheel down decreases). Fractional deltas from
    // smooth-scroll devices produce proportional micro-steps.
    const SLIDER_WHEEL_STEP: f64 = 0.05;
    let pointer_y = Rc::new(Cell::new(f64::NAN));
    let motion = gtk::EventControllerMotion::new();
    {
        let pointer_y = pointer_y.clone();
        motion.connect_motion(move |_, _, y| pointer_y.set(y));
    }
    {
        let pointer_y = pointer_y.clone();
        motion.connect_leave(move |_| pointer_y.set(f64::NAN));
    }
    scale.add_controller(motion);

    let controller = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
    controller.set_propagation_phase(gtk::PropagationPhase::Capture);
    let scale_for_scroll = scale.clone();
    let tools_scroll = tools_scroll.clone();
    controller.connect_scroll(move |_, _, dy| {
        // GTK Scale reacts to wheel input across its full allocation, including
        // value text and padding, and steps the adjustment by its own coarse
        // increments. Take over the narrow central track band and step the
        // value by SLIDER_WHEEL_STEP per click instead; everywhere else
        // scrolls the editing tools normally.
        let y = pointer_y.get();
        let center = scale_for_scroll.height() as f64 / 2.0;
        if y.is_finite() && (y - center).abs() <= 7.0 {
            if dy != 0.0 {
                let adjustment = scale_for_scroll.adjustment();
                // set_value clamps to [lower, upper]
                adjustment.set_value(adjustment.value() - dy * SLIDER_WHEEL_STEP);
            }
            return glib::Propagation::Stop;
        }

        if dy != 0.0 {
            let adjustment = tools_scroll.vadjustment();
            let max = (adjustment.upper() - adjustment.page_size()).max(adjustment.lower());
            let step = adjustment.step_increment().max(24.0);
            adjustment.set_value(
                (adjustment.value() + dy.signum() * step * 2.0).clamp(adjustment.lower(), max),
            );
        }
        glib::Propagation::Stop
    });
    scale.add_controller(controller);
}

fn connect_toggle(
    button: &gtk::ToggleButton,
    session: Rc<RefCell<EditSession>>,
    syncing: Rc<Cell<bool>>,
    queue_preview: Rc<dyn Fn()>,
    update_history: Rc<dyn Fn()>,
    assign: impl Fn(&mut EditRecipe, bool) + 'static,
) {
    button.connect_toggled(move |button| {
        if syncing.get() {
            return;
        }
        let value = button.is_active();
        session.borrow_mut().mutate(|recipe| assign(recipe, value));
        update_history();
        queue_preview();
    });
}
