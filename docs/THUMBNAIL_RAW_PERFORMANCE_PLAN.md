# Thumbnail / RAW Performance Plan

Status: created after folder smooth scrolling was accepted as working. The next work should target thumbnail/render latency, especially edited NEF/RAW thumbnails, without changing NEF/RAW decoder logic first.

## Current finding from `sscrollup3.log`

Folder smooth scrolling is not the main remaining bottleneck.

Evidence:

- Folder view thumbnail activity is mostly fast:
  - `folder_activity ... max_thumb_ms=0` or `1` for many scroll windows.
  - `fs_ms=0` and `apply_ms=0` in most folder scroll windows.
- Folder settled-viewport loader uses cached thumbnail files:
  - It calls `photo.cached_thumbnail_path()`.
  - It probes whether that cached JPEG exists.
  - If it exists, it sets `gtk::Picture::set_filename(Some(path))` using the cache file.
  - It stores the resulting paintable in the folder RAM cache for fast rebinding.
- The slow thumbnail lines are mostly not folder-scroll cache misses. They are edited RAW transform cases:
  - Example pattern: `cache=hit kind=raw_transform raw=true edited=true apply_ms=28-31`.
  - This means disk cache exists, but the app decodes the cached thumbnail and applies RAW crop/edit/rotation transforms on the GTK/main thread.
- Many later slow lines are full lightbox decode/cache misses, not folder grid thumbnail scrolling.

Conclusion: cached thumbnails are being used. The performance issue is mainly final transformed thumbnail reuse for RAW/edited photos, not missing base thumbnail cache files.

## Goal

Make thumbnail presentation cheaper and more consistent, especially when scrolling back over edited NEF/RAW photos.

Do this before touching NEF/RAW decoder internals.

## Phase 1 — Better measurements

Add more specific counters so logs distinguish these cases clearly:

- RAM paintable cache hit/miss.
- Disk thumbnail cache hit/miss.
- Base cached thumbnail displayed directly.
- Final transformed thumbnail generated.
- Final transformed thumbnail reused.
- RAW vs non-RAW.
- Edited vs unedited.
- Rotation applied vs not applied.
- Folder view vs normal grid view.
- Lightbox preview vs lightbox full decode.

Acceptance:

- One scroll session makes it obvious whether delay is disk I/O, transform CPU, GTK binding, or lightbox decode.

## Phase 2 — Cache final transformed thumbnails in RAM

Current risk-free improvement: cache the final `gtk::gdk::Paintable` for RAW/edited/rotated thumbnails using a key that includes:

- source photo path
- cached thumbnail path or mtime/size identity
- rotation
- edit recipe

This avoids re-opening the cached JPEG and re-applying edits every time the same edited RAW thumbnail is rebound.

Acceptance:

- Repeated visits over the same edited RAW thumbnails should show `final_transform_cache=hit`.
- `apply_ms` should drop on repeat/back-scroll.

## Phase 3 — Optional disk cache for final transformed thumbnails

If RAM caching is not enough, add a separate on-disk final thumbnail cache.

Key should include:

- original path
- original mtime
- original size
- base thumbnail cache version
- library rotation
- edit recipe hash
- output dimensions/cache version

This would make edited RAW thumbnails fast even after app restart.

Acceptance:

- Edited RAW thumbnails do not require repeated transform after restart unless source/edit/rotation changes.

## Phase 4 — Smarter prefetch around scroll direction

After scrolling settles, prefetch visible thumbnails plus nearby rows:

- ahead when scrolling down
- behind when scrolling up
- small batches per frame to avoid jank

Acceptance:

- Upward scrolling over recently passed photos should rarely need synchronous visible-row refresh work.

## Phase 5 — Only then inspect NEF/RAW decoder

Only touch `thumbnail/nef.rs`, `thumbnail/viewer.rs`, or RAW decoder code if phases 1–4 prove the decoder itself is still the bottleneck.

Possible later work:

- Reuse embedded previews more aggressively.
- Avoid unnecessary orientation/resize passes.
- Add better bad-file markers.
- Avoid full RAW decode where embedded preview is enough.

## Do not do first

- Do not rewrite NEF parsing first.
- Do not change thumbnail cache versions until the cache-key strategy is decided.
- Do not add synchronous thumbnail work to `GtkListView` bind callbacks.
- Do not remove folder scroll debounce/prefetch without replacing it with equivalent protection.

## Current completion state

Folder smooth scrolling can stay marked complete.

Next recommended code change: Phase 2 RAM cache for final transformed thumbnails, because logs already show `cache=hit` with expensive `raw_transform`/`apply_ms` on edited RAW files.
