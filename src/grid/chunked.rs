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
const CHUNK_HEADER_HEIGHT: i32 = 70;

#[derive(Debug, Clone, PartialEq, Eq)]
struct ChunkMeta {
    folder_start: u32,
    folder_end: u32,
    logical_index: u32,
    logical_count: u32,
    label: String,
    photo_count: u32,
}

fn logical_chunk_count(photo_count: u32) -> u32 {
    photo_count.max(1).div_ceil(PHOTOS_PER_CHUNK)
}

fn aligned_chunk_bounds(meta: &ChunkMeta, columns: u32) -> (u32, u32) {
    let columns = columns.max(1);
    let count = meta.folder_end.saturating_sub(meta.folder_start);
    let boundary = |index: u32| -> u32 {
        if index == 0 {
            return 0;
        }
        if index >= meta.logical_count {
            return count;
        }
        let nominal = index.saturating_mul(PHOTOS_PER_CHUNK);
        let aligned = ((nominal + columns / 2) / columns).saturating_mul(columns);
        aligned.clamp(1, count.saturating_sub(1))
    };
    (
        meta.folder_start + boundary(meta.logical_index),
        meta.folder_start + boundary(meta.logical_index + 1),
    )
}

fn build_chunk_meta(ranges: &[super::GroupRange]) -> Vec<ChunkMeta> {
    let mut out = Vec::new();
    for range in ranges {
        let count = range.end.saturating_sub(range.start) as u32;
        if count == 0 {
            continue;
        }
        let logical_count = logical_chunk_count(count);
        for logical_index in 0..logical_count {
            out.push(ChunkMeta {
                folder_start: range.start as u32,
                folder_end: range.end as u32,
                logical_index,
                logical_count,
                label: range.label.clone(),
                photo_count: count,
            });
        }
    }
    out
}

/// How many chunk rows beyond the viewport keep their photo models mounted
/// in each scroll direction. Keeps mounted tiles bounded while making
/// ordinary scrolling and small jumps seamless.
const REALIZATION_OVERSCAN_CHUNKS: i64 = 4;
/// Keep an already-mounted edge chunk until the viewport has moved well past
/// the mount threshold. A model can change GTK's scroll estimate by ~44px;
/// without hysteresis that small shift repeatedly mounts/unmounts the row.
const REALIZATION_RETAIN_MARGIN_CHUNKS: f64 = 0.5;

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
    chunk_in_realization_window_with_margin(
        position,
        viewport_center_chunks,
        slice_n_items,
        total_items,
        columns,
        fill_complete,
        0.0,
    )
}

fn chunk_in_realization_window_with_margin(
    position: u32,
    viewport_center_chunks: f64,
    slice_n_items: u32,
    total_items: u32,
    _columns: u32,
    fill_complete: bool,
    retain_margin: f64,
) -> bool {
    let _ = (total_items, columns);
    if !fill_complete || slice_n_items == 0 {
        return false;
    }
    let distance = (position as f64 - viewport_center_chunks).abs();
    distance <= REALIZATION_OVERSCAN_CHUNKS as f64 + retain_margin
}

#[derive(Clone)]
pub(crate) struct ChunkedPrototype {
    pub(crate) root: gtk::ListView,
    grids: Rc<RefCell<Vec<glib::WeakRef<gtk::GridView>>>>,
    store: gio::ListStore,
    chunks: gio::ListStore,
    section_ranges: Rc<RefCell<Vec<super::GroupRange>>>,
    chunk_meta: Rc<RefCell<Vec<ChunkMeta>>>,
    reconcile_pending: Rc<Cell<bool>>,
    /// (tile_height, columns) the wrapper height requests are computed from.
    metrics: Rc<Cell<(i32, u32)>>,
    /// Chunk-row wrapper boxes, for metric updates on already-realized rows.
    wrappers: Rc<RefCell<Vec<glib::WeakRef<gtk::Box>>>>,
    /// Factory items can stay bound even when their chunk scrolls away. Keep
    /// weak references so an idle can move photo models with the viewport.
    bound_items: Rc<RefCell<Vec<glib::WeakRef<gtk::ListItem>>>>,
    realization_pending: Rc<Cell<bool>>,
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
    ranges: &[super::GroupRange],
    chunk_meta: &Rc<RefCell<Vec<ChunkMeta>>>,
    columns: u32,
) {
    let wanted_meta = if ranges.is_empty() {
        let count = store.n_items();
        let logical_count = logical_chunk_count(count);
        (0..logical_count)
            .map(|logical_index| ChunkMeta {
                folder_start: 0,
                folder_end: count,
                logical_index,
                logical_count,
                label: String::new(),
                photo_count: count,
            })
            .collect::<Vec<_>>()
    } else {
        build_chunk_meta(ranges)
    };

    if *chunk_meta.borrow() == wanted_meta {
        return;
    }

    for grid_weak in grids.borrow().iter() {
        if let Some(grid) = grid_weak.upgrade() {
            grid.set_model(None::<&gtk::NoSelection>);
        }
    }
    sync_chunks(store, chunks, &wanted_meta, columns);
    chunk_meta.replace(wanted_meta);
}

impl ChunkedPrototype {
    pub(crate) fn new(store: &gio::ListStore, photo_factory: &gtk::SignalListItemFactory) -> Self {
        let chunks = gio::ListStore::new::<gtk::SliceListModel>();
        let chunks_for_updates = chunks.clone();
        let chunks_for_struct = chunks.clone();
        let store_for_updates = store.clone();
        let section_ranges: Rc<RefCell<Vec<super::GroupRange>>> =
            Rc::new(RefCell::new(Vec::new()));
        let section_ranges_for_updates = section_ranges.clone();
        let section_ranges_for_struct = section_ranges.clone();
        let chunk_meta: Rc<RefCell<Vec<ChunkMeta>>> = Rc::new(RefCell::new(Vec::new()));
        let chunk_meta_for_updates = chunk_meta.clone();
        let chunk_meta_for_struct = chunk_meta.clone();
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
        let metrics_for_updates = metrics.clone();
        let wrappers: Rc<RefCell<Vec<glib::WeakRef<gtk::Box>>>> =
            Rc::new(RefCell::new(Vec::new()));
        let bound_items: Rc<RefCell<Vec<glib::WeakRef<gtk::ListItem>>>> =
            Rc::new(RefCell::new(Vec::new()));
        let realization_pending = Rc::new(Cell::new(false));
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
                reconcile_chunks(
                    &cols,
                    &chunks,
                    &grids,
                    &section_ranges_for_updates.borrow(),
                    &chunk_meta_for_updates,
                    metrics_for_updates.get().1,
                );
            });
        });
        reconcile_chunks(
            store,
            &chunks,
            &grids,
            &section_ranges.borrow(),
            &chunk_meta,
            CHUNK_COLUMNS,
        );

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
        let items_for_setup = bound_items.clone();
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
            grid.add_css_class("section-grid");
            // The wrapper owns the row height so the outer ListView can size
            // and virtualize rows whose photos have not streamed in yet.
            let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 0);
            wrapper.set_widget_name("picasa-chunk-wrapper");

            let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            header.set_widget_name("picasa-chunk-header");
            header.set_hexpand(true);
            header.set_margin_start(6);
            header.set_margin_end(6);
            header.set_margin_top(10);
            header.set_margin_bottom(8);

            let icon = gtk::Image::from_icon_name("folder-symbolic");
            icon.set_pixel_size(16);
            header.append(&icon);

            let title = gtk::Label::new(None);
            title.set_widget_name("picasa-chunk-header-title");
            title.set_xalign(0.0);
            title.set_ellipsize(gtk::pango::EllipsizeMode::End);
            title.add_css_class("folder-section-title");
            header.append(&title);

            let count = gtk::Label::new(None);
            count.set_widget_name("picasa-chunk-header-count");
            count.set_xalign(0.0);
            count.add_css_class("dim-label");
            count.add_css_class("folder-section-count");
            header.append(&count);

            wrapper.append(&header);
            wrapper.append(&grid);
            wrapper.set_height_request(
                CHUNK_HEADER_HEIGHT
                    + chunk_row_height_px(PHOTOS_PER_CHUNK, tile_height, columns),
            );
            item.set_child(Some(&wrapper));
            wrappers_for_setup.borrow_mut().push(wrapper.downgrade());
            items_for_setup.borrow_mut().push(item.downgrade());
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
        let chunk_meta_for_bind = chunk_meta.clone();
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
            let Some(header) = wrapper.first_child().and_downcast::<gtk::Box>() else {
                return;
            };
            let Some(grid) = wrapper.last_child().and_downcast::<gtk::GridView>() else {
                return;
            };
            let position = item.position();
            let total = store_for_bind.n_items();
            let (tile_height, columns) = metrics_for_bind.get();
            let meta = chunk_meta_for_bind
                .borrow()
                .get(position as usize)
                .cloned();

            let is_first = meta
                .as_ref()
                .is_some_and(|meta| meta.logical_index == 0);
            header.set_visible(is_first);
            if let Some(meta) = meta.as_ref().filter(|meta| meta.logical_index == 0) {
                if let Some(title) = header
                    .first_child()
                    .and_then(|child| child.next_sibling())
                    .and_downcast::<gtk::Label>()
                {
                    title.set_text(&meta.label);
                }
                if let Some(count) = header.last_child().and_downcast::<gtk::Label>() {
                    count.set_text(&format!(
                        "{} {}",
                        meta.photo_count,
                        if meta.photo_count == 1 { "photo" } else { "photos" }
                    ));
                }
            }
            let header_height = if is_first { CHUNK_HEADER_HEIGHT } else { 0 };
            wrapper.set_height_request(
                header_height
                    + chunk_row_height_px(slice.n_items().max(1), tile_height, columns),
            );
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
                grid.set_model(Some(&gtk::NoSelection::new(Some(slice))));
            } else {
                grid.set_model(None::<&gtk::NoSelection>);
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
                if let Some(wrapper) = item.child().and_downcast::<gtk::Box>() {
                    if let Some(grid) = wrapper.last_child().and_downcast::<gtk::GridView>() {
                        grid.set_model(None::<&gtk::NoSelection>);
                    }
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
        root.add_css_class("folder-stream");
        *bind_trace_root.borrow_mut() = Some(root.clone());
        let pending_self_clone = Self {
            root: root.clone(),
            grids: grids.clone(),
            store: store.clone(),
            chunks: chunks_for_struct.clone(),
            section_ranges: section_ranges_for_struct.clone(),
            chunk_meta: chunk_meta_for_struct.clone(),
            reconcile_pending: reconcile_pending_for_struct.clone(),
            metrics: metrics.clone(),
            wrappers: wrappers.clone(),
            bound_items: bound_items.clone(),
            realization_pending: realization_pending.clone(),
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
            let grids_status = grids.clone();
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
                let expected_upper = {
                    let meta = prototype_for_status.chunk_meta.borrow();
                    if meta.is_empty() {
                        chunk_row_height_px(PHOTOS_PER_CHUNK, tile_height, columns) as f64
                            * logical.max(1) as f64
                    } else {
                        meta.iter()
                            .map(|entry| {
                                let (start, end) = aligned_chunk_bounds(entry, columns);
                                let header = if entry.logical_index == 0 {
                                    CHUNK_HEADER_HEIGHT
                                } else {
                                    0
                                };
                                f64::from(
                                    header
                                        + chunk_row_height_px(
                                            end.saturating_sub(start).max(1),
                                            tile_height,
                                            columns,
                                        ),
                                )
                            })
                            .sum::<f64>()
                    }
                };
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
                let realized_grids = grids_status
                    .borrow()
                    .iter()
                    .filter_map(|weak| weak.upgrade())
                    .filter(|grid| grid.model().is_some())
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
                    "PIC_NAV chunk_realization logical_chunks={logical} realized_wrappers={mounted} realized_grids={realized_grids} value={value:.0} upper={upper:.0} page={page:.0} expected={expected_upper:.0} self_visible={} self_mapped={}{} t={}",
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
            section_ranges: section_ranges_for_struct,
            chunk_meta: chunk_meta_for_struct,
            reconcile_pending: reconcile_pending_for_struct,
            metrics,
            wrappers,
            bound_items,
            realization_pending,
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
        reconcile_chunks(
            &self.store,
            &self.chunks,
            &self.grids,
            &self.section_ranges.borrow(),
            &self.chunk_meta,
            self.metrics.get().1,
        );
    }

    /// Adjustment signals can fire during GTK layout. Coalesce them into one
    /// idle and move photo models on already-bound rows after GTK unwinds.
    pub(crate) fn request_realization_update(&self) {
        if self.realization_pending.replace(true) {
            return;
        }
        let this = self.clone();
        glib::idle_add_local_once(move || {
            this.realization_pending.set(false);
            this.refresh_realization_window();
        });
    }

    fn refresh_realization_window(&self) {
        if !self.fill_complete.get() {
            return;
        }
        let Some(adjustment) = self.root.vadjustment() else {
            return;
        };
        let (tile_height, columns) = self.metrics.get();
        let row_height = chunk_row_height_px(PHOTOS_PER_CHUNK, tile_height, columns) as f64;
        if row_height <= 0.0 {
            return;
        }
        let center = (adjustment.value() + adjustment.page_size() / 2.0) / row_height;
        let total = self.store.n_items();
        let items = {
            let mut items = self.bound_items.borrow_mut();
            items.retain(|weak| weak.upgrade().is_some());
            items.iter().filter_map(|weak| weak.upgrade()).collect::<Vec<_>>()
        };
        let mut realized = 0;
        let mut changed = 0;
        for item in items {
            let Some(slice) = item.item().and_downcast::<gtk::SliceListModel>() else {
                continue;
            };
            let Some(wrapper) = item.child().and_downcast::<gtk::Box>() else {
                continue;
            };
            let Some(grid) = wrapper.last_child().and_downcast::<gtk::GridView>() else {
                continue;
            };
            let overscan_px = adjustment.page_size().max(row_height);
            let mount = wrapper
                .compute_bounds(&self.root)
                .map(|bounds| {
                    let retain = if grid.model().is_some() {
                        overscan_px * 0.5
                    } else {
                        0.0
                    };
                    let top = bounds.y() as f64;
                    let bottom = top + bounds.height() as f64;
                    bottom >= -overscan_px - retain
                        && top <= adjustment.page_size() + overscan_px + retain
                })
                .unwrap_or_else(|| {
                    chunk_in_realization_window_with_margin(
                        item.position(),
                        center,
                        slice.n_items(),
                        total,
                        columns,
                        true,
                        if grid.model().is_some() {
                            REALIZATION_RETAIN_MARGIN_CHUNKS
                        } else {
                            0.0
                        },
                    )
                });
            if mount {
                realized += 1;
                if grid.model().is_none() {
                    grid.set_model(Some(&gtk::NoSelection::new(Some(slice))));
                    changed += 1;
                }
            } else if grid.model().is_some() {
                grid.set_model(None::<&gtk::NoSelection>);
                changed += 1;
            }
        }
        if crate::diagnostics::trace_enabled() && changed > 0 {
            eprintln!(
                "PIC_NAV chunk_realization update center={center:.1} realized_grids={realized} changed={changed} value={:.0} upper={:.0} page={:.0} t={}",
                adjustment.value(), adjustment.upper(), adjustment.page_size(),
                crate::diagnostics::t_ms()
            );
        }
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

        let items = {
            let mut items = self.bound_items.borrow_mut();
            items.retain(|weak| weak.upgrade().is_some());
            items.iter().filter_map(|weak| weak.upgrade()).collect::<Vec<_>>()
        };
        let meta = self.chunk_meta.borrow();
        for item in items {
            let Some(wrapper) = item.child().and_downcast::<gtk::Box>() else {
                continue;
            };
            let Some(slice) = item.item().and_downcast::<gtk::SliceListModel>() else {
                continue;
            };
            let position = item.position() as usize;
            let header_height = meta
                .get(position)
                .is_some_and(|entry| entry.logical_index == 0)
                .then_some(CHUNK_HEADER_HEIGHT)
                .unwrap_or(0);
            wrapper.set_height_request(
                header_height
                    + chunk_row_height_px(slice.n_items().max(1), tile_height, columns),
            );
            if let Some(grid) = wrapper.last_child().and_downcast::<gtk::GridView>() {
                grid.set_min_columns(columns);
                grid.set_max_columns(columns);
                grid.queue_resize();
            }
        }

        let mut wrappers = self.wrappers.borrow_mut();
        wrappers.retain(|weak| weak.upgrade().is_some());
        let mut grids = self.grids.borrow_mut();
        grids.retain(|weak| weak.upgrade().is_some());
    }

    pub(crate) fn set_columns(&self, columns: u32) {
        let columns = columns.max(1);
        let (tile_height, old_columns) = self.metrics.get();
        if old_columns == columns {
            return;
        }

        // Keep the OUTER section/chunk model stable. Only move each slice's
        // internal photo boundary to a complete row for the new column count.
        // That preserves gapless folder sections without a ListStore splice.
        let meta = self.chunk_meta.borrow();
        for (position, entry) in meta.iter().enumerate() {
            let Some(slice) = self
                .chunks
                .item(position as u32)
                .and_downcast::<gtk::SliceListModel>()
            else {
                continue;
            };
            let (start, end) = aligned_chunk_bounds(entry, columns);
            slice.set_offset(start);
            slice.set_size(end.saturating_sub(start));
        }
        drop(meta);

        self.metrics.set((tile_height, columns));
        self.apply_row_metrics();
    }

    pub(crate) fn set_folder_ranges(&self, ranges: &[super::GroupRange]) {
        if self.section_ranges.borrow().as_slice() == ranges {
            return;
        }
        self.section_ranges.replace(ranges.to_vec());
        reconcile_chunks(
            &self.store,
            &self.chunks,
            &self.grids,
            &self.section_ranges.borrow(),
            &self.chunk_meta,
            self.metrics.get().1,
        );
        self.apply_row_metrics();
        self.request_realization_update();
        if crate::diagnostics::trace_enabled() {
            eprintln!(
                "PIC_CHUNK sections ranges={} chunks={} outer_model_update=section_change",
                ranges.len(),
                self.chunks.n_items()
            );
        }
    }

    /// Scroll the visible outer chunk list so the photo at `position` in the
    /// shared stream is revealed by its owning chunk's GridView. Because only
    /// the outer ListView scrolls, this lands at the top of the enclosing
    /// chunk; a later step can additionally scroll the inner grid by the
    /// intra-chunk offset.
    pub(crate) fn scroll_to_photo(&self, position: usize) {
        self.flush_reconcile();
        let columns = self.metrics.get().1;
        let position = position as u32;
        let chunk_index = self
            .chunk_meta
            .borrow()
            .iter()
            .enumerate()
            .find_map(|(index, meta)| {
                let (start, end) = aligned_chunk_bounds(meta, columns);
                (position >= start && position < end).then_some(index as u32)
            })
            .unwrap_or_else(|| position / PHOTOS_PER_CHUNK);
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
fn sync_chunks(
    store: &gio::ListStore,
    chunks: &gio::ListStore,
    wanted_meta: &[ChunkMeta],
    columns: u32,
) {
    if chunks.n_items() == wanted_meta.len() as u32 {
        let all_match = wanted_meta.iter().enumerate().all(|(position, meta)| {
            let Some(slice) = chunks
                .item(position as u32)
                .and_downcast::<gtk::SliceListModel>()
            else {
                return false;
            };
            let (start, end) = aligned_chunk_bounds(meta, columns);
            slice.offset() == start && slice.size() == end.saturating_sub(start)
        });
        if all_match {
            return;
        }
    }

    let mut slices = Vec::with_capacity(wanted_meta.len());
    for meta in wanted_meta {
        let (start, end) = aligned_chunk_bounds(meta, columns);
        slices.push(gtk::SliceListModel::new(
            Some(store.clone()),
            start,
            end.saturating_sub(start),
        ));
    }
    let removed = chunks.n_items();
    chunks.splice(0, removed, &slices);

    if crate::diagnostics::trace_enabled() {
        eprintln!(
            "PIC_CHUNK sync action=sections chunks={} replaced={} columns={}",
            slices.len(),
            removed,
            columns
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(start: usize, end: usize) -> super::super::GroupRange {
        super::super::GroupRange {
            start,
            end,
            label: "Folder".to_string(),
            folder_id: 1,
        }
    }

    #[test]
    fn aligned_chunks_cover_each_folder_without_gaps() {
        for columns in [2_u32, 3, 4, 5, 6, 7, 8, 9, 11, 12] {
            for count in [1_usize, 63, 64, 65, 127, 128, 129, 1000] {
                let metas = build_chunk_meta(&[range(0, count)]);
                assert!(!metas.is_empty());
                let mut cursor = 0_u32;
                for (index, meta) in metas.iter().enumerate() {
                    let (start, end) = aligned_chunk_bounds(meta, columns);
                    assert_eq!(start, cursor, "columns={columns} count={count}");
                    assert!(end > start, "columns={columns} count={count}");
                    if index + 1 < metas.len() {
                        assert_eq!(
                            end % columns,
                            0,
                            "internal boundary columns={columns} count={count}"
                        );
                    }
                    cursor = end;
                }
                assert_eq!(cursor, count as u32, "columns={columns} count={count}");
            }
        }
    }

    #[test]
    fn chunk_count_is_independent_of_columns() {
        for count in [1_usize, 63, 64, 65, 127, 128, 129, 1000] {
            let metas = build_chunk_meta(&[range(10, 10 + count)]);
            let expected = (count as u32).div_ceil(PHOTOS_PER_CHUNK);
            assert_eq!(metas.len() as u32, expected);
            for columns in [2_u32, 3, 4, 5, 6, 7, 8, 9, 11, 12] {
                assert_eq!(metas.len() as u32, expected);
                let first = aligned_chunk_bounds(&metas[0], columns).0;
                let last = aligned_chunk_bounds(metas.last().unwrap(), columns).1;
                assert_eq!(first, 10);
                assert_eq!(last, (10 + count) as u32);
            }
        }
    }
}
