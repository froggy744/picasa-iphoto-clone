# Library Redesign Integration Plan

Base branch: `rc-bugfixes` @ `09bb8259f1dbee7ece84af415977c14644de1abf`
Reference prototype: `prototype/library-folder-groups`
Integration branch: `integration/library-redesign`

## Why this base

`rc-bugfixes` is the correct full-app base. It includes the current editor/UI work, including the newer Text and Overlay design, network-share fixes, album/sidebar fixes, and the latest RC bug-fix work. The redesign must replace only the gallery/library rendering architecture, not these mature features.

## Non-negotiable architecture rules

1. Persistent photo models. Zoom must never reshape photo membership.
2. Folder mode is a continuous stream of normal, non-sticky folder sections.
3. Each folder section contains a real `GtkGridView`.
4. Zoom and resize only change presentation geometry and column count.
5. Thumbnail decode/cache work stays off the GTK main thread.
6. Recycled tiles ignore stale async results.
7. No zoom-dependent row/chunk model.
8. No custom chunk-mounting virtualization.
9. No gallery rebuild solely to navigate to a folder.
10. Never hold a `RefCell` borrow across GTK calls that may re-enter.

## Preserve from rc-bugfixes

The following are preserved and adapted to the new gallery rather than rewritten:

- SQLite library and scan/import pipeline
- Existing `Photo` / `PhotoObject` metadata
- Favourites
- Albums and album management
- Library Home
- Recently Added / History
- Sidebar and folder tree
- Search UX
- Sorting
- Existing non-folder grouping where still required
- Info bar
- Multi-selection
- Grid context menu
- Lightbox/viewer and keyboard navigation
- Edit Mode
- Crop
- Filters
- Overlay editor
- Text editor
- Copy/paste/reset edits
- Export and Print
- Collage
- Offline indicators/retry
- Network/NFS/SMB support
- Themes/settings
- Existing thumbnail/database cache where compatible

## Phase 0 — Freeze checkpoints

- Keep `rc-bugfixes` untouched.
- Keep `prototype/library-folder-groups` untouched.
- Work only on `integration/library-redesign`.
- Record current `cargo check` and test baseline.
- Add a feature switch if useful so old and new gallery can temporarily coexist.

Exit: integration branch behaves exactly like rc-bugfixes before gallery wiring changes.

## Phase 1 — Add Gallery V2 beside the old gallery

Create an isolated module:

```
src/gallery_v2/
    mod.rs
    state.rs
    photo_factory.rs
    folder_sections.rs
    thumbnail_loader.rs
    navigation.rs
    trace.rs
```

Port the proven prototype mechanics but use the real RC `PhotoObject`, DB IDs, edit state, favourite state, rotation, offline state and cached-thumbnail paths.

Important: do not copy the prototype's temporary simplified `PhotoObject` or temporary favourites file.

Exit:
- Gallery V2 can render real RC photos.
- No existing navigation uses it yet.
- `cargo check` and current tests still pass.

## Phase 2 — Replace Library → Photos first

Route All Photos through Gallery V2 as a flat persistent model.

Preserve:
- current sort order
- selection / multi-selection
- favourite badge
- edited badge
- offline badge
- right-click context menu
- info bar
- Enter/Space/double-click
- keyboard movement
- current collection identity
- exact return-to-grid behaviour

Zoom:
- updates tile size only
- recalculates columns only
- never rebuilds/replaces the photo model

Exit:
- All Photos scroll and zoom match prototype speed.
- Existing actions from a selected tile work unchanged.

## Phase 3 — Replace Folder browsing

This is the main architectural replacement.

```
outer folder-section ListStore
        ↓
GtkListView
        ↓
folder header + GtkGridView
                    ↓
             persistent photo model
```

Rules:
- folder header is normal, never sticky
- one continuous stream
- sidebar click scrolls to existing section
- no row/chunk membership based on zoom
- no folder model rebuild for navigation
- preserve real folder counts/tree/network roots

Remove the old folder row/chunk path from active use once this phase is proven.

Exit:
- fast wheel/touchpad scrolling
- fast scrollbar dragging
- fast zoom through all column transitions
- folder sidebar jump works
- local + network folders work

## Phase 4 — Integrate production thumbnail pipeline

Use the prototype behaviour as the performance contract, but integrate with the mature app cache.

Target flow:

```
bind tile
  ↓
memory texture cache?
  ├─ yes → display immediately
  └─ no
       ↓
existing cached display thumbnail?
  ├─ yes → decode off GTK thread
  └─ no  → background source fetch/decode/cache
       ↓
main-thread texture update only if tile still represents same photo
```

Requirements:
- local JPEG/PNG/WebP/TIFF/etc.
- EXIF/library rotation preserved
- edited preview behaviour preserved
- offline cached thumbnails remain visible
- SMB/NFS reads never block GTK
- bounded worker queue
- stale/cancelled requests do not flood work
- memory cache has a sensible bound

Exit:
- no synchronous original-image load in tile bind
- cold and warm cache both remain interactive

## Phase 5 — Search

Keep the existing RC search UI and folder suggestions, but feed results into Gallery V2.

Rules:
- global search stays global
- filename and folder matching retained
- no disk rescan while typing
- search result model is independent of zoom
- clearing search restores previous destination/grouping cleanly

Exit:
- typing search remains responsive
- opening a search result and returning preserves expected state

## Phase 6 — Favourites, Albums, Recently Added, History

Move one destination at a time to Gallery V2:

1. Favourites
2. Albums
3. Recently Added
4. History

Preserve each destination's DB semantics and ordering. Only presentation changes.

History's existing date grouping can remain a separate grouping mode if needed; do not force folder-section architecture onto views whose grouping semantics are different.

Exit:
- all main library destinations use Gallery V2 where appropriate
- no regressions in album/favourite/history workflows

## Phase 7 — Selection and action parity

Verify and reconnect every tile action:

- single selection
- Ctrl multi-selection
- keyboard selection
- favourite
- add/remove album
- edit
- reset/copy/paste edits
- collage creation
- export
- print
- rotate
- open with
- retry/remove offline
- context menu at pointer
- info bar metadata

Exit: action parity with rc-bugfixes.

## Phase 8 — Viewer / editor return paths

Ensure the new grid provides stable collection snapshots/IDs to:

- lightbox
- previous/next
- Edit Mode
- Done/back
- Crop
- Export
- collage/editor transitions

Do not make the viewer query widget rows to determine navigation order; it should use the current logical photo collection/model.

Exit:
- Grid → Viewer → Edit → Done → Grid returns correctly
- arrow navigation follows the exact active collection

## Phase 9 — Sidebar integration

Keep the current mature sidebar visuals and behaviours.

Change only destination routing:
- Photos → Gallery V2 flat
- Favourites → Gallery V2 flat
- Album → Gallery V2 flat
- Folder → Gallery V2 continuous folder stream + jump
- Home → current Home
- History → appropriate Gallery V2 grouped mode

Preserve:
- folder tree
- albums
- live counts
- network share tree
- rename/create actions
- sidebar show/hide

## Phase 10 — Remove old gallery architecture

Only after feature parity:

Remove or retire active use of:
- zoom-dependent chunk planning
- old folder row construction
- old custom folder virtualization/mount logic
- duplicate thumbnail bind paths
- obsolete scroll-to-folder calculations
- diagnostics that only serve the old architecture

Do not delete useful shared helpers merely because they lived under the old grid module.

Exit:
- one authoritative gallery engine
- no hidden fallback path rebuilding old folder rows

## Phase 11 — Performance validation

Keep structured `PICASA_TRACE=1` instrumentation.

Measure:
- startup
- first visible thumbnails
- warm-cache startup
- continuous wheel scroll
- scrollbar drag top→bottom
- zoom min→max→min
- column-boundary changes
- folder jump
- search
- favourite view
- large folder
- 4k+ photo library
- network share library

Performance guardrails:
- no GTK-main-thread image decode
- ordinary zoom callbacks remain sub-millisecond where practical
- column transitions must not trigger full model replacement
- no repeated full-gallery thumbnail enqueue on a simple zoom
- no panic/reentrant `RefCell` borrow

## Phase 12 — Final replacement

After manual approval:

1. Run full tests.
2. Run `cargo check --release`.
3. Test Linux local folders.
4. Test SMB/NFS folders.
5. Test editor/Text/Overlay/Collage.
6. Test search/favourites/albums.
7. Keep `rc-bugfixes` as rollback branch.
8. Merge `integration/library-redesign` into the chosen RC branch only after approval.

## First implementation slice

Start with Phase 0–2 only:

- baseline rc-bugfixes
- add Gallery V2 module
- wire only Photos/All Photos
- retain old folder browsing temporarily
- verify selection/context menu/lightbox/info bar
- measure zoom/scroll

Once that is stable, move Folder mode. This minimizes the chance of breaking the mature editor, Text, Overlay, Collage and network-share work while replacing the part that caused the performance problem.
