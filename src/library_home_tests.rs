use super::*;

fn fixture() -> (std::path::PathBuf, rusqlite::Connection) {
    let path = std::env::temp_dir().join(format!(
        "pic-home-{}-{}.db",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap()
    ));
    let connection = db::open(&path).unwrap();
    connection.execute("INSERT INTO photos(id,path,favorite) VALUES (1,'smb://offline/share/one.jpg',1),(2,'smb://offline/share/two.jpg',0)", []).unwrap();
    db::set_edit_recipe(&connection, 1, "edited").unwrap();
    db::create_album(&connection, "Family").unwrap();
    (path, connection)
}

#[test]
fn home_worker_refreshes_favorites_from_another_connection() {
    let (path, connection) = fixture();
    let (requests, receiver) = mpsc::sync_channel(1);
    let (sender, snapshots) = mpsc::channel();
    let worker_path = path.clone();
    let thread = std::thread::spawn(move || worker(worker_path, receiver, sender));
    requests.send(true).unwrap();
    let initial = snapshots
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        initial
            .data
            .favorites
            .iter()
            .map(|p| p.id)
            .collect::<Vec<_>>(),
        vec![1]
    );
    assert!(initial.images.is_empty());
    db::set_favorite(&connection, 1, false).unwrap();
    db::set_favorite(&connection, 2, true).unwrap();
    requests.send(false).unwrap();
    let refreshed = snapshots
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        refreshed
            .data
            .favorites
            .iter()
            .map(|p| p.id)
            .collect::<Vec<_>>(),
        vec![2]
    );
    assert_eq!(db::history_photos(&connection).unwrap().len(), 1);
    requests.send(false).unwrap();
    assert!(snapshots
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap()
        .is_none());
    drop(requests);
    thread.join().unwrap();
    drop(connection);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn home_cached_thumbnail_does_not_delete_or_regenerate_corrupt_cache() {
    let path = std::env::temp_dir().join(format!("pic-home-cache-{}.jpg", std::process::id()));
    std::fs::write(&path, b"corrupt cached thumbnail").unwrap();
    let mut request = thumbnail_display::request_for(
        path.to_string_lossy().into_owned(),
        "smb://offline/original.jpg".into(),
        0,
        0,
        0,
        String::new(),
        0,
        0,
        false,
    );
    assert!(matches!(
        thumbnail_display::load_cached_display_thumbnail(&request),
        thumbnail_display::DisplayOutcome::Failed
    ));
    assert_eq!(std::fs::read(&path).unwrap(), b"corrupt cached thumbnail");
    image::RgbImage::from_pixel(4, 2, image::Rgb([200, 40, 40]))
        .save(&path)
        .unwrap();
    request.rotation = 90;
    match thumbnail_display::load_cached_display_thumbnail(&request) {
        thumbnail_display::DisplayOutcome::Loaded {
            width,
            height,
            pixels,
        } => {
            assert_eq!((width, height), (2, 4));
            assert_eq!(pixels.len(), 32);
        }
        outcome => panic!("Expected cached image for unavailable original, got {outcome:?}"),
    }
    std::fs::remove_file(path).unwrap();
}

fn collect<T: IsA<gtk::Widget> + glib::object::ObjectType>(
    widget: &gtk::Widget,
    result: &mut Vec<T>,
) {
    if let Ok(item) = widget.clone().downcast::<T>() {
        result.push(item);
    }
    let mut child = widget.first_child();
    while let Some(item) = child {
        child = item.next_sibling();
        collect(&item, result);
    }
}

#[test]
#[ignore = "requires a GTK display; run with --ignored --test-threads=1"]
fn home_page_sections_navigation_and_favorite_refresh() {
    libadwaita::init().unwrap();
    let (path, connection) = fixture();
    let destinations = Rc::new(std::cell::RefCell::new(Vec::new()));
    let opened = Rc::new(std::cell::RefCell::new(Vec::new()));
    let home = LibraryHome::new(
        path.clone(),
        {
            let destinations = destinations.clone();
            Rc::new(move |filter| destinations.borrow_mut().push(filter))
        },
        {
            let opened = opened.clone();
            Rc::new(move |photos, index, edited| {
                opened.borrow_mut().push((photos[index].id(), edited))
            })
        },
    );
    let window = gtk::Window::new();
    window.set_default_size(900, 700);
    window.set_child(Some(&home.root));
    window.present();
    let context = glib::MainContext::default();
    let mut sections = Vec::<gtk::FlowBox>::new();
    collect(home.root.upcast_ref(), &mut sections);
    let wait_for = |ready: &dyn Fn() -> bool| {
        let start = std::time::Instant::now();
        while !ready() && start.elapsed() < Duration::from_secs(8) {
            context.iteration(false);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(ready(), "Home Page did not update before timeout");
    };
    wait_for(&|| sections[0].child_at_index(1).is_some());
    if let Some(path) = std::env::var_os("PIC_HOME_SCREENSHOT") {
        // Let the newly populated FlowBoxes receive their first allocation.
        wait_for(&|| sections[0].child_at_index(0).unwrap().width() > 0);
        let snapshot = gtk::Snapshot::new();
        gtk::WidgetPaintable::new(Some(&window)).snapshot(
            &snapshot,
            window.width() as f64,
            window.height() as f64,
        );
        let node = snapshot.to_node().unwrap();
        window
            .renderer()
            .unwrap()
            .render_texture(&node, None)
            .save_to_png(path)
            .unwrap();
    }
    let mut buttons = Vec::<gtk::Button>::new();
    collect(home.root.upcast_ref(), &mut buttons);
    for button in buttons
        .iter()
        .filter(|b| matches!(b.label().as_deref(), Some("View All" | "View History")))
    {
        button.emit_clicked();
    }
    assert_eq!(
        *destinations.borrow(),
        vec![
            SidebarFilter::RecentlyAdded,
            SidebarFilter::History,
            SidebarFilter::Favorites,
            SidebarFilter::Albums
        ]
    );
    let click_card = |section: usize| {
        let mut buttons = Vec::<gtk::Button>::new();
        collect(sections[section].upcast_ref(), &mut buttons);
        buttons[0].emit_clicked();
    };
    click_card(1);
    click_card(2);
    click_card(3);
    assert_eq!(*opened.borrow(), vec![(1, true), (1, false)]);
    assert_eq!(destinations.borrow().last(), Some(&SidebarFilter::Album(1)));
    db::set_favorite(&connection, 1, false).unwrap();
    wait_for(&|| {
        let mut labels = Vec::<gtk::Label>::new();
        collect(sections[2].upcast_ref(), &mut labels);
        labels
            .iter()
            .any(|label| label.text() == "No favourites yet")
    });
    window.destroy();
    drop(home);
    while context.pending() {
        context.iteration(false);
    }
    drop(connection);
    std::fs::remove_file(path).unwrap();
}
