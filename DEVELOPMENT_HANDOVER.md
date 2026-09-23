# Development handover — Phase 1 History

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
