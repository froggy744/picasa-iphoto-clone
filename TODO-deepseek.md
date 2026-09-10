# PIC Performance Rebuild — Phase 1 TODO

## Mission
PIC is too slow. Phase 1 = measure first, then fix ONE bottleneck, prove it with numbers.
Do NOT rewrite. Do NOT blank src/. Do NOT change UI.

## Hard Rules
1. No speculative optimization. Every edit cites a measured number.
2. Max 3 files changed. If more needed, write PHASE1_PROPOSAL.md instead.
3. Before editing: quote function name + line numbers.
4. No new dependencies unless measurement proves it.
5. No refactors, renames, or "while I'm here" changes.
6. Preserve all behavior: albums, favorites, edits, collage, offline, RAW/HEIC, watcher, search, grouping, sort, keyboard nav.
7. GTK main thread: no std::fs, image::open, rusqlite, or imageops.
8. Commit per fix: perf(scope): what and why (baseline → result).

## Step 1 — Recon (no edits)
Read: Cargo.toml, src/main.rs, src/window.rs + src/window/*.rs, src/grid.rs, src/db.rs, src/thumbnail.rs, src/photo_object.rs, src/scanner.rs, src/diagnostics.rs, src/lightbox.rs.
Write ARCHITECTURE.md answering:
- Scan → DB transaction shape?
- Thumbnail generation, cache, request_priority threading?
- Main-thread sync I/O, decode, or DB work?
- replace/append_photos/remove_photos model touch?
- PhotoObject GObject signals, live count?
- What runs on every vadjustment value-changed, tick_callback, selection-changed?

STOP after ARCHITECTURE.md. Do not edit source.

## Step 2 — Instrument (PICASA_TRACE-gated, follow existing pattern)
Log: refresh_grid ms+count, gallery.replace ms, append_photos ms, rebuild_folder_rows_for ms, folder_bind ms (exists), refresh_thumbnails walk ms, refresh_folder_selection_styles ms, selection_position_for_id call count, request_priority call site (main vs worker).
Add startup: STARTUP cold_start_ms=<N> photos=<N> folders=<N>.
No logic changes. Stop after Step 2.

## Step 3 — Baseline (human runs this, not LLM)
cargo build --release 2>&1 | tee build.log
PICASA_TRACE=1 cargo run --release 2>&1 | tee run.log
Record in PERFORMANCE.md: build time, cold start, import 1k, import 5k, scroll 5k worst bind ms, click selection_position_for_id calls, lightbox open/close ms, peak RSS.

## Step 4 — Rank top 3 bottlenecks
File::fn, line range, measurement, proposed fix. No edits.
Suspects to verify (do not assume):
A. grid.rs::selection_position_for_id — O(n) per bind/click/style
B. grid.rs::FolderRowObject::data() — clones Vec<PhotoObject> per call
C. grid.rs::folder_row_index_for_photo — O(rows) with clone per row
D. grid.rs::refresh_thumbnails / refresh_folder_selection_styles — full tree walk, O(n²)
E. grid.rs::raw_cached_thumbnail — sync image::open on GTK thread
F. window.rs tick callback — diagnostics::scroll_tick every frame
G. window.rs::refresh_grid callers — DB re-query + full rebuild on sort/group/folder change

## Step 5 — Fix #1 only
Smallest correct change. Re-run Step 3 measurement. Paste before/after.
If number didn't move, revert and report.

## Step 6 — Write PHASE1_REPORT.md
Baseline numbers, top 3 ranked, fix applied (file/fn/change/before/after/delta), deferred items, reproduce steps.

## Do Not
- Rewrite in iced/egui/slint
- Add tokio/rayon/dashmap/tracing
- Redesign UI, change DB schema
- Touch lightbox.rs, edit/, collage.rs, albums_view.rs unless measured
- cargo fmt / clippy --fix across repo

## Budget
Read each file once. Quote only edited functions. Diffs ≤40 lines. >3 files or >40 lines → PHASE1_PROPOSAL.md instead. Stop after Step 3 for human confirmation.

## First Action
git checkout -b perf/phase1
cargo build --release 2>&1 | tee build.log
Read files in order above. Write ARCHITECTURE.md. Do not edit source until reviewed.

## Phase Roadmap (do not start)
2: thumbnail pipeline. 3: DB/indexing. 4: UI virtualization. 5: edit pipeline. 6: platform tuning.