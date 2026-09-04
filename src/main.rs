mod albums_view;
mod db;
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

/// When tracing is enabled, persist the complete stderr stream beside the
/// checkout. A new timestamped file is created for every application run.
#[cfg(unix)]
fn init_trace_log() {
    if std::env::var_os("PICASA_TRACE").is_none() {
        return;
    }
    use std::fs::OpenOptions;
    use std::os::fd::AsRawFd;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let path = std::env::current_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
        .join(format!("picasa-trace-{stamp}.log"));
    if let Ok(file) = OpenOptions::new().create(true).append(true).open(&path) {
        // Keep all existing eprintln!-based instrumentation, including GTK
        // warnings and panic diagnostics, in the same per-run file.
        unsafe {
            libc::dup2(file.as_raw_fd(), libc::STDERR_FILENO);
        }
        eprintln!("\n===== PICASA TRACE RUN {stamp} =====");
        eprintln!("PICASA TRACE LOG: {}", path.display());
    }
}

#[cfg(not(unix))]
fn init_trace_log() {}
