# PIC Performance

Measured on the 66 008-photo / 620-folder library.

## Cold start
- `STARTUP cold_start_ms`: **178–354 ms** (typical ~283 ms)
- Startup RSS: **~105 MB** (model not yet expanded; only 50 photos displayed)

## Peak memory
- `/usr/bin/time -v` Maximum resident set size: **631 MB**
- Includes the 66k `PhotoObject` model, GTK's realized tile pool, and the
  lightbox's 8-entry display-texture cache + 1:1 native texture (both decoded
  images). Not yet broken down per component.

## Folder scroll (66 008 photos / 8976 rows)
- worst `worst_frame_ms`: **321 ms** (was 23410)
- `folder_virtual_bind_slow`: **0** (was 1325)
- initial folder stream build: **102 ms** (was 1012)
- Tree ↔ Imported Only: **186 ms** (was 2565)
- max `fs_ms`: **0** (was 4484)

## Other views (All Photos / Favorites / Albums / Recently Added / search)
- `refresh_grid` DB+sort: **25–175 ms**, off the GTK thread
- `grid_bind_slow`: 10×, max 26 ms
- All ↔ Folder same-set switch: **115–305 ms** (was 658–1363)
- worst `worst_frame_ms`: **318 ms**

## Notes
- The 6207 ms attached-splice figure in older notes predated the O(n)
  selection fix; attached splicing now reuses the realized pool.
- Thumbnail decoding is eager while scrolling and suppressed only during a
  folder-model swap (GTK binds its whole offscreen pool there).
- Remaining memory headroom, if needed: `DISPLAY_TEXTURE_CACHE_CAPACITY`
  (lightbox) is 8; the 66k `PhotoObject` strings include a precomputed
  `cached_thumbnail_path` per photo.
