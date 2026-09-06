mod albums_view;
mod db;
mod diagnostics;
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
mod window;

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
    let application = adw::Application::new(
        Some("io.github.you.PicasaRs"),
        gio::ApplicationFlags::default(),
    );
    application.connect_activate(|application| {
        // Startup intentionally opens SQLite and reads indexed rows only. Scanner and
        // thumbnail generation are reachable only from explicit import/refresh actions.
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
