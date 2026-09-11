# Theme Asset Hierarchy Design

## Goal

Let album covers and bookshelf backgrounds be extended by dropping new files on
disk, and let every album choose its own cover frame independently, without
changing the existing bookshelf row geometry.

## Asset layout

```
images/theme/
├── album-covers/
│   ├── <theme>/<name>-frame.png
│   └── ...            (any depth)
└── bookshelf/
    └── <name>-bookshelf.png | <name>-bookshelf.jpg
```

- Cover frames are discovered by recursing every directory under
  `images/theme/album-covers` for files ending in `-frame.png`.
- Bookshelf backgrounds are the `*-bookshelf.png` / `*-bookshelf.jpg` files
  directly inside `images/theme/bookshelf`.
- Adding a file makes it available on the next refresh; no code change is
  required.
- The legacy flat assets (`images/*-album.png`, `images/bookshelf*.png`) are
  replaced by this hierarchy.

## Background ordering

Discovered backgrounds are sorted by path, except `single-row-bookshelf.png`,
which is pinned to index 0. Index 0 is the one-row asset that uses the
responsive 288-pixel row geometry; the remaining multi-shelf images keep their
legacy `background-size: 100% auto` treatment until matching one-row redesigns
exist.

`Next Background` cycles the stored index modulo the discovered count, so the
value stays valid when files are added or removed.

## Per-album cover frames

- `albums.cover_frame` stores the selected frame path relative to
  `images/theme/album-covers`, for example `vintage/blue-frame.png`.
- An album with no stored frame (or a stored frame whose file disappeared) falls
  back to a deterministic assignment hashed from the album id and name, so the
  choice is stable across refreshes.
- Right-clicking an album opens a menu whose `Next Album Cover` action steps
  only that album to the next theme folder and enables album covers if they were
  off. `Reset Album Cover` clears that stored frame so the album falls back to
  the automatic assignment; it stays disabled for albums that never picked a
  frame.
- Right-clicking empty grid space opens the page menu: Bookshelf and Next
  Background for the wooden shelf, Album Covers and Next Album Covers for the
  frame art, then Disable All Themes and Theme Settings.

## Themes

Every top-level folder under `images/theme/album-covers` is one theme. A theme
holds either a single frame file or several colour variants of one design. The
five shipped themes are `pink`, `standard`, `vintage`, `wedding` and
`white-panel`, with `vintage` holding five variants.

A page uses one theme at a time. The page-wide index chooses that theme, so
albums that never picked a frame of their own all show the same theme, whether
that theme carries one file or several. Two menus still cycle themes, never
single files:

- `Next Album Covers` mirrors `Next Background`: it cycles a stored
  `albums-cover-frame` index modulo the number of themes and turns album covers
  on if they were off. One action advances the whole page to the next theme, and
  a full turn of the themes returns the page to where it started.
- An album's own `Next Album Cover` steps from the theme it is showing to the
  alphabetically following theme, wrapping at the end, so the five themes cycle
  in five clicks instead of walking every vintage variant.
- Within a multi-file theme the album keeps a stable variant chosen by hashing
  the album id and name, so returning to a theme shows that album's usual colour;
  a single-file theme simply shows its one file for every album.
- Albums with a stored `cover_frame` keep their choice until their own menu
  changes or resets it. `Disable All Themes` leaves both stored indexes intact,
  so re-enabling restores the same appearance.
- `Settings → Themes → Reset All Theme Settings` clears every custom choice at
  once: the two switches, both stored indexes, the legacy style keys and every
  album's own cover. The page then reads as freshly installed, with the albums
  view back on its defaults.

## Per-frame photo openings

Frame artwork is not required to share one opening geometry. Each `-frame.png`
is measured from its own alpha channel: the transparent rectangle enclosed by
the frame's opaque pixels is the opening, stored as normalized ratios of that
image. The cover photo and its placeholder are inset to exactly that rectangle,
so a new design only has to ship artwork.

- Detection scans outward from the frame's centre, which skips the transparent
  margins around the book that also reach the image border.
- Frames without a readable opening, such as a JPEG or a fully opaque image,
  fall back to the ratios of the original supplied skins.
- Openings are cached per path and modification time, because decoding nine
  frames on every album refresh would be wasted work.
- The shipped frames use two different openings: the 805x500 / 827x500 "panel"
  skins and the 1359x1024 "book" skins (`standard`, `vintage/*`, `wedding`).

## Persistence

`migrate_album_schema` adds a nullable `cover_frame TEXT` column to `albums`
when it is missing, so existing libraries upgrade in place without losing
albums or memberships.

## Deferred

Bookshelf row and album fine-tuning for the multi-shelf backgrounds is out of
scope here and stays unchanged; only the index-0 single-row asset uses the
responsive geometry.
