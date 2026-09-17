mod editor;
mod layout;
mod model;
mod render;
mod smart_mosaic;

pub use editor::{build as build_editor, CollageEditor};
pub use model::{draft_from_json, CollageDraft};

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;
use rusqlite::Connection;

use crate::db;
use crate::photo_object::PhotoObject;

/// Settings-table key holding the serialized in-progress collage.
pub const DRAFT_SETTING_KEY: &str = "collage-draft";

pub fn open(
    parent: &gtk::Window,
    connection: Rc<RefCell<Connection>>,
    ids: Vec<i64>,
    on_open: Rc<dyn Fn(Vec<PhotoObject>, Option<CollageDraft>)>,
) {
    let had_selection = !ids.is_empty();
    let photos: Vec<PhotoObject> = ids
        .into_iter()
        .filter_map(|id| crate::db::photo(&connection.borrow(), id).ok().flatten())
        .map(|photo| PhotoObject::from_photo(&photo))
        .collect();
    // Resume is a property of the saved draft, not of the current grid
    // selection. Previously an explicit (or stale) selection silently skipped
    // this prompt, which made the same Collage button behave inconsistently.
    let saved_draft = db::setting(&connection.borrow(), DRAFT_SETTING_KEY)
        .ok()
        .flatten()
        .and_then(|json| draft_from_json(&json))
        .filter(|draft| !draft.items.is_empty());

    

    let Some(draft) = saved_draft else {
        if had_selection && photos.is_empty() {
            let dialog = adw::AlertDialog::builder()
                .heading("Create Collage")
                .body("The selected photos are no longer available in the library.")
                .close_response("close")
                .build();
            dialog.add_response("close", "Close");
            dialog.present(Some(parent));
            return;
        }
        on_open(photos, None);
        return;
    };

    let draft_count = draft.items.len();
    let dialog = adw::AlertDialog::builder()
        .heading("Resume Collage?")
        .body(format!(
            "Your last collage with {} photos was saved when you left the editor. Resume it or start a new one.",
            draft_count
        ))
        .close_response("fresh")
        .default_response("resume")
        .build();
    dialog.add_response("fresh", "Start New");
    dialog.add_response("resume", "Resume");
    dialog.set_response_appearance("resume", adw::ResponseAppearance::Suggested);
    {
        let connection = connection.clone();
        let on_open = on_open.clone();
        let draft = draft.clone();
        dialog.connect_response(Some("resume"), move |_, _| {
            
            // Re-fetch photos from the library so edits, rotations and
            // thumbnails are current; missing photos are dropped silently.
            let draft_photos: Vec<PhotoObject> = draft
                .items
                .iter()
                .filter_map(|entry| {
                    crate::db::photo(&connection.borrow(), entry.id)
                        .ok()
                        .flatten()
                })
                .map(|photo| PhotoObject::from_photo(&photo))
                .collect();
            on_open(draft_photos, Some(draft.clone()));
        });
    }
    {
        let on_open = on_open.clone();
        let fresh_photos = photos.clone();
        dialog.connect_response(Some("fresh"), move |_, _| {
            
            on_open(fresh_photos.clone(), None);
        });
    }
    dialog.present(Some(parent));
}
