# Albums Bookshelf + Cover Overlay — Implementation Guide

Status: **superseded**. The bookshelf and cover-frame work described here is
implemented; see `docs/superpowers/specs/2026-09-11-theme-asset-hierarchy-design.md`
for the shipped asset hierarchy and behavior. The notes below are kept for history only.

## Goal

Make the Albums home look like the iPhone Photos albums view:

- a **bookshelf background** image behind the whole album grid;
- each album rendered as a photo with a transparent **"album book cover" overlay PNG**
  on top, plus a **bottom scrim and overlaid title/count**.

## Current state (where things live)

- `src/albums_view.rs`
  - `build()` creates a heading plus a `GtkFlowBox` with CSS class
    `albums-home-grid` (`albums_view.rs:40-52`). The FlowBox is the album grid.
  - `album_card()` (`albums_view.rs:189-321`) returns a `GtkButton.album-card`
    containing a vertical `GtkBox`:
    1. cover = `GtkOverlay` with classes `album-cover photo-frame photo-tile`,
       wrapped in a `crate::grid::SquareTile` (fixed `cover_width` x
       `cover_height`) — `albums_view.rs:213-300`;
    2. album name label — `albums_view.rs:302-309`;
    3. photo-count label — `albums_view.rs:311-320`.
  - `cover_width()` / `cover_height()` derive the fixed cover rectangle from the
    grid thumbnail width.
  - `insert_child()` (`albums_view.rs:159`) inserts into the FlowBox and resets
    the auto `GtkFlowBoxChild` wrapper.
- CSS lives in two string providers:
  - light: `src/window.rs` — `.albums-home-grid > flowboxchild`,
    `button.album-card` (~lines 86-88).
  - dark: `src/window/build.rs` (~lines 3158-3160).
- The Albums page is added to `main_stack` as name `"albums"`
  (`build.rs`, `main_stack.add_named(&albums_home, Some("albums"))`).
- **No GResource system.** Assets live in `icon/` (only `pic-icon.png` today).
  Images are loaded from disk with `GtkPicture::set_filename`. CSS is loaded
  from string data via `CssProvider::load_from_data`.

## Decisions still needed

1. **Asset delivery**: embed with `include_bytes!` (self-contained binary,
   bigger) or load from disk at runtime (swappable art).
2. **Background technique**: full-bleed `GtkPicture` inside a `GtkOverlay`
   behind the scrolled grid (preferred: scales, no tiling) vs CSS
   `background-image` on `.albums-home-grid`.
3. **Asset files**: proposed `icon/albums-bookshelf.png` and
   `icon/albums-frame.png` (transparent), plus their dimensions.

## Implementation plan

### 1. Bookshelf background

- In `albums_view::build`, wrap the returned root in a `GtkOverlay`:
  - main child: `GtkPicture` with the bookshelf PNG,
    `ContentFit::Cover`, `set_can_shrink(false)`, `hexpand/vexpand(true)`;
  - overlay child: the existing heading + scrolled `FlowBox` content.
- Keep the page height/scroll behaviour unchanged; the background should not
  scroll with the grid (it is the page backdrop).
- If the user only wants the grid area, apply the same picture behind the
  `FlowBox` instead of the whole page.
- Fallback: if the asset is missing, leave the current solid background so the
  app still runs.

### 2. Per-album "book cover" overlay

- In `album_card`, after `cover.set_child(Some(&picture))`, add:
  - `frame = GtkPicture` from the transparent PNG;
  - `frame.set_content_fit(ContentFit::Fill)` (or `Contain` if the art must keep
    aspect and letterbox);
  - `frame.set_can_shrink(false)`, `hexpand/vexpand(true)`,
    `halign/valign(Fill)`;
  - `cover.add_overlay(&frame)`.
- `GtkOverlay` draws overlays in add order, so the frame must be added after the
  picture and before/after the scrim depending on whether it should sit above
  or below the text. Decide: frame above photo, scrim+title above frame.

### 3. Bottom scrim + overlaid title/count

- Remove the name and count from the vertical `Box` (`albums_view.rs:302-320`).
- Inside `cover` add a bottom-anchored `GtkBox` (class `album-scrim`) with
  `valign(End)`, containing the name and count labels:
  - name: xalign 0, ellipsize End, max width chars as today;
  - count: dim white.
- The card's `GtkBox` then contains only the `SquareTile`, so the cover is the
  whole card. Keep `cover_width`/`cover_height` so the FlowBox layout is stable.

### 4. CSS (both providers)

```css
.album-scrim {
    background: linear-gradient(to top, rgba(0,0,0,0.75), rgba(0,0,0,0.0));
    padding: 10px 12px;
}
.album-title { color: #ffffff; font-weight: 600; }
.album-count { color: rgba(255,255,255,0.8); }
```

Add any bookshelf rules to the same two providers (light `src/window.rs`, dark
`src/window/build.rs`). Keep existing `.album-card` reset rules.

### 5. Asset helper (only if embedding)

- Add a tiny helper that decodes embedded PNG bytes to a `gdk::Texture`
  (e.g. `gdk::Texture::from_bytes(&glib::Bytes::from_static(include_bytes!(...)))`)
  and use it for both the bookshelf and the frame. No new dependencies.

### 6. Verify

- `cargo build`, `cargo test folder_stream_tests` (sanity), `cargo fmt --check`
  (note: repo baseline is not fully rustfmt-clean; only format touched code).
- Open Albums: background fills the page; covers show the frame PNG at the right
  scale; title/count legible over the scrim; hover/pressed still work.
- Check widths from 1..6 FlowBox columns; check albums with no cover photo show
  the placeholder and still get the frame + scrim.
- Switch Library -> Albums -> Library to confirm no layout/scroll regressions.

## Files likely touched

- `src/albums_view.rs`
- `src/window.rs` and `src/window/build.rs` (CSS)
- `icon/albums-bookshelf.png`, `icon/albums-frame.png` (user-provided)
- maybe a small asset helper module if embedding

## Open questions for the user

- Exact PNG filenames and dimensions.
- Embed or load from disk?
- Does the bookshelf background cover the whole Albums page or only the grid?
- Should the frame sit above the photo and below the scrim, or on top of the
  title as well?
