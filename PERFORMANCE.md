# Phase 1 Baseline

## Cold start
- STARTUP cold_start_ms: X
- Photos: X, Folders: X

## Import
- Samples: X ms / X photos
- Large (1k): X ms / X photos

## Scroll (large)
- Worst folder_virtual_bind_slow: X ms
- Count >12ms: X

## Click a photo
- selection_position_for_id calls: X
- folder_selection_styles tiles/items/spfid_calls: X / X / X

## refresh_thumbnails
- Tiles walked: X, ms: X

## Peak RSS
- (use /usr/bin/time -v cargo run --release ... )
