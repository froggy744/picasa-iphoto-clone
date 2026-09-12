# PIC Project Map

This document is a working map of the PIC / Picasa iPhoto Clone codebase so future changes can start quickly.

## What this project is

PIC is a native Linux photo manager written in Rust with GTK4/libadwaita. It indexes existing photo folders into a local SQLite library, generates thumbnails, shows a fast browsable grid, supports albums/favourites/search, opens photos in a full-window viewer, includes basic editing, and can export collages.

## Tech stack

- Language: Rust 2021
- UI: GTK4 (`gtk4`) and libadwaita (`libadwaita`)
- Database: SQLite via `rusqlite` with bundled SQLite
- Image handling: `image`, `turbojpeg`, `heif-oxide`, `rawler`, `kamadak-exif`, `fast_image_resize`
- Filesystem scanning/watching: `walkdir`, `notify`, GIO
- Cache/hash: `blake3`

Useful commands:

```bash
cargo check
cargo test
cargo run
cargo build --release
```

Current `cargo check` status: builds successfully, but has warnings for unused code/variables and deprecated GTK APIs.

## Runtime data locations

The app does not move the user's original photos. It stores metadata and generated cache files separately:

- Library database: `$XDG_DATA_HOME/picasa-rs/library.db` through `dirs::data_dir()`
- Thumbnail cache: `$XDG_CACHE_HOME/picasa-rs/thumbs` through `dirs::cache_dir()`
- Remote/raw materialized sources: inside the thumbnail cache under `source/`

## Top-level files and folders

| Path | Purpose |
| --- | --- |
| `Cargo.toml` | Rust package metadata, dependencies, build profiles. |
| `Cargo.lock` | Locked dependency versions. Commit this for the application. |
| `README.md` | Public-facing project overview, screenshots and feature list. |
| `REWRITE.md` | Rewrite/development notes. Read before larger refactors. |
| `ABOUT ME.md` | Author/project background text. |
| `docs/` | Design/development notes. Currently includes album bookshelf docs and superpowers plans/specs. |
| `icon/` | Application icon assets. |
| `images/` | Design/theme/image assets and templates. Some runtime theme paths are referenced by album view code. |
| `samples/` | Screenshot images used by the README. |
| `src/` | Main Rust source code. |
| `target/` | Cargo build output. Not source; do not edit manually. |

## Application startup flow

1. `src/main.rs` declares all modules and creates the libadwaita application.
2. On activation it opens the default SQLite database with `db::open_default()`.
3. If the database opens, it calls `window::build(application, connection)`.
4. `window::build` constructs the main window, sidebar, gallery, info bar, lightbox, menus, search, scan jobs and callbacks.
5. Folder import/refresh actions start scanner jobs, update the DB, create thumbnails, and refresh UI lists/grids.

## Source modules

### `src/main.rs`

Entry point. Initializes libadwaita, sets a panic hook, opens the library database, and presents the main application window.

### `src/window.rs` and `src/window/*`

The main application controller. `window.rs` defines shared types/settings and then includes the split implementation files.

| File | Purpose |
| --- | --- |
| `window.rs` | Shared window constants, sort/group settings, scan job state, `PhotoActionContext`, and includes. |
| `window/build.rs` | Builds the main UI: application window, headers, gallery, sidebar, toolbar/menu controls, import/refresh actions, theme CSS, scrolling/zoom wiring. This is the largest central file. |
| `window/library.rs` | Refreshes and filters the gallery; sorting, grouping, folder ordering, recently-added limiting, enabled-format filtering. |
| `window/search.rs` | Search entry behaviour, type-to-search, folder suggestion popup, search normalization. |
| `window/albums.rs` | Album actions from the UI: create, delete, add/remove selected photos, album menu refresh. |
| `window/photo_actions.rs` | Photo context menu/actions: favourite, rotate, edit, open with, reveal in file manager, wallpaper, rename/delete, properties. |
| `window/availability.rs` | Offline/reconnected source handling and thumbnail recovery after sources become available. |

### `src/db.rs` and `src/db/*`

SQLite persistence layer. `src/db.rs` defines the schema and shared structs (`Photo`, `Folder`, `Album`, counts, metadata), then includes implementation files.

| File | Purpose |
| --- | --- |
| `db/core.rs` | Database path/opening, schema creation, migrations and repair of folder/album/photo schema. |
| `db/photos.rs` | Folder insertion/removal, import roots, photo upsert/query/update, favourites, rotations, edit recipes, missing photo cleanup. |
| `db/albums.rs` | Album CRUD, album cover frame/photo, add/remove photos to/from albums. |
| `db/settings.rs` | Key/value settings, library/sidebar counts, cache/database stats, trash/clear helpers. |
| `db/tests.rs` | DB-related tests and row helpers. |

Schema tables:

- `photos`: path, folder, metadata, rotation, edit recipe, favourite/trashed flags
- `folders`: path tree, imported root flag, watched flag
- `albums`: named virtual albums and album cover settings
- `album_photos`: many-to-many album membership
- `settings`: app preferences and cached stats

### `src/grid.rs`

Photo grid/gallery widget and selection model. Handles thumbnail tile widgets, grouped folder streams, grid sizing/zooming, drag/multi-selection, offline badges, folder virtual rows, thumbnail cache interaction, keyboard/mouse selection and group headers.

Key public types:

- `Gallery`
- `GroupMode`
- `GroupDate`

### `src/sidebar.rs`

Left navigation sidebar. Builds Library, Albums and Folders sections, maintains selected filter/folder, counts, folder tree expansion, scrolling/pinning/autohide state, keyboard navigation, and folder row actions.

Key public types:

- `SidebarFilter`
- `FolderDisplayMode`

### `src/infobar.rs`

Bottom/right photo information/action bar. Shows selected photo preview, filename and metrics, and exposes action buttons/menus including albums/grid zoom.

### `src/lightbox.rs` and `src/lightbox/*`

Full-window photo viewer. Opens selected photos with cached preview first, then decodes higher quality image. Supports next/previous navigation, zoom, fit, 1:1, panning, rotation-aware display and a small display texture cache.

| File | Purpose |
| --- | --- |
| `lightbox.rs` | Shared viewer structs, decode semaphore/cache, `Lightbox`. |
| `lightbox/impl.rs` | Main `Lightbox` implementation. |
| `lightbox/render.rs` | Photo rendering/loading, decode targets, fit geometry, viewer cache, orientation handling. |
| `lightbox/tests.rs` | Viewer presentation tests. |

### `src/scanner.rs`

Indexes folders and extracts metadata. It walks roots, ignores unsupported/disabled formats, reads EXIF/date/camera/dimensions/size/mtime, writes DB entries, emits `ScanEvent`s for UI progress, and supports cancellation through `ScanControl`.

### `src/folder_watcher.rs`

Watches imported folders using `notify`. Sends changed folder paths back to the app so imports can be refreshed when files change.

### `src/source.rs`

Abstraction over photo locations. A stored reference can be a local path or a GIO URI. Provides availability caching, `gio::File` conversion, materializing remote files for RAW decoders, and reading bytes.

### `src/image_format.rs`

Supported image formats and decoder kind mapping. Also stores per-format enabled/disabled settings. Current families include JPEG, PNG, WebP, GIF, BMP, TIFF, HEIF/HEIC, AVIF, and multiple RAW formats.

### `src/thumbnail.rs` and `src/thumbnail/*`

Thumbnail generation/cache and priority queue.

| File | Purpose |
| --- | --- |
| `thumbnail.rs` | Shared constants, priority thumbnail request queue/workers, completions. |
| `thumbnail/cache.rs` | Cache directory, cache path hash, cache stats, create cached JPEG thumbnail. |
| `thumbnail/batch.rs` | Batch thumbnail creation and cache clearing. |
| `thumbnail/decoders.rs` | Thumbnail dimension/decode helpers, turbojpeg/image decoders. |
| `thumbnail/viewer.rs` | Viewer/full preview decode path, RAW/HEIF handling, EXIF orientation. |
| `thumbnail/nef.rs` | Nikon NEF embedded preview/thumbnail extraction helpers. |
| `thumbnail/recovery.rs` | Selects thumbnails for recovery/regeneration. |
| `thumbnail/thumbs.priot.txt` | Notes/data related to thumbnail priority work. |

### `src/photo_object.rs`

GTK/GObject wrapper around a `db::Photo`. Used as the model item for GTK list/grid selection and binding.

### `src/photo_texture.rs`

Helpers for edited/rotated thumbnail textures with a small in-memory cache.

### `src/albums_view.rs`

Albums home view and album card rendering. Handles album covers, bookshelf appearance, theme/frame asset discovery, album card context menus, sorting/size settings, and refresh/presentation updates.

Note: `src/albums_view.orig.rs` is an older/original copy kept alongside the active implementation. Treat `albums_view.rs` as the live file unless deliberately comparing/restoring old behaviour.

### `src/settings.rs`

Settings window/pages. Covers supported image formats, folders, albums, library/cache stats, recently-added limit, album view style/theme settings, and availability statistics.

### `src/edit/*`

In-app editor.

| File | Purpose |
| --- | --- |
| `edit/mod.rs` | Module exports. |
| `edit/model.rs` | `EditRecipe`, `CropRect`, `EditSession`; serialization/parsing and edit state. |
| `edit/render.rs` | Applies library rotation, tone controls, crop, auto contrast/color, rotate/autocrop and JPEG export. |
| `edit/editor.rs` | GTK editing UI, sliders/toggles/crop overlay, preview canvas, save/export flow. |

### `src/collage/*`

Collage creator.

| File | Purpose |
| --- | --- |
| `collage/mod.rs` | Public `open` entry point. |
| `collage/model.rs` | Collage project/item/photo data, layout kind, aspect ratio, orientation, background. |
| `collage/layout.rs` | Applies grid/mosaic layout geometry. |
| `collage/smart_mosaic.rs` | Candidate/refinement algorithm for smart mosaic layouts. |
| `collage/editor.rs` | GTK collage editor UI, preview canvas, controls, export path selection. |
| `collage/render.rs` | Exports the final collage image with crop/fit/rounding/rotation support. |

### `src/diagnostics.rs`

Optional performance/diagnostic logging helpers for refresh timing, memory and scroll/thumbnail stats.

## UI structure at a high level

- Main window: built in `window/build.rs`
- Left navigation: `sidebar.rs`
- Center photo content: `grid.rs` or album home from `albums_view.rs`
- Photo details/actions: `infobar.rs`
- Full viewer overlay/window: `lightbox/*`
- Edit mode: `edit/editor.rs`
- Collage creator: `collage/editor.rs`
- Settings: `settings.rs`

## Common change locations

| If you want to change... | Start here |
| --- | --- |
| Startup/app identity | `src/main.rs`, `Cargo.toml` |
| Main window layout/buttons/menus | `src/window/build.rs` |
| Sidebar filters/folder tree | `src/sidebar.rs` |
| Grid tiles, selection, grouping, thumbnail size | `src/grid.rs` |
| Photo sort/filter/search | `src/window/library.rs`, `src/window/search.rs` |
| Photo context menu/action | `src/window/photo_actions.rs` |
| Photo info bar | `src/infobar.rs` |
| Viewer zoom/navigation/decode | `src/lightbox.rs`, `src/lightbox/render.rs` |
| Import/index metadata | `src/scanner.rs`, `src/db/photos.rs` |
| Database schema/migrations | `src/db.rs`, `src/db/core.rs` |
| Supported formats | `src/image_format.rs`, decoder modules under `src/thumbnail/` |
| Thumbnail quality/cache/performance | `src/thumbnail/*`, `src/photo_texture.rs`, `src/grid.rs` |
| Albums/bookshelf appearance | `src/albums_view.rs`, `docs/ALBUMS_BOOKSHELF.md`, `images/` |
| Editing controls/effects/export | `src/edit/*` |
| Collage layouts/export | `src/collage/*` |
| Settings UI | `src/settings.rs` |
| Offline/missing source behaviour | `src/source.rs`, `src/window/availability.rs` |

## Development notes

- This codebase uses `include!(...)` in `db.rs` and `window.rs` to split large modules while keeping private shared types visible. When searching for functions, remember included files are part of the parent module.
- UI state is often stored on GTK widgets using object data keys. Search for `*_KEY` constants when tracking state.
- Many UI callbacks share `PhotoActionContext`; update it carefully if adding actions that need more shared state.
- Photo paths can be local paths or GIO URIs. Use `source.rs` helpers rather than assuming `std::path::Path` works everywhere.
- Thumbnails are content/cache-version/mtime/size based. If thumbnail generation logic changes, cache version constants in `thumbnail.rs` may need bumping.
- Edit recipes are persisted as text in `photos.edit_recipe`; changing their format requires compatibility in `edit/model.rs`.
- Albums are virtual: `album_photos` links to existing photos; removing from an album must not delete the original file.
- Folder import roots and discovered subfolders are both stored in `folders`; watch `imported_root`, `parent_id`, and `watched` when changing folder logic.

## Existing docs worth reading

- `README.md` — user-facing feature overview and screenshots.
- `REWRITE.md` — historical rewrite notes.
- `docs/ALBUMS_BOOKSHELF.md` — album bookshelf/card appearance notes.
- `docs/superpowers/specs/*` and `docs/superpowers/plans/*` — recent implementation specs/plans for responsive bookshelf/theme assets.
