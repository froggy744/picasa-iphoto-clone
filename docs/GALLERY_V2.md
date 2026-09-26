# Gallery v2 architecture

Branch: `architecture/gallery-v2`

Base: `rc-bugfixes`

## Goal

Keep the deployment-ready RC application and replace only the Folder gallery layout behavior that made zoom structurally expensive.

## Architecture rule

Zoom must never rebuild, splice, reorder, or recreate photo membership.

The gallery keeps one direct photo model and zoom only changes presentation geometry:

- tile width / height
- GridView column count
- layout invalidation

The legacy Folder ListView represented each display row as a model item containing up to the current number of columns. A 6→5 column zoom therefore changed the model structure and required rebuilding Folder rows.

Gallery v2 routes Folder mode through the existing direct photo `GtkGridView`, where one model item remains one photo regardless of zoom level.

## Measured evidence from the prior experiment

On the same 395-photo Folder test:

- legacy Folder ListView zoom: about 219.9 ms median, 255.3 ms maximum
- direct photo GridView zoom: about 4.9 ms median, 51.7 ms maximum
- direct path performed zero Folder-row rebuilds

This was strong prototype evidence rather than a final controlled benchmark.

## Current branch behavior

The direct Folder GridView is the default.

The original Folder ListView is still present as a fallback. Launch with:

```sh
PICASA_LEGACY_FOLDER_LIST=1 cargo run
```

to compare against the old implementation.

The direct GridView currently uses the existing external folder indicator because GTK GridView does not expose the same full-width section-header factory available to GtkListView in the GTK API used by PIC.

## What has not been changed

This branch deliberately preserves the RC application around the gallery:

- database
- scanner/import pipeline
- albums
- favourites
- editor
- collage
- lightbox/viewer
- context actions
- themes
- network shares

## Immediate validation targets

1. Ctrl-wheel zoom repeatedly from minimum to maximum.
2. Confirm no Folder row rebuild appears in trace output on the default path.
3. Scroll large local folders while zooming.
4. Verify folder navigation lands on the intended photos.
5. Verify multi-selection, activation, context menu and viewer opening.
6. Compare with `PICASA_LEGACY_FOLDER_LIST=1` if a regression appears.
7. Only after interaction parity is stable, solve in-flow Folder section headings/actions without tying model membership to column count.

## Do not merge yet

Keep this branch separate from `rc-bugfixes` and `main` until runtime behavior is verified with the real library.

## Known issue: window resize pause

On application/window resize there is still a visible pause while the gallery recalculates layout/columns. Treat this separately from Ctrl-wheel zoom smoothness. The target is for resize to update presentation geometry without rebuilding photo membership or triggering expensive synchronous work proportional to the library size.

### Resize investigation update

The resize warning pattern showed the GridView retaining its previous minimum column geometry while the window was being dragged narrower (for example, a ~747 px requested width while the available width fell below it). The old three-frame width settle gate was designed to protect the legacy Folder ListView from repeated row-model rebuilds, but Gallery v2 no longer has that cost.

Gallery v2 now updates column count live during window resize and skips explicit GridView `queue_resize()` calls for width-only changes that do not cross a column boundary. The legacy Folder ListView fallback keeps the settle gate.

## Work completed since branch creation

The branch was created directly from `rc-bugfixes` so the deployment-ready RC line remained untouched.

### 1. Created the Gallery v2 branch

- Branch: `architecture/gallery-v2`
- Base: `rc-bugfixes`
- Purpose: keep the complete RC application and replace only the gallery architecture responsible for expensive Folder zoom/reflow.

No changes were made to `rc-bugfixes` or `main`.

### 2. Transplanted the proven direct-photo Folder GridView path

The earlier `experiment/folder-gridview` branch had already demonstrated that Folder mode could use the same direct `GtkGridView` / `PhotoObject` model as the normal photo grid.

The measured prototype result on the same 395-photo folder was:

- legacy Folder ListView: ~219.9 ms median zoom apply, ~255.3 ms maximum
- direct-photo GridView: ~4.9 ms median zoom apply, ~51.7 ms maximum
- direct path: zero Folder-row rebuilds during zoom

Only the direct GridView work was brought across. The later chunked/nested-GridView prototype was deliberately not imported because its model churn caused excessive rebinds/flicker.

Initial transplant commits:

- `71a3883` — Grid architecture switch/helper
- `4588153` — Folder grouping behavior for direct GridView
- `532fce8` — selection/navigation adaptation
- `c2543a7` — direct GridView view-path changes
- `eb89dfb` — virtualization/zoom-path changes
- `80c59ee` — window routing to the GridView Folder path

### 3. Made Gallery v2 the default Folder architecture

Commit:

- `0c5d997` — `Gallery v2: make direct Folder GridView default`

Folder mode now uses the photo-per-item GridView by default. The old Folder ListView remains in the codebase as a comparison/emergency fallback:

```sh
PICASA_LEGACY_FOLDER_LIST=1 cargo run
```

The architectural rule is that changing zoom/column count must not change photo membership.

### 4. Added the Gallery v2 handover document

Commit:

- `a2044a1` — `Docs: record Gallery v2 architecture`

This established this file as the architecture/status document.

### 5. Recorded the window-resize pause

The first real-world observation after switching to Gallery v2 was:

- Ctrl-wheel / gallery interaction felt substantially faster without trace logging.
- Window resizing still showed a visible pause, especially with a larger library.

Commit:

- `c638fd9` — `Docs: note Gallery v2 resize pause`

Resize was therefore separated from zoom as its own performance problem.

### 6. Fixed missing dependencies from the GridView transplant

The first release build exposed an incomplete dependency transplant:

- `SquareTile::set_filename_visible()` was missing.
- `THUMBNAIL_FILE_NAMES_SETTING_KEY` was missing.

The direct GridView experiment had been developed after an earlier thumbnail-filename feature, so the transplanted files expected that support to already exist.

Supporting pieces restored:

- filename caption CSS
- thumbnail filename DB setting
- `SquareTile` filename-caption implementation
- Settings UI support
- filename tile tests

Commits:

- `c0e9ffd` — restore thumbnail filename CSS
- `828662a` — restore thumbnail filename DB setting
- `0abbc4b` — restore `SquareTile` filename support
- `92dfd68` — restore Settings support
- `54277d8` — `Gallery v2: restore filename tile tests`

This fixed the reported build errors caused by `set_filename_visible` and `THUMBNAIL_FILE_NAMES_SETTING_KEY`.

### 7. Real-library observation after the dependency fix

A normal run without heavy tracing felt fast to the user.

A captured run with 3,086 photos showed:

- cold start: 10 ms
- displayed photos: 3,086
- folders: 5
- albums: 0
- scan disabled
- RSS: ~62 MB
- no panic/crash in the supplied log

This supports the conclusion that the basic direct GridView path is healthy and that heavy trace logging itself can affect perceived responsiveness.

### 8. Resize log exposed a minimum-width/layout negotiation loop

A second run captured repeated Adwaita warnings while resizing:

```text
GtkOverlay ... requested 747 px, 745 px available
GtkOverlay ... requested 747 px, 740 px available
...
GtkOverlay ... requested 747 px, 505 px available
```

and the same sequence occurred while growing the window again.

The key observation was that the requested ~747 px closely matched the previous GridView column geometry. The old width-settle gate intentionally delayed column-count updates for several frames. That made sense for the legacy Folder ListView because changing columns rebuilt Folder rows, but Gallery v2 no longer has that structural cost.

With Gallery v2, the delay meant the GridView could continue advertising the old multi-column minimum while the parent window was already much narrower, forcing GTK/Adwaita into repeated layout negotiation.

The user also reported that the app is fast with only a few photos, while resize pauses become more noticeable with a larger library. That remains an important scale-related validation point even after the minimum-width fix.

### 9. Changed Gallery v2 resize behavior

Two focused code changes were made.

Commit:

- `914fa58` — `Gallery v2: avoid same-column resize churn`

Behavior:

- width-only changes that keep the same column count no longer explicitly call `GridView::queue_resize()`
- GTK is already allocating the GridView for the new parent width, so forcing another resize on every drag frame was unnecessary

Commit:

- `e3dbbb7` — `Gallery v2: update columns live during window resize`

Behavior:

- Gallery v2 now updates the GridView column count immediately when live window resize crosses a column boundary
- the old three-frame width-settle behavior remains for the legacy Folder ListView fallback, where a column change can still rebuild its row model

Documentation commit:

- `645ca7d` — `Docs: record Gallery v2 resize fix`

The next runtime test must verify whether this reduces/removes the repeated ~747 px Adwaita warnings and improves resize smoothness with the 3,000+ photo library.

### 10. Made this a living handover

Commit:

- `c558b8b` — `Docs: make Gallery v2 status a living handover`

From this point onward, every meaningful Gallery v2 architecture change or confirmed test result should be recorded here with:

- what changed
- why it changed
- relevant commits
- observed/measured result
- regressions or remaining issues
- next concrete validation target

## Current state

Gallery v2 currently means:

```text
rc-bugfixes application
        +
existing mature features
        +
direct photo-per-item GtkGridView for Folder mode
        +
zoom/columns independent from photo membership
        +
legacy Folder ListView retained as fallback
```

Known/active validation items:

1. Re-test live window resize with 3,000+ photos after `914fa58` + `e3dbbb7`.
2. Check whether the repeated ~747 px GtkOverlay warnings disappear or reduce substantially.
3. Verify repeated Ctrl-wheel zoom remains smooth.
4. Verify scrolling, selection, context menus, viewer opening and folder navigation.
5. Verify behavior with a much larger library, not only the current ~3,086-photo test.
6. Investigate any remaining work whose cost grows with total photo count rather than realized/visible tiles.
7. Restore true in-flow Folder section headings/actions without reintroducing a column-sized photo-row model.
8. Do not merge into `rc-bugfixes` or `main` until these runtime checks pass.

## Working-document rule

Keep this file updated continuously while Gallery v2 work proceeds. After every meaningful architecture change or confirmed test result, record:

- what changed
- why it changed
- relevant commit(s)
- measured or observed result
- known regressions / remaining issues
- next concrete validation target

This document is the handover/source-of-truth for the Gallery v2 migration so work can continue across chats/tools without reconstructing the investigation from memory.

### Confirmed result: resize pause fixed

User validation on the real ~3,086-photo library confirmed that the visible application/window resize pause is fixed after the Gallery v2 live-column update changes.

Relevant commits:

- `914fa58` — `Gallery v2: avoid same-column resize churn`
- `e3dbbb7` — `Gallery v2: update columns live during window resize`

Important distinction from the follow-up log:

- The app now feels responsive during resize.
- Adwaita still prints repeated `GtkOverlay exceeds AdwApplicationWindow width` warnings while the window is dragged below the grid's requested width.
- Those warnings are therefore no longer treated as evidence of the user-visible pause. They remain a layout/minimum-size cleanup item, not a current Gallery v2 performance blocker.

Current conclusion: the old width-settle behavior was materially contributing to the resize stutter. Letting the direct GridView respond to column-boundary changes live, while avoiding unnecessary same-column `queue_resize()` calls, fixed the observed responsiveness issue.

Next priority should move back to Gallery v2 interaction parity and large-library behavior rather than further resize optimization, unless the remaining warning indicates a separate UI sizing bug.

### Resize warning attribution: sidebar collapse / auto-hide

Follow-up user observation: when the application is resized below the compact breakpoint, the sidebar automatically collapses/auto-hides. This matches the remaining Adwaita warning timing.

The window uses `AdwOverlaySplitView`. On narrow widths it enters collapsed mode, where the sidebar becomes an overlay rather than permanently consuming content width. The code also changes the collapsed sidebar maximum width and supports hover reveal/auto-hide while collapsed.

Current interpretation of the remaining warning:

```text
GtkOverlay exceeds AdwApplicationWindow width
```

is that it occurs during the responsive sidebar transition while the overlay/split-view hierarchy briefly retains a wider requested size than the shrinking window allocation. Since the user-confirmed resize pause is fixed, this warning is no longer treated as evidence of Gallery v2 performance work.

Status:

- live resize responsiveness: fixed
- Gallery v2 column reflow: responsive
- sidebar compact auto-hide: expected responsive behavior
- remaining Adwaita width warning: likely transition/layout diagnostic associated with sidebar collapse; cleanup only unless it produces a visible UI defect

Do not optimize Gallery v2 around this warning unless a visible regression accompanies it.

### Responsive sidebar breakpoint now uses the existing slide animation

User clarified that the sidebar already slides smoothly when manually hidden/revealed, and requested the same visual effect when the application is resized below/above the compact breakpoint.

Implemented in commit:

- `ffb012e` — `Gallery v2: animate sidebar at responsive breakpoint`

Behavior:

- entering collapsed/compact mode explicitly calls the existing animated `set_show_sidebar(false)` path instead of relying on the breakpoint state change alone
- the sidebar's logical pinned state is preserved
- leaving collapsed mode restores a pinned sidebar through `set_show_sidebar(true)`, so it slides back in rather than abruptly reappearing
- hover reveal behavior in compact mode remains unchanged

Validation target: resize slowly across the compact breakpoint in both directions and confirm the sidebar slides out/in with the same feel as the existing manual hide/reveal action, without reintroducing gallery resize stutter.

### Sidebar breakpoint animation: first attempt superseded

The first implementation (`ffb012e`) reacted to `collapsed-notify` by calling `set_show_sidebar(false/true)`. User testing showed no visible improvement: the sidebar still cut out/in at the breakpoint.

Root cause: the 1050 px `AdwBreakpoint` was setting both `collapsed=true` and `show-sidebar=false` in the same breakpoint update. In addition, `AdwOverlaySplitView` automatically changes sidebar visibility when collapsing unless `pin-sidebar` is enabled. Therefore the sidebar had already been hidden before the notify callback could request the normal slide animation.

Superseding fix:

- `d79c286` — `Gallery v2: decouple sidebar hide from compact breakpoint`
  - breakpoint now changes only `collapsed`
  - removes the simultaneous `show-sidebar=false` setter
- `8b14ac0` — `Gallery v2: preserve sidebar through collapse transition`
  - enables `AdwOverlaySplitView::pin-sidebar` so libadwaita does not auto-hide/show the sidebar when `collapsed` changes
  - PIC's existing collapsed-notify handler now owns the visibility transition and calls the normal animated `set_show_sidebar(false/true)` path

Validation target: resize slowly across 1050 px with the sidebar open. Expected behavior is now slide-out on entering compact mode and slide-in when widening back out, matching the manual sidebar hide/reveal animation.
