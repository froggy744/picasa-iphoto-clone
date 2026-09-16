fn history_button_updater(
    session: Rc<RefCell<EditSession>>,
    undo: gtk::Button,
    redo: gtk::Button,
) -> Rc<dyn Fn()> {
    Rc::new(move || {
        undo.set_sensitive(session.borrow().can_undo());
        redo.set_sensitive(session.borrow().can_redo());
    })
}

fn connect_history_actions(
    undo: &gtk::Button,
    redo: &gtk::Button,
    session: Rc<RefCell<EditSession>>,
    sync_controls: Rc<dyn Fn()>,
    queue_preview: Rc<dyn Fn()>,
    update_history_buttons: Rc<dyn Fn()>,
) {
    {
        let session = session.clone();
        let sync_controls = sync_controls.clone();
        let queue_preview = queue_preview.clone();
        let update_history_buttons = update_history_buttons.clone();
        undo.connect_clicked(move |_| {
            if session.borrow_mut().undo() {
                sync_controls();
                update_history_buttons();
                queue_preview();
            }
        });
    }
    {
        let session = session.clone();
        let sync_controls = sync_controls.clone();
        let queue_preview = queue_preview.clone();
        let update_history_buttons = update_history_buttons.clone();
        redo.connect_clicked(move |_| {
            if session.borrow_mut().redo() {
                sync_controls();
                update_history_buttons();
                queue_preview();
            }
        });
    }
}
