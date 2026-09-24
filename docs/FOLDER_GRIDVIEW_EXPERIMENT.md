# Folder View: GtkGridView Experiment Plan

## Purpose

Investigate whether Folder view can use the same direct, photo-per-item `GtkGridView` path as Library/All Photos. The goal is to remove the zoom pause caused by rebuilding the current Folder row model when the column count changes, while preserving folder section appearance and all existing interaction behavior.

This document is a research and implementation plan. The current branch does **not** contain a Folder `GtkGridView` implementation yet.

## Current evidence

- Library/All Photos uses `Gallery::root`, a `GtkGridView` backed by `PhotoObject` items. GTK changes the columns without PIC rebuilding photo row objects.
- Folder mode uses `Gallery::folder_root`, a `GtkListView` backed by `FolderRowObject`. Each photo row object refers to a `current_photos` range sized to the current column count; folder heading rows are interleaved.
- `apply_tile_size()` already resizes only GTK-realized `SquareTile` widgets. The trace showed about 900–2,250 realized tiles and less than 0.5 ms in tile resize.
- In the captured run, Folder zooms that changed columns spent about 150–291 ms in `rebuild_folder_rows`; same-column resize took about 1–2 ms. Filename label writes were negligible in those traces.
- Folder mode has intentional behavior beyond appearance: section headers/actions, folder-target navigation, scroll anchoring, selection synchronization, rubberband selection, keyboard activation, and recycled tile lifecycle.

## GTK/API research gate

PIC is built with `gtk4 = 0.10` and the `v4_12` feature. The inspected host has GTK **4.22.5**. Rust's generated `GtkGridView` API in this checkout exposes no `header_factory` setter, and the installed GTK GIR likewise exposes no `gtk_grid_view_set_header_factory` symbol. The `GtkSectionModel` interface is available.

The current online `GtkSectionModel` page says list widgets such as `GtkListView` and `GtkGridView` can display section headers with a header factory, while the `GtkGridView` class API page does not list such a factory. Resolve this apparent API/documentation mismatch against the exact GTK release used by PIC before depending on native full-width headers.

References:

- [GtkSectionModel](https://docs.gtk.org/gtk4/iface.SectionModel.html)
- [GtkGridView API](https://docs.gtk.org/gtk4/class.GridView.html)
- [GtkListView header factory](https://docs.gtk.org/gtk4/method.ListView.set_header_factory.html)
- Local checks: `pkg-config --modversion gtk4`; inspect `/usr/share/gir-1.0/Gtk-4.0.gir` and the gtk4-rs `auto/grid_view.rs` generated for the pinned crate.

## Proposed investigation and implementation sequence

### 1. Verify section support for the deployed GTK

Check the minimum GTK version PIC supports and the deployed runtime versions. Confirm whether `GtkGridView` actually consumes `GtkSectionModel` section boundaries and can render a full-width section header in that version. Do not infer this solely from the generic `GtkSectionModel` description.

If needed, make a tiny throwaway GTK probe with a section-aware `GListModel`, a `GtkGridView`, and two photo sections. Verify header width, placement, reflow after changing min/max columns, and behavior under scrolling. Keep this probe outside production code unless a focused test helper belongs in the repository.

### 2. Prototype a direct-photo Folder model

Build on the existing folder-ordered `current_photos`/`store` model: one `PhotoObject` per model item, with each folder represented as a contiguous section. Prefer stable photo items and section metadata over `FolderRowObject` arrays grouped by the current column count.

If native GridView section headers are supported, implement only the adapter/model needed to expose the existing folder ranges as sections. On zoom, column changes should update grid geometry; section metadata should not require regrouping every photo into row objects. Ensure progressive scan appends update the final section and section counts safely.

Do not fake a full-width heading by inserting ordinary header items into the grid unless the prototype proves alignment remains correct at every column count. Grid cells do not inherently span all columns, so this can shift photo alignment.

### 3. Preserve Folder interactions before replacing the view

Map each existing behavior to the prototype and test it before making the new view the default:

- folder header label/count/actions and navigation target
- exact folder ordering, including parent/container navigation nodes
- photo activation and open-in-folder behavior
- selection synchronization with the shared `GtkMultiSelection`
- rubberband selection and keyboard navigation
- scroll-to-folder/photo and zoom scroll anchoring
- progressive scan updates, cancellation, and folder-stream cache restore
- thumbnail cache use, tile bind/unbind, and filename captions
- folder sidebar selection and search-to-folder navigation

Keep the existing `GtkListView` path available behind an opt-in development setting or build-time experiment until the prototype passes these checks. Do not change the production default during the first comparison.

### 4. Compare performance under controlled conditions

Use the same library, view width, scroll position, warmed thumbnail cache, and zoom sequence on both paths. Include at least a 2,000-photo network folder and a 20,000+ photo library. Capture `PICASA_TRACE=1` logs.

Record:

- zoom wall time and `PIC_ZOOM` synchronous stage timings
- number of model items/sections and realized tiles
- whether a column change emits/rebinds all items or only section metadata
- frame/input responsiveness during repeated zoom changes
- scroll-anchor error and whether any photo jumps or disappears
- thumbnail request/cache counts to detect duplicate generation or reads

The prototype should remove work proportional to all photos on a zoom column change. Aim for no visible main-thread pause; treat repeated synchronous work above 50 ms as a failure to investigate, not as an accepted target.

### 5. Select a fallback if GridView headers are unavailable

If full-width section headers are not supported cleanly by PIC's minimum GTK version, compare these options before choosing:

1. Keep the current Folder `GtkListView` and optimize row-model updates/splices, including avoiding unnecessary full-row object rebuilds and preserving the existing model where possible.
2. Prototype an outer virtualized folder-section list with an inner photo GridView per folder; measure nested scrolling, allocation, selection, and navigation before considering it.
3. Evaluate a separately rendered, scroll-synchronized folder header layer over the direct photo GridView only if header spacing and hit-testing can be made reliable without per-photo row rebuilding.

Do not accept a design that removes folder headings or changes them to sticky headings without an explicit product decision.

## Regression and acceptance checks

- Unit tests for folder range/section boundaries, empty folders, progressive appends, duplicate folder basenames, and section counts.
- GTK integration coverage for headers and photos across several column counts, selection/activation, and scroll anchoring.
- Existing folder scroll/anchor, zoom, history, and thumbnail-cache tests.
- Manual comparison at small and large library sizes, including an NFS-backed folder.
- Run `cargo fmt --check`, `cargo check`, and relevant tests. Avoid project-wide formatting churn.

Accept only if zoom no longer rebuilds/rebinds row objects proportional to the whole photo stream, all listed Folder behavior remains correct, and fast scrolling/scan cancellation do not regress.

## Branch checkpoint

The working filename-caption feature and `PIC_ZOOM`/`PIC_FILENAME` diagnostics were pushed first to `experiment/folder-gridview` in commit `9a0f0abc`. They are a test baseline, not the GridView experiment itself. Keep all experiment commits on this branch; do not merge or push them to `main` unless requested.
