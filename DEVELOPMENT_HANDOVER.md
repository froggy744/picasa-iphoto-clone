# Development handover — Phase 3 History date grouping

Updated: 2026-09-23. This section supersedes the horizontal-rows and Phase 2 reports below.

## Current status

Phase 3 (History grouped by last edit date) is implemented in the working tree. Phase 1 and Phase 2 remain implemented. Batch-event navigation from TODO-HISTORY.md is **not** started.

- Branch: `rc5`.
- HEAD: `1622a42` (Phase 2 commit). No new commit was made.
- Workspace: `/home/peet/Downloads/picasa/picasa-iphoto-clone`.
- Uncommitted work includes Phase 3 plus the earlier horizontal Home rows.
- `.flatpak-builder/` remains untracked and untouched.

## Task and completed features

Group History entries by last editing date (newest-edited-first), without breaking virtualization, search, cached thumbnails, offline handling, or photo/collage navigation.

Completed:

1. `Photo.edited_at` (epoch millis, 0 for ordinary library rows) is populated from `recently_edited.edited_at` in the History query only.
2. `PhotoObject.edited_at` GObject property mirrors the field; `Gallery::replace` equality includes it so History↔library swaps rebuild tiles.
3. Internal sticky `GroupMode::History` (not user-selectable, never persisted — `group_mode_key` maps it to `"none"`).
4. `history_group_label` buckets: **Today**, **Yesterday**, **Earlier This Week** (rolling 7 days excluding today/yesterday), **Earlier This Month** (same `%Y-%m` as today), then `%b %Y` for older; `<= 0`/invalid → **Unknown Date**.
5. `apply_gallery_grouping(..., search_is_empty)` forces History mode when the History filter is active and search is empty; active search uses `GroupMode::None` (search results are ordinary photos).
6. Search handlers treat History like Folder: restore grouping on empty query, leave the stream while text is active.
7. Toolbar group control treats History like None/Folder (None active); sort/group toggles pass `search.borrow().is_empty()`.
8. Collection navigation and other `apply_gallery_grouping` call sites pass `search.is_empty()` (or `true` where search is already cleared).

## Files changed for Phase 3

```text
src/db.rs                  Photo.edited_at field
src/db/history.rs          history_photos_limited sets photo.edited_at from row timestamp
src/db/tests.rs            photo_from_row helper zeroes edited_at
src/grid/grouping.rs       GroupMode::History, group_label branch, history_group_label
src/grid/history_tests.rs  Label buckets, edited_at plumbing, range merge (display), caption reuse
src/grid/virtualization.rs replace() equality includes edited_at
src/photo_object.rs        edited_at property + set_from_photo
src/window.rs              group_mode_key, apply_gallery_grouping(search_is_empty) + History branch
src/window/build.rs        Call sites pass search.is_empty() / true
src/window/layout.rs       Search activate/clear restore History grouping; destination/collage call sites
src/window/library.rs      Test Photo helper zeroes edited_at
src/window/toolbar.rs      History arm in group-mode match; sort/group pass search emptiness
DEVELOPMENT_HANDOVER.md    This report
```

Horizontal Home rows (previous session, still uncommitted): `src/db/library_home.rs`, `src/library_home.rs`, `src/library_home_tests.rs`.

## Design notes

- Buckets use `recently_edited.edited_at`, not EXIF or import date.
- `GroupMode::History` uses the sticky external heading like Day/Month (not Folder in-stream rows).
- "Earlier This Week" is a rolling 7-day window (age_days <= 7 after Today/Yesterday), matching the Phase 3 wording; older same-month dates fall through to "Earlier This Month".
- Month comparison uses `%Y-%m` string equality to avoid private chrono `year()`/`month()` without `Datelike`.
- No schema migration.

## Validation

- `cargo test --quiet`: **362 passed, 0 failed, 23 ignored** (385 tests). Full suite run once after implementation.
- `cargo check --quiet`: **passed**.
- `cargo test --quiet history`: **9 passed, 0 failed, 2 ignored** (adds label bucket and `edited_at` unit tests).
- `cargo test --quiet home_`: **5 passed, 0 failed, 1 ignored** (horizontal rows still pass).
- `git diff --check`: **passed** after normalizing added lines to LF (repo has mixed CRLF; git flags CR on added lines).
- Broad `cargo fmt` was **not** run; it reformatted unrelated files and was reverted. Only intentional files remain modified.
- Log: `/tmp/pic-phase3-full-tests.log`.

## Outstanding issues and next steps

1. Review the uncommitted Phase 3 + horizontal-rows diff and this report.
2. Manually verify History sticky headers, search enter/leave on History, and photo/editor/collage open from a History tile.
3. Batch-event navigation (TODO Phase 3 second bullet) is still deferred.
4. Do not commit or push without further user instruction.

Useful commands:

```bash
cd /home/peet/Downloads/picasa/picasa-iphoto-clone
git status --short
git diff --stat
cargo test --quiet history
cargo test --quiet home_
cargo check --quiet
cargo run --quiet
# Full suite after further substantive changes:
cargo test --quiet
```

---

# Archived handover — Library Home horizontal rows

Updated: 2026-09-23. Superseded by the Phase 3 report above; itself superseded the Phase 2 report below.

## Current status

Phase 2 remains implemented. Horizontal scrolling rows were added to the Library Home Page. Phase 3 (History date grouping) was completed afterwards (see top section).

- Branch: `rc5`.
- HEAD: `1622a42` (Phase 2 commit). No new commit was made.
- Workspace: `/home/peet/Downloads/picasa/picasa-iphoto-clone`.
- Uncommitted changes: `src/db/library_home.rs`, `src/library_home.rs`, `src/library_home_tests.rs`.
- `.flatpak-builder/` remains untracked and untouched.

## Task and completed features

Convert all four Home sections into horizontally scrollable thumbnail rows with more previews.

Completed:

1. Each section is a `GtkScrolledWindow` with horizontal `Automatic` / vertical `Never` policy, kinetic scrolling enabled, and a horizontal `GtkBox` track of cards (`home-section-row` / `home-section-track` CSS classes). The outer Home page remains a vertical `ScrolledWindow`.
2. Per-section query limit increased from 6 to 10 (`HOME_PREVIEW_LIMIT = 10`) for Recently Added, Recently Edited, Favourites and Albums. Existing bounded SQL and cache-only thumbnail loading are unchanged.
3. Preview frames are fixed 140×140 squares, independent of image/caption natural size.
4. Compact flat `pan-start-symbolic` / `pan-end-symbolic` buttons sit in each section heading. They appear only when the row overflows and become insensitive at the ends (`connect_changed` on the horizontal adjustment).
5. Touchpad horizontal scrolling uses native GTK kinetic/`Surface` handling. Shift+wheel is GTK’s built-in horizontal mapping on `ScrolledWindow`. Pure vertical wheel is not consumed by a horizontal-only row (`may_vscroll` is false, so GTK propagates to the outer vertical scroller).
6. Row `hadjustment` values are saved before a snapshot rebuild and reapplied shortly after (`restore_row_positions`), so returning to Home or refreshing favourite data keeps horizontal offsets.
7. View All / View History, photo/editor/collage open callbacks, album navigation, favourite refresh, placeholders and search/edit return paths are unchanged.

## Files changed in this session

```text
src/db/library_home.rs        HOME_PREVIEW_LIMIT 6→10; DB tests expect 10
src/library_home.rs           Horizontal rows, scroll buttons, square cards, position restore
src/library_home_tests.rs     Row/policy/button/limit/overflow/position-memory coverage
DEVELOPMENT_HANDOVER.md       This report
```

No other modules were modified. History and gallery behaviour outside Home are untouched.

## Design notes

- `HomeSection` owns the row scroller, track box and prev/next buttons.
- Worker, `PRAGMA data_version` polling, read-only connection, thumbnail cache pruning and `load_cached_display_thumbnail` are unchanged aside from the higher LIMIT.
- Scroll-button visibility: `scrollable = (upper - page_size) > lower + 1`; sensitive within 0.5 px of the ends.
- Position restore: immediate `set_value` plus retries at 16 ms and 50 ms after rebuild so measure/allocate can update `upper` first.

## Validation

- `cargo test --quiet`: **360 passed, 0 failed, 22 ignored** (382 tests). Full suite run once after implementation.
- `cargo check --quiet`: **passed**.
- `cargo test --quiet home_`: **5 passed, 0 failed, 1 ignored**. Adds `home_preview_limit_is_ten`.
- `cargo test --quiet home_page_sections_navigation_and_favorite_refresh -- --ignored --test-threads=1`: **passed**. Covers row policies (Automatic/Never, kinetic), square 140×140 frames, scroll-button show/hide on overflow, position memory across a favourite-driven rebuild, section actions, card/album navigation and favourite clearing.
- `cargo test --quiet history_`: **7 passed, 0 failed, 1 ignored**.
- `rustfmt --check` on the three changed Rust files: **passed**. `git diff --check`: **passed**.
- `cargo fmt --all -- --check` still fails on pre-existing unrelated formatting (unchanged).
- Log: `/tmp/pic-home-scroll-full-tests.log`.

## Outstanding issues and next steps

1. Review the uncommitted diff and this report.
2. Manually check Shift+wheel and vertical wheel over a Home row in the live app; automated tests cover structure, overflow and position memory but not synthetic scroll events.
3. Confirm layout with the real theme and many cached network photos.
4. Horizontal-rows work remains uncommitted alongside Phase 3; do not commit or push without further user instruction.

Useful commands:

```bash
cd /home/peet/Downloads/picasa/picasa-iphoto-clone
git status --short
git diff --stat
cargo test --quiet home_
cargo test --quiet home_page_sections_navigation_and_favorite_refresh -- --ignored --test-threads=1
cargo check --quiet
cargo run --quiet
# Full suite after further substantive changes:
cargo test --quiet
```

---

# Archived handover — Phase 2 Library Home Page

Updated: 2026-09-23. This section is superseded by the horizontal-rows report above.

## Current status

Phase 2 is implemented and validated in the working tree. Phase 1 was already implemented and manually tested by the user before this session; its editing behavior was preserved. Phase 3 has not been started.

- Branch: `rc5`.
- HEAD at the start/end of Phase 2 implementation: `ef8eb2b`.
- Workspace: `/home/peet/Downloads/picasa/picasa-iphoto-clone`.
- No commit or push was performed in this session.
- The only untracked path present before Phase 2 was `.flatpak-builder/`; it remains untouched.
- The five-hour usage quota is not exposed by available tools, so its remaining percentage could not be monitored. This report is saved at the completion of the authorized phase.

## Task and completed features

Implement a clickable Library heading and a themed overview with four sections, in order: Recently Added, Recently Edited, Favourites, Albums. Keep previews bounded, use existing database records/cached thumbnails, preserve existing destinations, refresh favourite changes, and implement only Phase 2.

Completed:

1. Clicking the Library heading opens the new Home Page. Its separate collapse control still works. The Home destination is saved/restored as `library`.
2. Recently Added previews six non-trashed photos ordered by `added_at DESC, id DESC`; View All opens the existing Recently Added view.
3. Recently Edited previews six items using the existing History query with an optional SQL limit. View History opens History. Clicking an edited photo opens its editor; clicking a saved collage opens its editable project through existing callbacks.
4. Favourites previews six photos using the existing Favourites selection/order with a SQL limit. View All opens Favourites. Clicking a photo opens the existing viewer. Favourite changes refresh Home automatically without creating History events or duplicate records.
5. Albums previews six albums with covers, names and non-trashed photo counts. An explicit chosen cover is respected; otherwise a first photo is selected in existing album ordering. Clicking an album opens it; View All opens Albums. Empty albums and empty sections have placeholders.
6. All preview cards use theme-provided GTK/libadwaita styles, consistent dimensions, section headings and spacing.
7. Global search can start from Home, show the ordinary photo results and return to Home when cleared. Returning from photo/collage editing goes back to Home. A collage opened from Home can use Add Photos to enter the existing gallery picker.

## Files changed in Phase 2

New files:

```text
src/library_home.rs              Home widgets, background loading and cache reuse
src/library_home_tests.rs        Worker/cache and display-based Home tests
src/db/library_home.rs           Bounded preview queries and database tests
```

Modified files:

```text
DEVELOPMENT_HANDOVER.md
src/main.rs                     Registers the Home module
src/db.rs                       Includes Home query module
src/db/history.rs               Adds limited query entry point; existing full query remains unchanged in behavior
src/db/photos.rs                Adds limited query entry point; existing full/Favourites queries retain behavior
src/thumbnail_display.rs        Adds cache-only loading policy
src/sidebar.rs                  Library destination and clickable heading
src/window.rs                   Home destination persistence
src/window/build.rs             Home page, viewer placement and editor return paths
src/window/layout.rs            Home navigation, search and collage picker integration
src/window/library.rs           Avoids ordinary grid queries for the Home destination
```

All listed changes remain uncommitted. `.flatpak-builder/` is unrelated and must be preserved.

## Database and performance decisions

- No schema migration or new database table was needed for Phase 2.
- `db::library_home_data` returns at most six records per section; it does not construct the full photo library.
- `history_photos` and `photos` retain their original public behavior by calling their limited forms with SQLite `LIMIT -1`. Home calls the same implementations with `LIMIT 6`; no History writes or sorting rules were refactored.
- Album counts are aggregated for the selected preview albums. Cover queries fetch one candidate rather than materializing every photo in each album.
- Home owns a background worker with a persistent read-only SQLite connection. It does not call `db::open`, folder availability probes, scanners or schema migrations.
- While mapped, Home checks `PRAGMA data_version` approximately every two seconds. A changed database refreshes the small snapshot. Mapping Home forces a refresh; missing thumbnails are checked again so newly cached previews can appear.
- Decoded thumbnails are reused by their existing presentation key and pruned to the currently previewed items. Unchanged snapshots do not rebuild the widgets.
- `load_cached_display_thumbnail` reuses the normal thumbnail transforms but returns a placeholder on missing/corrupt cache entries. It never queues source decoding, deletes corrupt cache files or scans network locations. Existing gallery thumbnail loading retains its recovery behavior.
- Starting directly on Home skips loading the full gallery and initially constructing the full Albums page. The existing Albums destination builds normally when opened.
- The shared lightbox/context-menu overlay now wraps the page stack so Home previews can use the normal viewer. Switching into Albums or an editor closes the viewer. Existing gallery viewing continues to use the same lightbox.
- No original photo files were modified.

## Validation

- `cargo test --quiet`: **359 passed, 0 failed, 22 ignored** (381 tests). Full suite run once after implementation.
- `cargo check --quiet`: **passed**.
- `cargo test --quiet home_`: **4 passed, 0 failed, 1 ignored**. Tests cover limits/ordering, album covers/counts, favourite changes from another connection, and cache-only behavior.
- `cargo test --quiet home_cached_thumbnail`: **passed** after adding an additional cached-image rotation check with an unavailable original.
- `cargo test --quiet home_page_sections_navigation_and_favorite_refresh -- --ignored --test-threads=1`: **passed** on the available GTK display. Covers all four section actions, edited/favourite photo callbacks, album navigation, and live favourite removal.
- `cargo test --quiet history_`: **7 passed, 0 failed, 1 ignored**. Existing History regression tests still pass.
- New Rust modules pass `rustfmt --check`; `git diff --check` passes.
- `cargo fmt --all -- --check` still fails on existing formatting across unrelated files and pre-existing sections of shared files. Broad formatting changes were deliberately avoided.
- The existing warnings remain. No unrelated warning cleanup was performed.
- A standalone Home screenshot was captured and inspected; thumbnail dimensions were then made independent of image/caption natural size. The GTK test passed after that adjustment.

Logs:

```text
/tmp/pic-home-full-tests.log
/tmp/pic-home-final-check.log
/tmp/pic-home-tests.log
/tmp/pic-home-ui-tests.log
/tmp/pic-home-cache-test.log
/tmp/pic-home-history-regression.log
/tmp/pic-home-format.log
```

## Outstanding issues and next steps

No known failing Phase 2 test or build error remains. Automated GTK coverage exercises the Home component with callbacks; a full interactive application session covering sidebar/search/editor/viewer transitions is still advisable. Other ignored tests were not run. The user previously confirmed Phase 1 manually.

1. Review the uncommitted Phase 2 diff and this report; do not repeat Phase 1 investigation or implementation.
2. Manually check the complete application: click Library; open each View All/View History action; open a favourite in the viewer and toggle its favourite; search and clear search; reopen/edit a History photo/collage from Home; use Add Photos in a reopened collage; verify return to Home.
3. Check the Home layout with the user's actual theme and cached network photos. Missing cache entries intentionally show placeholders; Home never downloads originals to fill them.
4. Fix any concrete issue found with targeted validation. The full suite has already passed for the saved implementation.
5. Do not start Phase 3, commit or push without further user instruction.

Useful commands:

```bash
cd /home/peet/Downloads/picasa/picasa-iphoto-clone
git status --short
git diff --stat
cargo test --quiet home_
cargo test --quiet home_page_sections_navigation_and_favorite_refresh -- --ignored --test-threads=1
cargo check --quiet
cargo run --quiet
# Optional standalone Home screenshot from the display test:
PIC_HOME_SCREENSHOT=/tmp/pic-library-home.png cargo test --quiet home_page_sections_navigation_and_favorite_refresh -- --ignored --test-threads=1
# Re-run the full suite after substantive further changes:
cargo test --quiet
```

Normal app launch opens the user's actual library. Tests use temporary/in-memory databases. The display test requires a working GTK display.

---

# Archived handover — Phase 1 History

Date: 2026-09-23

## Current state

Phase 1 is implemented in the working tree and passes the automated checks described below. Phase 2 (Library Home Page) and Phase 3 have not been implemented. Development stopped after writing this handover. No commits, pushes, resets, or destructive Git operations were performed.

Branch: `rc5`  
HEAD: `ed601fc`  
Workspace: `/home/peet/Downloads/picasa/picasa-iphoto-clone`

The five-hour account usage quota is not exposed by the available tools. No claim is made that its 10% threshold was reached; this report preserves the completed phase for continuation.

## Original task and constraints

Implement Phase 1 of `TODO-HISTORY.md`: persistent editing History, recently edited photos/collages, grouped batch events, a History sidebar destination, existing cached thumbnails and offline handling, and reopening editable collage projects. Imports, browsing, selection, unchanged saves and ordinary photo exports must not create events.

User requested token-efficient work, targeted development tests, one full-suite run after implementation, notification sounds at milestones, and one phase at a time. Notifications were played. Do not start Phase 2 automatically. Do not commit or push without explicit instructions. Preserve all existing/unrelated work.

## Git status at handover

Modified tracked files:

```text
src/collage/editor.rs
src/collage/mod.rs
src/collage/model.rs
src/db.rs
src/db/photos.rs
src/db/tests.rs
src/grid.rs
src/grid/tile.rs
src/grid/virtualization.rs
src/photo_object.rs
src/sidebar.rs
src/window.rs
src/window/build.rs
src/window/library.rs
src/window/photo_actions.rs
```

New task files:

```text
DEVELOPMENT_HANDOVER.md
docs/superpowers/plans/2026-09-23-history.md
src/collage/persistence.rs
src/db/history.rs
src/grid/history_tests.rs
```

Other untracked paths:

- `TODO-HISTORY.md`: supplied by the user, already untracked at session start; left unchanged.
- `.flatpak-builder/`: appeared during the session, contains a `libnfs-6.0.2.tar.gz` download. Not created or changed deliberately by this task; leave it alone.

## Completed implementation

### Database and committed editing operations

`src/db/history.rs` owns History persistence and its gallery query. `src/db.rs` includes it and adds four additive tables to the existing idempotent schema:

| Table | Purpose |
| --- | --- |
| `editing_events(id, action, edited_at)` | One committed operation; timestamp is UTC epoch milliseconds. |
| `editing_event_items(event_id, photo_id)` | Links an operation to its affected items, with a composite primary key. |
| `recently_edited(photo_id, event_id, edited_at)` | One row per item, updated on each changed commit. |
| `collage_projects(photo_id, draft)` | Persistent editable project JSON associated with its exported library photo. |

Foreign keys link History rows to photos/events; indexes support recent ordering and photo/event lookup. Existing libraries receive these tables through their normal `db::open` path. No historical imports are backfilled as edits and existing photo data is not rewritten by this migration.

`commit_edit_recipes(connection, updates, action, event_id)` commits recipe changes and History in one transaction. It returns an optional event ID, created only after a changed, non-trashed photo is updated. Batch chunks reuse that ID. Missing photos, unchanged recipes, and trashed photos do not create events. A failed chunk rolls back its edits and History together.

`set_edit_recipe` now records `edit` or `reset`; the existing editor Done callback already uses it. Rotation commits also record changed edits. The old bulk `set_edit_recipes` wrapper was replaced by `commit_edit_recipes`; its callers/tests were updated. UI batch paste/reset keeps one ID across existing 64-photo chunks. Copy alone does not create an event.

### History view

`SidebarFilter::History` is added after Recently Added, including persistent sidebar destination encoding/decoding. The background library query loads `history_photos`, preserving newest-edit ordering instead of applying ordinary gallery sorting. Active search retains the app's existing library-wide search behavior.

The existing virtualized gallery and thumbnail/offline infrastructure are reused. History metadata is supplied through `Photo.history_caption` and `PhotoObject.history_caption`; tiles overlay the item type and local editing date/time. Ordinary views have no History caption. Reused objects and realized tiles update captions and changed presentation keys correctly.

History activation opens photos in the editor and collages through their saved project. Returning to History from an editor reloads it; bulk operations also reload an active History view. History does not use the ordinary collection-switching shortcut behavior, which could otherwise navigate into unrelated folders.

### Collage persistence and safe export

The existing Create Collage/export action is the permanent save point. Reopened projects show Save Collage. The exported library photo supplies grid identity; the separate JSON record supplies editable layout and settings. Re-saving keeps the same photo/project identity. Exporting an unchanged project does not add another History event.

`CollageDraft.saved_photo_id` is optional with a serde default, preserving compatibility with older drafts. Canonical project snapshots omit a live identity (the field is `None`); explicit History reopening assigns the selected saved ID. `save_collage_project` also stores the resumable draft with the saved ID in the same transaction, so identity does not depend on a later GUI callback.

`src/collage/persistence.rs` stages rendered output beside the destination. It validates ownership before rendering and again before publication. An unrelated existing file/library item cannot be overwritten: choose a new filename. An existing output owned by this project is temporarily backed up and restored if database persistence fails. If restoration fails, the backup location is retained and reported. Successful exports populate the existing thumbnail cache on a worker thread.

The destination chooser is modal. A non-dismissable saving dialog prevents ordinary navigation and overlapping exports while the worker saves the image/project.

## Review and bugs fixed

One independent read-only review found three issues, all addressed:

1. Same-ID tile reuse skipped caption refresh and presentation invalidation. A real binding regression test reproduced the failure; the fix passes it.
2. Export overwrote a destination before the database rejected conflicting ownership. A filesystem regression test reproduced the damage; staged output, ownership checks and rollback now pass the test.
3. Asynchronous exports could duplicate project identity or persist a draft without it. Modal saving serializes user operations and resumable identity now commits transactionally. The identity persistence test failed before this fix and passes afterward.

## Validation results

- `cargo test --quiet`: **355 passed, 0 failed, 21 ignored** (376 total). Full suite run once after implementation and review fixes.
- `cargo test --quiet history_`: **7 passed, 0 failed, 1 ignored**. Covers changed/no-op commits, repeated editing, ordering, persistence/reopen, rollback, a 100-photo batch, chunk reuse/failed chunks, collage identity, protected files and failed export rollback.
- `cargo test --quiet history_grid_reuses_items -- --ignored --test-threads=1`: **1 passed** using the available GTK display. Covers History/ordinary view reuse, real tile binding, caption visibility and visual invalidation.
- `cargo check --quiet`: **passed**. Existing warnings remain; unrelated warnings were not repaired.
- `git diff --check`: **passed**.
- `rustfmt --edition 2021 --check src/db/history.rs src/collage/persistence.rs src/grid/history_tests.rs`: **passed**.
- `cargo fmt --all -- --check`: **fails on pre-existing formatting** across unrelated code, plus existing formatting in sidebar/window files. New modules were formatted; broad unrelated reformatting was avoided. Several existing files use mixed CRLF/LF endings, so avoid normalizing the entire repository accidentally.

Logs from this session:

```text
/tmp/pic-history-full-tests.log
/tmp/pic-history-tests.log
/tmp/pic-history-ui-tests.log
/tmp/pic-history-final-check.log
/tmp/pic-history-final-fmt.log
```

Earlier regression logs may show intentional pre-fix failures; the logs listed above describe the final checks. A final formatting-only adjustment to a session-restore assertion did not change test behavior.

## Outstanding work and limitations

- No known failing feature test or unresolved review finding remains.
- The other ignored tests were not run. Automated GTK coverage exercises grid behavior, not a full interactive application session.
- Manual end-to-end testing with real photos, network/offline sources, and the native collage destination dialog remains advisable. The running application and the user's live library were not used to exercise the new migration or export workflow during this session.
- Image-file publication and SQLite commit cannot form one crash-atomic transaction. Ordinary failures restore the previous export; an abrupt process/system crash may leave a `.pic-collage-*` staging directory beside the output. Inspect and preserve any `previous` backup before cleanup.
- Date grouping, batch-event navigation, broader refresh/polish and the Library Home Page remain deferred under the original phased plan.
- Repository-wide formatting is not clean; this predates this feature.

## Next steps, in priority order

1. Read this handover, `git diff`, and the three new Rust files. Do not repeat the repository investigation or discard the current changes.
2. If continuing verification, manually launch the app and exercise History: save a photo twice, paste to over 64 photos, reset, restart, reopen a saved collage, modify/save it, and check missing/offline originals. Use disposable test images for export validation.
3. Fix any concrete issue found, with targeted validation. Re-run the full suite only when further code changes justify it; it has already passed for this implementation.
4. Review the completed Phase 1 result with the user. Do not commit or push without explicit authorization.
5. Only on instruction to begin the next phase, implement the Library Home Page from Phase 2 of `TODO-HISTORY.md`, reusing the new History query and existing cache/album infrastructure.

Useful continuation commands:

```bash
cd /home/peet/Downloads/picasa/picasa-iphoto-clone
git status --short
git diff --stat
git diff -- src/db.rs src/db/photos.rs src/window/library.rs
cargo test --quiet history_
cargo test --quiet history_grid_reuses_items -- --ignored --test-threads=1
cargo check --quiet
cargo run --quiet
# After substantive additional implementation:
cargo test --quiet
# Known baseline formatting failure:
cargo fmt --all -- --check
```

The GTK test needs a display (`DISPLAY=:0` / `WAYLAND_DISPLAY=wayland-0` were available during this session). Normal tests use temporary or in-memory libraries; launching the app normally opens the user's actual default library, so account for that when choosing a manual test setup.
