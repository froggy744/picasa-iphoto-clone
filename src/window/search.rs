fn update_folder_suggestions(
    revealer: &gtk::Revealer,
    list: &gtk::Box,
    folders: &[db::Folder],
    query: &str,
    on_folder: Rc<dyn Fn(i64)>,
) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }

    if query.is_empty() {
        revealer.set_reveal_child(false);
        eprintln!("SEARCH TRACE query=\"\" folder_matches=0");
        return;
    }

    let query = query.to_lowercase();
    let matches = folders
        .iter()
        .filter(|folder| folder.name.to_lowercase().contains(&query))
        .take(8)
        .collect::<Vec<_>>();

    eprintln!(
        "SEARCH TRACE query={:?} folder_matches={}",
        query,
        matches.len()
    );
    for folder in &matches {
        eprintln!(
            "SEARCH TRACE folder_match id={} name={:?} path={:?}",
            folder.id,
            folder.name,
            folder.path
        );

        let button = gtk::Button::new();
        button.set_halign(gtk::Align::Fill);
        button.set_focusable(false);
        button.add_css_class("flat");
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.set_margin_start(8);
        row.set_margin_end(8);
        row.set_margin_top(5);
        row.set_margin_bottom(5);
        row.append(&gtk::Image::from_icon_name("folder-symbolic"));
        let labels = gtk::Box::new(gtk::Orientation::Vertical, 1);
        let name = gtk::Label::new(Some(&folder.name));
        name.set_xalign(0.0);
        let path = gtk::Label::new(Some(&folder.path));
        path.set_xalign(0.0);
        path.add_css_class("dim-label");
        path.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        labels.append(&name);
        labels.append(&path);
        labels.set_hexpand(true);
        row.append(&labels);
        button.set_child(Some(&row));

        let folder_id = folder.id;
        let revealer = revealer.clone();
        let on_folder = on_folder.clone();
        button.connect_clicked(move |_| {
            revealer.set_reveal_child(false);
            on_folder(folder_id);
        });
        list.append(&button);
    }

    if matches.is_empty() || query.chars().count() < 2 {
        revealer.set_reveal_child(false);
    } else {
        revealer.set_reveal_child(true);
    }
}
