# Folder GridView experiment handover

## Checkpoint

- Branch: `experiment/folder-gridview`.
- Starting commit before this WIP checkpoint: `392cb6df` (`Prototype Folder mode on GtkGridView`).
- Implementation checkpoint: `5ce0959e` (`WIP: chunked Folder GridView prototype and experiment handover`). This commit contains the implementation and the initial version of this handover document. The exact full hash is also shown by `git rev-parse 5ce0959e`.
- This file was updated in a documentation-only follow-up so it could record the implementation checkpoint's hash; no code changed after the WIP checkpoint.
- Do not merge this branch into `main` without a separate review and explicit request.
- The worktree also contains unrelated/pre-existing untracked items. The checkpoint deliberately excludes them; see the handover summary from the working session and `git status`.

## Current code paths

### Legacy Folder ListView (default)

`Gallery::folder_root` in `src/grid/view.rs` is a `GtkListView` over `FolderRowObject`s. `src/grid/virtualization.rs` builds one full-width header row followed by photo line rows sized to the current number of columns. Those rows contain reused `SquareTile`s. Folder counts and the header action container are constructed by the existing view factory. The outer Folder ListView lives in its own `GtkScrolledWindow`.

Zoom changes the row width/column geometry and rebuilds the virtual Folder row model. This is the behaviorally complete path and remains the default when no experiment variable is set.

### Direct GridView experiment

`Gallery::root` is the existing direct-photo `GtkGridView` over the shared `GtkMultiSelection`/`PhotoObject` model. Set `PICASA_FOLDER_GRIDVIEW=1` to show it for Folder mode. It retains folder photo ordering but presents the current folder as an external sticky indicator; it does not render the in-flow section headings/actions. Zoom changes `GtkGridView` columns without rebuilding PIC row objects.

The corrected 395-photo `bubs preg` trace (`/tmp/pic-folder-gridview2.log`) has 20 zoom applies: median 4.89 ms, maximum 51.67 ms, and zero row rebuilds. It recorded 129–321 realized tiles. This is a zoom-path comparison, not proof of Folder UI parity.

### Incomplete chunked ListView + GridView prototype

The step-1 prototype is in `src/grid/chunked.rs`, included by `src/grid.rs`, held optionally on `Gallery` in `src/grid/view.rs`, and mounted as a separate Stack child in `src/window/build.rs`. Set `PICASA_FOLDER_CHUNKED=1` to select it in Folder mode. It creates an outer `GtkListView` whose model contains `GtkSliceListModel`s of up to 64 shared `PhotoObject`s; each outer row hosts an inner `GtkGridView` using the existing photo item factory. There is one outer vertical `GtkScrolledWindow` and no inner scrollers.

It is deliberately incomplete and **has not been verified as a functioning replacement**. It has no folder headings/header factory, section actions, global selection mapping, keyboard/navigation behavior, or zoom integration. Inner grids currently use five columns. The existing tile factory stores the inner item's local position in `photo_index`, which is not a correct global index for chunks after the first; do not rely on selection/navigation/prefetch behavior in this mode.

## Trace diagnosis: `/tmp/chunkv1.log`

This trace is from the chunked experiment (`PIC_FOLDER_CHUNKED enabled chunk_size=64`). It starts with `STARTUP ... photos=22621 displayed=22621`. It contains one Folder zoom layout line at 46.410 ms (`action=rebuild_rows`, columns 5→6), no `PIC_ZOOM apply` samples, and no chunk-specific realized-tile count.

The log records:

- 243,082 `PIC_THUMBNAIL gtk_bind` events across 13,120 distinct photo IDs.
- Some IDs were bound 33 times.
- 22,568 thumbnail cache hits and 1,373 misses.
- 17,230 paintable assignments and 3,824 filename-label batches.

The chunk component connects to `store.items_changed`; its callback runs `rebuild_chunks()`, which calls `chunks.remove_all()` and appends every slice again. `Gallery::replace()` changes the shared store in batches, so each notification discards the entire outer chunk model and causes GTK to tear down/rebind many inner-grid items. This is the main identified cause of the repeated thumbnail work and visible flicker. The trace's cache hits show that many accesses found existing cached thumbnail files, so it does not prove every original image was fully decoded again; however, the repeated rebinds and paintable assignments are real and sufficient to explain the bad experience. Do not benchmark this implementation as a valid nested-virtualization design.

There is a second visible issue: zoom currently takes the existing Folder ListView branch and rebuilds Legacy rows while the chunked view remains fixed at five columns. The trace's 46.410 ms `PIC_ZOOM layout` line is that path; the new inner grids are not resized.

Reproduce with:

```sh
cd /home/peet/Downloads/picasa/picasa-iphoto-clone
PICASA_FOLDER_CHUNKED=1 PICASA_TRACE=1 cargo run 2>&1 | tee /tmp/chunkv1.log
```

Wait for startup, switch to Folder mode and select **Network Shares → `pic-peet` → `pictures exported` → `bubs preg`**. That folder has 395 direct photos and URI `nfs://DietPi.local/mnt/4TBP/mypics/pic-peet/pictures%20exported/bubs%20preg`. The 395-photo legacy/direct comparison traces are described in `docs/FOLDER_GRIDVIEW_EXPERIMENT.md`.

## Prior 395-photo benchmark results

Use the same warmed thumbnail cache, window size, and zoom sequence when comparing:

| Path | Zoom applies | Median | Maximum | Other result |
| --- | ---: | ---: | ---: | --- |
| Legacy Folder ListView, corrected `/tmp/pic-folder-legacy2.log` | 19 | 219.91 ms | 255.26 ms | 18 row rebuilds; remaining same-column resize 3.12 ms |
| Direct GridView, `/tmp/pic-folder-gridview2.log` | 20 | 4.89 ms | 51.67 ms | No row rebuilds; 129–321 realized tiles |
| Chunked prototype, `/tmp/chunkv1.log` | No `PIC_ZOOM apply` samples | Not measurable | Not measurable | Bad run; excessive binds, no chunk realization metric |

The Legacy and direct GridView sequences were similar, not identical, so the timing ratio is indicative rather than a controlled final result. There are no valid chunked-path zoom or tile-realization measurements.

## GTK findings and architecture notes

- This checkout uses `gtk4 = 0.10` with GTK 4.12 bindings; the investigated host GTK runtime was 4.22.5.
- The generated `GtkGridView` API and installed GTK GIR expose no GridView header factory. `GtkListView` does expose `set_header_factory` and `GtkListHeader`.
- A `GtkFlattenListModel` can flatten a list of per-folder ListModels and expose section boundaries; that is a candidate outer ListView model for a future implementation. The current chunk prototype does not use it.
- A nested GridView has no separate scroller in the prototype. The outer ListView must own the only vertical adjustment. GTK realization and natural-height behavior still need direct measurement after model churn is fixed.
- GTK `GtkSliceListModel::new(model, offset, size)` takes a concrete `u32` size in gtk4-rs 0.10.3.

## Environment variables

- Default Legacy Folder view: unset both experiment flags.
- Direct-photo GridView experiment: `PICASA_FOLDER_GRIDVIEW=1`.
- Incomplete chunked experiment: `PICASA_FOLDER_CHUNKED=1`.
- Instrumentation: add `PICASA_TRACE=1` to any run.
- If both view flags are set, the chunked flag currently takes precedence in the Folder Stack selection, although the direct flag can still affect other Folder-mode logic. Do not use both together for comparisons.

## Current validation

At handover, `cargo check --offline` succeeds. `cargo test --offline --no-default-features` passes 376 tests; 25 display-dependent tests are ignored. These checks establish compilation and current non-display test status only. They do not verify that the chunked UI looks or behaves correctly, that the outer ListView virtualizes chunks, or that zoom/scrolling works.

## Next concrete milestone

Keep the code in this WIP state until OpenCode takes over. First remove full-model churn: preserve chunk objects and update only the affected tail/range when the shared store changes, instead of `remove_all()` plus recreating every slice on each `items_changed`. Add trace counters for outer rows realized, inner GridViews realized, bound tiles, and per-photo bind frequency. Re-run the exact `bubs preg` view and confirm binds remain bounded and thumbnails stop repeatedly reloading. Only after that should the next step wire zoom to the realized inner grids' column counts and measure zoom timings. Folder headers, global selection, navigation, and anchoring are later steps.

Do not make the chunked path default or merge it to `main` based on the current implementation.
