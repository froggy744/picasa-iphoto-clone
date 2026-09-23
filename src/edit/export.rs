fn connect_export_action(
    export: &gtk::Button,
    photo: crate::photo_object::PhotoObject,
    session: Rc<RefCell<EditSession>>,
    active_rotation: Rc<Cell<i32>>,
    apply_crop: Rc<dyn Fn()>,
    start_export: Rc<dyn Fn(Vec<super::export_batch::ExportJob>)>,
) {
    export.connect_clicked(move |_| {
        apply_crop();
        let recipe = session.borrow().recipe.encode();
        let job = super::export_batch::ExportJob::from_record(
            photo.path(),
            active_rotation.get(),
            recipe,
            photo.width(),
            photo.height(),
            &photo.filename(),
        );
        start_export(vec![job]);
    });
}
