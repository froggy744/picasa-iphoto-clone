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

### Local commits (NOT pushed — push blocked by OpenCode deny rule)
- `e179cbe` — Add rc4 overnight session recovery notes
- `d27c2e1` — Fix multi-select export, bulk paste freeze, and _edit export names
- `65449cc` — Record final rc4 export and bulk-paste fix report
- + pending commit: paste RefCell double-borrow fix + header-bar Export button
- Starting commit was `f436c40`; branch `rc4` is ahead of `origin/rc4`. No merge, no branch switch.

### Manual test status (2026-06-23 GUI run)
- App launched (4116 photos). Editor opened; **Add Text** + text set to `PASTE TEST` via AT-SPI worked.
- **Paste Edits** (full recipe): works.
- **Paste Text & Overlays Only** (`BulkRecipePlan::LayersOnly`): **was crashing** — root cause found and fixed (see below).
- Header-bar Export button present in AT-SPI (`Export selected photos`); not fully GUI-driven yet.
- Multi-select + context-menu copy/paste automation incomplete (Wayland: xdotool cannot move pointer; uinput partial; AT-SPI cache broke mid-run).
- Automated tests green after export/progress fix: **347 passed / 0 failed / 20 ignored**.

### LayersOnly crash root cause (fixed)
- Core dumps (PID 237981/237740) showed panic in GTK `clicked_trampoline` of `show_photo_context_menus` with payload:
  `called Result::unwrap() on an Err value: BoolError { message: "Failed to remove source" }` from `glib::source::SourceId::remove`.
- `OperationProgressUi::finish` schedules a 4 s auto-hide timeout and stores its `SourceId`. When that source fires (or is already gone), the next `begin()` → `cancel_hide()` called `source.remove()` on a dead id → glib unwraps → abort (cannot unwind across FFI).
- Sequence matching the user report: first paste (or any finish) auto-hides; second paste (Text & Overlays) hits `begin()` → panic.
- Fix: `catch_unwind` around `SourceId::remove` in `operation_progress.rs`; also scope the LayersOnly `connection.borrow()` so it cannot span nested re-entry.

### Export progress + Stop (fixed)
- Multi-export progress lives in the **same single row** as the library refresh notification bar (spinner · label · inline bar · Stop) — not a second card.
- Live `n / total` + `%` + filename in the label; always repaints when the file counter advances.
- `run_batch_export` takes `Option<&AtomicBool>` and stops before the next job when cancelled; outcome gains a `cancelled after n / total` error.
- Shared **Stop** on that bar: cancels export when a batch is running, otherwise cancels library refresh (priority: export → scan).
- Stop stays enabled while either a scan or an export is active.

### Remaining limitations / notes
- Export size dialog uses deprecated `gtk::Dialog`/`FileChooserNative` deliberately for consistency with Collage export (adw::AlertDialog migration left as future work).
- Batch paste still runs DB writes on the main thread (chunked + transactional + yielding); for ~4000 photos chunks of 64 stay in the low-millisecond range per tick, which keeps GNOME's hang watchdog happy. A worker-thread writer would need a second SQLite connection.
- Single-photo Save dialog still allows native overwrite confirmation (existing convention); batch folder export never overwrites (unique `_2`, `_3`…).
- Suspend cancelled by user — no `systemctl suspend` was or will be run.
- OpenCode permission config requires an OpenCode restart to take effect (already recorded earlier).
- **OpenCode deny: `git push` / `git * push`** — local commits only unless user removes that rule.

### Edit-mode Export unified + file type (this commit)
- Editor toolbar **Export** no longer has its own FileChooser/save path. It builds an `ExportJob` from the live session recipe + active rotation and calls the same `show_photo_export_dialog` → destination chooser → `start_photo_export_*` → shared Stop/progress bar pipeline as the info-bar Export (`build.rs` `export_for_editor` → `edit::build(..., start_export)` → `connect_export_action`).
- Export dialog gained a **File type** dropdown: JPEG (default) / PNG / WebP. Selection forces the output extension via `file_name_for_format` and is threaded through `choose_photo_export_destination` → `run_batch_export`/`export_job_to` → new `render::save_export` (JPEG quality 92; PNG/WebP via `image` crate).
- New `ExportFormat` enum in `export_batch`; unit test `file_name_for_format_swaps_extension_and_keeps_edit_suffix` (+1 → **348 passed / 0 failed / 20 ignored**).
- Editor Export tooltip no longer claims "JPEG only".

## Recovery pointer for a future session
Read SESSION_PROGRESS.md for prior text-layer rules (never rustfmt model.rs / photo_actions.rs wholesale; 20 ignored GTK tests pre-existing). Latest commits: `d27c2e1`, `e179cbe`, `65449cc`, `9a8b3cc`, `cc083e7` (LayersOnly crash + export stop/count), plus single-row refresh/export bar merge, plus this commit (edit-mode export shares info-bar path + file type dropdown). **LayersOnly crash root-caused to dead `SourceId::remove` in `operation_progress`.**
