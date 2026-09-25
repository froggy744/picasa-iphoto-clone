mod library;
mod photo;

use libadwaita as adw;
use adw::prelude::*;
use gtk4 as gtk;

fn main() -> glib::ExitCode {
    let app = adw::Application::builder()
        .application_id("za.co.hmf.PicLibraryPrototype")
        .build();

    app.connect_activate(|app| {
        let window = library::build_window(app);
        window.present();
    });

    app.run()
}
