# PIC Performance — Phase 2 TODO (re-scoped)

## Context

Phase 1 complete (PHASE1_REPORT.md). Rank 2 fixed (dc199e8).
Phase 2 Step 1 instrumented the folder-scroll handler and **disproved Rank 3**.

## Step 1 result (done, scroll-baseline.log)

- Handler is cheap: `pick_calls=1` in 178/179 invocations, `pick_ms=0`,
  `scan_ms=0`, `header_ms=0`; `total_ms=0` in 173/179. Only `sidebar_ms` spikes
  (44 / 5 / 4 ms).
- Jank correlates with `thumbnail_hits` emitted from `load_visual`:
  fps=6 worst=1754 hits=1559 · fps=10 worst=236 hits=250 · fps=15 worst=114
  hits=381 · fps=23 worst=91 hits=1174. One `folder_virtual_bind_slow` = 13 ms.
- **Conclusion:** Rank 3 is disproven. The dominant cost is main-thread
  thumbnail/paintable loading and tile realization during scroll — Rank 4,
  now pulled into scope.

## Mission (re-scoped)

Cut main-thread thumbnail/paintable work during Folder-mode scroll.
Target: sustained fps ≥ 40 and `worst_frame_ms` ≤ 50 at 4515 photos, with the
first scroll through uncached tiles not stalling frames.

## Code in scope (verify line numbers before editing)

- `SquareTile::load_visual` (src/grid.rs:179) — fs probes + apply
- `SquareTile::unload_visual` (src/grid.rs:211)
- `SquareTile::refresh_thumbnail_with_probe` (src/grid.rs:285) —
  `picture.set_filename` at src/grid.rs:323 and :331 (synchronous texture load)
- `Gallery::refresh_folder_viewport_tiles` (src/grid.rs:1674)
- tile `connect_map` / `connect_unmap` (src/grid.rs:793 / :816)
- `tile_near_folder_viewport` (src/grid.rs:2983)

## Hard Rules (same as Phase 1)

1. No speculative optimization. Every edit cites a measured number.
2. Max 3 files changed. If more needed, write PHASE2_PROPOSAL.md instead.
3. Quote function + line numbers before editing.
4. No new dependencies.
5. No refactors, renames, or "while I'm here" changes.
6. Preserve all behavior.
7. GTK main thread: do not add std::fs, image::open, rusqlite, or imageops.
8. Commit per fix: `perf(scope): what and why (before → after)`.

## Step 2 — Instrument the thumbnail load path (no logic changes)

PICASA_TRACE-gated, existing style:

- In `load_visual` / `refresh_thumbnail_with_probe`:
  `UI PERF load_visual id= fs_ms= priority_ms= apply_ms= total_ms= cache=hit|miss`
  where `fs_ms` = `cached_file_available` + `Path::is_file`, `apply_ms` =
  `set_filename` / `set_paintable`.
- In `unload_visual`: `UI PERF unload_visual id= total_ms=`.
- In `refresh_folder_viewport_tiles`:
  `UI PERF viewport_tiles mapped= near= loaded= unloaded= ms=`.
- Per-second rollup (reuse the `scroll_handler_freq` 1 s tick):
  `UI PERF thumb_load_freq loads= reloads= max_ms= fs_ms= apply_ms=`
  where `reloads` counts the same photo id loaded more than once in the window.
- Keep the existing `PROFILE scroll` and `scroll_handler` lines.

`cargo check` must pass. Stop after Step 2.

## Step 3 — Baseline (human runs)

```bash
cargo build --release 2>&1 | tee build2.log
PICASA_TRACE=1 PICASA_PROFILE=1 cargo run --release 2>&1 | tee scroll-baseline2.log
```
Open Folder mode, scroll top→bottom→top ~30 s. Record in `PHASE2_PERF.md`:
loads/s, reloads/s, max load_ms, fs vs apply split, worst_frame_ms.

## Step 4 — Fix ONE, re-measure, revert if flat

Candidate order (stop at the first that moves the metric):
1. Cache decoded paintables in a bounded in-memory LRU keyed by cache path, so
   re-mapped tiles don't re-read/re-decode the JPEG via `set_filename`
   (src/grid.rs:323/:331).
2. Remove unload/reload churn: keep the loaded paintable for tiles inside the
   prefetch window; only clear far tiles (src/grid.rs:211 / :816).
3. If `fs_ms` dominates: stop re-probing `cached_file_available` + `is_file`
   on every bind (src/grid.rs:179).
4. If `apply_ms` dominates with `set_filename`: load the texture off the main
   thread or use the LRU from (1).

## Out of scope

- Rank 1 chunk size (`FOLDER_PHOTO_CHUNK_SIZE`), Rank 5 (`skip_known_bad`),
  lightbox, import/scan, DB schema, UI layout/CSS, cold start.
- No `cargo fmt` / clippy sweeps; no warning cleanup.

## Stop conditions

- If Step 2 shows `load_visual`/thumbnail apply is not the dominant frame cost,
  stop and re-rank before fixing.
- If the target is met after any single fix, stop and report.

## Reproduce

```
PICASA_TRACE=1 PICASA_PROFILE=1 cargo run --release
```
Import library, open Folder mode, click a photo, scroll.
