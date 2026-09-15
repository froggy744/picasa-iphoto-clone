// ---------------------------------------------------------------------------
// Photo printing
//
// The bottom bar's print button renders the selected photo (library rotation,
// crops and filters applied) on a worker thread, shows an Adwaita photo
// options dialog (exact print size, shrink-to-fit vs crop-to-fill, 1-4 prints
// per page) and then pops the standard GTK print dialog. Choices persist
// across sessions via the settings table, together with the GTK
// printer/paper selections.
//
// This file is include!()d into the `window` module, so it shares the
// imports at the top of window.rs (gtk, adw, glib, db, Rc/RefCell, ...).
// ---------------------------------------------------------------------------

/// One point of the GTK print coordinate space equals 1/72 inch, which the
/// print backends map to the printer's real resolution. Sizes expressed in
/// these points therefore come out at their true physical size on paper.
const PT_PER_INCH: f64 = 72.0;
const PT_PER_MM: f64 = 72.0 / 25.4;

const PRINT_EXTRAS_KEY: &str = "print-extras";
const PRINT_CONFIG_KEY: &str = "print-config";
const PRINT_SETTINGS_GROUP: Option<&str> = Some("photo-settings");
const PRINT_PAGE_SETUP_GROUP: Option<&str> = Some("photo-page-setup");

/// A physical photo print size. `width_pt == 0.0` marks the "Full page"
/// pseudo-size, which fills the whole cell instead of a fixed paper size.
struct SizePreset {
    id: &'static str,
    label: &'static str,
    width_pt: f64,
    height_pt: f64,
}

const SIZE_PRESETS: &[SizePreset] = &[
    SizePreset {
        id: "full",
        label: "Full page",
        width_pt: 0.0,
        height_pt: 0.0,
    },
    SizePreset {
        id: "wallet",
        label: "Wallet (2.5 × 3.5 in)",
        width_pt: 2.5 * PT_PER_INCH,
        height_pt: 3.5 * PT_PER_INCH,
    },
    SizePreset {
        id: "3.5x5",
        label: "3.5 × 5 in",
        width_pt: 3.5 * PT_PER_INCH,
        height_pt: 5.0 * PT_PER_INCH,
    },
    SizePreset {
        id: "4x6",
        label: "4 × 6 in",
        width_pt: 4.0 * PT_PER_INCH,
        height_pt: 6.0 * PT_PER_INCH,
    },
    SizePreset {
        id: "5x7",
        label: "5 × 7 in",
        width_pt: 5.0 * PT_PER_INCH,
        height_pt: 7.0 * PT_PER_INCH,
    },
    SizePreset {
        id: "8x10",
        label: "8 × 10 in",
        width_pt: 8.0 * PT_PER_INCH,
        height_pt: 10.0 * PT_PER_INCH,
    },
    SizePreset {
        id: "9x13",
        label: "9 × 13 cm",
        width_pt: 90.0 * PT_PER_MM,
        height_pt: 130.0 * PT_PER_MM,
    },
    SizePreset {
        id: "10x15",
        label: "10 × 15 cm",
        width_pt: 100.0 * PT_PER_MM,
        height_pt: 150.0 * PT_PER_MM,
    },
    SizePreset {
        id: "13x18",
        label: "13 × 18 cm",
        width_pt: 130.0 * PT_PER_MM,
        height_pt: 180.0 * PT_PER_MM,
    },
];

/// The photo-specific options shown in the print dialog's custom tab.
#[derive(Clone, Debug, PartialEq)]
struct PrintExtras {
    size_id: String,
    /// false = shrink the whole photo into the target box (letterboxed),
    /// true = fill the box and centre-crop the overflow.
    crop_to_fill: bool,
    per_page: u32,
}

impl Default for PrintExtras {
    fn default() -> Self {
        Self {
            size_id: "full".to_string(),
            crop_to_fill: false,
            per_page: 1,
        }
    }
}

fn parse_extras(raw: Option<&str>) -> PrintExtras {
    let mut extras = PrintExtras::default();
    let Some(raw) = raw else {
        return extras;
    };
    for field in raw.split(';') {
        let Some((key, value)) = field.split_once('=') else {
            continue;
        };
        match key {
            "size" => {
                if SIZE_PRESETS.iter().any(|preset| preset.id == value) {
                    extras.size_id = value.to_string();
                }
            }
            "crop" => extras.crop_to_fill = value == "1",
            "per" => {
                if let Ok(count) = value.parse::<u32>() {
                    extras.per_page = count.clamp(1, 4);
                }
            }
            _ => {}
        }
    }
    extras
}

fn extras_to_string(extras: &PrintExtras) -> String {
    format!(
        "size={};crop={};per={}",
        extras.size_id,
        u8::from(extras.crop_to_fill),
        extras.per_page
    )
}

/// Column/row split for the requested prints-per-page. 2 and 3 prints sit
/// side by side so portrait paper yields portrait cells; 4 uses a 2 × 2 grid.
fn cell_grid(per_page: u32) -> (u32, u32) {
    match per_page {
        2 => (2, 1),
        3 => (3, 1),
        4 => (2, 2),
        _ => (1, 1),
    }
}

fn find_size_preset(id: &str) -> &'static SizePreset {
    SIZE_PRESETS
        .iter()
        .find(|preset| preset.id == id)
        .unwrap_or(&SIZE_PRESETS[0])
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub(crate) struct PhotoPrintRequest {
    pub reference: String,
    pub rotation: i32,
    pub edit_recipe: String,
    pub source_width: i64,
    pub source_height: i64,
    pub job_name: String,
}

/// Render the photo in the background, then show the photo options dialog
/// followed by the print dialog on the main thread.
pub(crate) fn print_photo<W: IsA<gtk::Window>>(
    parent: &W,
    connection: &Rc<RefCell<Connection>>,
    request: PhotoPrintRequest,
) {
    // GTK objects are not Send, so keep a thread-safe weak reference that the
    // worker thread can hand back to the main loop.
    let weak_parent = glib::SendWeakRef::<gtk::Window>::default();
    weak_parent.set(Some(parent.upcast_ref()));

    // Load persisted state up front (cheap SQLite reads on the main thread).
    // Only Send-safe plain data crosses back into the main-loop callback.
    let saved_config = db::setting(&connection.borrow(), PRINT_CONFIG_KEY)
        .ok()
        .flatten();
    let saved_extras = db::setting(&connection.borrow(), PRINT_EXTRAS_KEY)
        .ok()
        .flatten();
    let initial_extras = parse_extras(saved_extras.as_deref());
    let job_name = request.job_name;

    std::thread::spawn(move || {
        let rendered = crate::edit::render::render_for_export(
            &request.reference,
            request.rotation,
            &request.edit_recipe,
            request.source_width,
            request.source_height,
        )
        .map(downscale_for_print)
        .map(Box::new);

        glib::MainContext::default().invoke(move || match rendered {
            Ok(image) => run_print_dialog(
                weak_parent,
                initial_extras,
                Rc::new(*image),
                job_name,
                saved_config,
            ),
            Err(error) => eprintln!("Could not prepare photo for printing: {error:#}"),
        });
    });
}

/// Bound the page raster so a very large photo cannot exhaust memory while
/// the print dialog is open. 4000 px keeps a full-bleed A4 page above
/// 300 DPI print quality.
const PRINT_MAX_DIMENSION: u32 = 4000;

fn downscale_for_print(image: image::RgbaImage) -> image::RgbaImage {
    let largest = image.width().max(image.height());
    if largest == 0 || largest <= PRINT_MAX_DIMENSION {
        return image;
    }
    let factor = PRINT_MAX_DIMENSION as f64 / largest as f64;
    let width = ((image.width() as f64 * factor).round() as u32).max(1);
    let height = ((image.height() as f64 * factor).round() as u32).max(1);
    image::imageops::resize(
        &image,
        width,
        height,
        image::imageops::FilterType::Lanczos3,
    )
}

// ---------------------------------------------------------------------------
// Dialogs
//
// A standalone photo-options dialog runs BEFORE the print operation. Print
// dialog custom tabs (create-custom-widget) only exist in GTK's native Unix
// print dialog; Flatpak portals and the Windows/macOS native dialogs never
// show them, so the options live in our own Adwaita dialog instead.
// ---------------------------------------------------------------------------

fn run_print_dialog(
    weak_parent: glib::SendWeakRef<gtk::Window>,
    initial_extras: PrintExtras,
    image: Rc<image::RgbaImage>,
    job_name: String,
    saved_config: Option<String>,
) {
    let Some(parent) = weak_parent.upgrade() else {
        eprintln!("Could not print photo: the window is gone");
        return;
    };
    let extras = Rc::new(RefCell::new(initial_extras));

    let dialog = adw::AlertDialog::new(Some("Print photo"), Some(&job_name));
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("print", "Print");
    dialog.set_response_appearance("print", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("print"));
    dialog.set_close_response("cancel");
    dialog.set_extra_child(Some(&build_extras_widget(&extras)));

    let extras_for_response = extras.clone();
    let image_for_print = image;
    let job_name_for_print = job_name;
    let parent_for_print = weak_parent;
    let config_for_print = saved_config;
    dialog.choose(Some(&parent), None::<&gio::Cancellable>, move |response| {
        if response == "print" {
            run_print_operation(
                &parent_for_print,
                &extras_for_response,
                image_for_print,
                &job_name_for_print,
                config_for_print.as_deref(),
            );
        } else {
            // Keep whatever sizes the user previewed, even on cancel.
            persist_print_state(None, &extras_for_response.borrow());
        }
    });
}

fn run_print_operation(
    weak_parent: &glib::SendWeakRef<gtk::Window>,
    extras: &Rc<RefCell<PrintExtras>>,
    image: Rc<image::RgbaImage>,
    job_name: &str,
    saved_config: Option<&str>,
) {
    let operation = gtk::PrintOperation::new();
    operation.set_job_name(job_name);
    operation.set_n_pages(1);

    // Restore the printer, paper and orientation choices from the previous
    // print so the system dialog opens ready-to-go.
    if let Some(config) = saved_config {
        let key_file = glib::KeyFile::new();
        if key_file
            .load_from_data(config, glib::KeyFileFlags::NONE)
            .is_ok()
        {
            if let Ok(settings) =
                gtk::PrintSettings::from_key_file(&key_file, PRINT_SETTINGS_GROUP)
            {
                operation.set_print_settings(Some(&settings));
            }
            if let Ok(setup) = gtk::PageSetup::from_key_file(&key_file, PRINT_PAGE_SETUP_GROUP) {
                operation.set_default_page_setup(Some(&setup));
            }
        }
    }

    let extras_for_draw = extras.clone();
    let image_for_draw = image;
    operation.connect_draw_page(move |_operation, context, page_number| {
        if page_number == 0 {
            draw_print_page(context, &image_for_draw, &extras_for_draw.borrow());
        }
    });

    let response = operation.run(
        gtk::PrintOperationAction::PrintDialog,
        weak_parent.upgrade().as_ref(),
    );

    match response {
        // Apply/InProgress mean the user accepted the print dialog: persist
        // the printer, paper and orientation choices for the next time.
        Ok(result @ (gtk::PrintOperationResult::Apply
        | gtk::PrintOperationResult::InProgress)) => {
            let key_file = glib::KeyFile::new();
            if let Some(settings) = operation.print_settings() {
                settings.to_key_file(&key_file, PRINT_SETTINGS_GROUP);
            }
            operation
                .default_page_setup()
                .to_key_file(&key_file, PRINT_PAGE_SETUP_GROUP);
            persist_print_state(Some(&key_file), &extras.borrow());
            let _ = result;
        }
        Ok(_cancelled) => {
            persist_print_state(None, &extras.borrow());
        }
        Err(error) => eprintln!("Could not print photo: {error}"),
    }
}

/// Save the print state with a short-lived connection. The shared library
/// connection is intentionally not captured by the main-loop callbacks, so a
/// fresh open keeps non-Send handles out of the worker -> main handoff.
fn persist_print_state(config_key_file: Option<&glib::KeyFile>, extras: &PrintExtras) {
    let Ok(connection) = db::open_default() else {
        eprintln!("Could not open the photo library to save print options");
        return;
    };
    if let Some(key_file) = config_key_file {
        if let Err(error) = db::set_setting(&connection, PRINT_CONFIG_KEY, &key_file.to_data()) {
            eprintln!("Could not save print settings: {error}");
        }
    }
    if let Err(error) = db::set_setting(&connection, PRINT_EXTRAS_KEY, &extras_to_string(extras)) {
        eprintln!("Could not save print options: {error}");
    }
}

fn build_extras_widget(extras: &Rc<RefCell<PrintExtras>>) -> gtk::Widget {
    let grid = gtk::Grid::new();
    grid.set_row_spacing(10);
    grid.set_column_spacing(12);
    grid.set_margin_top(12);
    grid.set_margin_bottom(12);
    grid.set_margin_start(12);
    grid.set_margin_end(12);

    let size_label = gtk::Label::with_mnemonic("Print _size:");
    size_label.set_halign(gtk::Align::End);
    let size_model = gtk::StringList::new(
        &SIZE_PRESETS.iter().map(|preset| preset.label).collect::<Vec<_>>(),
    );
    let size_combo = gtk::DropDown::new(Some(size_model), None::<&gtk::Expression>);
    let active_index = SIZE_PRESETS
        .iter()
        .position(|preset| preset.id == extras.borrow().size_id)
        .unwrap_or(0);
    size_combo.set_selected(active_index as u32);
    size_label.set_mnemonic_widget(Some(&size_combo));
    let extras_for_size = extras.clone();
    size_combo.connect_selected_notify(move |dropdown| {
        // try_borrow_mut: the dialog may emit changed signals while the page
        // draw borrows `extras`; dropping the write then is harmless because
        // draw_page reads once per print.
        let index = usize::try_from(dropdown.selected()).unwrap_or(0);
        if let Some(preset) = SIZE_PRESETS.get(index) {
            if let Ok(mut current) = extras_for_size.try_borrow_mut() {
                current.size_id = preset.id.to_string();
            }
        }
    });

    let sizing_label = gtk::Label::with_mnemonic("_Sizing:");
    sizing_label.set_halign(gtk::Align::End);
    sizing_label.set_valign(gtk::Align::Start);
    let sizing_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let shrink_radio = gtk::CheckButton::with_label("Shrink to fit (whole photo)");
    let crop_radio = gtk::CheckButton::with_label("Crop to fill (trim edges)");
    crop_radio.set_group(Some(&shrink_radio));
    if extras.borrow().crop_to_fill {
        crop_radio.set_active(true);
    } else {
        shrink_radio.set_active(true);
    }
    let extras_for_shrink = extras.clone();
    shrink_radio.connect_toggled(move |button| {
        if button.is_active() {
            if let Ok(mut current) = extras_for_shrink.try_borrow_mut() {
                current.crop_to_fill = false;
            }
        }
    });
    let extras_for_crop = extras.clone();
    crop_radio.connect_toggled(move |button| {
        if button.is_active() {
            if let Ok(mut current) = extras_for_crop.try_borrow_mut() {
                current.crop_to_fill = true;
            }
        }
    });
    sizing_box.append(&shrink_radio);
    sizing_box.append(&crop_radio);

    let per_page_label = gtk::Label::with_mnemonic("Prints per _page:");
    per_page_label.set_halign(gtk::Align::End);
    let per_page_model = gtk::StringList::new(&[
        "1 per page",
        "2 per page (side by side)",
        "3 per page (in a row)",
        "4 per page (2 × 2 grid)",
    ]);
    let per_page_combo = gtk::DropDown::new(Some(per_page_model), None::<&gtk::Expression>);
    per_page_combo.set_selected(extras.borrow().per_page.clamp(1, 4) - 1);
    per_page_label.set_mnemonic_widget(Some(&per_page_combo));
    let extras_for_per_page = extras.clone();
    per_page_combo.connect_selected_notify(move |dropdown| {
        let count = dropdown.selected().clamp(1, 4);
        if let Ok(mut current) = extras_for_per_page.try_borrow_mut() {
            current.per_page = count;
        }
    });

    grid.attach(&size_label, 0, 0, 1, 1);
    grid.attach(&size_combo, 1, 0, 1, 1);
    grid.attach(&sizing_label, 0, 1, 1, 1);
    grid.attach(&sizing_box, 1, 1, 1, 1);
    grid.attach(&per_page_label, 0, 2, 1, 1);
    grid.attach(&per_page_combo, 1, 2, 1, 1);

    grid.upcast()
}

// ---------------------------------------------------------------------------
// Page layout
// ---------------------------------------------------------------------------

/// Draw the photo onto the print page: split the printable area into cells,
/// then place the photo in every cell at the requested size and fit mode.
fn draw_print_page(context: &gtk::PrintContext, image: &image::RgbaImage, extras: &PrintExtras) {
    let image_width = image.width() as f64;
    let image_height = image.height() as f64;
    if image_width < 1.0 || image_height < 1.0 {
        return;
    }
    let page_width = context.width();
    let page_height = context.height();
    if !(page_width > 0.0 && page_height > 0.0) {
        return;
    }

    let surface = match build_page_surface(image) {
        Some(surface) => surface,
        None => return,
    };

    let (columns, rows) = cell_grid(extras.per_page);
    let cell_width = page_width / f64::from(columns);
    let cell_height = page_height / f64::from(rows);
    // A small gutter keeps multi-up prints from touching; single prints use
    // the driver's own margins.
    let gutter = if extras.per_page > 1 {
        3.0 * PT_PER_MM
    } else {
        0.0
    };
    let preset = find_size_preset(&extras.size_id);

    let cairo = context.cairo_context();
    for row in 0..rows {
        for column in 0..columns {
            let x = f64::from(column) * cell_width + gutter / 2.0;
            let y = f64::from(row) * cell_height + gutter / 2.0;
            let cell_width_here = cell_width - gutter;
            let cell_height_here = cell_height - gutter;
            if cell_width_here <= 0.0 || cell_height_here <= 0.0 {
                continue;
            }

            // Full page fills the cell; a physical size keeps its exact
            // aspect ratio, scaled down only when the cell is smaller.
            let (box_width, box_height) = if preset.width_pt > 0.0 && preset.height_pt > 0.0 {
                let scale = (cell_width_here / preset.width_pt)
                    .min(cell_height_here / preset.height_pt)
                    .min(1.0);
                (preset.width_pt * scale, preset.height_pt * scale)
            } else {
                (cell_width_here, cell_height_here)
            };
            let box_x = x + (cell_width_here - box_width) / 2.0;
            let box_y = y + (cell_height_here - box_height) / 2.0;

            draw_image_into(
                &cairo,
                &surface,
                image_width,
                image_height,
                box_x,
                box_y,
                box_width,
                box_height,
                extras.crop_to_fill,
            );
        }
    }
}

/// Convert the rendered RGBA photo into a Cairo surface (premultiplied BGRA).
fn build_page_surface(image: &image::RgbaImage) -> Option<gtk::cairo::ImageSurface> {
    let mut data = image.as_raw().to_vec();
    for pixel in data.chunks_exact_mut(4) {
        let alpha = u32::from(pixel[3]);
        let (red, green, blue) =
            (u32::from(pixel[0]), u32::from(pixel[1]), u32::from(pixel[2]));
        pixel[0] = ((blue * alpha + 127) / 255) as u8;
        pixel[1] = ((green * alpha + 127) / 255) as u8;
        pixel[2] = ((red * alpha + 127) / 255) as u8;
    }

    let stride = gtk::cairo::Format::stride_for_width(gtk::cairo::Format::ARgb32, image.width())
        .ok()?;
    gtk::cairo::ImageSurface::create_for_data(
        data,
        gtk::cairo::Format::ARgb32,
        image.width() as i32,
        image.height() as i32,
        stride,
    )
    .ok()
}

/// Paint the photo into a rectangle, either whole (shrink to fit) or cropped
/// to fill the rectangle, always centred and aspect-true.
#[allow(clippy::too_many_arguments)]
fn draw_image_into(
    cairo: &gtk::cairo::Context,
    surface: &gtk::cairo::ImageSurface,
    image_width: f64,
    image_height: f64,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    crop_to_fill: bool,
) {
    if width <= 0.0 || height <= 0.0 {
        return;
    }
    let horizontal = width / image_width;
    let vertical = height / image_height;
    let scale = if crop_to_fill {
        horizontal.max(vertical)
    } else {
        horizontal.min(vertical)
    };
    if !scale.is_finite() || scale <= 0.0 {
        return;
    }

    if cairo.save().is_err() {
        return;
    }
    cairo.rectangle(x, y, width, height);
    cairo.clip();
    cairo.scale(scale, scale);
    // Centre the (possibly overflowing) photo inside the target rectangle.
    let offset_x = (x + (width - image_width * scale) / 2.0) / scale;
    let offset_y = (y + (height - image_height * scale) / 2.0) / scale;
    if cairo.set_source_surface(surface, offset_x, offset_y).is_ok() {
        let _ = cairo.paint();
    }
    let _ = cairo.restore();
}

#[cfg(test)]
mod print_extras_tests {
    use super::*;

    #[test]
    fn extras_parse_uses_defaults_for_garbage() {
        let extras = parse_extras(None);
        assert_eq!(extras.size_id, "full");
        assert!(!extras.crop_to_fill);
        assert_eq!(extras.per_page, 1);

        let extras = parse_extras(Some("nonsense"));
        assert_eq!(extras.size_id, "full");

        let extras = parse_extras(Some("size=bogus;crop=yes;per=99"));
        assert_eq!(extras.size_id, "full"); // unknown size rejected
        assert!(!extras.crop_to_fill); // only 1 counts as true
        assert_eq!(extras.per_page, 4); // clamped
    }

    #[test]
    fn extras_round_trip() {
        let extras = PrintExtras {
            size_id: "4x6".to_string(),
            crop_to_fill: true,
            per_page: 2,
        };
        let parsed = parse_extras(Some(&extras_to_string(&extras)));
        assert_eq!(parsed, extras);
    }

    #[test]
    fn per_page_grids_place_prints_side_by_side() {
        assert_eq!(cell_grid(1), (1, 1));
        assert_eq!(cell_grid(2), (2, 1));
        assert_eq!(cell_grid(3), (3, 1));
        assert_eq!(cell_grid(4), (2, 2));
    }

    #[test]
    fn physical_sizes_map_to_print_points() {
        let four_by_six = find_size_preset("4x6");
        assert!((four_by_six.width_pt - 4.0 * PT_PER_INCH).abs() < 0.01);
        assert!((four_by_six.height_pt - 6.0 * PT_PER_INCH).abs() < 0.01);
        let ten_by_fifteen = find_size_preset("10x15");
        assert!((ten_by_fifteen.width_pt - 100.0 * PT_PER_MM).abs() < 0.01);
        // Full page has no fixed dimensions.
        assert_eq!(find_size_preset("full").width_pt, 0.0);
    }
}
