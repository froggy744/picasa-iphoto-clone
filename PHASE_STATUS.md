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
`UI PERF folder_store_first_row ...` trace (below) prints it on the next run.

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

### Status: awaiting human GUI measurement

See **PENDING_MEASUREMENT.md**. Expected: `model_ms` ~0 when the stream is
unchanged; worst_frame_ms ≤ 50 / fps ≥ 40. The run will also print the first
differing row field, closing the root-cause question.

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

The main query runs on a worker thread and the measured `refresh_grid ms=25`
(4515 photos) shows the DB is not on the UI critical path. **No safe,
UI-relevant win; no edit made.** Revisit only if a fresh trace shows DB time
inside `refresh_grid`.

## Phase 4 — UI virtualization (blocked on measurement)

`folder_selection_styles tiles=1632` (scroll-baseline2.log) implies ~200
photo-chunk rows realized at once. The folder `ListView` uses variable-height
rows (header 58px; chunks via `folder_chunk_height`) and a per-row pool of up
to 8 tiles. Whether this is over-realization or normal recycling cannot be
decided without a fresh scroll trace. No edit made.

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
- `??` docs(phase2): pending GUI measurement
- `??` docs(phase2): this status

## Push

Remote `origin` is HTTPS (`https://github.com/froggy744/pic.git`) and the
sandbox has no credentials, so `git push` cannot authenticate. The commits are
local on branch `deepseek`; push with the user's credentials, or
`git remote set-url origin git@github.com:froggy744/pic.git`.
