# PIC — History & Library Home Page Development Plan

## Objective

Implement a Picasa/iPhoto-inspired History system and Library Home Page in PIC (Rust + GTK4/libadwaita).

Work in three phases, starting with History.

**Important: Inspect the current repository and architecture before making changes. Do not assume the code structure matches older versions.**

## PHASE 1 — History

Implement a persistent History system that tracks editing activity, not photo imports or browsing.

### Track these actions

* Photo edited and saved.
* Edits copied and pasted onto another photo.
* Batch paste of edits onto multiple photos.
* Reset edits.
* Collage created.
* Existing collage modified and saved.

Importing, scanning, viewing, selecting or exporting an unchanged photo must not create History entries.

### Database

Extend the existing SQLite database safely.

Maintain two related concepts:

1. Recently edited items — one entry per photo or collage, ordered by last editing time.
2. Editing events — individual actions, including grouped batch operations.

A batch paste onto 100 photos must be recorded as one batch event associated with all 100 affected photos.

The Recently Edited gallery should still show the individual photos.

Never modify original image files to store History.

### Sidebar

Under Library, add:

* All Photos
* Favourites
* Recently Added
* History

History should use the existing GTK4 photo-grid infrastructure wherever practical.

### History behaviour

* Most recently edited items first.
* Show cached thumbnails.
* Show editing date and time.
* Distinguish photos from collages.
* Re-editing an existing photo moves it to the top.
* Persist across application restarts.
* Clicking a photo opens it for further editing.
* Clicking a collage opens its saved editable collage project, not merely the exported JPEG.
* Handle unavailable network originals using the existing offline-indicator behaviour.

Do not create a new History entry for every slider movement.

## PHASE 2 — Library Home Page

Make the Library heading clickable.

Clicking Library should open a dedicated Library Home Page containing:

**Recently Added**

Show a preview of recently imported photos, with a View All action that opens Recently Added.

**Recently Edited**

Show recently edited photos and collages from the History database, with a View History action.

**Albums**

Show album cover thumbnails, names and photo counts, with navigation to individual albums.

Use the existing thumbnail cache rather than decoding full-resolution originals.

The Library Home Page must not trigger network scans or unnecessary image decoding.

Keep All Photos as a separate view containing the complete photo collection.

## PHASE 3 — Polish

* Group History by Today, Yesterday and earlier dates.
* Allow navigation from a batch-paste event to the affected photos.
* Preserve editing state when reopening photos and collages.
* Refresh the History and Library Home Page after editing activity.
* Ensure performance remains good with thousands of imported photos.
* Handle missing or offline originals without freezing the GTK interface.

## Development rules

1. First inspect the current editor, collage, SQLite, sidebar, grid and window implementations.
2. Identify the actual save/commit points for edits and collages.
3. Propose the minimum database and UI changes needed.
4. Implement Phase 1 first.
5. Do not implement Phase 2 until Phase 1 is working and tested.
6. Preserve the existing non-destructive editing behaviour.
7. Do not delete or overwrite existing library data.
8. Do not introduce regressions in Albums, Favourites, Recently Added, search, lightbox or thumbnail caching.
9. Add tests for History persistence, ordering, batch paste and repeated editing.
10. Run cargo fmt, cargo check and cargo test.
11. Report what was changed and any outstanding issues.

**Start by inspecting the repository and implementing Phase 1 — History.**
