// ---------------------------------------------------------------------------
// Photo printing
//
// The bottom bar's print button renders every selected photo (library
// rotation, crops and filters applied) on a worker thread, shows an Adwaita
// photo options dialog with a live page preview (exact print size,
// shrink-to-fit vs crop-to-fill, 1-4 prints per page) and then pops the
// standard GTK print dialog. Choices persist
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
    SizePreset {
        id: "a4",
        label: "A4 (210 × 297 mm)",
        width_pt: 210.0 * PT_PER_MM,
        height_pt: 297.0 * PT_PER_MM,
    },
    SizePreset {
        id: "a3",
        label: "A3 (297 × 420 mm)",
        width_pt: 297.0 * PT_PER_MM,
        height_pt: 420.0 * PT_PER_MM,
    },
    SizePreset {
        id: "letter",
        label: "US Letter (8.5 × 11 in)",
        width_pt: 8.5 * PT_PER_INCH,
        height_pt: 11.0 * PT_PER_INCH,
    },
    SizePreset {
        id: "legal",
        label: "US Legal (8.5 × 14 in)",
        width_pt: 8.5 * PT_PER_INCH,
        height_pt: 14.0 * PT_PER_INCH,
    },
    SizePreset {
        id: "tabloid",
        label: "US Tabloid (11 × 17 in)",
        width_pt: 11.0 * PT_PER_INCH,
        height_pt: 17.0 * PT_PER_INCH,
    },
    SizePreset {
        id: "custom",
        label: "Custom…",
        width_pt: 0.0,
        height_pt: 0.0,
    },
];

/// The photo-specific options shown in the print dialog's custom tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum PageOrientation {
    #[default]
    Portrait,
    Landscape,
}

/// The photo-specific options shown in the print dialog's custom tab.
#[derive(Clone, Debug, PartialEq)]
struct PrintExtras {
    size_id: String,
    /// Free-form print size (size_id == "custom"), stored in points so the
    /// mm/inch unit selector is pure display and survives round trips.
    custom_width_pt: f64,
    custom_height_pt: f64,
    /// false = shrink the whole photo into the target box (letterboxed),
    /// true = fill the box and centre-crop the overflow.
    crop_to_fill: bool,
    per_page: u32,
    orientation: PageOrientation,
}

impl Default for PrintExtras {
    fn default() -> Self {
        Self {
            size_id: "full".to_string(),
            custom_width_pt: 4.0 * PT_PER_INCH,
            custom_height_pt: 6.0 * PT_PER_INCH,
            crop_to_fill: false,
            per_page: 1,
            orientation: PageOrientation::Portrait,
        }
    }
}

/// Hard bounds for stored custom sizes, in points (10 mm to ~1.76 m).
const CUSTOM_SIZE_MIN_PT: f64 = 10.0;
const CUSTOM_SIZE_MAX_PT: f64 = 5000.0;

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
                if value == "custom" || SIZE_PRESETS.iter().any(|preset| preset.id == value) {
                    extras.size_id = value.to_string();
                }
            }
            "w" => {
                if let Ok(value) = value.parse::<f64>() {
                    if value.is_finite() {
                        extras.custom_width_pt = value.clamp(CUSTOM_SIZE_MIN_PT, CUSTOM_SIZE_MAX_PT);
                    }
                }
            }
            "h" => {
                if let Ok(value) = value.parse::<f64>() {
                    if value.is_finite() {
                        extras.custom_height_pt = value.clamp(CUSTOM_SIZE_MIN_PT, CUSTOM_SIZE_MAX_PT);
                    }
                }
            }
            "crop" => extras.crop_to_fill = value == "1",
            "per" => {
                if let Ok(count) = value.parse::<u32>() {
                    extras.per_page = count.clamp(1, 4);
                }
            }
            "orient" => {
                extras.orientation = if value == "landscape" {
                    PageOrientation::Landscape
                } else {
                    PageOrientation::Portrait
                };
            }
            _ => {}
        }
    }
    extras
}

fn extras_to_string(extras: &PrintExtras) -> String {
    format!(
        "size={};w={:.2};h={:.2};crop={};per={};orient={}",
        extras.size_id,
        extras.custom_width_pt,
        extras.custom_height_pt,
        u8::from(extras.crop_to_fill),
        extras.per_page,
        match extras.orientation {
            PageOrientation::Landscape => "landscape",
            PageOrientation::Portrait => "portrait",
        }
    )
}

/// Paper dimensions with the chosen orientation applied: landscape swaps
/// the saved paper's width and height.
fn oriented_paper(paper: (f64, f64), orientation: PageOrientation) -> (f64, f64) {
    match orientation {
        PageOrientation::Portrait => paper,
        PageOrientation::Landscape => (paper.1, paper.0),
    }
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

/// Effective fixed print box for the chosen size, in points. `(0, 0)` marks
/// "full page" (fill the cell); the custom size keeps the user's dimensions.
fn print_box_size(extras: &PrintExtras) -> (f64, f64) {
    if extras.size_id == "custom" {
        (extras.custom_width_pt, extras.custom_height_pt)
    } else {
        let preset = find_size_preset(&extras.size_id);
        (preset.width_pt, preset.height_pt)
    }
}

/// Pages needed for a selection: every photo prints once per round of
/// cells, and the final page cycles back through the selection rather
/// than leaving blanks.
fn print_page_count(photo_count: usize, per_page: u32) -> usize {
    if photo_count == 0 || per_page == 0 {
        return 0;
    }
    photo_count.div_ceil(per_page as usize)
}

/// The per-page DropDown reports a zero-based index; "N per page" sits at
/// index N - 1. (Clamping the index as if it were a count once made
/// "2 per page" lay out one cell and "4 per page" lay out three.)
fn per_page_for_index(index: u32) -> u32 {
    index.clamp(0, 3) + 1
}

/// One photo rectangle on the page, in points measured from the page origin.
struct PagePlacement {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

/// Lay out one page: split it into cells for the prints-per-page setting and
/// return the photo rectangle for every cell. Shared by the printer drawing
/// and the dialog preview so the two can never drift apart.
fn page_placements(page_width: f64, page_height: f64, extras: &PrintExtras) -> Vec<PagePlacement> {
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
    let (preset_width, preset_height) = print_box_size(extras);

    let mut placements = Vec::with_capacity((columns * rows) as usize);
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
            let (box_width, box_height) = if preset_width > 0.0 && preset_height > 0.0 {
                let scale = (cell_width_here / preset_width)
                    .min(cell_height_here / preset_height)
                    .min(1.0);
                (preset_width * scale, preset_height * scale)
            } else {
                (cell_width_here, cell_height_here)
            };
            placements.push(PagePlacement {
                x: x + (cell_width_here - box_width) / 2.0,
                y: y + (cell_height_here - box_height) / 2.0,
                width: box_width,
                height: box_height,
            });
        }
    }
    placements
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

/// Render every selected photo in the background, then show the photo
/// options dialog followed by the print dialog on the main thread.
pub(crate) fn print_photos<W: IsA<gtk::Window>>(
    parent: &W,
    connection: &Rc<RefCell<Connection>>,
    requests: Vec<PhotoPrintRequest>,
) {
    if requests.is_empty() {
        return;
    }

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

    std::thread::spawn(move || {
        // Render each photo independently; a single unreadable file skips
        // itself instead of sinking the whole job.
        let rendered: Vec<image::RgbaImage> = requests
            .iter()
            .filter_map(|request| {
                crate::edit::render::render_for_export(
                    &request.reference,
                    request.rotation,
                    &request.edit_recipe,
                    request.source_width,
                    request.source_height,
                )
                .map(downscale_for_print)
                .ok()
            })
            .collect();

        glib::MainContext::default().invoke(move || {
            if rendered.is_empty() {
                eprintln!("Could not prepare the selected photos for printing");
                return;
            }
            // Cairo surfaces back both the live preview and the final print;
            // the raw rasters are dropped right after conversion.
            let surfaces: Vec<gtk::cairo::ImageSurface> = rendered
                .iter()
                .filter_map(|image| build_page_surface(image))
                .collect();
            if surfaces.is_empty() {
                eprintln!("Could not prepare the selected photos for printing");
                return;
            }
            let job_name = if surfaces.len() == 1 {
                requests[0].job_name.clone()
            } else {
                format!("{} photos", surfaces.len())
            };
            run_print_dialog(
                weak_parent,
                initial_extras,
                Rc::new(surfaces),
                job_name,
                saved_config,
            );
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
    surfaces: Rc<Vec<gtk::cairo::ImageSurface>>,
    job_name: String,
    saved_config: Option<String>,
) {
    let Some(parent) = weak_parent.upgrade() else {
        eprintln!("Could not print photo: the window is gone");
        return;
    };
    let extras = Rc::new(RefCell::new(initial_extras));

    let heading = if surfaces.len() == 1 {
        "Print photo"
    } else {
        "Print photos"
    };
    let dialog = adw::AlertDialog::new(Some(heading), Some(&job_name));
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("print", "Print");
    dialog.set_response_appearance("print", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("print"));
    dialog.set_close_response("cancel");
    dialog.set_content_width(440);
    // The preview mirrors the exact page layout the printer will receive:
    // same paper (from the previous print, or the system default) and the
    // same placement math as draw_print_page.
    dialog.set_extra_child(Some(&build_extras_widget(
        &extras,
        surfaces.clone(),
        preview_paper_size(saved_config.as_deref()),
    )));

    let extras_for_response = extras.clone();
    let surfaces_for_print = surfaces;
    let job_name_for_print = job_name;
    let parent_for_print = weak_parent;
    let config_for_print = saved_config;
    dialog.choose(Some(&parent), None::<&gio::Cancellable>, move |response| {
        if response == "print" {
            run_print_operation(
                &parent_for_print,
                &extras_for_response,
                surfaces_for_print,
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
    surfaces: Rc<Vec<gtk::cairo::ImageSurface>>,
    job_name: &str,
    saved_config: Option<&str>,
) {
    let operation = gtk::PrintOperation::new();
    operation.set_job_name(job_name);
    operation.set_n_pages(print_page_count(surfaces.len(), extras.borrow().per_page) as i32);

    // Restore the printer, paper and orientation choices from the previous
    // print so the system dialog opens ready-to-go. This dialog's own
    // orientation choice always wins over the saved page setup's.
    let mut setup = gtk::PageSetup::new();
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
            if let Ok(saved) = gtk::PageSetup::from_key_file(&key_file, PRINT_PAGE_SETUP_GROUP) {
                setup = saved;
            }
        }
    }
    setup.set_orientation(match extras.borrow().orientation {
        PageOrientation::Landscape => gtk::PageOrientation::Landscape,
        PageOrientation::Portrait => gtk::PageOrientation::Portrait,
    });
    operation.set_default_page_setup(Some(&setup));

    let extras_for_draw = extras.clone();
    let surfaces_for_draw = surfaces;
    operation.connect_draw_page(move |_operation, context, page_number| {
        draw_print_page(
            context,
            &surfaces_for_draw,
            page_number.max(0) as u32,
            &extras_for_draw.borrow(),
        );
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

fn build_extras_widget(
    extras: &Rc<RefCell<PrintExtras>>,
    surfaces: Rc<Vec<gtk::cairo::ImageSurface>>,
    paper: (f64, f64),
) -> gtk::Widget {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    content.set_margin_top(12);
    content.set_margin_bottom(12);
    content.set_margin_start(12);
    content.set_margin_end(12);

    // Live page preview: every page of the job in a row, horizontally
    // scrollable when a larger selection spans multiple pages. It shares the
    // page_placements math with the printer drawing and is redrawn whenever
    // any option below changes.
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_height_request(PREVIEW_HEIGHT);
    scroll.set_hexpand(true);
    scroll.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Never);

    let preview = gtk::DrawingArea::new();
    let extras_for_preview = extras.clone();
    let surfaces_for_preview = surfaces.clone();
    let surfaces_for_sizing = surfaces_for_preview.clone();
    preview.set_draw_func(move |_area, cairo, width, height| {
        draw_pages_preview(
            &cairo,
            f64::from(width),
            f64::from(height),
            &surfaces_for_preview,
            &extras_for_preview.borrow(),
            paper,
        );
    });
    scroll.set_child(Some(&preview));

    let redraw_preview: Rc<dyn Fn()> = {
        let preview = preview.clone();
        let extras_for_sizing = extras.clone();
        Rc::new(move || {
            // Grow the drawing area when the current options need more
            // pages; the scrolled window takes over from there.
            preview.set_width_request(preview_width_request(
                surfaces_for_sizing.len(),
                &extras_for_sizing.borrow(),
                paper,
            ));
            preview.queue_draw();
        })
    };
    redraw_preview();
    content.append(&scroll);

    let grid = gtk::Grid::new();
    grid.set_row_spacing(10);
    grid.set_column_spacing(12);

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
    let redraw_for_size = redraw_preview.clone();

    // Custom size row (mirrors the collage editor): width × height spins
    // with an mm/inch unit selector, revealed only while "Custom…" is the
    // chosen print size. Stored values are points; the spins are display.
    let unit_combo = gtk::DropDown::from_strings(&["mm", "in"]);
    let custom_width = gtk::SpinButton::with_range(10.0, 2000.0, 1.0);
    let custom_height = gtk::SpinButton::with_range(10.0, 2000.0, 1.0);
    for spin in [&custom_width, &custom_height] {
        spin.set_numeric(true);
        spin.set_digits(0);
        spin.set_width_chars(6);
    }
    let custom_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    custom_box.append(&custom_width);
    custom_box.append(&gtk::Label::new(Some("×")));
    custom_box.append(&custom_height);
    custom_box.append(&unit_combo);
    {
        let borrowed = extras.borrow();
        let inches = false; // spin values start in millimetres
        let to_display = |pt: f64| {
            if inches {
                pt / PT_PER_INCH
            } else {
                pt
            }
        };
        custom_width.set_value(to_display(borrowed.custom_width_pt).round());
        custom_height.set_value(to_display(borrowed.custom_height_pt).round());
        custom_box.set_visible(borrowed.size_id == "custom");
    }

    let apply_custom_size: Rc<dyn Fn()> = {
        let extras = extras.clone();
        let unit_combo = unit_combo.clone();
        let custom_width = custom_width.clone();
        let custom_height = custom_height.clone();
        let redraw = redraw_preview.clone();
        Rc::new(move || {
            let to_pt = |value: f64| {
                if unit_combo.selected() == 1 {
                    value * PT_PER_INCH
                } else {
                    value
                }
            };
            if let Ok(mut current) = extras.try_borrow_mut() {
                current.custom_width_pt = to_pt(custom_width.value())
                    .clamp(CUSTOM_SIZE_MIN_PT, CUSTOM_SIZE_MAX_PT);
                current.custom_height_pt = to_pt(custom_height.value())
                    .clamp(CUSTOM_SIZE_MIN_PT, CUSTOM_SIZE_MAX_PT);
            }
            redraw();
        })
    };
    let apply_for_width = apply_custom_size.clone();
    custom_width.connect_value_changed(move |_| apply_for_width());
    let apply_for_height = apply_custom_size.clone();
    custom_height.connect_value_changed(move |_| apply_for_height());
    let apply_for_unit = apply_custom_size.clone();
    let unit_width = custom_width.clone();
    let unit_height = custom_height.clone();
    let redraw_for_unit = redraw_preview.clone();
    unit_combo.connect_selected_notify(move |dropdown| {
        let inches = dropdown.selected() == 1;
        for spin in [&unit_width, &unit_height] {
            if inches {
                spin.set_value(spin.value() / PT_PER_INCH);
                spin.set_range(0.4, 80.0);
                spin.set_digits(1);
                spin.set_increments(0.1, 1.0);
            } else {
                spin.set_value(spin.value() * PT_PER_INCH);
                spin.set_range(10.0, 2000.0);
                spin.set_digits(0);
                spin.set_increments(1.0, 10.0);
            }
        }
        apply_for_unit();
        redraw_for_unit();
    });

    let apply_for_size = apply_custom_size.clone();
    let custom_box_for_size = custom_box.clone();
    size_combo.connect_selected_notify(move |dropdown| {
        // try_borrow_mut: the dialog may emit changed signals while the page
        // draw borrows `extras`; dropping the write then is harmless because
        // draw_page reads once per print.
        let index = usize::try_from(dropdown.selected()).unwrap_or(0);
        if let Some(preset) = SIZE_PRESETS.get(index) {
            let is_custom = preset.id == "custom";
            if is_custom {
                // Commit the spin values before switching to custom.
                apply_for_size();
            }
            if let Ok(mut current) = extras_for_size.try_borrow_mut() {
                current.size_id = preset.id.to_string();
            }
            custom_box_for_size.set_visible(is_custom);
        }
        redraw_for_size();
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
    let redraw_for_shrink = redraw_preview.clone();
    shrink_radio.connect_toggled(move |button| {
        if button.is_active() {
            if let Ok(mut current) = extras_for_shrink.try_borrow_mut() {
                current.crop_to_fill = false;
            }
            redraw_for_shrink();
        }
    });
    let extras_for_crop = extras.clone();
    let redraw_for_crop = redraw_preview.clone();
    crop_radio.connect_toggled(move |button| {
        if button.is_active() {
            if let Ok(mut current) = extras_for_crop.try_borrow_mut() {
                current.crop_to_fill = true;
            }
            redraw_for_crop();
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
    let redraw_for_per_page = redraw_preview.clone();
    per_page_combo.connect_selected_notify(move |dropdown| {
        // The DropDown reports a zero-based index; "N per page" is index
        // N - 1, so convert instead of clamping the index as a count.
        let count = per_page_for_index(dropdown.selected());
        if let Ok(mut current) = extras_for_per_page.try_borrow_mut() {
            current.per_page = count;
        }
        redraw_for_per_page();
    });

    let orientation_label = gtk::Label::with_mnemonic("_Orientation:");
    orientation_label.set_halign(gtk::Align::End);
    let orientation_box = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let portrait_radio = gtk::CheckButton::with_label("Portrait");
    let landscape_radio = gtk::CheckButton::with_label("Landscape");
    landscape_radio.set_group(Some(&portrait_radio));
    if extras.borrow().orientation == PageOrientation::Landscape {
        landscape_radio.set_active(true);
    } else {
        portrait_radio.set_active(true);
    }
    let extras_for_portrait = extras.clone();
    let redraw_for_portrait = redraw_preview.clone();
    portrait_radio.connect_toggled(move |button| {
        if button.is_active() {
            if let Ok(mut current) = extras_for_portrait.try_borrow_mut() {
                current.orientation = PageOrientation::Portrait;
            }
            redraw_for_portrait();
        }
    });
    let extras_for_landscape = extras.clone();
    let redraw_for_landscape = redraw_preview.clone();
    landscape_radio.connect_toggled(move |button| {
        if button.is_active() {
            if let Ok(mut current) = extras_for_landscape.try_borrow_mut() {
                current.orientation = PageOrientation::Landscape;
            }
            redraw_for_landscape();
        }
    });
    orientation_box.append(&portrait_radio);
    orientation_box.append(&landscape_radio);

    grid.attach(&size_label, 0, 0, 1, 1);
    grid.attach(&size_combo, 1, 0, 1, 1);
    grid.attach(&custom_box, 1, 1, 1, 1);
    grid.attach(&sizing_label, 0, 2, 1, 1);
    grid.attach(&sizing_box, 1, 2, 1, 1);
    grid.attach(&per_page_label, 0, 3, 1, 1);
    grid.attach(&per_page_combo, 1, 3, 1, 1);
    grid.attach(&orientation_label, 0, 4, 1, 1);
    grid.attach(&orientation_box, 1, 4, 1, 1);

    content.append(&grid);
    content.upcast()
}

// ---------------------------------------------------------------------------
// Live page preview
// ---------------------------------------------------------------------------

/// Live preview geometry inside the photo options dialog (logical pixels).
const PREVIEW_HEIGHT: i32 = 260;
const PREVIEW_WIDTH: i32 = 360;
/// Blank margin between the preview widget edge and the paper rectangles.
const PREVIEW_MARGIN: f64 = 10.0;
/// Gap between neighbouring page previews.
const PREVIEW_PAGE_GAP: f64 = 8.0;

/// Paper size for the preview, in points, honouring the printer, paper and
/// orientation chosen on the previous print. Falls back to the system
/// default paper (A4 or Letter, portrait).
fn preview_paper_size(saved_config: Option<&str>) -> (f64, f64) {
    if let Some(config) = saved_config {
        let key_file = glib::KeyFile::new();
        if key_file.load_from_data(config, glib::KeyFileFlags::NONE).is_ok() {
            if let Ok(setup) = gtk::PageSetup::from_key_file(&key_file, PRINT_PAGE_SETUP_GROUP) {
                let width = setup.paper_width(gtk::Unit::Points);
                let height = setup.paper_height(gtk::Unit::Points);
                if width > 0.0 && height > 0.0 {
                    return (width, height);
                }
            }
        }
    }
    let default_setup = gtk::PageSetup::new();
    (
        default_setup.paper_width(gtk::Unit::Points),
        default_setup.paper_height(gtk::Unit::Points),
    )
}

/// Width the preview canvas needs to show every page side by side at full
/// preview height; the scrolled window grows or scrolls accordingly.
fn preview_width_request(photo_count: usize, extras: &PrintExtras, paper: (f64, f64)) -> i32 {
    let pages = print_page_count(photo_count, extras.per_page).max(1) as f64;
    let (paper_width, paper_height) = oriented_paper(paper, extras.orientation);
    let usable_height = f64::from(PREVIEW_HEIGHT) - 2.0 * PREVIEW_MARGIN;
    if usable_height <= 0.0 || paper_height <= 0.0 {
        return PREVIEW_WIDTH;
    }
    let page_width = paper_width * (usable_height / paper_height);
    let total = 2.0 * PREVIEW_MARGIN + pages * (page_width + PREVIEW_PAGE_GAP) - PREVIEW_PAGE_GAP;
    (total.ceil() as i32).max(PREVIEW_WIDTH)
}

/// Draw the preview: every page of the job in a row, each a white paper
/// rectangle scaled to the preview height, with the photos placed exactly
/// where draw_print_page will place them on paper.
fn draw_pages_preview(
    cairo: &gtk::cairo::Context,
    area_width: f64,
    area_height: f64,
    surfaces: &[gtk::cairo::ImageSurface],
    extras: &PrintExtras,
    paper: (f64, f64),
) {
    let (saved_paper_width, saved_paper_height) = paper;
    let (paper_width, paper_height) = oriented_paper(paper, extras.orientation);
    let usable_height = area_height - 2.0 * PREVIEW_MARGIN;
    if saved_paper_width <= 0.0 || saved_paper_height <= 0.0 || usable_height <= 0.0 {
        return;
    }
    let page_count = print_page_count(surfaces.len(), extras.per_page);
    if page_count == 0 {
        return;
    }
    let scale = usable_height / paper_height;
    let page_width = paper_width * scale;
    let content_width =
        page_count as f64 * (page_width + PREVIEW_PAGE_GAP) - PREVIEW_PAGE_GAP;
    // Centre when the row fits; hug the left edge once it scrolls.
    let start_x = ((area_width - content_width) / 2.0).max(0.0);

    for page in 0..page_count {
        let origin_x = start_x + page as f64 * (page_width + PREVIEW_PAGE_GAP);
        let origin_y = PREVIEW_MARGIN;

        // Paper: white with a thin border so it reads against dark themes.
        cairo.set_source_rgb(1.0, 1.0, 1.0);
        cairo.rectangle(origin_x, origin_y, page_width, usable_height);
        let _ = cairo.fill();
        cairo.set_source_rgb(0.55, 0.55, 0.55);
        cairo.set_line_width(1.0);
        cairo.rectangle(
            origin_x + 0.5,
            origin_y + 0.5,
            page_width - 1.0,
            usable_height - 1.0,
        );
        let _ = cairo.stroke();

        let placements = page_placements(paper_width, paper_height, extras);
        for (cell, placement) in placements.iter().enumerate() {
            // Cells fill round-robin through the selection — the same
            // photo the printer will draw in this very cell.
            let Some(surface) =
                surfaces.get((page * placements.len() + cell) % surfaces.len())
            else {
                continue;
            };
            let image_width = f64::from(surface.width());
            let image_height = f64::from(surface.height());
            if image_width < 1.0 || image_height < 1.0 {
                continue;
            }
            draw_image_into(
                cairo,
                surface,
                image_width,
                image_height,
                origin_x + placement.x * scale,
                origin_y + placement.y * scale,
                placement.width * scale,
                placement.height * scale,
                extras.crop_to_fill,
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Page layout
// ---------------------------------------------------------------------------

/// Draw one print page: split the printable area into cells, then place a
/// selected photo in every cell at the requested size and fit mode. Cells
/// fill round-robin through the selection, so a single photo still prints
/// per_page copies and a partial last page stays full.
fn draw_print_page(
    context: &gtk::PrintContext,
    surfaces: &[gtk::cairo::ImageSurface],
    page_number: u32,
    extras: &PrintExtras,
) {
    let page_width = context.width();
    let page_height = context.height();
    if !(page_width > 0.0 && page_height > 0.0) || surfaces.is_empty() {
        return;
    }

    let cairo = context.cairo_context();
    let placements = page_placements(page_width, page_height, extras);
    for (cell, placement) in placements.iter().enumerate() {
        let surface = match surfaces
            .get((page_number as usize * placements.len() + cell) % surfaces.len())
        {
            Some(surface) => surface,
            None => continue,
        };
        let image_width = f64::from(surface.width());
        let image_height = f64::from(surface.height());
        if image_width < 1.0 || image_height < 1.0 {
            continue;
        }
        draw_image_into(
            &cairo,
            surface,
            image_width,
            image_height,
            placement.x,
            placement.y,
            placement.width,
            placement.height,
            extras.crop_to_fill,
        );
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
    fn custom_size_round_trips_and_drives_layout() {
        let extras = parse_extras(Some("size=custom;w=288;h=432;crop=1;per=99"));
        assert_eq!(extras.size_id, "custom");
        assert!((extras.custom_width_pt - 288.0).abs() < 0.01);
        assert!((extras.custom_height_pt - 432.0).abs() < 0.01);
        assert!(extras.crop_to_fill);
        assert_eq!(extras.per_page, 4);

        let parsed = parse_extras(Some(&extras_to_string(&extras)));
        assert_eq!(parsed, extras);

        // Nonsense custom dimensions fall back to sane bounds.
        let garbage = parse_extras(Some("size=custom;w=zebra;h=-500"));
        assert!((garbage.custom_width_pt - 4.0 * PT_PER_INCH).abs() < 0.01); // default
        assert!((garbage.custom_height_pt - CUSTOM_SIZE_MIN_PT).abs() < 0.01); // clamped

        // The custom box reaches the page layout: a 100 x 200 pt custom
        // size appears centred on the page at exactly its own dimensions.
        let mut custom = PrintExtras::default();
        custom.size_id = "custom".to_string();
        custom.custom_width_pt = 100.0;
        custom.custom_height_pt = 200.0;
        let placements = page_placements(A4.0, A4.1, &custom);
        assert_eq!(placements.len(), 1);
        assert!((placements[0].width - 100.0).abs() < 0.01);
        assert!((placements[0].height - 200.0).abs() < 0.01);
    }

    #[test]
    fn extras_round_trip() {
        let extras = PrintExtras {
            size_id: "4x6".to_string(),
            custom_width_pt: 4.0 * PT_PER_INCH,
            custom_height_pt: 6.0 * PT_PER_INCH,
            crop_to_fill: true,
            per_page: 2,
            orientation: PageOrientation::Landscape,
        };
        let parsed = parse_extras(Some(&extras_to_string(&extras)));
        assert_eq!(parsed, extras);
    }

    #[test]
    fn orientation_parses_and_applies_to_paper() {
        // Missing / garbage orient falls back to portrait.
        let portrait = parse_extras(Some("size=full;orient=nonsense"));
        assert_eq!(portrait.orientation, PageOrientation::Portrait);
        let landscape = parse_extras(Some("size=full;orient=landscape"));
        assert_eq!(landscape.orientation, PageOrientation::Landscape);

        // Landscape swaps the paper; portrait keeps it as saved.
        let a4 = (595.28, 841.89);
        assert_eq!(oriented_paper(a4, PageOrientation::Portrait), a4);
        assert_eq!(oriented_paper(a4, PageOrientation::Landscape), (a4.1, a4.0));

        // The preview layout math works with the swapped paper: one full
        // page print now covers the whole landscape sheet.
        let placements = page_placements(a4.1, a4.0, &landscape);
        assert_eq!(placements.len(), 1);
        assert!((placements[0].width - a4.1).abs() < 0.01);
        assert!((placements[0].height - a4.0).abs() < 0.01);
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

    #[test]
    fn a4_a3_and_us_paper_sizes_map_to_print_points() {
        let a4 = find_size_preset("a4");
        assert!((a4.width_pt - 210.0 * PT_PER_MM).abs() < 0.01);
        assert!((a4.height_pt - 297.0 * PT_PER_MM).abs() < 0.01);
        let a3 = find_size_preset("a3");
        assert!((a3.width_pt - 297.0 * PT_PER_MM).abs() < 0.01);
        assert!((a3.height_pt - 420.0 * PT_PER_MM).abs() < 0.01);
        let letter = find_size_preset("letter");
        assert!((letter.width_pt - 8.5 * PT_PER_INCH).abs() < 0.01);
        assert!((letter.height_pt - 11.0 * PT_PER_INCH).abs() < 0.01);
        let legal = find_size_preset("legal");
        assert!((legal.height_pt - 14.0 * PT_PER_INCH).abs() < 0.01);
        let tabloid = find_size_preset("tabloid");
        assert!((tabloid.width_pt - 11.0 * PT_PER_INCH).abs() < 0.01);
        assert!((tabloid.height_pt - 17.0 * PT_PER_INCH).abs() < 0.01);
        // The custom entry exists in the dropdown; its static stub is never
        // consulted for dimensions (print_box_size handles it instead).
        assert_eq!(find_size_preset("custom").id, "custom");
        assert_eq!(print_box_size(&PrintExtras::default()), (0.0, 0.0));
    }

    #[test]
    fn per_page_index_maps_to_counts() {
        assert_eq!(per_page_for_index(0), 1);
        assert_eq!(per_page_for_index(1), 2);
        assert_eq!(per_page_for_index(2), 3);
        assert_eq!(per_page_for_index(3), 4);
        // Out-of-range indexes guard to the closest end.
        assert_eq!(per_page_for_index(99), 4);
    }

    #[test]
    fn page_count_covers_every_photo() {
        assert_eq!(print_page_count(0, 4), 0);
        assert_eq!(print_page_count(1, 4), 1);
        assert_eq!(print_page_count(2, 4), 1);
        assert_eq!(print_page_count(5, 2), 3);
        assert_eq!(print_page_count(9, 4), 3);
    }

    /// A4 portrait in points — the layout math must work for any paper.
    const A4: (f64, f64) = (595.28, 841.89);

    #[test]
    fn page_layout_matches_what_the_printer_will_draw() {
        let mut extras = PrintExtras::default();

        // Full page, one per page: the photo box covers the whole page.
        let placements = page_placements(A4.0, A4.1, &extras);
        assert_eq!(placements.len(), 1);
        assert!((placements[0].width - A4.0).abs() < 0.01);
        assert!((placements[0].height - A4.1).abs() < 0.01);

        // Four per page: a 2 x 2 grid, each box filling its guttered cell.
        extras.per_page = 4;
        let placements = page_placements(A4.0, A4.1, &extras);
        assert_eq!(placements.len(), 4);
        let cell_width = A4.0 / 2.0 - 3.0 * PT_PER_MM;
        let cell_height = A4.1 / 2.0 - 3.0 * PT_PER_MM;
        for placement in &placements {
            assert!((placement.width - cell_width).abs() < 0.01);
            assert!((placement.height - cell_height).abs() < 0.01);
        }

        // A physical size keeps its exact point dimensions when the cell is
        // larger, and stays centred in the page.
        extras.per_page = 1;
        extras.size_id = "wallet".to_string();
        let placements = page_placements(A4.0, A4.1, &extras);
        assert_eq!(placements.len(), 1);
        assert!((placements[0].width - 2.5 * PT_PER_INCH).abs() < 0.01);
        assert!((placements[0].height - 3.5 * PT_PER_INCH).abs() < 0.01);
        assert!((placements[0].x + placements[0].width / 2.0 - A4.0 / 2.0).abs() < 0.01);
        assert!((placements[0].y + placements[0].height / 2.0 - A4.1 / 2.0).abs() < 0.01);
    }
}
