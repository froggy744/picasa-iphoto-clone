# Pending measurement — Phase 2 Rank 1 (folder-store rebuild)

The agent cannot drive the GTK GUI, so the following must be run once by a human
on a machine with the 4515-photo library and a display.

## Command

```bash
cargo build --release 2>&1 | tee build3.log
PICASA_TRACE=1 PICASA_PROFILE=1 cargo run --release 2>&1 | tee scroll-baseline3.log
```

## Steps

1. Open **Folder** mode.
2. Scroll top → bottom → top for ~30 s (this is the Phase 2 workload).
3. Click two or three folders in the sidebar to trigger a folder-stream
   refresh / scroll-to-folder.
4. Close the app.

## What to read from `scroll-baseline3.log`

```bash
grep -nE "folder_store_first_row|folder_store_update|folder_virtual_plan|PROFILE scroll" scroll-baseline3.log
```

### A. Did the short-circuit / incremental path fire?

Look for `folder_store_update strategy=...`:

- `strategy=unchanged` — best: the rebuilt stream matched and the model was
  left untouched (`model_ms` should be ~0).
- `strategy=incremental_splice prefix=<N> suffix=<M>` — good: only the changed
  span was spliced. Expect `model_ms` to fall roughly in proportion to
  `old_rows - prefix - suffix`.
- `strategy=virtual_chunks_detached` — still a full replace. Read
  `prefix=0 suffix=0`.

### B. Why did it rebuild? (`folder_store_first_row`)

When `prefix` is less than both row counts, this line names the first
differing row index and its fields:

```
UI PERF folder_store_first_row index=.. old_kind=.. new_kind=.. old_folder_id=.. new_folder_id=.. old_label=.. new_label=.. old_count=.. new_count=.. old_photos=.. new_photos=..
```

Whichever pair differs is the root cause of the old all-or-nothing check
failing. Record it here after the run:

- first differing index: __________
- differing field: __________
- expected if sort / folder-display-mode changed: `label` / `folder_id` / `photos`
- suspicious if only `folder_path` differs: real bug in row construction

### C. Frame target (Phase 2 mission)

`PROFILE scroll fps=.. worst_frame_ms=..` should reach **fps ≥ 40** and
**worst_frame_ms ≤ 50** at 4515 photos. Worst frames previously coincided with
`folder_store_update model_ms=1036..1467`.

### D. Main-thread availability stat (commit 47106d8)

`load_visual` no longer stats the original on the GTK thread. In the previous
run one cold external-drive stat blocked for `fs_ms=4484`
(`worst_frame_ms=7408`). Verify:

```bash
grep -oE "load_visual id=[0-9]+ fs_ms=[0-9]+" scroll-baseline3.log \
  | grep -oE "fs_ms=[0-9]+" | sort -t= -k2 -n | uniq -c | tail
```

Expected: no `fs_ms` above a few ms; the offline "!" badge still appears for
disconnected drives (now shortly after the tile is shown rather than during the
bind).

## Result (human fills in)

- [ ] model_ms before/after: __________
- [ ] worst_frame_ms before/after: __________
- [ ] first differing field: __________
- [ ] accepted / reverted: __________
