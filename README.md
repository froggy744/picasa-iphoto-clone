# PIC Library Prototype

A deliberately small Rust + GTK4/libadwaita prototype for testing a replacement photo-library architecture.

## Phase 1

- One Library view
- One persistent `gio::ListStore<PhotoObject>`
- One `gtk::GridView`
- Virtualized photo tiles
- Folder chooser / command-line folder
- Thumbnail-size zoom
- No folders sidebar
- No albums
- No search
- No editor
- No lightbox
- No grouping
- No sticky headers
- No nested ListView/GridView

## Critical rule

Zoom never rebuilds, splices, replaces, reorders, or recreates the photo model. It only changes realized tile geometry and requests a new layout.

## Run

```bash
cargo run --release -- /path/to/photos
```

or:

```bash
PIC_LIBRARY_DIR=/path/to/photos cargo run --release
```

Phase 2 will add ordinary non-sticky grouping headers only after this baseline is proven smooth.
