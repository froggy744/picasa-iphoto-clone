# Phase 1 Report

## Baseline (a1edf96, PICASA_TRACE=1 PICASA_PROFILE=1, 4515-photo library)
- Cold start: 16 ms
- Folder-mode load refresh_finished: 1284 ms
- folder_store_update model_ms: 1033 ms
- folder_selection_styles: tiles=1304 items=4515 spfid_calls=1182 ms=114
  (second invocation same change: tiles=1352 spfid_calls=972 ms=137)
- Folder-mode scroll: fps 2–7, worst_frame_ms 516–743
- THUMB PRIORITY: request_priority on GTK main thread; skip_known_bad re-probes disk every tick

## Ranking (from run.log)
1. rebuild_folder_rows_for / folder store change — model_ms 1033
2. refresh_folder_selection_styles + selection_position_for_id — 1182 spfid_calls per selection
3. Folder scroll handler (refresh_folder_viewport_tiles + photo_for_visible_folder_row)
4. load_visual → thumbnail::request_priority on main thread
5. thumbnail::request_priority skip_known_bad repeated disk probes

## Step 5 attempts

### Attempt 1 — Rank 1 (rejected)
Proposed: replace set_model(None) → splice → set_model(Some) with a live splice.
Result: no improvement. Detached: splice 0 ms + attach 1033 ms.
Attached: splice 1003 ms. The cost is the ListStore change itself with
a ListView observing it, not the swap strategy. Reverted.

### Attempt 2 — Rank 2 (accepted)
Change: compute selected ids once per refresh into a HashSet<i64>;
test tile membership against the set instead of scanning selection per tile.
Collapsed the duplicate invocation.
Result: spfid_calls 1182 → 0. ms 114 → 3. Same for the second invocation.
Committed as dc199e8.

## Deferred to Phase 2
- Rank 1 — larger chunk size changes layout; needs its own design, not a one-liner.
- Rank 3 — folder-scroll main-thread work; the actual "jerky" complaint. Deserves
  its own instrumentation pass (throttle value-changed to idle; resolve folder id
  from the picked row, not by scanning current_photos).
- Rank 4/5 — thumbnail request_priority on main thread; skip_known_bad disk probes.

## Reproduce
```
PICASA_TRACE=1 PICASA_PROFILE=1 cargo run --release
```
Import library, open Folder mode, click a photo, scroll.
