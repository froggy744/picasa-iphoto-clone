# Lightbox / RAW Performance Plan — Phase 3

Status: planned after folder Open-in-Folder + smooth scrolling were accepted as
working and tagged `known-good-open-in-folder-v2`.

Scope: make the full-screen lightbox fast and predictable, especially for
NEF/RAW photos, without touching the RAW decoder first.

## Current state (before Phase 3)

- Folder view builds the 66k stream in ~0.9–1.0 s.
- Folder scrolling and thumbnails during scroll are acceptable.
- Lightbox already has:
  - a display-texture LRU (`DISPLAY_TEXTURE_CACHE_CAPACITY`, currently `8`),
  - a decode gate / generation counter that cancels stale decodes,
  - a preview-first path (`lightbox_preview_*`) before the full decode.
- Uncommitted experiment to raise `DISPLAY_TEXTURE_CACHE_CAPACITY` from `8` to
  `32` was reverted and saved for this phase.

## Evidence from logs

Representative lines (`thumbs1.log`, `auto-1.log`, `auto-2.log`, `auto-3.log`):

- `UI PERF lightbox_display_cache_miss path=... target=1124x794 request_ms=0`
  repeated on forward and backward NEF navigation.
- `VIEW PERF lightbox_worker_total_ms=302` / `465` / `191` / `152` for RAW.
- `UI TRACE lightbox_decode_cancelled stage=decode_gate wait_ms=15` / `400`
  when scrolling faster than decode.
- `UI PERF lightbox_preview_visible_ms=0 source=thumbnail` shows the preview
  path works and is cheap.

Conclusion: the cost is full-size decode + texture creation for RAW, plus cache
misses when the user scrolls faster than the LRU can be populated.

## Goal

Make forward/backward lightbox scrolling over NEF/RAW feel smooth:

- Recently viewed photos should be cache hits.
- The next photo should already be decoding when the user steps to it.
- No multi-hundred-ms stall when stepping through a burst.

## Phase 3.1 — Measurement first  ✅ done

- [x] Add distinct lightbox counters: preview hit, display cache hit, display
      cache miss, decode queued, decode completed, decode cancelled.
- [x] Log cache capacity, current size, and evictions.
- [x] Separate RAW vs non-RAW and edited vs unedited in the miss reason.
- [ ] Log directional prefetch hit rate (once 3.3 exists).

Acceptance: one lightbox session makes it obvious whether time is decode,
texture upload, cache eviction, or cancellation.

Result: `UI PERF lightbox_activity ...` summary every 2 s. First run showed
constant eviction at capacity 8, justifying Phase 3.2.

## Phase 3.2 — Tune and correctly size the display cache  ✅ done

- [x] Re-test `DISPLAY_TEXTURE_CACHE_CAPACITY = 32` with the new counters.
- [x] Decide key: include path, rotation, edit recipe, and target size, so
      zooming/rotation does not evict valid entries.
- [x] Consider a soft byte budget instead of a pure count, clamped so a single
      delete or zoom does not drop the whole cache.

Acceptance: stepping 10–20 photos forward then back stays cache-hit for the
recent window; memory stays bounded.

Result: capacity 32 + 256 MB byte budget. 20 NEF steps forward then back are
now all cache hits with 0 evictions; cache peaked at ~57 MB for that run.

## Phase 3.3 — Directional prefetch

- [ ] After displaying photo N, prefetch N+1 (and N+2) at low priority.
- [ ] When the user reverses direction, switch prefetch to N-1.
- [ ] Cancel prefetch when the lightbox closes or the user jumps far away.
- [ ] Prefetch must never exceed one in-flight decode beyond the current photo,
      and must not block the visible decode (keep the decode gate ordering).

Acceptance: stepping forward through a RAW burst rarely shows
`lightbox_display_cache_miss` for the photo the user is about to view.

## Phase 3.4 — Optional disk cache for final lightbox previews

Only if RAM + prefetch is not enough:

- [ ] Cache the final decoded/transformed full-screen preview on disk.
- [ ] Key must include: source path, source mtime/size, base cache version,
      rotation, edit recipe hash, target size, and decoder version.
- [ ] Invalidate on source, rotation, or edit change.

Acceptance: reopening the lightbox after app restart is fast for previously
viewed RAW photos.

## Phase 3.5 — Only then inspect the RAW decoder

Only if 3.1–3.4 prove the decoder itself is the bottleneck:

- [ ] Reuse embedded previews more aggressively instead of a full RAW demosaic.
- [ ] Avoid redundant orientation/resize passes.
- [ ] Consider a two-stage decode: embedded preview immediately, full decode
      only when idle/near.
- [ ] Bad-file markers so a broken NEF is not retried repeatedly.

## Do not do first

- Do not rewrite NEF parsing before cache/prefetch is measured.
- Do not add synchronous decode work to the GTK main thread.
- Do not remove the decode gate / generation cancellation.
- Do not change thumbnail cache versions while tuning the lightbox cache.

## Files likely involved

- `src/lightbox.rs` — cache capacity, LRU, decode gate.
- `src/lightbox/render.rs` — `show_cached_preview`, `show_photo`,
  `display_texture_cache_*`, worker decode and delivery.
- `src/thumbnail/nef.rs` / `src/thumbnail/viewer.rs` — only in Phase 3.5.

## References

- `docs/THUMBNAIL_RAW_PERFORMANCE_PLAN.md` — earlier thumbnail/RAW plan and the
  phases already completed for the grid.
