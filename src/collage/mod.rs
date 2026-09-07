mod editor;
mod layout;
mod model;
mod render;

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;
use rusqlite::Connection;

use crate::photo_object::PhotoObject;

pub fn open(parent: &gtk::Window, connection: Rc<RefCell<Connection>>, ids: Vec<i64>) {
    if ids.len() < 2 {
        let dialog = adw::AlertDialog::builder()
            .heading("Create Collage")
            .body("Select at least two photos to create a collage.")
            .close_response("close")
            .build();
        dialog.add_response("close", "Close");
        dialog.present(Some(parent));
        return;
    }

    let photos: Vec<PhotoObject> = ids
        .into_iter()
        .filter_map(|id| crate::db::photo(&connection.borrow(), id).ok().flatten())
        .map(|photo| PhotoObject::from_photo(&photo))
        .collect();
    if photos.len() < 2 {
        let dialog = adw::AlertDialog::builder()
            .heading("Create Collage")
            .body("The selected photos are no longer available in the library.")
            .close_response("close")
            .build();
        dialog.add_response("close", "Close");
        dialog.present(Some(parent));
        return;
    }

    editor::present(parent, photos);
}
