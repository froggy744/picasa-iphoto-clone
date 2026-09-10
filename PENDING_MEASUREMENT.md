# Pending measurement — verify Session 2 fixes (scroll-baseline5)

The agent cannot drive the GTK GUI, so this must be run once by a human with a
display. Library is ~66 008 photos.

## Command

```bash
cargo build --release 2>&1 | tee build5.log
PICASA_TRACE=1 PICASA_PROFILE=1 cargo run --release 2>&1 | tee scroll-baseline5.log
```

## Steps

1. Open **Folder** mode and let the 8976-row stream build.
2. Scroll top → bottom → top ~30 s.
3. While in Folder mode, toggle the folder display mode **Tree ↔ Imported Only**
   (this forces a full stream reorder and exercises the folder-store update).
4. Click two or three folders in the sidebar.
5. Close the app.

## What to check

### A. Per-row bind cost (the main freeze — commit 9bb3073)

```bash
grep -c "folder_virtual_bind_slow" scroll-baseline5.log
grep -oE "elapsed_ms=[0-9]+" scroll-baseline5.log | sort -t= -k2 -n | tail -3
```

Before: 1325 slow binds at ~29 ms each. Expected after: near zero, because the
O(66k) `selection_position_for_id` scan was removed from `connect_bind`.

### B. Worst frame gap (the "UI stuck")

```bash
grep -oE "worst_frame_ms=[0-9]+" scroll-baseline5.log | sort -t= -k2 -n | tail -8
```

Before: 23410 / 7347 / 3700 / 2849 ms. Expected after: well under 500 ms, ideally
under 100 ms.

### C. Folder-store update strategy (commits 9bb3073 / 3efd6ff)

```bash
grep -nE "folder_store_first_row|folder_store_update" scroll-baseline5.log
```

- `strategy=unchanged` or `incremental_splice prefix=.. suffix=..` with small
  `model_ms` → good.
- `strategy=virtual_chunks_detached` with a large `changed` span → expected for
  the display-mode toggle; check `model_ms` is ~800 ms, not ~6000 ms.

### D. Availability (commit 47106d8)

```bash
grep -oE "load_visual id=[0-9]+ fs_ms=[0-9]+" scroll-baseline5.log \
  | grep -oE "fs_ms=[0-9]+" | sort -t= -k2 -n | uniq -c | tail
```

Expected: no multi-second `fs_ms`; the offline "!" badge still appears for
disconnected drives.

### E. Over-realization (Phase 4, still open)

```bash
grep -oE "viewport_tiles mapped=[0-9]+" scroll-baseline5.log | sort -t= -k2 -n | uniq -c | tail
```

Before: peaks of 1524 mapped tiles. If still high after A–D, the folder
`ListView` item-height estimation is the next target.

## Result (human fills in)

- [ ] slow binds before/after: 1325 / ______
- [ ] worst_frame_ms before/after: 23410 / ______
- [ ] folder_store_update model_ms: ______
- [ ] max mapped tiles: ______
