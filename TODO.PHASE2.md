# PIC Performance — Phase 2 TODO

## Context

Phase 1 (see PHASE1_REPORT.md, branch deepseek) is complete.
Baseline and ranking are on record. Rank 2 was fixed (spfid_calls 1182 → 0).

## Mission

Phase 2 attacks **Rank 3 only**: folder-scroll main-thread work, the actual
"jerky" symptom. Ranks 1, 4, 5 remain deferred.

## Known evidence from Phase 1

- Folder-mode scroll: PROFILE scroll fps=2..7, worst_frame_ms=516..743 (4515 photos).
- Handler is `folder_scroll.vadjustment().connect_value_changed` at src/window/build.rs:1150.
- It calls:
  - `gallery.update_group_header_for_scroll(scroll_y)` → `refresh_folder_viewport_tiles`
  - `gallery.folder_id_for_scroll_position(scroll_y)` → up to 6 `pick()` probes
  - optionally `sidebar::set_scroll_location`
- `photo_for_visible_folder_row` (grid.rs:1660) does up to 6 `pick()` probes
  plus `current_photos.iter().find(...)` over 4515 photos.

## Hard Rules (same as Phase 1)

1. No speculative optimization. Every edit cites a measured number.
2. Max 3 files changed. If more needed, write PHASE2_PROPOSAL.md instead.
3. Quote function + line numbers before editing.
4. No new dependencies.
5. No refactors, renames, or "while I'm here" changes.
6. Preserve all behavior.
7. GTK main thread: no std::fs, image::open, rusqlite, imageops.
8. Commit per fix: `perf(scope): what and why (before → after)`.

## Step 1 — Instrument the scroll handler (no logic changes)

Add PICASA_TRACE-gated timing inside the `value_changed` closure in
src/window/build.rs around each of these, and inside `folder_id_for_scroll_position`
and `photo_for_visible_folder_row` in src/grid.rs:

- total handler ms
- pick() call count and ms
- current_photos.iter().find() ms
- sidebar::set_scroll_location ms
- update_group_header_for_scroll ms

Log format must match existing style:
`UI PERF scroll_handler pick_calls=… pick_ms=… scan_ms=… sidebar_ms=… total_ms=… y=…`

Also: count handler invocations per second while scrolling. Add a 1s tick
that logs `UI PERF scroll_handler_freq calls=…`.

cargo check must pass. Stop after Step 1.

## Step 2 — Baseline the scroll handler (human runs)

```bash
cargo build --release 2>&1 | tee build2.log
PICASA_TRACE=1 PICASA_PROFILE=1 cargo run --release 2>&1 | tee scroll-baseline.log
