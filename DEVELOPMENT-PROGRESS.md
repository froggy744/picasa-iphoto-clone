# DEVELOPMENT-PROGRESS.md — PIC rc4 Autonomous Session

## SLEEP/SUSPEND STATUS
- Wed Sep 23 2026 06:04 SAST — user: "cancel the suspend".
- Overnight run never executed (session stopped after permission config; zero code work done overnight).
- No suspend ever occurred (machine up since Sep 22 15:11; suspend.target inactive).
- `~/.config/opencode/overnight-suspend.sh` replaced with a refusing stub; suspend step CANCELLED permanently.
- Task completed WITHOUT suspend. Machine stays on.

## Session start
- Date: Tue Sep 22 2026
- Branch: `rc4` (work ONLY here; never touch picasa-image-overlays)
- Starting commit: `f436c40` ("Fix swapped red and blue when rendering text colours")
- Pre-existing untracked: `SESSION_PROGRESS.md` (prior session notes — preserved and committed)
- Prior session state: colour-picker fix + lightbox paste-alignment fix already implemented and verified (332 tests pass, 120 warnings baseline).
- Done before code work: OpenCode 1.18.32 permission config in `~/.config/opencode/opencode.jsonc`.

## FINAL REPORT

### Root causes
1. **BUG 1 (multi-export exports one photo)**: `info.export.connect_clicked` in `src/window/build.rs` read only the `selected_photo` "current photo" slot (single PhotoObject), never `gallery.selected_photo_ids()`. Print already used the multi-selection; Export did not.
2. **BUG 2 (bulk paste freezes GTK / GNOME Force Close)**: paste/reset loops ran entirely inside one click handler on the GTK thread:
   - N individual `db::set_edit_recipe` autocommit statements (each its own SQLite transaction) — no batching despite `unchecked_transaction` patterns elsewhere;
   - `gallery.update_edit_recipe` did a linear scan of `current_photos` **plus a full widget-tree walk per id** → O(N × photos);
   - `lightbox.update_edit_recipe` scanned its whole photo vec per id;
   - error handling aborted the batch with a modal dialog.
   Result: main loop blocked for seconds → watchdog "Force Close / Wait".
3. **BUG 3 (no `_edit` suffix)**: infobar export built `{stem}.jpg` with no edit detection; editor export used `-edited.jpg`; no collision handling existed anywhere.

### Implementation
| Area | What changed |
|---|---|
| `src/edit/export_batch.rs` (new) | Pure export helpers: `recipe_is_edited` (uses `EditRecipe::is_default`), `export_file_name` (`_edit`, never doubled, extension case preserved), `unique_file_name`/`unique_export_path` (`_edit_2`, `_edit_3`…), `fit_max_edge` (aspect-preserving, no upscale), `ExportJob`, `run_batch_export` (skip missing, continue on failure, progress callback), `export_job_to`. 10 unit tests. |
| `src/window/operation_progress.rs` (new) | Reusable top-notification progress controller: title, detail (`done / total  pct  filename  (n failed)`), `gtk::ProgressBar`, 100 ms paint throttle, finish summary + auto-hide. One instance, updated in place. |
| `src/window/build.rs` | Progress card appended under refresh status; export button now collects **full multi-selection** → `show_photo_export_dialog` (size dropdown modelled on Collage export: Original/3840/2048/1920/1280) → folder chooser (multi) or save-file (single) → background thread + 80 ms progress poll; continue-on-error with exported/skipped/failed summary. |
| `src/edit/export.rs` | Editor export uses shared `export_file_name` + `recipe_is_edited` (replaces `-edited`). |
| `src/window/photo_actions.rs` | Paste Edits / Paste Text & Overlays / Reset Edits → `start_bulk_recipe_update`: 64-id chunks on 1 ms main-loop timeouts, one SQLite transaction per chunk, batched gallery/lightbox updates, single infobar/lightbox refresh at end, progress bar wired. Reset menu sensitivity uses one chunked SQL (`db::any_edited`) instead of N SELECTs. Failures counted, batch continues. |
| `src/grid/selection.rs` | New `update_edit_recipes_batch` (one photo-list pass + one tree walk per chunk); single-id API delegates to it. |
| `src/lightbox/impl.rs` | New `update_edit_recipes_batch`; single-id API delegates. |
| `src/db/photos.rs` | New `set_edit_recipes` (one `unchecked_transaction`), `any_edited` (chunked IN queries). |
| `src/db/tests.rs` | 2 new bulk-write tests. |
| `src/window.rs` | `PhotoActionContext.operation_progress` field; include `operation_progress.rs`. |
| `src/edit/mod.rs` | Register `export_batch` module. |

### Files changed
- src/edit/export_batch.rs (new)
- src/window/operation_progress.rs (new)
- src/edit/mod.rs, src/edit/export.rs
- src/window.rs, src/window/build.rs, src/window/photo_actions.rs
- src/grid/selection.rs, src/lightbox/impl.rs
- src/db/photos.rs, src/db/tests.rs
- DEVELOPMENT-PROGRESS.md, SESSION_PROGRESS.md (docs)

### Unchanged (verified)
- `src/edit/model.rs` still pure CRLF (1706 CRLF / 0 LF-only). Not rustfmt'd.
- Prior text-layer / colour-picker / paste-alignment work untouched (`text_ui.rs`, lightbox `update_edit_recipe` behaviour preserved via delegation).
- Collage export, image overlays, POSITION controls, undo/redo: no code paths modified.

### Commands run / Test results
- `cargo check`: **0 errors**, **130 warnings** (baseline 120; +10 = deprecated GTK Dialog/FileChooserNative APIs in the new export dialog — same APIs Collage export already uses; no new lint classes).
- `cargo test --bin pic-rs`: **346 passed, 0 failed, 20 ignored** (baseline 332 → +14 new tests: export naming/collisions/aspect/failure-isolation ×10, db bulk ×2, plus existing paste idempotency tests still green).
- `rustfmt`: not run wholesale on photo_actions.rs/build.rs (per repo rules). New files follow local style.

### Local commits (NOT pushed)
- `e179cbe` — Add rc4 overnight session recovery notes
- `d27c2e1` — Fix multi-select export, bulk paste freeze, and _edit export names
- Starting commit was `f436c40`; branch `rc4` is ahead of `origin/rc4` by 2. No push, no merge, no branch switch.

### Manual test status
- **NOT performed** (no interactive GTK session driven for this run). Automated coverage only. Suggested manual checks when a display is available: select 5 photos → Export → size+folder → 5 outputs with `_edit` where edited and progress bar; bulk-select thousands → Paste Edits → top progress updates, UI scrollable, no Force Close.

### Remaining limitations / notes
- Export size dialog uses deprecated `gtk::Dialog`/`FileChooserNative` deliberately for consistency with Collage export (adw::AlertDialog migration left as future work).
- Batch paste still runs DB writes on the main thread (chunked + transactional + yielding); for ~4000 photos chunks of 64 stay in the low-millisecond range per tick, which keeps GNOME's hang watchdog happy. A worker-thread writer would need a second SQLite connection.
- Single-photo Save dialog still allows native overwrite confirmation (existing convention); batch folder export never overwrites (unique `_2`, `_3`…).
- Suspend cancelled by user — no `systemctl suspend` was or will be run.
- OpenCode permission config requires an OpenCode restart to take effect (already recorded earlier).

## Recovery pointer for a future session
Read SESSION_PROGRESS.md for prior text-layer rules (never rustfmt model.rs / photo_actions.rs wholesale; 20 ignored GTK tests pre-existing). Latest commits: `d27c2e1`, `e179cbe`.
