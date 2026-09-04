# PIC - Picasa iPhoto Clone - a fast photo manager inspired by Picasa and iPhoto

So hwat is PIC. Picasa iPhone Clone is a lightweight desktop photo manager inspired by the simplicity of **Picasa** and **iPhoto**.

Built in **Rust** using **GTK4 / libadwaita**, with a focus on fast browsing, local photo libraries and a clean desktop interface.  Meant to be Linux only but decided to with Windows a go.

## Screenshot

<img src="samples/Picasa Iphoto Clone-PIC.jpg" alt="Picasa iPhoto Clone" width="700">

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

In the photo grid, use the **arrow keys** to move through photos and **Ctrl + mouse wheel** to quickly resize the thumbnail grid.

Use the **Up / Down arrow keys** at the end of a photo collection to move to the previous or next folder.

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

This is an independent open-source project inspired by classic desktop photo-management applications. It is not affiliated with or endorsed by Google or Apple. have not lost any photos while building or testing this app, but it is still under active development. To be safe, please test it first with copies of your photos or a temporary test folder. I do not want to be responsible for anyone losing their photos, so please make sure your important originals are backed up before testing.
