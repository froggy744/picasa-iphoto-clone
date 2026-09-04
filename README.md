# Picasa iPhoto Clone

A lightweight desktop photo manager inspired by the simplicity of **Google Picasa** and classic **iPhoto**.

Built in **Rust** using **GTK4 / libadwaita**, with a focus on fast browsing, local photo libraries and a clean desktop interface.

## Features

* Import and browse photo folders
* Fast cached thumbnail grid
* Albums and favourites
* Search and photo sorting
* Full-screen photo viewer
* 1:1 viewing, zoom and rotation
* Photo metadata and camera information
* Export photos
* SQLite local photo library
* Detects unavailable/offline source folders
* GTK light and dark appearance support

## Keyboard Shortcuts

When viewing a photo:

| Shortcut  | Action                |
| --------- | --------------------- |
| `←` / `→` | Previous / next photo |
| `Space`   | Toggle 1:1 view       |
| `+` / `=` | Zoom in               |
| `-`       | Zoom out              |
| `0`       | Fit photo to window   |
| `1`       | View at 100%          |
| `Esc`     | Close photo viewer    |

In the photo grid, use the **arrow keys** to move through photos and **Ctrl + mouse wheel** to quickly resize the thumbnail grid. **up and arrow keys at the end of photo access next or previous folder**  

## Image Support

Supports:

**JPEG, PNG, WebP, GIF, BMP, TIFF, AVIF, HEIC/HEIF**

RAW formats include:

**NEF, NRW, CR2, CR3, ARW, DNG, RAF, ORF, RW2, PEF, SRW and RAW**

Optimized thumbnail caching and embedded RAW previews are used where possible for responsive browsing.

## Running

```bash
cargo run --release
```

Requires the Rust toolchain and GTK4/libadwaita development libraries.

## Project Status

This project is under active development. Features and UI behaviour may still change.

Contributions, testing and suggestions are welcome.

## Disclaimer

This is an independent open-source project inspired by classic desktop photo-management applications. It is not affiliated with or endorsed by Google or Apple.
