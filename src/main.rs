mod albums_view;
mod collage;
mod db;
mod diagnostics;
mod edit;
mod grid;
mod image_format;
mod infobar;
mod lightbox;
mod photo_object;
mod photo_texture;
mod scanner;
mod settings;
mod sidebar;
mod source;
mod thumbnail;
mod thumbnail_display;
mod window;

/// Icons the app references by name (all Adwaita symbolic names). Keep in
/// sync with resources/icons.gresource.xml.
const REFERENCED_ICONS: &[&str] = &[
    "pan-end-symbolic",
    "pan-down-symbolic",
    "pan-start-symbolic",
    "emote-love-symbolic",
    "image-x-generic-symbolic",
    "folder-symbolic",
    "object-select-symbolic",
    "folder-pictures-symbolic",
    "document-edit-symbolic",
    "view-sort-descending-symbolic",
    "view-sort-ascending-symbolic",
    "view-sidebar-symbolic",
    "view-list-symbolic",
    "sidebar-hide-symbolic",
    "emblem-system-symbolic",
    "view-refresh-symbolic",
    "view-grid-symbolic",
    "view-app-grid-symbolic",
    "sidebar-show-symbolic",
    "process-stop-symbolic",
    "object-rotate-right-symbolic",
    "list-add-symbolic",
    "go-previous-symbolic",
    "folder-open-symbolic",
    "folder-new-symbolic",
    "edit-undo-symbolic",
    "edit-redo-symbolic",
    "document-save-symbolic",
    "appointment-soon-symbolic",
];

/// Register the bundled hicolor icon subset (resources/icons.gresource).
/// Linux picks icons from the system Adwaita theme, but Windows/macOS
/// bundles ship no icon theme at all, leaving every symbolic icon blank.
/// The bundled subset guarantees all referenced names resolve everywhere.
fn register_bundled_icons() {
    const ICONS_GRESOURCE: &[u8] = include_bytes!("../resources/icons.gresource");
    let resource = gio::Resource::from_data(&glib::Bytes::from_static(ICONS_GRESOURCE))
        .expect("bundled icon resource is valid");
    gio::resources_register(&resource);
}

/// Log which referenced icons the display can actually resolve. Run with
/// PICASA_TRACE=1 to debug missing icons on platform bundles. Note:
/// sidebar-hide-symbolic is intentionally not bundled; window::build falls
/// back to view-sidebar-symbolic when it is absent.
pub fn trace_icon_resolution() {
    if std::env::var_os("PICASA_TRACE").is_none() {
        return;
    }
    let Some(display) = gtk4::gdk::Display::default() else {
        eprintln!("ICONS TRACE no display; cannot check icon theme");
        return;
    };
    let theme = gtk4::IconTheme::for_display(&display);
    let missing: Vec<&str> = REFERENCED_ICONS
        .iter()
        .filter(|name| !theme.has_icon(name))
        .copied()
        .collect();
    if missing.is_empty() {
        eprintln!(
            "ICONS TRACE all {} referenced icons resolve",
            REFERENCED_ICONS.len()
        );
    } else {
        eprintln!("ICONS TRACE missing {} icons: {missing:?}", missing.len());
    }
}

fn main() {
    use gio::prelude::*;
    use gtk::prelude::*;
    use gtk4 as gtk;
    use libadwaita as adw;

    init_trace_log();

    std::panic::set_hook(Box::new(|panic| {
        eprintln!("PICASA PANIC: {panic}");
        eprintln!(
            "PICASA PANIC BACKTRACE:\n{}",
            std::backtrace::Backtrace::force_capture()
        );
    }));

    adw::init().expect("libadwaita initialization failed");
    register_bundled_icons();
    let application = adw::Application::new(
        Some("io.github.you.PicasaRs"),
        gio::ApplicationFlags::default(),
    );
    application.connect_activate(|application| {
        // Make the bundled hicolor subset resolvable. Windows/macOS bundles
        // ship no system icon theme, so every symbolic icon would be blank
        // without this (Linux keeps using the system Adwaita theme).
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::IconTheme::for_display(&display).add_resource_path("/picrs/icons");
        }
        trace_icon_resolution();
        // Startup loads indexed rows and recovers missing cached previews.
        // Folder discovery runs only through explicit import/refresh actions.
        match db::open_default() {
            Ok(connection) => window::build(application, connection).present(),
            Err(error) => {
                eprintln!("Could not open photo library: {error:#}");
                let dialog = gtk::MessageDialog::builder()
                    .message_type(gtk::MessageType::Error)
                    .buttons(gtk::ButtonsType::Close)
                    .text("Could not open the photo library")
                    .secondary_text(error.to_string())
                    .build();
                dialog.connect_response(|dialog, _| dialog.close());
                dialog.present();
            }
        }
    });
    application.run();
}

#[cfg(unix)]
fn init_trace_log() {}

#[cfg(not(unix))]
fn init_trace_log() {}
