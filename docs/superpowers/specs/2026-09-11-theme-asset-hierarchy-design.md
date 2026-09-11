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
- Right-clicking an album opens a menu whose `Next Album Cover` action advances
  only that album and enables album covers if they were off.
- Right-clicking empty grid space opens the page menu (Bookshelf, Next
  Background, Album Covers, Disable All Themes, Theme Settings).

## Persistence

`migrate_album_schema` adds a nullable `cover_frame TEXT` column to `albums`
when it is missing, so existing libraries upgrade in place without losing
albums or memberships.

## Deferred

Bookshelf row and album fine-tuning for the multi-shelf backgrounds is out of
scope here and stays unchanged; only the index-0 single-row asset uses the
responsive geometry.
