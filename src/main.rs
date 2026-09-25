mod catalog;
mod library;
mod photo;
mod viewer;

use libadwaita as adw;
use adw::prelude::*;
use gtk4 as gtk;

fn main() -> glib::ExitCode {
    let app = adw::Application::builder()
        .application_id("io.github.you.PicasaRs")
        .build();

    app.connect_activate(|app| {
        let window = library::build_window(app);
        window.present();
    });

    // Do not let GApplication interpret our optional photo-folder argument as a file-open request.
    // library.rs reads std::env::args_os() directly.
    app.run_with_args(&["pic-rs"])
}
