# Picasa Clone Rebuild — Status Report

**Branch:** `rebuild/prototype-rc-shell`  
**Last verified integrated head before this report:** `8e3792c5c4485ba7b1542ba4133c2b9425615344`  
**Head title:** `Rebuild: restore multi-selection in folder grids`  
**CI status at that head:** Passed successfully  
**Report date:** 2026-09-26

## Rebuild goal

Rebuild the Picasa/iPhoto-style application around the stable prototype gallery architecture, while restoring RC features incrementally without reintroducing the previous competing gallery/state paths.

The main architecture rule is:

> Preserve the prototype gallery stream as the single source of truth. New features should observe or wrap that state rather than replace it with a second gallery model.

## Completed work

### Prototype gallery and catalogue foundation
- Restored the grouped continuous folder gallery.
- Uses the GTK/GIO list model and GridView architecture from the stable prototype.
- Restored catalogue/library groundwork.
- Kept gallery state centralized instead of adding competing folder/photo replacement paths.

### Albums and Recently Added
- Restored Albums.
- Restored Recently Added.
- Albums use the same prototype gallery stream.
- Added right-click Add to Album support.
- Fixed album UI lifetime issues.

### Overlay viewer / lightbox
- Added RC-style overlay viewer instead of a separate window.
- Previous/next navigation with arrow keys.
- Mouse-wheel navigation.
- Escape and double-click close.
- Native 1:1 viewing.
- Incremental +/- zoom.
- Ctrl+wheel zoom.
- Drag-to-pan.
- Space/1 toggles 1:1.
- 0 resets to Fit.
- Viewer navigation resets zoom appropriately.
- Background image decoding uses worker threads and a generation guard so stale decodes are discarded.
- Persisted photo rotation is applied in the viewer.
- Viewer selection remains synchronized with the bottom information bar.

### Selection and bottom information bar
- Added photo-selection observation without creating a second gallery state.
- Displays filename, folder, dimensions, file size, date and camera metadata.
- Added Favourite action.
- Added Export action.
- Added Rotate action.
- Added 1:1 action.
- Restored multi-selection in folder grids.

### Favourites
- Favourite state persists in the catalogue.
- Favourite changes update the active photo state and relevant views.

### Export
- Added FileDialog-based export/copy of the original.
- Current export is still an original-file copy; a later rendering pipeline is required to bake rotation and future non-destructive edits into exported images.

### Themes
- Added lightweight ThemeManager.
- Loads `themes/<name>/theme.css`.
- Falls back to embedded standard CSS.
- Reads and writes the existing `appearance-theme` setting.
- Added theme selection to the header.

### Persistent settings
- Added catalogue-backed setting read/write support.
- Thumbnail size persists across restarts.
- Photo sorting persists across restarts.

### Offline cached thumbnails
- Standardized cache under `picasa-rs`.
- Thumbnail identity includes stored source fingerprint data.
- Cached thumbnails can be displayed when the original is offline without requiring the original file to be decoded again.
- Offline photos show a warning badge.
- Added explicit Refresh to re-import catalogue roots asynchronously.

### Rotation
- Added persisted 0/90/180/270 rotation.
- Rotation participates in thumbnail cache identity.
- Thumbnail and viewer decoding apply the stored rotation.
- Rotating a photo refreshes the affected thumbnail/viewer state.

### Sorting
Persistent sorting now supports:
- Date Taken
- Name
- File Size
- Dimensions
- Date Added
- Ascending / Descending direction

The selected sort is applied to catalogue records before gallery PhotoObjects are built.

### Thumbnail zoom
- Grid thumbnail size persists.
- Restored Ctrl+mouse-wheel thumbnail zoom.
- It adjusts the existing gallery scale rather than rebuilding/replacing the prototype layout.

### EXIF and metadata indexing
- Added EXIF support.
- Indexes DateTimeOriginal/DateTime where available.
- Indexes camera Make/Model.
- Stores dimensions, file size and modification fingerprint.
- Uses file modification time as a date fallback.
- Avoids rereading metadata when the stored file fingerprint has not changed.
- Date and camera information are exposed in the bottom information area.

### CI
- Rebuild workflow is configured with branch concurrency and `cancel-in-progress`.
- Intermediate CI runs may therefore show Cancelled when a newer commit supersedes them.
- The last verified integrated rebuild head, `8e3792c`, completed successfully.

## Deliberately not restored yet

The following larger subsystems were intentionally kept out of the initial rebuild while the core gallery/viewer architecture was stabilized:

- Non-destructive Edit mode
- Collage
- Network Shares

These should be restored only after the core navigation, selection, context-menu and offline workflows are stable.

## Important remaining work

### Core runtime polish
- Smoke-test folder, album, favourite and search navigation.
- Verify selection clearing/preservation when changing views.
- Runtime-test cached thumbnails with an actually disconnected/missing source.
- Add richer offline actions such as Retry and Remove from Library.
- Polish sort-menu selected-state feedback.

### Context menu
Restore a compact Nautilus-style context menu using the stable gallery observer architecture:
- Favourite
- Add to Album
- Rotate
- Export
- Open in folder
- Trash, with safe confirmation/handling

The lightbox/viewer should later share the same action model.

### Viewer polish
- Continue synchronization of bottom controls and viewer zoom state.
- Verify pan cursor and 1:1 state behavior.
- Restore viewer right-click actions without introducing per-tile unstable popovers.

### Editor
When restored, Edit mode must remain non-destructive:
- edits stored in the database
- original file untouched
- export renders edits to a new file
- exposure / contrast / saturation
- crop
- rotate
- reset
- copy/paste edits
- single-action undo behavior
- incremental zoom
- 1:1
- drag-pan
- edited-state thumbnail indicator

### Collage
Restore after Edit mode/core actions are stable:
- Grid
- Mosaic
- Picture Pile
- Shuffle
- Aspect/background/spacing controls
- 3840px JPEG export
- Rotation and offline handling

### Network Shares
Restore last. Previous RC testing showed occasional multi-second NFS read stalls, so network sources should not be allowed to destabilize the rebuilt local-library/gallery architecture.

## Current rebuild checkpoint

The rebuild has moved beyond the bare prototype and now has a functional Picasa-style core:

**catalogue + continuous grouped gallery + albums + recently added + favourites + selection + viewer + zoom/pan/1:1 + rotation + export + themes + persistent sorting + persistent grid zoom + Ctrl-wheel zoom + offline cached thumbnails + EXIF metadata + refresh + multi-selection.**

The next development phase should concentrate on runtime polish and shared photo actions/context menus before restoring the larger Edit, Collage and Network Shares subsystems.
