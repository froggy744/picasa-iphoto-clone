# Collage — TODO / debug plan

Scope: `src/collage/` (editor.rs, layout.rs, model.rs, render.rs, smart_mosaic.rs).

## How to reproduce

```
PICASA_TRACE=1 PICASA_PROFILE=1 cargo run --release
```

Open a collage with several photos, change layout / orientation / spacing, drag
tiles, shuffle, export. Existing traces: `COLLAGE TRACE` (editor) and
`COLLAGE EXPORT` (`render.rs`, `trace_export`).

## Issue 3.2 — preview thumbnails are low quality

**Cause:** the editor preview draws each tile from the **320 px cached
thumbnail** (`thumbnail::THUMBNAIL_SIZE = 320`) via
`photo_texture::edited_thumbnail` / `picture.set_filename`
(`collage/editor.rs:518-534`). The preview canvas is the editor pane width
(~800 px), so a large tile (single photo, or a full-width mosaic cell) is
upscaled from 320 px and looks soft.

**Fix plan (chosen):** decode a preview source at the tile's on-screen size.
- Compute each tile's display pixel size from the canvas width and
  `item.width/height` (`update_geometry` already knows the fitted rect).
- If the target is <= the cached thumbnail size, keep the current path.
- Otherwise decode the original with `thumbnail::decode_for_viewer(path, w, h)`
  on a worker thread (never on the GTK thread), apply the recipe/rotation, and
  set the resulting texture. Key a small LRU by `(path, rotation, recipe, w, h)`
  and cancel stale decodes with a generation counter.
- Keep the cached thumbnail as the instant placeholder until the sharp decode
  lands.

**Alternative (not chosen):** render the whole collage offscreen at the canvas
size and show one texture. WYSIWYG but re-renders the entire collage on every
change.

## Issue 3.1 — B&W + Sepia cannot both be on

**Note:** the collage editor has **no** B&W/Sepia controls; those toggles live
in Edit Mode (`src/edit/editor.rs:278-283`, `connect_toggle` at :487-488). The
recipe and `edit::render::apply_tone` already apply both flags independently
(B&W first, then Sepia). Repro needed: is the report about Edit Mode toggles
turning each other off, or about the collage preview not reflecting a photo
that has both in its stored recipe?

- [ ] Reproduce and confirm where (Edit Mode vs collage preview).
- [ ] If Edit Mode: check whether the two `GtkToggleButton`s are being grouped
      (they are created ungrouped, so this needs a live repro).
- [ ] Add a `COLLAGE TRACE preview_recipe bw=.. sepia=..` line to confirm the
      preview receives both flags.

## Other collage areas to trace (backlog)

- Export time and peak memory for a large collage (3840 px canvas).
- `smart_mosaic` determinism and dead space.
- Drag/drop reordering hit-testing.
- Offline photo handling in preview vs export.
