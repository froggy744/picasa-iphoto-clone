//! Minimal nested ListView/GridView experiment. Each outer row owns one
//! bounded photo slice; there is deliberately only one vertical scroller.

use gio::prelude::*;
use gtk::prelude::*;
use gtk4 as gtk;
use std::cell::RefCell;
use std::rc::Rc;

const PHOTOS_PER_CHUNK: u32 = 64;

#[derive(Clone)]
pub(crate) struct ChunkedPrototype {
    pub(crate) root: gtk::ListView,
    grids: Rc<RefCell<Vec<gtk::GridView>>>,
}

impl ChunkedPrototype {
    pub(crate) fn new(store: &gio::ListStore, photo_factory: &gtk::SignalListItemFactory) -> Self {
        let chunks = gio::ListStore::new::<gtk::SliceListModel>();
        rebuild_chunks(store, &chunks);
        let chunks_for_updates = chunks.clone();
        let store_for_updates = store.clone();
        store.connect_items_changed(move |_, _, _, _| {
            rebuild_chunks(&store_for_updates, &chunks_for_updates);
        });

        let model = gtk::NoSelection::new(Some(chunks));
        let factory = gtk::SignalListItemFactory::new();
        let grids = Rc::new(RefCell::new(Vec::new()));
        let grids_for_setup = grids.clone();
        let item_factory = photo_factory.clone();
        factory.connect_setup(move |_, object| {
            let Some(item) = object.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let grid = gtk::GridView::new(None::<gtk::NoSelection>, Some(item_factory.clone()));
            grid.set_min_columns(5);
            grid.set_max_columns(5);
            grid.set_single_click_activate(false);
            grid.set_hexpand(true);
            grid.set_halign(gtk::Align::Fill);
            grid.add_css_class("section-grid");
            item.set_child(Some(&grid));
            grids_for_setup.borrow_mut().push(grid);
        });
        factory.connect_bind(|_, object| {
            let Some(item) = object.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let Some(slice) = item.item().and_downcast::<gtk::SliceListModel>() else {
                return;
            };
            let Some(grid) = item.child().and_downcast::<gtk::GridView>() else {
                return;
            };
            grid.set_model(Some(&gtk::NoSelection::new(Some(slice))));
        });
        factory.connect_unbind(|_, object| {
            if let Some(item) = object.downcast_ref::<gtk::ListItem>() {
                if let Some(grid) = item.child().and_downcast::<gtk::GridView>() {
                    grid.set_model(None::<&gtk::NoSelection>);
                }
            }
        });

        let root = gtk::ListView::new(Some(model), Some(factory));
        root.set_show_separators(false);
        root.set_hexpand(true);
        root.set_vexpand(true);
        root.add_css_class("folder-stream");
        Self { root, grids }
    }

    pub(crate) fn set_columns(&self, columns: u32) {
        let columns = columns.max(1);
        for grid in self.grids.borrow().iter() {
            grid.set_min_columns(columns);
            grid.set_max_columns(columns);
            grid.queue_resize();
        }
    }
}

fn rebuild_chunks(store: &gio::ListStore, chunks: &gio::ListStore) {
    chunks.remove_all();
    let mut start = 0;
    let total = store.n_items();
    while start < total {
        let size = PHOTOS_PER_CHUNK.min(total - start);
        chunks.append(&gtk::SliceListModel::new(Some(store.clone()), start, size));
        start += size;
    }
}
