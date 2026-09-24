# Folder View: GtkGridView Experiment Plan

## Purpose

Investigate whether Folder view can use the same direct, photo-per-item `GtkGridView` path as Library/All Photos. The goal is to remove the zoom pause caused by rebuilding the current Folder row model when the column count changes, while preserving folder section appearance and all existing interaction behavior.

The branch now contains an opt-in first prototype. It is intentionally not the default and does not yet provide in-flow, full-width folder section headers.

## Current evidence

- Library/All Photos uses `Gallery::root`, a `GtkGridView` backed by `PhotoObject` items. GTK changes the columns without PIC rebuilding photo row objects.
- Folder mode uses `Gallery::folder_root`, a `GtkListView` backed by `FolderRowObject`. Each photo row object refers to a `current_photos` range sized to the current column count; folder heading rows are interleaved.
- `apply_tile_size()` already resizes only GTK-realized `SquareTile` widgets. The trace showed about 900–2,250 realized tiles and less than 0.5 ms in tile resize.
- In the captured run, Folder zooms that changed columns spent about 150–291 ms in `rebuild_folder_rows`; same-column resize took about 1–2 ms. Filename label writes were negligible in those traces.
- Folder mode has intentional behavior beyond appearance: section headers/actions, folder-target navigation, scroll anchoring, selection synchronization, rubberband selection, keyboard activation, and recycled tile lifecycle.

## GTK/API research gate

PIC is built with `gtk4 = 0.10` and the `v4_12` feature. The inspected host has GTK **4.22.5**. Rust's generated `GtkGridView` API in this checkout exposes no `header_factory` setter, and the installed GTK GIR likewise exposes no `gtk_grid_view_set_header_factory` symbol. The `GtkSectionModel` interface is available.

The current online `GtkSectionModel` page says list widgets such as `GtkListView` and `GtkGridView` can display section headers with a header factory, while the `GtkGridView` class API page does not list such a factory. The local GTK 4.22.5 GIR confirms that `GtkGridView` has no `header-factory` property or setter; the current upstream GridView class API also lists neither. `GtkListView` does expose that API. Therefore this experiment cannot rely on native full-width GridView section headers on the installed GTK. A section-model-only probe cannot establish header rendering without a GridView header factory.

References:

- [GtkSectionModel](https://docs.gtk.org/gtk4/iface.SectionModel.html)
- [GtkGridView API](https://docs.gtk.org/gtk4/class.GridView.html)
- [GtkListView header factory](https://docs.gtk.org/gtk4/method.ListView.set_header_factory.html)
- Local checks: `pkg-config --modversion gtk4`; inspect `/usr/share/gir-1.0/Gtk-4.0.gir` and the gtk4-rs `auto/grid_view.rs` generated for the pinned crate.

## Prototype trace and static check, 2026-09-24

The existing uncommitted `nscroll.filename.log` contains 32 `PIC_ZOOM apply` samples with `view=photo_grid`: median 4.15 ms, maximum 41.55 ms. Its 43 GridView column-change layout samples have a 5.79 ms median and 40.11 ms maximum. The trace also alternates 1,346 px and 1,386 px layout inputs during zoom, producing extra column changes. The zoom path was reading `GridView.width()` while the resize tick used the wider scroll-stack allocation; the experimental Folder path now reuses the resize tick's most recent width. A fresh trace is needed to confirm the extra changes are gone. `ns.log` and `ns2.log` have too few zoom samples to compare. None of these logs provides a matching legacy Folder run or reliable frame timings, so they do not satisfy the acceptance comparison.

The follow-up `/tmp/pic-folder-gridview2.log` confirms every logged GridView layout used 1,386 px. It has 20 `PIC_ZOOM apply` samples (median 4.89 ms, maximum 51.67 ms) and 21 layout samples (median 3.62 ms, maximum 49.76 ms). The width alternation is gone. One column-change layout is just under the plan's 50 ms investigation threshold, so retain that as a point to watch in a repeat run. This remains an experiment trace, not a controlled comparison with the legacy Folder view.

The user supplied `/tmp/pic-folder-legacy.log` for the legacy `GtkListView` path, on the same 395-photo `bubs preg` folder. Its 16 zoom applies had a 185.08 ms median and 273.96 ms maximum; 15 included row rebuild work. That trace recorded 22 column-change events and alternated 1,346 px and 1,386 px widths, so it included redundant rebuilds. The zoom path was updated to use the resize tick's latest width for legacy Folder mode as well.

The corrected follow-up `/tmp/pic-folder-legacy2.log` confirms every layout used 1,386 px. It has 19 zoom applies: 18 row rebuilds (median 217.26 ms, maximum 252.17 ms) and one same-column resize (3.12 ms). The apply median is 219.91 ms, maximum 255.26 ms. Against the GridView trace's 4.89 ms apply median, the timings indicate a substantial reduction in synchronous zoom work; sample sequences differ, so treat the ratio as indicative. Both logs contain cache hits from the same 395-photo folder. The legacy trace still shows the expected column reflow rebuilding row objects proportional to the stream.

`cargo test --no-default-features` passed (376 passed, 25 display-dependent tests ignored). `cargo fmt --check` currently fails on formatting differences across existing source files; avoid running `cargo fmt` over the full tree as part of this experiment.

## Proposed investigation and implementation sequence

### Prototype currently on this branch

Launch with `PICASA_FOLDER_GRIDVIEW=1` to route Folder mode through `Gallery::root` and its `GtkGridView` photo model. Folder order is retained, and the existing group-header area displays the current folder as a sticky indicator. The experimental path skips building unused `FolderRowObject` rows and routes zoom, visible thumbnail refresh, scroll positioning, folder navigation, selection, and focus through the photo grid. Without the variable, the existing Folder `GtkListView` path is unchanged.

Run the prototype with `PICASA_FOLDER_GRIDVIEW=1 PICASA_TRACE=1 cargo run`. Unset `PICASA_FOLDER_GRIDVIEW` for the current Folder `GtkListView` comparison. Prototype zoom lines identify `view=photo_grid`; normal folder mode reports `view=folder_list`.

This first prototype deliberately exposes the API/design tradeoff: folder headers are sticky and their existing per-section action row is not rendered. It is a responsiveness and interaction probe, not a replacement ready to merge. Keep all observations and follow-up work on this branch.

### 1. Verify section support for the deployed GTK

Check the minimum GTK version PIC supports and deployed runtime versions. Confirm whether `GtkGridView` actually consumes `GtkSectionModel` section boundaries and can render a full-width section header in that version. Do not infer this solely from the generic `GtkSectionModel` description.

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

Keep the existing `GtkListView` path available behind `PICASA_FOLDER_GRIDVIEW=1` until the prototype passes these checks. Do not change the production default during the first comparison.

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

The working filename-caption feature and `PIC_ZOOM`/`PIC_FILENAME` diagnostics were pushed first to `experiment/folder-gridview` in commit `9a0f0abc`. Keep all experiment commits on this branch; do not merge or push them to `main` unless requested.

## Repeatable Bubs comparison

Use **Network Shares → `pic-peet` → `pictures exported` → `bubs preg`** as the benchmark folder. Its native NFS URI is:

`nfs://DietPi.local/mnt/4TBP/mypics/pic-peet/pictures%20exported/bubs%20preg`

The library currently records 395 photos directly in this folder. Wait for the initial library scan to settle, select `bubs preg`, and confirm the gallery heading/count before timing. Keep window size, theme, thumbnail cache state, and the exact action sequence the same between builds. Use a warmed cache for both runs.

Run one build at a time and close it before launching the other:

```sh
# Experiment branch
cd /home/peet/Downloads/picasa/picasa-iphoto-clone
PICASA_FOLDER_GRIDVIEW=1 PICASA_TRACE=1 cargo run 2>&1 | tee /tmp/pic-bubs-experiment.log

# Main worktree
cd /tmp/pic-main-zoom-test
PICASA_TRACE=1 cargo run 2>&1 | tee /tmp/pic-bubs-main.log
```

For each run, start resource sampling after the gallery is settled. Record one idle minute, then perform the same sequence for one minute: scroll from the top to the bottom and back five times, then zoom from the current size to the smallest and largest tile sizes five times. Capture app CPU and RSS once per second; also record AMD GPU busy percent, VRAM, and GTT from `/sys/class/drm/card1/device/`. GPU counters are device-wide, so keep other GPU activity unchanged. Close PIC to stop sampling.

Compare average and peak CPU/RSS during the active minute separately from the idle minute. Compare average GPU busy percent and VRAM/GTT changes against each run's idle baseline. In the trace, compare `PIC_ZOOM apply` duration and `PIC_NAV current_columns_changed` counts. Main currently does not emit `PIC_ZOOM apply` timings for the legacy Folder `GtkListView` path, so use its resource samples and visible responsiveness for that part of the comparison. Keep the generated logs under `/tmp`; do not commit them.

## Independent branch checkouts for Pi and OpenCode

The current checkout is already on `experiment/folder-gridview`. Keep it open in one coding tool and create a separate worktree for `main` for the other tool. From the experiment checkout:

```sh
git worktree add ../pic-main main
git worktree list
git -C ../pic-main status --short --branch
```

Open `/home/peet/Downloads/picasa/picasa-iphoto-clone` in Pi and `/home/peet/Downloads/picasa/pic-main` in OpenCode (or swap them). Each directory has its own checked-out branch and working files; editing one does not switch the other. To inspect or run commands later, use `git -C <worktree-path> status --short --branch` and `git -C <worktree-path> log -1 --oneline`.

Do not run both PIC builds at the same time with the default user data directory: both use `~/.local/share/picasa-rs/library.db`. Test them one at a time unless you intentionally configure separate `XDG_DATA_HOME` directories and import the same benchmark library into each. To remove the extra checkout after checking that it has no work to keep, run `git worktree remove ../pic-main`. Do not use `--force` to remove a worktree with changes.

## Experiment conclusion and next phase

### Conclusion

The direct-photo GridView path removes the column-change row-model rebuild from zoom. In the supplied traces, its 20 zoom applies had a 4.89 ms median and 51.67 ms maximum, with zero `rebuild_rows` operations. The corrected legacy trace had 19 applies: 18 row rebuilds with a 217.26 ms median and 252.17 ms maximum, plus one same-column resize at 3.12 ms. The two medians indicate roughly 45 times less synchronous zoom work in the GridView trace. The action sequences were similar but not identical, so use this as strong prototype evidence rather than a controlled final benchmark.

The GridView probe is not ready to replace Folder mode. It uses a sticky current-folder heading and does not render the existing in-flow section headings and per-section actions. Those are intentional Folder behaviors. Keep the experiment opt-in and do not make it the default based on zoom timings alone.

### Next phase: prototype bounded section chunks with inner GridViews

Do not spend another phase micro-optimizing the legacy per-column row rebuild. Keep the legacy `GtkListView` as the default and keep the current direct-photo GridView behind `PICASA_FOLDER_GRIDVIEW=1`.

The smallest promising architecture is one outer, vertically scrolling `GtkListView` whose stable items are bounded folder-photo chunks. The outer model implements `GtkSectionModel` over those chunks, grouping adjacent chunks by folder id; `GtkListView`'s native header factory renders the existing full-width heading, count, and actions once per folder. Each list item contains a `GtkGridView` backed by that chunk's photo range. Split large folders into fixed-size chunks (start with a small probe around 64 photos per chunk); do not size chunks by the current column count. Give each inner GridView a height derived from its item count, column count, and tile pitch, without a nested `GtkScrolledWindow`. The outer ListView then owns the only vertical adjustment and virtualizes chunks; a column change only updates grid geometry and chunk heights instead of regrouping every photo into display-row GObjects.

This reuses the existing photo objects, folder ranges, header UI, and per-photo tile factory. Chunk records need stable photo start/end positions and folder metadata so the `GtkListHeader` can read folder title/count/actions from the first chunk in its section. The photo model remains photo-per-item inside each inner GridView, while the outer model remains small relative to the photo count. Folder headers stay in-flow and actions remain native widgets.

The main risks are the nested widgets and shared behavior. A chunk GridView may realize every photo in its bounded allocation, so chunk size trades tile count against outer-row count. Outer ListView height estimates must settle after zoom; calculate chunk heights from current geometry and restore the viewport by photo id to prevent jumps. Selection currently uses one global model, while each inner GridView uses a photo slice; selection and keyboard navigation must map local chunk positions to global photo positions. Rubberband selection, activation, context menus, folder navigation, progressive appends, and cancellation must continue to target the correct global photo. Avoid independent scrollers, which would break continuous Picasa-style scrolling.

Build only a focused prototype first. Start with one folder split across several chunks and verify in-flow heading/count/actions, column changes, scroll anchoring, selection across chunk boundaries, and realized tile counts. Then test multiple folders, parent/container navigation rows, progressive scan, and the 395-photo `bubs preg` benchmark. Proceed to the full Folder stream only if chunk realization stays bounded and zoom remains close to the measured direct GridView timings.

The alternative is a single GridView with blank marker rows and a separate full-width header overlay. It retains one global photo grid but requires a mixed marker/photo model, reserved grid rows at section boundaries, and an overlay that tracks scroll offset and column reflow. It also needs selection mapping that excludes marker items and careful header hit testing. This likely has lower per-widget overhead but higher geometry and interaction risk; keep it as a fallback if the chunked prototype's nested-grid realization or height estimation performs poorly.

Native `GtkSectionModel` headers are not a current option: local GTK 4.22.5 and generated gtk4-rs APIs expose no `GtkGridView` header factory. Section ranges alone do not create visible headers in GridView. The current `GtkListView` header factory API remains available for existing list-based designs.
