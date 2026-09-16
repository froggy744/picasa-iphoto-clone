fn connect_export_action(
    export: &gtk::Button,
    parent: gtk::Window,
    photo: crate::photo_object::PhotoObject,
    session: Rc<RefCell<EditSession>>,
    active_rotation: Rc<Cell<i32>>,
    apply_crop: Rc<dyn Fn()>,
) {
    export.connect_clicked(move |_| {
        apply_crop();
        let dialog = gtk::FileChooserNative::new(
            Some("Export Edited Photo"),
            Some(&parent),
            gtk::FileChooserAction::Save,
            Some("Export"),
            Some("Cancel"),
        );
        let filename = std::path::Path::new(&photo.filename())
            .file_stem()
            .and_then(|value| value.to_str())
            .map(|stem| format!("{stem}-edited.jpg"))
            .unwrap_or_else(|| "export-edited.jpg".to_string());
        dialog.set_current_name(&filename);

        let reference = photo.path();
        let rotation = active_rotation.get();
        let source_width = photo.width();
        let source_height = photo.height();
        let session = session.clone();
        dialog.connect_response(move |dialog, response| {
            if response == gtk::ResponseType::Accept {
                if let Some(destination) = dialog.file().and_then(|file| file.path()) {
                    let edit_recipe = session.borrow().recipe.encode();
                    let reference = reference.clone();
                    std::thread::spawn(move || {
                        let result = super::render::render_for_export(
                            &reference,
                            rotation,
                            &edit_recipe,
                            source_width,
                            source_height,
                        )
                        .and_then(|image| super::render::save_jpeg(&image, &destination, 92));
                        if let Err(error) = result {
                            eprintln!("Could not export edited photo: {error:#}");
                        }
                    });
                }
            }
            dialog.destroy();
        });
        dialog.show();
    });
}
