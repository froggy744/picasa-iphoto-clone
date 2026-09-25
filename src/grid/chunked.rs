//! Minimal nested ListView/GridView experiment. Each outer row owns one
//! bounded photo slice; there is deliberately only one vertical scroller.

use gio::prelude::*;
use gtk::prelude::*;
use gtk4 as gtk;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

const PHOTOS_PER_CHUNK: u32 = 64;

#[derive(Clone)]
pub(crate) struct ChunkedPrototype {
    pub(crate) root: gtk::ListView,
    grids: Rc<RefCell<Vec<glib::WeakRef<gtk::GridView>>>>,
    store: gio::ListStore,
    chunks: gio::ListStore,
    reconcile_pending: Rc<Cell<bool>>,
}

/// Clear the models of grids beyond the surviving chunk count, then reconcile
/// the outer chunk model with the store. Splitting the surplus GridViews'
/// models away before `sync_chunks` lets those rows finalize cleanly.
///
/// This must never run reentrantly from inside the base store's items-changed
/// dispatch: a `GtkGridView`'s model is a `GtkSliceListModel` over the same
/// store, so swapping it mid-emission re-enters GTK's list item manager while
/// it is mid-bind, which segfaulted in `gtk_list_item_manager_remove_items`.
/// Run it only from an idle source or a call site outside the emission.
fn reconcile_chunks(
    store: &gio::ListStore,
    chunks: &gio::ListStore,
    grids: &Rc<RefCell<Vec<glib::WeakRef<gtk::GridView>>>>,
) {
    let want = store.n_items().div_ceil(PHOTOS_PER_CHUNK);
    if want == chunks.n_items() {
        return;
    }
    for grid_weak in grids.borrow().iter().skip(want as usize) {
        if let Some(grid) = grid_weak.upgrade() {
            grid.set_model(None::<&gtk::NoSelection>);
        }
    }
    sync_chunks(store, chunks);
}

impl ChunkedPrototype {
    pub(crate) fn new(store: &gio::ListStore, photo_factory: &gtk::SignalListItemFactory) -> Self {
        let chunks = gio::ListStore::new::<gtk::SliceListModel>();
        let chunks_for_updates = chunks.clone();
        let chunks_for_struct = chunks.clone();
        let store_for_updates = store.clone();
        let grids: Rc<RefCell<Vec<glib::WeakRef<gtk::GridView>>>> = Rc::new(RefCell::new(Vec::new()));
        let grids_for_sync = grids.clone();
        // Re-modeling the surplus GridViews synchronously inside the base
        // store's items-changed dispatch re-enters GTK's list item manager
        // while it is mid-emission, which crashed in
        // gtk_list_item_manager_remove_items (a `GtkGridView` cannot swap its
        // model from within the signal that bounds its own items). Defer the
        // chunk reconciliation to an idle so the emission has unwound; bursts
        // of items-changed coalesce onto a single pending reconcile.
        let reconcile_pending = Rc::new(Cell::new(false));
        let reconcile_pending_for_sync = reconcile_pending.clone();
        let reconcile_pending_for_struct = reconcile_pending.clone();
        store.connect_items_changed(move |_, _, _, _| {
            if reconcile_pending.replace(true) {
                return;
            }
            let cols = store_for_updates.clone();
            let chunks = chunks_for_updates.clone();
            let grids = grids_for_sync.clone();
            let pending = reconcile_pending_for_sync.clone();
            glib::idle_add_local_once(move || {
                // Dropping the last photos at teardown lets their inner
                // GridViews die while still realized with a model, which GTK
                // flags as "Finalizing GtkGridView with a model." Rows are
                // otherwise bound in setup order, so grids beyond the
                // surviving chunk count are the ones this sync splices away;
                // give those grids no model first so they finalize cleanly.
                // Rows the ListView recycles are not touched here (their
                // unbind clears the model instead).
                pending.set(false);
                reconcile_chunks(&cols, &chunks, &grids);
            });
        });
        sync_chunks(store, &chunks);

        let model = gtk::NoSelection::new(Some(chunks));
        let factory = gtk::SignalListItemFactory::new();
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
            grids_for_setup.borrow_mut().push(grid.downgrade());
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
        Self {
            root,
            grids,
            store: store.clone(),
            chunks: chunks_for_struct,
            reconcile_pending: reconcile_pending_for_struct,
        }
    }

    /// Flush any reconciliation still pending from the last items-changed
    /// burst. The outer chunk list must contain the target chunk before
    /// `scroll_to` runs or GTK clamps the scroll to the last existing row.
    fn flush_reconcile(&self) {
        if !self.reconcile_pending.replace(false) {
            return;
        }
        reconcile_chunks(&self.store, &self.chunks, &self.grids);
    }

    /// Unmount the outer chunk rows before a wholesale base-store
    /// replacement. With no mounted chunk GridViews, the splice's
    /// items-changed dispatch becomes plain GObject disposal instead of
    /// per-chunk list item manager work across hundreds of slices
    /// (measured ~700 ms for a 22k-photo folder to a small album view).
    /// Call from outside any items-changed emission, pair with
    /// `reattach_after_replace` on a later idle.
    pub(crate) fn detach_for_replace(&self) {
        self.root.set_model(None::<&gtk::NoSelection>);
    }

    /// Re-mount the outer chunk model after a replacement splice. The
    /// pending reconcile (if any) must run first so the chunk list matches
    /// the new store size; `flush_reconcile` is a no-op otherwise.
    pub(crate) fn reattach_after_replace(&self) {
        self.flush_reconcile();
        let model = gtk::NoSelection::new(Some(self.chunks.clone()));
        self.root.set_model(Some(&model));
    }

    pub(crate) fn set_columns(&self, columns: u32) {
        let columns = columns.max(1);
        let mut grids = self.grids.borrow_mut();
        grids.retain(|weak| {
            let Some(grid) = weak.upgrade() else {
                return false;
            };
            grid.set_min_columns(columns);
            grid.set_max_columns(columns);
            grid.queue_resize();
            true
        });
    }

    /// Scroll the visible outer chunk list so the photo at `position` in the
    /// shared stream is revealed by its owning chunk's GridView. Because only
    /// the outer ListView scrolls, this lands at the top of the enclosing
    /// chunk; a later step can additionally scroll the inner grid by the
    /// intra-chunk offset.
    pub(crate) fn scroll_to_photo(&self, position: usize) {
        self.flush_reconcile();
        let chunk_index = position as u32 / PHOTOS_PER_CHUNK;
        self.root
            .scroll_to(chunk_index, gtk::ListScrollFlags::FOCUS, None);
        self.root.grab_focus();
    }
}

/// Incrementally keep one `GtkSliceListModel` tail per `PHOTOS_PER_CHUNK`
/// photos of `store`. A full rebuild is unnecessary: each `GtkSliceListModel`
/// connects to the base model itself and forwards `items-changed` into its own
/// window using absolute offsets (the slice translates position/removed/added
/// in its items-changed handler). Slices are pinned at absolute
/// `[i * PHOTOS_PER_CHUNK, i * PHOTOS_PER_CHUNK + PHOTOS_PER_CHUNK)` of the
/// store, so an existing slice's offset never needs to change as the store
/// mutates, and a fixed size of `PHOTOS_PER_CHUNK` lets the tail slice's
/// `n-items` grow/shrink with the base (`get-n-items` clamps to what is
/// available). Therefore the outer chunks model only changes when the chunk
/// *count* changes: append a new tail slice, or splice surplus tails away.
/// Same-count replacements and appends that land inside an existing window
/// need no outer mutation at all — destroying/recreating slices there is what
/// caused the mass outer-row rebind churn.
fn sync_chunks(store: &gio::ListStore, chunks: &gio::ListStore) {
    let want = store.n_items().div_ceil(PHOTOS_PER_CHUNK);
    let have = chunks.n_items();
    if want == have {
        return;
    }
    if want > have {
        let mut created = 0u32;
        let mut index = have;
        while index < want {
            chunks.append(&gtk::SliceListModel::new(
                Some(store.clone()),
                index * PHOTOS_PER_CHUNK,
                PHOTOS_PER_CHUNK,
            ));
            index += 1;
            created += 1;
        }
        if std::env::var_os("PICASA_TRACE").is_some() {
            eprintln!(
                "PIC_CHUNK sync action=create chunks={} created={} removed=0",
                want, created
            );
        }
    } else {
        let removed = have - want;
        chunks.splice(want, removed, &[] as &[gtk::SliceListModel]);
        if std::env::var_os("PICASA_TRACE").is_some() {
            eprintln!(
                "PIC_CHUNK sync action=truncate chunks={} created=0 removed={}",
                want, removed
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // `gtk::SliceListModel::new` asserts that `gtk::init` ran on the calling
    // thread, and `gtk::init` panics when a second thread initializes GTK.
    // Follow the repo's display-test convention: keep these scenarios out of
    // the parallel default harness with #[ignore] and initialize GTK on the
    // harness thread, which is single when run with `--test-threads=1`.
    fn init_gtk() {
        gtk::init().unwrap();
    }

    fn strings(start: u32, count: u32) -> Vec<gtk::StringObject> {
        (start..start + count)
            .map(|i| gtk::StringObject::new(&format!("photo-{i}")))
            .collect()
    }

    /// Mirrors `ChunkedPrototype::new`'s model wiring: connect `sync_chunks`
    /// to the store, then perform the initial sync.
    fn wired() -> (gio::ListStore, gio::ListStore) {
        let store = gio::ListStore::new::<gtk::StringObject>();
        let chunks = gio::ListStore::new::<gtk::SliceListModel>();
        let store_for_updates = store.clone();
        let chunks_for_updates = chunks.clone();
        store.connect_items_changed(move |_, _, _, _| {
            sync_chunks(&store_for_updates, &chunks_for_updates);
        });
        sync_chunks(&store, &chunks);
        (store, chunks)
    }

    fn capture_chunks(chunks: &gio::ListStore) -> Vec<glib::Object> {
        (0..chunks.n_items())
            .map(|i| chunks.item(i).expect("chunk item"))
            .collect()
    }

    fn slice(chunks: &gio::ListStore, index: u32) -> gtk::SliceListModel {
        chunks
            .item(index)
            .and_downcast::<gtk::SliceListModel>()
            .expect("slice")
    }

    fn record_outer_changes(chunks: &gio::ListStore) -> Rc<RefCell<Vec<(u32, u32, u32)>>> {
        let log = Rc::new(RefCell::new(Vec::new()));
        let log_for_handler = log.clone();
        chunks.connect_items_changed(move |_, position, removed, added| {
            log_for_handler.borrow_mut().push((position, removed, added));
        });
        log
    }

    #[test]
    #[ignore = "requires a GTK display; run with --ignored --test-threads=1"]
    fn initial_sync_on_empty_store_then_fill() {
        init_gtk();
        let store = gio::ListStore::new::<gtk::StringObject>();
        let chunks = gio::ListStore::new::<gtk::SliceListModel>();
        sync_chunks(&store, &chunks);
        assert_eq!(chunks.n_items(), 0);

        store.extend_from_slice(&strings(0, 100));
        sync_chunks(&store, &chunks);
        assert_eq!(chunks.n_items(), 2);

        let first = slice(&chunks, 0);
        assert_eq!(first.offset(), 0);
        assert_eq!(first.size(), PHOTOS_PER_CHUNK);
        assert_eq!(first.n_items(), 64);

        let second = slice(&chunks, 1);
        assert_eq!(second.offset(), PHOTOS_PER_CHUNK);
        assert_eq!(second.size(), PHOTOS_PER_CHUNK);
        assert_eq!(second.n_items(), 36);
    }

    #[test]
    #[ignore = "requires a GTK display; run with --ignored --test-threads=1"]
    fn tail_slice_grows_without_replacing_and_new_tail_appears() {
        init_gtk();
        let (store, chunks) = wired();
        store.extend_from_slice(&strings(0, 100));
        assert_eq!(chunks.n_items(), 2);
        let before0 = chunks.item(0).expect("chunk0");
        let before1 = chunks.item(1).expect("chunk1");

        store.extend_from_slice(&strings(100, 40));
        assert_eq!(chunks.n_items(), 3);
        assert_eq!(chunks.item(0).expect("chunk0"), before0);
        assert_eq!(chunks.item(1).expect("chunk1"), before1);

        assert_eq!(slice(&chunks, 1).n_items(), 64);
        let third = slice(&chunks, 2);
        assert_eq!(third.offset(), 128);
        assert_eq!(third.size(), PHOTOS_PER_CHUNK);
        assert_eq!(third.n_items(), 12);
    }

    #[test]
    #[ignore = "requires a GTK display; run with --ignored --test-threads=1"]
    fn progressive_batches_keep_chunk_identity_and_offsets() {
        init_gtk();
        let (store, chunks) = wired();
        let mut total = 0u32;
        let mut next_id = 0u32;
        let mut previous: Vec<glib::Object> = Vec::new();

        for batch in 0..4u32 {
            let items = strings(next_id, 2000);
            if batch == 0 {
                store.splice(0, 0, &items);
            } else {
                store.splice(store.n_items(), 0, &items);
            }
            next_id += 2000;
            total += 2000;

            let want = total.div_ceil(PHOTOS_PER_CHUNK);
            let now = capture_chunks(&chunks);
            assert_eq!(now.len() as u32, want, "batch {batch} total {total}");
            for (i, item) in now.iter().enumerate() {
                let s = item
                    .clone()
                    .downcast::<gtk::SliceListModel>()
                    .expect("slice");
                assert_eq!(s.offset(), i as u32 * PHOTOS_PER_CHUNK, "batch {batch}");
                assert_eq!(s.size(), PHOTOS_PER_CHUNK, "batch {batch}");
            }
            for (i, item) in previous.iter().enumerate() {
                assert_eq!(now[i], *item, "batch {batch} replaced prefix chunk {i}");
            }
            previous = now;
        }
    }

    #[test]
    #[ignore = "requires a GTK display; run with --ignored --test-threads=1"]
    fn same_count_full_replacement_leaves_outer_model_untouched() {
        init_gtk();
        let (store, chunks) = wired();
        store.extend_from_slice(&strings(0, 100));
        assert_eq!(chunks.n_items(), 2);
        let before = capture_chunks(&chunks);
        let changes = record_outer_changes(&chunks);

        store.splice(0, store.n_items(), &strings(1000, 100));

        assert_eq!(chunks.n_items(), 2);
        assert_eq!(capture_chunks(&chunks), before);
        assert!(
            changes.borrow().is_empty(),
            "outer chunks model must not emit items-changed on same-count replace, got {:?}",
            changes.borrow()
        );
    }

    #[test]
    #[ignore = "requires a GTK display; run with --ignored --test-threads=1"]
    fn truncation_splices_only_surplus_tail_chunks() {
        init_gtk();
        let (store, chunks) = wired();
        store.extend_from_slice(&strings(0, 200));
        assert_eq!(chunks.n_items(), 4);
        let before = capture_chunks(&chunks);
        let changes = record_outer_changes(&chunks);

        store.splice(100, 100, &[] as &[gtk::StringObject]);

        assert_eq!(chunks.n_items(), 2);
        assert_eq!(chunks.item(0).expect("chunk0"), before[0]);
        assert_eq!(chunks.item(1).expect("chunk1"), before[1]);
        let log = changes.borrow();
        assert_eq!(log.len(), 1, "expected one outer splice, got {log:?}");
        assert_eq!(log[0], (2, 2, 0));
    }

    #[test]
    #[ignore = "requires a GTK display; run with --ignored --test-threads=1"]
    fn pure_appends_only_grow_the_chunks_model() {
        init_gtk();
        let (store, chunks) = wired();
        store.extend_from_slice(&strings(0, 30));
        assert_eq!(chunks.n_items(), 1);
        let first = chunks.item(0).expect("chunk0");
        let changes = record_outer_changes(&chunks);

        store.extend_from_slice(&strings(30, 20));
        assert_eq!(chunks.n_items(), 1);
        assert!(changes.borrow().is_empty());
        assert_eq!(chunks.item(0).expect("chunk0"), first);

        store.extend_from_slice(&strings(50, 20));
        assert_eq!(chunks.n_items(), 2);
        assert_eq!(chunks.item(0).expect("chunk0"), first);
        assert_eq!(*changes.borrow(), vec![(1, 0, 1)]);

        store.extend_from_slice(&strings(70, 50));
        assert_eq!(chunks.n_items(), 2);
        assert_eq!(chunks.item(0).expect("chunk0"), first);
        assert_eq!(*changes.borrow(), vec![(1, 0, 1)]);

        store.extend_from_slice(&strings(120, 10));
        assert_eq!(chunks.n_items(), 3);
        assert_eq!(chunks.item(0).expect("chunk0"), first);
        assert_eq!(*changes.borrow(), vec![(1, 0, 1), (2, 0, 1)]);
    }
}
