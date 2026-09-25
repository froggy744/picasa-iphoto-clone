# PIC Library Prototype

Clean GTK4/libadwaita photo-library architecture experiment.

## Current prototype

- Picasa-style non-sticky folder groups
- One outer folder stream with a real `GtkGridView` per visible folder section
- Zoom changes tile geometry/column count only; it does not rebuild photo membership
- Background thumbnail decoding off the GTK main thread
- Persistent thumbnail cache under the user's XDG cache directory
- In-memory texture cache for instant revisits
- Library sidebar with Photos and Favourites
- Persistent favourites stored under the user's XDG config directory
- Folder list/tree with recursive counts and click-to-jump navigation
- Responsive grid columns on window resize
- Single-photo selection
- Double-click photo lightbox; Escape closes it
- Multiple command-line roots are supported
- No search yet by design

## Run

```bash
cargo run --release -- /path/to/photos
```

Multiple roots / shell wildcards also work:

```bash
cargo run --release -- /run/media/peet/Data-500GB/Work-FB/FB-2026-*
```

Or:

```bash
PIC_LIBRARY_DIR=/path/to/photos cargo run --release
```

## Architecture rule

The persistent photo models are not reshaped by zoom. Zoom and window resize only update presentation geometry and the realized grids. Thumbnail decoding and disk-cache work stay off the GTK main thread.
