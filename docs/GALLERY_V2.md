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

### Responsive sidebar breakpoint: staged transition implementation

User testing confirmed that both earlier breakpoint approaches still produced a hard cut at roughly the 3-column/2-column width. The reason is structural: changing `AdwOverlaySplitView::collapsed` immediately changes the split from side-by-side to overlay. Once that happens, a later `show-sidebar` animation cannot recreate the visual slide-out from the original layout.

The responsive behavior was therefore rewritten as an explicit staged state machine instead of an `AdwBreakpoint` setter.

Commits:

- `d7b2929` — `Gallery v2: separate collapse state from sidebar animation`
- `3f77a9a` — `Gallery v2: stage sidebar collapse after slide animation`
- `be15005` — `Gallery v2: fix staged sidebar callback ownership`

New sequence:

```text
narrowing past 1050 px
    -> set_show_sidebar(false)
    -> allow normal drawer slide-out (~280 ms)
    -> set_collapsed(true)

widening past 1080 px
    -> keep sidebar hidden
    -> set_collapsed(false)
    -> next main-loop turn set_show_sidebar(true) if logically pinned
    -> normal drawer slide-in
```

A 30 px hysteresis band (1050 enter / 1080 exit) prevents repeated collapse/expand chatter while dragging near the threshold. The sidebar's logical pin state remains unchanged, and compact hover reveal remains available after collapse.

Validation target: resize slowly and quickly through the 1050–1080 px region with the sidebar visible. The sidebar should visibly complete its slide-out before the content switches to compact overlay mode, and slide back in after returning to expanded mode. Confirm that the previously fixed gallery resize responsiveness remains intact.

### Responsive sidebar shrink path corrected after v4 test

User test result: widening worked correctly (sidebar slid back in), but shrinking still looked like a hard cut rather than a slide-out.

The v4 log showed why. At the shrink boundary GTK was already producing invalid allocations while the sidebar was still participating in the side-by-side layout, including a header width of `-1`, a scrolled-window width of `-1`, and several zero/negative child allocations. Holding the sidebar in the expanded split for ~280 ms while waiting for the slide to finish therefore allowed the shrinking window to squeeze the layout before collapse.

Superseding fix:

- `79d907b` — `Gallery v2: slide sidebar out from overlay mode`

New shrink sequence:

```text
cross compact threshold while sidebar is visible
    -> set collapsed=true immediately while keeping sidebar visible
    -> gallery receives compact/overlay width immediately
    -> next main-loop turn call set_show_sidebar(false)
    -> sidebar slides out as an overlay instead of being squeezed by the shrinking split
```

The working widening sequence is retained: expand while hidden, then slide the pinned sidebar back in.

Validation target: shrink slowly and quickly through the compact threshold. The sidebar itself should remain visible through the side-by-side -> overlay switch and then visibly slide off the left edge, without the negative allocation warnings seen in v4.

### Responsive sidebar animation timing matched to pin/hover behavior

The existing pin button and hover auto-hide/reveal do not use a separate duration; they rely on libadwaita's built-in `AdwOverlaySplitView::set_show_sidebar()` animation.

Commit:

- `ca2366a` — `Gallery v2: match responsive sidebar drawer timing`

The responsive shrink path still switches to collapsed/overlay mode first to free gallery width, but now waits for one actual rendered frame with the sidebar still visible in overlay mode before calling `set_show_sidebar(false)`. This prevents the collapse and hide from visually merging into a faster cut and ensures the drawer motion uses the same libadwaita animation path/speed as the existing pin and hover auto-hide behavior.

Validation target: compare manual pin hide, hover auto-hide, and resize-triggered hide side-by-side. Their drawer motion should now feel the same; only the responsive path has the extra one-frame setup needed to establish overlay mode before the animation starts.

## Completed: responsive sidebar transition

Status: **COMPLETE**

The compact-window sidebar behavior is now considered complete for Gallery v2.

Final behavior:

- wide layout: sidebar is part of the normal split view
- shrinking through the compact threshold: sidebar moves into overlay mode and uses the normal libadwaita drawer animation to hide
- widening back out: layout expands while the sidebar is hidden, then a logically pinned sidebar uses the same drawer animation to slide back in
- manual pin hide/show and hover auto-hide/reveal continue to use the same `AdwOverlaySplitView::set_show_sidebar()` animation path
- Gallery v2 resize responsiveness remains fixed

Final relevant commits:

- `79d907b` — slide sidebar out from overlay mode
- `ca2366a` — match responsive sidebar drawer timing
- `26175b6` — document matched timing

Earlier breakpoint-animation attempts are retained in the history above for context but are superseded by the final overlay-first implementation.

## Next target: animated thumbnail zoom transitions

User identified the remaining visual difference from Picasa: Picasa does not simply cut from one thumbnail size/layout to the next when zooming. Its thumbnails visibly transition between geometries, which makes zoom feel continuous even when the column count changes.

PIC currently has the correct Gallery v2 architecture for fast zoom — photo membership stays stable and the direct GridView avoids Folder-row rebuilds — but the presentation still snaps directly to the new tile width/height and column count.

Current behavior:

```text
zoom input
-> apply new tile width/height
-> change GridView column count when needed
-> GTK relayout
-> visible cut/snap to new geometry
```

Target behavior:

```text
zoom input
-> keep stable photo model
-> interpolate realized thumbnail geometry from old size/position to new size/position
-> animate over a short frame-based transition
-> settle at the new GridView geometry
```

Important constraint: this must remain a presentation-only animation. It must not reintroduce photo-model replacement, Folder-row rebuilding, database queries, thumbnail decoding, or work proportional to the full library during each animation frame.

The implementation should primarily operate on the realized/visible tile pool and preserve the existing fast Gallery v2 zoom path.

### Animated thumbnail zoom implemented

Gallery v2 now has a first implementation of Picasa-style animated thumbnail zoom instead of snapping directly between tile sizes.

Commits:

- `8442e59` — add zoom animation generation/state
- `b22e0e0` — animate thumbnail geometry on GTK's frame clock
- `1f191c9` — fix animated zoom frame updates

Implementation:

- canonical zoom targets remain unchanged (`100, 117, 137, 160, 187, 219, 256, 300` px)
- zoom animation duration is ~180 ms
- easing is cubic ease-out
- each frame interpolates realized tile width/height between the current visual size and target size
- the direct `GtkGridView` keeps the same photo model and naturally repositions tiles as their geometry changes, producing a combined scale + slide/reflow effect
- settings are persisted only once when the animation reaches the final canonical zoom level
- a generation counter cancels an older animation when a newer zoom target starts, allowing rapid wheel input to retarget from the current visual size instead of queueing animations
- the legacy Folder ListView fallback remains on immediate zoom because its row membership is still column-dependent
- trace output is suppressed for intermediate animation frames to avoid distorting frame timing

Performance invariant remains unchanged: no DB query, photo-model replacement, Folder-row rebuild, or full-library operation should be introduced per animation frame. Only the realized GridView tile pool and GTK layout should participate.

Validation target:

1. Build and run normal Gallery v2.
2. Test one-step + / - zoom and Ctrl-wheel zoom.
3. Confirm thumbnails visibly grow/shrink and slide into their new grid positions instead of cutting to the destination layout.
4. Test rapid repeated wheel input; it should retarget smoothly without queueing long animations.
5. Verify the viewport does not jump unexpectedly when a column boundary is crossed.
6. Verify 3,000+ photo performance remains responsive.
7. Compare animation feel against Picasa; tune duration/easing only after behavior is stable.

### Zoom animation follow-up: thumbnail flicker suppression

User validation: the new scale + slide zoom effect looks good, but a slight thumbnail flicker remains during some reflow frames.

Likely cause: `GtkGridView` may recycle/rebind a small number of realized tiles while animated geometry crosses a column boundary. The normal GridView bind path clears the old paintable before loading/applying the correct presentation paintable. That blank frame is visible as flicker during animation.

Fix commits:

- `e9bb449` — `Gallery v2: track zoom visual motion`
- `97763b3` — `Gallery v2: preserve thumbnails during zoom motion`
- `8da6a25` — `Gallery v2: suppress thumbnail blanking during zoom`

Behavior during active zoom animation now mirrors the existing fast-scroll visual backstop:

- do not blank an already painted tile merely because GridView recycled/rebound it during motion
- if the correct target paintable is already in RAM, replace atomically
- if it is not yet available, keep the previous paintable temporarily rather than flashing the empty placeholder
- async completion still validates the presentation key before applying, so a stale completion cannot permanently land on the wrong recycled tile
- the preservation flag is enabled only for the active zoom animation and is cleared when the current animation completes

Validation target: repeat single-step and rapid Ctrl-wheel zoom across column boundaries and confirm the previous slight blank/flash is reduced or eliminated without persistent wrong-thumbnail artifacts.

### Zoom jitter root cause identified and fixed

User supplied a focused `PICASA_ZOOM_TRACE` capture for the visibly jittery end of the zoom sequence.

Finding: thumbnail decoding/rebinding was not the bottleneck. Recycled tiles were rebinding from RAM in roughly 27–37 us. The visible jitter came from column-count oscillation during one animation: the GridView repeatedly alternated between two content widths (for example ~1088 and ~1128 px), causing repeated 5 -> 4 -> 5 -> 4 column transitions. Each column flip recycled another block of realized tiles, producing the visible twitch despite paintable preservation.

Fix commits:

- `7b26262` — `Gallery v2: freeze layout width during zoom`
- `c77fb57` — `Gallery v2: stop column ping-pong during zoom`

Implementation:

- capture one stable GridView content width at zoom-animation start
- use that frozen width for every per-frame column calculation during the animation
- clear the frozen width when the animation finishes
- tile width/height still animate frame-by-frame, so column count may legitimately cross a boundary once as geometry changes
- GTK's transient competing allocations can no longer make the same animation bounce repeatedly between adjacent column counts

Validation target: repeat the previously jittery boundary several times. The trace should show at most the intended one-way column transition for that zoom step, not repeated alternating old/new column pairs.

### Zoom jitter v9: complete width-path checkout and corrected freeze point

User reported the same jitter now visible around the 4/5-column boundary and supplied `galv2.v9.log`.

The v9 tail proved that the previous frozen-width fix was applied at the wrong layer. During a single animation, `update_layout()` was still receiving alternating widths of roughly 1088 and 1128 px, causing repeated adjacent-column transitions and realized-tile recycling. The animation helper had frozen `GtkGridView::width()`, but the GridView's own requisition changes as columns change, so that value participates in the feedback loop rather than providing a stable viewport measurement.

A complete call-path checkout found:

- `apply_tile_geometry()` is the zoom-driven layout path
- the outer gallery frame callback in `window/build.rs` also calls `Gallery::update_width()` continuously
- `window/layout.rs` contains one additional deferred width update path
- `view.rs::update_layout()` is the only production path that changes GridView min/max columns
- tile-level `queue_resize()` calls are expected geometry invalidations, not independent column calculations
- no second model replacement or hidden column setter is causing the jitter

Corrected fix commits:

- `6304a75` — `Gallery v2: freeze outer viewport width during zoom`
  - `update_width()` now ignores transient resize/requisition feedback while a zoom animation owns a frozen viewport width
- `c468955` — `Gallery v2: anchor zoom to outer gallery width`
  - zoom now captures `last_layout_width`, which originates from the outer gallery surface, instead of `GridView::width()`

Expected result: a zoom step may cross each column boundary once as tile geometry changes, but the same animation must no longer alternate between two widths/column counts such as 1088/1128 and 4/5 repeatedly.

Keep `PICASA_ZOOM_TRACE=1` enabled for the next validation run. If the trace shows one-way boundary crossings and the visible jitter is gone, remove the temporary trace instrumentation and mark animated zoom stable.
