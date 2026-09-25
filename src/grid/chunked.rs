//! Minimal nested ListView/GridView experiment. Each outer row owns one
//! bounded photo slice; there is deliberately only one vertical scroller.

use gio::prelude::*;
use gtk::prelude::*;
use gtk4 as gtk;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

const PHOTOS_PER_CHUNK: u32 = 64;
/// Columns each chunk's inner GridView is pinned to, so every chunk row's
/// final height is known before a single tile is loaded.
const CHUNK_COLUMNS: u32 = 5;
/// Vertical pitch of one tile line inside an inner GridView: the tile plus
/// the per-item padding the shared photo-grid CSS applies (6px top/bottom).
const CHUNK_LINE_SPACING: i32 = 12;
/// How many chunk rows beyond the viewport keep their photo models mounted
/// in each scroll direction. Keeps mounted tiles bounded while making
/// ordinary scrolling and small jumps seamless.
const REALIZATION_OVERSCAN_CHUNKS: i64 = 4;

/// Final height of one chunk row holding `items` photos on `CHUNK_COLUMNS`
/// lines. Wrapped around the inner GridView so GtkListView measures the row's
/// FINAL size immediately: without it, rows measure a few px while their
/// photos stream in, the list reports the whole stream as one viewport tall
/// (upper == page), treats every row as visible, and mounts hundreds of
/// chunk GridViews that then cost ~600 ms to tear down.
fn chunk_row_height_px(items: u32, tile_height: i32, columns: u32) -> i32 {
    let lines = (items as f64 / columns.max(1) as f64).ceil() as i32;
    lines * (tile_height.max(1) + CHUNK_LINE_SPACING)
}

/// Photo count the `position`-th chunk would hold in a store of `total_items`
/// photos: a full chunk, or only the remainder for the final one.
fn filled_items_for_chunk(total_items: u32, position: u32) -> u32 {
    total_items
        .saturating_sub(position * PHOTOS_PER_CHUNK)
        .min(PHOTOS_PER_CHUNK)
}

/// Inclusive `[start, end]` window of chunk indices whose photo models stay
/// mounted for a viewport centered on chunk row `center_chunk`, bounded by
/// the total chunk count. Everything outside the window is an empty wrapper:
/// a bounded mount cost regardless of how far the user scrolls.
pub(crate) fn realization_window(center_chunk: i64, total_chunks: u32) -> (u32, u32) {
    if total_chunks == 0 {
        return (0, 0);
    }
    let center = center_chunk.clamp(0, total_chunks as i64 - 1);
    let start = (center - REALIZATION_OVERSCAN_CHUNKS).max(0) as u32;
    let end = (center + REALIZATION_OVERSCAN_CHUNKS).max(0).min(total_chunks as i64 - 1) as u32;
    (start, end)
}

/// Whether chunk `position` should keep its photo models mounted: the stream
/// fill must be complete, the chunk's own slice fully filled, and the chunk
/// within `REALIZATION_OVERSCAN_CHUNKS` chunk-rows of the viewport center.
/// During a progressive fill slices measure a few px tall and the adjustment
/// upper collapses to one viewport, so unmounted chunks must stay model-less
/// until measurement is trustworthy again.
fn chunk_in_realization_window(
    position: u32,
    viewport_center_chunks: f64,
    slice_n_items: u32,
    total_items: u32,
    columns: u32,
    fill_complete: bool,
) -> bool {
    let filled = filled_items_for_chunk(total_items, position);
    if !fill_complete || slice_n_items < filled {
        return false;
    }
    let distance = (position as f64 - viewport_center_chunks).abs()
        - (filled as f64 / columns.max(1) as f64);
    distance <= REALIZATION_OVERSCAN_CHUNKS as f64
}

#[derive(Clone)]
pub(crate) struct ChunkedPrototype {
    pub(crate) root: gtk::ListView,
    grids: Rc<RefCell<Vec<glib::WeakRef<gtk::GridView>>>>,
    store: gio::ListStore,
    chunks: gio::ListStore,
    reconcile_pending: Rc<Cell<bool>>,
    /// (tile_height, columns) the wrapper height requests are computed from.
    metrics: Rc<Cell<(i32, u32)>>,
    /// Chunk-row wrapper boxes, for metric updates on already-realized rows.
    wrappers: Rc<RefCell<Vec<glib::WeakRef<gtk::Box>>>>,
    /// False while a Folder-stream fill is in progress. Rows must not mount
    /// photo models while their slice is only partly filled: GtkListBase
    /// caches the (tiny) measured row height at first materialization and
    /// averages it into the adjustment's upper estimate. A collapsed upper
    /// (measured upper == page_size == 872) makes GTK treat every row as
    /// visible, mounts all 205-354 chunk GridViews, and costs ~600 ms at the
    /// next cross-view teardown.
    fill_complete: Rc<Cell<bool>>,
    /// Pending post-fill remount, postponed while fill signals keep arriving
    /// (startup batches set building=false after every append).
    remount_pending: Rc<RefCell<Option<glib::SourceId>>>,
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
        let metrics = Rc::new(Cell::new((0_i32, CHUNK_COLUMNS)));
        let wrappers: Rc<RefCell<Vec<glib::WeakRef<gtk::Box>>>> =
            Rc::new(RefCell::new(Vec::new()));
        let fill_complete = Rc::new(Cell::new(false));
        let remount_pending: Rc<RefCell<Option<glib::SourceId>>> =
            Rc::new(RefCell::new(None));
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
        let bind_trace_root: Rc<RefCell<Option<gtk::ListView>>> = Rc::new(RefCell::new(None));
        let bind_trace_root_for_bind = bind_trace_root.clone();
        let metrics_for_setup = metrics.clone();
        let metrics_for_bind = metrics.clone();
        let fill_complete_for_bind = fill_complete.clone();
        let wrappers_for_setup = wrappers.clone();
        factory.connect_setup(move |_, object| {
            let Some(item) = object.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let grid = gtk::GridView::new(None::<gtk::NoSelection>, Some(item_factory.clone()));
            let (tile_height, columns) = metrics_for_setup.get();
            grid.set_min_columns(columns);
            grid.set_max_columns(columns);
            grid.set_single_click_activate(false);
            grid.set_hexpand(true);
            grid.set_halign(gtk::Align::Fill);
            // Fill the wrapper vertically: the wrapper owns the row height;
            // without vexpand the grid would render at its (possibly empty)
            // natural height and the row would look blank.
            grid.set_vexpand(true);
            grid.set_valign(gtk::Align::Fill);
            if std::env::var_os("PIC_BISECT_NO_CSS").is_none() {
                grid.add_css_class("section-grid");
            }
            // The wrapper owns the row height so the outer ListView can size
            // and virtualize rows whose photos have not streamed in yet.
            let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 0);
            wrapper.set_height_request(chunk_row_height_px(
                PHOTOS_PER_CHUNK,
                tile_height,
                columns,
            ));
            wrapper.append(&grid);
            item.set_child(Some(&wrapper));
            wrappers_for_setup.borrow_mut().push(wrapper.downgrade());
            grids_for_setup.borrow_mut().push(grid.downgrade());
            if crate::diagnostics::trace_enabled() {
                static SETUP_COUNT: std::sync::atomic::AtomicU32 =
                    std::sync::atomic::AtomicU32::new(0);
                let count =
                    SETUP_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                eprintln!(
                    "PIC_NAV chunk_factory setup n={count} t={}",
                    crate::diagnostics::t_ms()
                );
            }
        });
        let store_for_bind = store.clone();
        factory.connect_bind(move |_, object| {
            let self_root = bind_trace_root_for_bind.clone();
            let Some(item) = object.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let Some(slice) = item.item().and_downcast::<gtk::SliceListModel>() else {
                return;
            };
            let Some(wrapper) = item.child().and_downcast::<gtk::Box>() else {
                return;
            };
            let Some(grid) = wrapper.first_child().and_downcast::<gtk::GridView>() else {
                return;
            };
            // Correct the final partial chunk's wrapper height once the
            // position is known (setup optimistically assumes a full chunk).
            let position = item.position();
            let total = store_for_bind.n_items();
            let filled = total.saturating_sub(position * PHOTOS_PER_CHUNK).min(PHOTOS_PER_CHUNK);
            let (tile_height, columns) = metrics_for_bind.get();
            wrapper.set_height_request(chunk_row_height_px(
                filled.max(1),
                tile_height,
                columns,
            ));
            // Mount photos only when the fill is complete AND this chunk is
            // near the viewport (viewport-windowed realization). Everything
            // else stays a cheap empty wrapper: bounded mounted tiles, and
            // the row still measures its full wrapper height so the
            // adjustment upper spans the whole library.
            if crate::diagnostics::trace_enabled() {
                let wrapper_probe = wrapper.downgrade();
                let slice_probe = slice.clone();
                glib::timeout_add_local_once(std::time::Duration::from_millis(600), move || {
                    let Some(wrapper) = wrapper_probe.upgrade() else { return };
                    let allocated = wrapper.allocated_height();
                    if allocated == 0 {
                        return; // off-screen pre-roll row, never allocated
                    }
                    eprintln!(
                        "PIC_NAV chunk_wrapper_alloc allocated_h={allocated} request={} slice_items={} t={}",
                        wrapper.height_request(),
                        slice_probe.n_items(),
                        crate::diagnostics::t_ms()
                    );
                });
            }
            let row_height = chunk_row_height_px(PHOTOS_PER_CHUNK, tile_height, columns) as f64;
            let viewport_center = if row_height > 0.0 {
                let adjustment = self_root.borrow().as_ref().and_then(|root| root.vadjustment());
                adjustment
                    .map(|adj| (adj.value() + adj.page_size() / 2.0) / row_height)
                    .unwrap_or(0.0)
            } else {
                0.0
            };
            let mount = chunk_in_realization_window(
                position,
                viewport_center,
                slice.n_items(),
                total,
                columns,
                fill_complete_for_bind.get(),
            );
            if mount {
                grid.set_model(None::<&gtk::NoSelection>);
            } else {
                grid.set_model(Some(&gtk::NoSelection::new(Some(slice))));
            }
            if crate::diagnostics::trace_enabled() {
                static BIND_COUNT: std::sync::atomic::AtomicU32 =
                    std::sync::atomic::AtomicU32::new(0);
                let count = BIND_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                let adjust = self_root.borrow().as_ref().and_then(|root| {
                    let adjustment = root.vadjustment()?;
                    Some(format!(
                        "value={:.0} upper={:.0} page={:.0}",
                        adjustment.value(),
                        adjustment.upper(),
                        adjustment.page_size()
                    ))
                });
                eprintln!(
                    "PIC_NAV chunk_factory bind n={count} {} t={}",
                    adjust.unwrap_or_default(),
                    crate::diagnostics::t_ms()
                );
            }
        });
        factory.connect_unbind(|_, object| {
            if let Some(item) = object.downcast_ref::<gtk::ListItem>() {
                if let Some(grid) = item.child().and_downcast::<gtk::GridView>() {
                    grid.set_model(None::<&gtk::NoSelection>);
                }
            }
            if crate::diagnostics::trace_enabled() {
                static UNBIND_COUNT: std::sync::atomic::AtomicU32 =
                    std::sync::atomic::AtomicU32::new(0);
                let count = UNBIND_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                eprintln!(
                    "PIC_NAV chunk_factory unbind n={count} t={}",
                    crate::diagnostics::t_ms()
                );
            }
        });

        let root = gtk::ListView::new(Some(model), Some(factory));
        root.set_show_separators(false);
        root.set_hexpand(true);
        root.set_vexpand(true);
        if std::env::var_os("PIC_BISECT_NO_CSS").is_none() {
            root.add_css_class("folder-stream");
        }
        *bind_trace_root.borrow_mut() = Some(root.clone());
        let pending_self_clone = Self {
            root: root.clone(),
            grids: grids.clone(),
            store: store.clone(),
            chunks: chunks_for_struct.clone(),
            reconcile_pending: reconcile_pending_for_struct.clone(),
            metrics: metrics.clone(),
            wrappers: wrappers.clone(),
            fill_complete: fill_complete.clone(),
            remount_pending: remount_pending.clone(),
        };
        if crate::diagnostics::trace_enabled() {
            // Realization status: bounded-mounted-row evidence plus the
            // adjustment health of the outer list. Doubles as a self-heal:
            // if the post-fill remount raced the window's first allocations
            // and the upper collapsed to a degenerate value, re-run the
            // remount (bounded attempts) so measurement recovers.
            let status_root = root.downgrade();
            let fill_status = fill_complete.clone();
            let pending_status = remount_pending.clone();
            let metrics_status = metrics.clone();
            let recovery_attempts = Rc::new(Cell::new(0u8));
            let recovery_for_timer = recovery_attempts.clone();
            let prototype_for_status = pending_self_clone.clone();
            let wrappers_status = wrappers.clone();
            glib::timeout_add_local(std::time::Duration::from_millis(2000), move || {
                let Some(root) = status_root.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                let (value, upper, page) = root
                    .vadjustment()
                    .map(|adj| (adj.value(), adj.upper(), adj.page_size()))
                    .unwrap_or_default();
                let logical = root.model().map(|m| m.n_items()).unwrap_or_default();
                let (tile_height, columns) = metrics_status.get();
                let expected_upper =
                    chunk_row_height_px(PHOTOS_PER_CHUNK, tile_height, columns) as f64
                        * logical.max(1) as f64;
                let degenerate = fill_status.get()
                    && logical > 0
                    && upper + 1.0 < expected_upper * 0.5;
                if degenerate && recovery_for_timer.get() < 5 {
                    recovery_for_timer.set(recovery_for_timer.get() + 1);
                    eprintln!(
                        "PIC_NAV chunk_realization recovery remount attempt={} t={}",
                        recovery_for_timer.get(),
                        crate::diagnostics::t_ms()
                    );
                    prototype_for_status.remount_for_measurement();
                }
                let mounted = wrappers_status
                    .borrow()
                    .iter()
                    .filter(|weak| weak.upgrade().is_some())
                    .count();
                let parent_desc = root
                    .parent()
                    .map(|parent| {
                        format!(
                            " parent={} visible={} mapped={}",
                            parent.widget_name(),
                            parent.is_visible(),
                            parent.is_mapped()
                        )
                    })
                    .unwrap_or_default();
                eprintln!(
                    "PIC_NAV chunk_realization logical_chunks={logical} realized_wrappers={mounted} value={value:.0} upper={upper:.0} page={page:.0} expected={expected_upper:.0} self_visible={} self_mapped={}{} t={}",
                    root.is_visible(),
                    root.is_mapped(),
                    parent_desc,
                    crate::diagnostics::t_ms()
                );
                glib::ControlFlow::Continue
            });
        }
        Self {
            root: root.clone(),
            grids,
            store: store.clone(),
            chunks: chunks_for_struct,
            reconcile_pending: reconcile_pending_for_struct,
            metrics,
            wrappers,
            fill_complete: fill_complete.clone(),
            remount_pending: remount_pending.clone(),
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

    /// Called by the Gallery when a Folder-stream fill starts (`true`) and
    /// when it completes (`false`). On completion, the outer list's model is
    /// remounted once: every chunk row re-measures against its now-complete
    /// slice, so the adjustment upper spans the real library height and the
    /// realization window binds only chunks near the viewport.
    pub(crate) fn notify_stream_building(&self, building: bool) {
        if crate::diagnostics::trace_enabled() {
            eprintln!(
                "PIC_NAV chunk_realization notify building={building} fill_complete={} t={}",
                self.fill_complete.get(),
                crate::diagnostics::t_ms()
            );
        }
        if let Some(pending) = self.remount_pending.borrow_mut().take() {
            pending.remove();
        }
        if building {
            self.fill_complete.set(false);
            return;
        }
        if self.fill_complete.get() {
            return;
        }
        // Postpone the remount while fill signals keep arriving: startup
        // appends set building=false after every batch, so only a quiet
        // period (no new batches for 300 ms) means the stream is complete.
        let pending_self = self.clone();
        let source = glib::timeout_add_local(std::time::Duration::from_millis(300), move || {
            if pending_self.fill_complete.get() {
                return glib::ControlFlow::Break;
            }
            pending_self.fill_complete.set(true);
            pending_self.remount_pending.borrow_mut().take();
            if crate::diagnostics::trace_enabled() {
                eprintln!(
                    "PIC_NAV chunk_realization remount t={} chunks={} store={}",
                    crate::diagnostics::t_ms(),
                    pending_self.chunks.n_items(),
                    pending_self.store.n_items()
                );
            }
            // Fresh model pass: unmount everything and let the list rebind
            // with complete slices. Bind runs the realization window per
            // row, so only chunks near the viewport mount photo models.
            pending_self.root.set_model(None::<&gtk::NoSelection>);
            let model = gtk::NoSelection::new(Some(pending_self.chunks.clone()));
            pending_self.root.set_model(Some(&model));
            glib::ControlFlow::Break
        });
        *self.remount_pending.borrow_mut() = Some(source);
    }

    /// Re-mount the outer chunk model so every row re-measures against its
    /// now-complete slice. Used after a Folder-stream fill and as a bounded
    /// self-heal when the adjustment upper collapsed to a degenerate value
    /// (progressive-fill measurement race), which otherwise leaves every
    /// chunk row mounted and costs ~600 ms at the next teardown.
    fn remount_for_measurement(&self) {
        self.root.set_model(None::<&gtk::NoSelection>);
        let model = gtk::NoSelection::new(Some(self.chunks.clone()));
        self.root.set_model(Some(&model));
    }

    /// Track thumbnail zoom so every chunk wrapper's height request keeps
    /// matching the final laid-out size of its 64 photos.
    pub(crate) fn set_tile_height(&self, tile_height: i32) {
        let (_, columns) = self.metrics.get();
        if self.metrics.replace((tile_height, columns)) == (tile_height, columns) {
            return;
        }
        self.apply_row_metrics();
    }

    fn apply_row_metrics(&self) {
        let (tile_height, columns) = self.metrics.get();
        let mut wrappers = self.wrappers.borrow_mut();
        wrappers.retain(|weak| {
            let Some(wrapper) = weak.upgrade() else {
                return false;
            };
            // Rows keep the full-chunk height here; bind corrects the final
            // partial chunk once its position is known.
            wrapper.set_height_request(chunk_row_height_px(
                PHOTOS_PER_CHUNK,
                tile_height,
                columns,
            ));
            true
        });
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
        if crate::diagnostics::trace_enabled() {
            eprintln!(
                "PIC_CHUNK sync action=create chunks={} created={} removed=0",
                want, created
            );
        }
    } else {
        let removed = have - want;
        chunks.splice(want, removed, &[] as &[gtk::SliceListModel]);
        if crate::diagnostics::trace_enabled() {
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

    // Pure realization-window math, no GTK display required.

    #[test]
    fn filled_items_for_chunk_clamps_full_and_tail() {
        assert_eq!(filled_items_for_chunk(0, 0), 0);
        assert_eq!(filled_items_for_chunk(10, 0), 10);
        assert_eq!(filled_items_for_chunk(PHOTOS_PER_CHUNK, 0), PHOTOS_PER_CHUNK);
        assert_eq!(filled_items_for_chunk(PHOTOS_PER_CHUNK + 1, 0), PHOTOS_PER_CHUNK);
        assert_eq!(filled_items_for_chunk(PHOTOS_PER_CHUNK + 1, 1), 1);
        assert_eq!(filled_items_for_chunk(2 * PHOTOS_PER_CHUNK, 1), PHOTOS_PER_CHUNK);
        assert_eq!(filled_items_for_chunk(3 * PHOTOS_PER_CHUNK, 2), PHOTOS_PER_CHUNK);
        assert_eq!(filled_items_for_chunk(3 * PHOTOS_PER_CHUNK + 7, 3), 7);
        assert_eq!(filled_items_for_chunk(3 * PHOTOS_PER_CHUNK + 7, 99), 0);
    }

    #[test]
    fn realization_window_is_bounded_and_clamped() {
        assert_eq!(realization_window(0, 0), (0, 0));
        assert_eq!(realization_window(-50, 354), (0, 4));
        assert_eq!(realization_window(0, 354), (0, 4));
        assert_eq!(realization_window(5, 354), (1, 9));
        assert_eq!(realization_window(50, 354), (46, 54));
        assert_eq!(realization_window(353, 354), (349, 353));
        assert_eq!(realization_window(999, 354), (349, 353));
        assert_eq!(realization_window(1, 1), (0, 0));
        // Window width is 2*overscan+1 chunk rows except at the ends.
        let (start, end) = realization_window(50, 1000);
        assert_eq!(end - start, 8);
    }

    #[test]
    fn realization_window_empty_store_bounds() {
        assert_eq!(realization_window(0, 1), (0, 0));
        assert_eq!(realization_window(i64::MIN, 3), (0, 1));
        assert_eq!(realization_window(i64::MAX, 3), (1, 2));
    }

    #[test]
    fn chunk_not_mounted_while_fill_in_progress() {
        // fill_complete=false gates every chunk, even the one under the
        // viewport: a partially-filled slice measures a few px tall and would
        // poison the adjustment upper.
        assert!(!chunk_in_realization_window(0, 0.0, 64, 2272, 5, false));
        assert!(!chunk_in_realization_window(0, 0.0, 64, 2272, 5, true));
        assert!(!chunk_in_realization_window(0, 0.0, 0, 2272, 5, false));
        assert!(!chunk_in_realization_window(50, 50.0, 64, 2272, 5, false));
    }

    #[test]
    fn incomplete_slice_not_mounted_after_fill() {
        // Store thinks the chunk holds PHOTOS_PER_CHUNK but the slice only
        // grew 13: not mounted — mounting a half-filled slice repeats the
        // measurement poisoning.
        assert!(!chunk_in_realization_window(0, 0.0, 13, 2272, 5, true));
    }

    #[test]
    fn mount_window_matches_realization_window() {
        let total_chunks = 354u32;
        let total_items = total_chunks * PHOTOS_PER_CHUNK;
        let (start, end) = realization_window(50, total_chunks);
        // Chunk just inside the window mounts...
        assert!(chunk_in_realization_window(
            start,
            50.0,
            PHOTOS_PER_CHUNK,
            total_items,
            5,
            true
        ));
        assert!(chunk_in_realization_window(
            end,
            50.0,
            PHOTOS_PER_CHUNK,
            total_items,
            5,
            true
        ));
        // ...one chunk past the window's far edge does not.
        assert!(!chunk_in_realization_window(
            end + 1,
            50.0,
            PHOTOS_PER_CHUNK,
            total_items,
            5,
            true
        ));
    }

    #[test]
    fn final_partial_chunk_mounts_when_filled_and_near() {
        let total_items = 354 * PHOTOS_PER_CHUNK + 7;
        let tail = filled_items_for_chunk(total_items, 354);
        assert_eq!(tail, 7);
        assert!(chunk_in_realization_window(
            354, 354.0, tail, total_items, 5, true
        ));
        // If the fill never delivered those 7, the tail stays unmounted.
        assert!(!chunk_in_realization_window(
            354, 354.0, 0, total_items, 5, true
        ));
    }

    #[test]
    fn distant_chunk_stays_unmounted() {
        let total_items = 354 * PHOTOS_PER_CHUNK;
        assert!(chunk_in_realization_window(
            0, 0.0, PHOTOS_PER_CHUNK, total_items, 5, true
        ));
        assert!(!chunk_in_realization_window(
            200, 0.0, PHOTOS_PER_CHUNK, total_items, 5, true
        ));
        assert!(!chunk_in_realization_window(
            0, 200.0, PHOTOS_PER_CHUNK, total_items, 5, true
        ));
    }
}
