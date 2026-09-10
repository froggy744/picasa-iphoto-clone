# PIC Performance — Phase Status

Date: 2026-09-10 · branch `deepseek` · base `e026510`

## Phase 2 — Folder-scroll jank (Rank 1 ListStore rebuild)

### Root cause

The measured stall is `folder_store_update ... model_ms=1036..1467` during a
full `refresh_grid photos=4515` (scroll-baseline2.log:66343/70496). The old
short-circuit was all-or-nothing:

```rust
let unchanged = old_rows as usize == new_rows.len()
    && new_rows.iter().enumerate().all(|(i, new)| { ... });
```

`strategy=unchanged` was **never** observed to fire in any run
(`run4.log`, `scroll-baseline.log`, `scroll-baseline2.log`), even when
`old_rows == new_rows == 949`. So at least one row failed
`folder_virtual_row_matches`, and the code fell back to detach + full splice +
attach, which makes the live `ListView` rebind every visible tile
(thousands of `load_visual`/`unload_visual` lines between the plan and update
log lines).

The exact differing field could not be named from the existing logs. The new
`UI PERF folder_store_first_row index=N old_...= new_...=` trace prints the
first differing row and field on the next run.

### Fix (commit `3efd6ff`)

`src/grid.rs::rebuild_folder_rows_for` now computes the longest common
**prefix/suffix** of the old store and the new rows:

- identical stream → return without touching the model (`strategy=unchanged`);
- localized change → `folder_store.splice(prefix, removed, inserted)` in place
  so only the changed rows rebind (`strategy=incremental_splice`);
- full change (`prefix=suffix=0`) → keep the existing detach strategy.

The resulting model is byte-for-byte identical to the previous full splice, so
behaviour is preserved; only the amount of ListView work changes.

`cargo test --release`: 76 passed, 0 failed. Unit test
`folder_virtual_row_match_compares_identity_fields` covers the comparator.

### Status: fix not yet exercised

`scroll-baseline3.log` (library now 66 008 photos / 8976 rows) shows only the
**initial** build:

```
folder_store_update strategy=virtual_chunks_detached old_rows=0 new_rows=8976 prefix=0 suffix=0 model_ms=1229
```

`old_rows=0` is a from-scratch build, so the full-replace path is correct; the
incremental/unchanged path never ran because no second folder refresh was
triggered. To exercise it, toggle folder display mode **Tree ↔ Imported Only**
(or change sort) while in Folder mode and check for
`strategy=unchanged` / `strategy=incremental_splice prefix=.. suffix=..` and
`folder_store_first_row`. See **PENDING_MEASUREMENT.md**.

### 2b — Main-thread availability stat (fixed, commit `47106d8`)

`scroll-baseline3.log` exposed a second, larger stall:

```
load_visual id=41165 fs_ms=4484 ... cache=hit
folder_virtual_bind_slow row=1351 elapsed_ms=4506
PROFILE scroll fps=28 worst_frame_ms=7408
```

`grid.rs::load_visual` called `source::cached_file_available(&photo.path())` —
a synchronous `Path::is_file()` on the original — on the GTK thread. id 41165
is on `/mnt/4TBP/...`, a cold spinning disk, so the stat blocked for 4.5 s.

Fix: probe via `gio::File::query_info_async` (worker + main-context callback),
gated by a new `PhotoObject.original_checked` flag, so each original is stat'ed
once off-thread and the offline badge updates when it returns.
`SquareTile::refresh_availability` and `apply_availability` now read the shared
value instead of re-statting. 2 files, 76 tests pass.

## Phase 3 — DB / indexing (analysed, no edit)

Measured against the real 66 008-photo / 620-folder
`~/.local/share/picasa-rs/library.db` (read-only copy):

| query | baseline | + ORDER BY index |
| --- | --- | --- |
| `photos(None)` full library | 167 ms | 130 ms |
| `photos(folder=4)` recursive CTE | 5.6 ms | 18.5 ms |

Candidate index
`photos((taken_at IS NULL), taken_at DESC, path COLLATE NOCASE)` removes the
temp B-tree sort (`SCAN p` + `USE TEMP B-TREE FOR ORDER BY` → index scan) but
regresses the folder-filtered recursive-CTE path ~3×. The partial
`WHERE trashed = 0` variant behaves the same.

The main query runs on a worker thread and the measured `refresh_grid
ms=175..201` for the full 66 008-photo library (`scroll-baseline3.log`) shows
the DB is not on the UI critical path. **No safe, UI-relevant win; no edit
made.** Revisit only if a fresh trace shows DB time inside `refresh_grid`.

## Phase 4 — UI virtualization (measured, next target)

`scroll-baseline3.log`: 409 `folder_virtual_bind_slow` (≥12 ms each), binding
rows from **519 to 6979** — far outside the viewport — during long scroll
jumps, with `worst_frame_ms` 2220 / 1316. Each newly realized row builds up to
8 tile widgets synchronously. Most seconds are otherwise smooth (fps 60–120,
worst 16–25 ms). This is the next concrete jank target, but it needs a fresh
trace to separate over-realization from normal recycling before editing.

## Phase 5 — Edit pipeline (blocked on measurement)

Preview (`render_for_viewer`), export (`render_for_export` + `save_jpeg`), and
the wallpaper path all already run on `std::thread`. The only main-thread edit
work is `photo_texture::edited_thumbnail` / `grid::raw_cached_thumbnail`, both
on small cached thumbnails and behind caches. No measured hotspot; no edit.

## Blocker

The agent cannot drive the GTK GUI (no `DISPLAY`), so Phase 2 acceptance and
the Phase 4/5 traces need one human `cargo run` each (commands in
PENDING_MEASUREMENT.md). Phase 3 was analysed directly against the real DB.

## Commits this session

- `3efd6ff` perf(grid): incremental folder-store splice + mismatch trace
- `46ed7f0` docs(phase2): pending GUI measurement
- `0802ab5` docs(phase2): this status
- `c5e2466` perf(grid): trace first differing folder row, not just row 0
- `47106d8` perf(grid): probe original availability off the GTK thread

## Push

Remote `origin` is HTTPS (`https://github.com/froggy744/pic.git`) and the
sandbox has no credentials, so `git push` cannot authenticate. The commits are
local on branch `deepseek`; push with the user's credentials, or
`git remote set-url origin git@github.com:froggy744/pic.git`.
