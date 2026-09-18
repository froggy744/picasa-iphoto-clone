# PIC / Picasa iPhoto Clone — Offline LLM Handoff (Summary)

**Project:** PIC — Picasa iPhoto Clone  
**Stack:** Rust + GTK4 + libadwaita  
**Status:** Release Candidate (post-v4 stabilization + theme system overhaul)  
**Date:** 2026-09-18  
**Primary Linux test environment:** Fedora  
**Purpose:** Give a local/offline LLM enough context to make small UI/behavior tweaks safely. Summary only — read the relevant source files for detail. User-facing docs live in `README.md` and the `README.*.md` guides.

---

## 1. Project Overview

PIC is a Linux-first, local-first photo manager inspired by classic Picasa and iPhoto.

Capabilities:

- Imported folder library (originals never touched), SQLite library, cached thumbnails
- Very large libraries (tested with ~70,000+ photos), virtualized grid, Day/Month grouping
- Albums, Favourites, Recently Added, global search with folder suggestions
- Lightbox viewer: wheel/keyboard navigation, 1:1, fit, incremental zoom, drag-to-pan
- Non-destructive editor: Tools (fixes + sliders + straighten), Filters (12 live presets), Crop (aspect ratios + straighten)
- Collage creator: Grid, Mosaic, Smart Mosaic; draft resume; high-res export
- Printing with page sizes, orientation and per-page layout
- Offline indicators for disconnected drives (folder-level detection)
- Runtime-discovered visual themes (folders, not hardcoded)
- Windows build is secondary / work in progress

---

## 2. Working Rules

**DO:** smallest possible change; fix shared logic; `cargo fmt --all -- --check`, `cargo check`, `cargo test`; test the exact UI behavior affected; keep changes isolated; preserve lightbox navigation, external CSS, non-destructive editing and original images.

**DO NOT:** rewrite large modules for small tweaks; mix unrelated cleanup into fixes; use one-photo lightbox opening (`lightbox.open(vec![photo], 0)`) for normal navigation; move CSS into Rust string constants; alter editor rendering algorithms, photo ordering or original photos without cause; churn GTK deprecated APIs in unrelated work; treat warnings or cancelled decodes as failures.

---

## 3. Structure Map

```text
src/
├── css/            base/linux/windows/macos css, square_corners.css,
│   └── theme_discovery.rs   runtime theme scanning
├── platform/       linux / windows / macos helpers
├── window/         build, layout, layout_settle, toolbar, navigation, dialogs,
│                   shortcuts, theme, photo_actions, availability, library,
│                   albums, search, print
├── albums/         view, card, bookshelf, actions
├── edit/           editor, controls, filters, crop, zoom, preview, undo,
│                   export, render, model
├── grid/           view, tile, grouping, navigation, selection, virtualization
├── collage/        editor, layout, model, smart_mosaic, render
├── thumbnail/ lightbox/ db/ platform/ ...
css/themes/<id>/theme.css    shipped + community themes (13 at present)
docs/THEMES.md, docs/theme-template/   theme docs and community template
```

Platform typography stays separate from themes (Segoe UI / GTK / Apple stacks). Theme selection, metadata and traffic-light window-control overrides are defined by each theme folder.

---

## 4. What Changed Since the v4 Summary (2026-09-16)

- **Theme system rewritten:** hardcoded theme list replaced by runtime discovery of `css/themes/<id>/` folders; new themes appear without restart. Docs: `docs/THEMES.md`; community template: `docs/theme-template/`.
- **Shipped themes:** iphone (base), standard, blue, green, teal, glass, retro, claude, archive ledger, tron, velvet-dusk, aqua-garageband-lights, aqua-garageband-v2. Traffic-light window controls are theme-overridable.
- **Header/Settings:** top-bar gear icon removed; Appearance/Themes picker lives in the header; thumbnail appearance toggles moved into the Themes tab; clear-database functions moved into Settings → Library.
- **Sidebar:** major rewrite — collapse-all, resizable album area, pin/auto-close button, corrected counts/totals, smooth scrolling fixes.
- **Info bar:** file name shown as heading with formatted "dd Mmm yyyy" taken date; stable column widths.
- **Grid/viewer:** natural (incremental) zoom steps, resolution-aware default thumbnail level, rubber-band drag selection in folders, delete-every-selected action in the context menu.
- **Editor:** now organized as Tools / Filters / Crop tabs; filter preset tiles render live previews of the actual photo; Apply/Reset Crop buttons.
- **Collage:** resume-draft prompts wired to all entry buttons; per-photo edit-and-return.
- **Tracing removed:** all runtime trace logging (PICASA_TRACE) was stripped. Debug via `cargo test` plus manual UI verification — do not reintroduce tracing casually.
- **Docs:** new user-facing README set (release candidate + navigation/folders/library/edit/crop guides) with updated `samples/` screenshots.

---

## 5. Verified Behavior (keep intact)

- Lightbox always opens with the full active collection (gallery/folder/album/favourites), never a one-photo viewer.
- Wheel + keyboard navigation, Space/1:1, fit, incremental zoom, drag-to-pan all work in every mode.
- Context menus open at the pointer, close on outside click/Escape, and restore focus to the lightbox.
- Grid virtualization, selection, grouping and scroll anchoring survive sidebar toggling, thumbnail zoom and fast scrolling.
- Editor previews are cached and non-destructive; export writes a new file; Reset restores the original look.
- Original images are never modified.

---

## 6. Non-Destructive Editor Rules

Edits are recipes stored in the library and applied at display/export time. Tools include: Auto Contrast, Auto Colour, B&W, Sepia; Exposure, Contrast, Fill Light, Highlights, Shadows, Saturation, Warmth, Sharpen; Straighten; Filter presets; Crop (free + fixed ratios, orientation); Undo/Redo/Reset; copy/paste edits; Export. Never overwrite the original image.

---

## 7. Known Non-Issues

- Editor produces repeated (fast, cached) preview renders during slider drags — do not optimize without measurement.
- Collage may log a transient `canvas=0x0` during initialization — normal.
- Fast scrolling cancels obsolete lightbox decodes — expected.
- RADV "not conformant" Vulkan warning on Fedora/AMD — graphics stack, not PIC.
- Build warnings (deprecated GTK APIs, unused items) exist — cleanup must be separate work.

---

## 8. Current Status

**Good:** release-candidate feature set compiles and runs on Fedora; large library, albums, folders, search, viewer, editor, crop, filters, collage, print, offline badges and theme system all working; automated test suite in place.

**Open:** Windows build/runtime, broader performance profiling, visual regression testing, GTK deprecation migration, warning cleanup.

---

## 9. Final Instruction

Before any change ask: does this preserve current verified behavior? If the change touches lightbox opening, photo collection state, focus, context menus, zoom, selection, gallery virtualization or editor rendering, test that interaction explicitly after `cargo test`.

**Preserve working behavior first. Optimize second.**
