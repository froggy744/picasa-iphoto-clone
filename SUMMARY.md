# PIC / Picasa iPhoto Clone — Offline LLM Handoff

**Project:** PIC — Picasa iPhoto Clone  
**Stack:** Rust + GTK4 + libadwaita  
**Current stabilization baseline:** v4  
**Date:** 2026-09-16  
**Primary Linux test environment:** Fedora  
**Purpose of this file:** Give a local/offline LLM enough context to safely make small UI/behavior tweaks without undoing the recent refactor and lightbox fixes.

---

## 1. Project Overview

PIC is a Linux-first photo manager inspired by classic Picasa and iPhoto.

Main capabilities include:

- Imported folder library
- Cached thumbnails
- Large photo libraries
- Albums and Favorites
- Global search
- Fullscreen/lightbox photo viewer
- 1:1 view
- Incremental zoom
- Mouse-wheel navigation
- Drag-to-pan
- Rotation
- Metadata
- Export
- SQLite library
- Offline/missing-source indicators
- Non-destructive editing
- Collage editor
- Multiple visual themes
- Folder grouping / virtualized gallery
- Windows support under development

This project is written in Rust using GTK4/libadwaita.

---

## 2. Important Working Rules for Any LLM

### DO

- Preserve the existing v4 architecture.
- Make the smallest possible change.
- Prefer fixing shared logic rather than adding special-case code.
- Run `cargo fmt --all -- --check`.
- Run `cargo check`.
- Run `cargo test`.
- Test the exact UI behavior affected.
- Keep changes isolated.
- Commit only after tests and runtime behavior are good.
- Preserve current lightbox navigation behavior.
- Preserve external CSS files.
- Preserve non-destructive editing behavior.
- Preserve the original image file at all times.

### DO NOT

- Do not rewrite large working modules for a small UI tweak.
- Do not merge unrelated warning cleanup into feature/bug work.
- Do not replace working navigation logic with one-photo viewer logic.
- Do not create `lightbox.open(vec![photo], 0)` for normal navigation paths.
- Do not move CSS back into giant Rust string constants.
- Do not modify editor rendering algorithms unless the bug specifically requires it.
- Do not change photo ordering unless requested.
- Do not alter original photos.
- Do not “clean up” deprecated GTK APIs during unrelated work.
- Do not assume warnings are failures.
- Do not remove tracing until the related behavior is stable.

---

## 3. Current Refactor Structure

The large original source files were split into focused modules.

### CSS

```text
src/css/
├── mod.rs
├── base.css
├── linux.css
├── windows.css
├── macos.css
├── themes/
│   ├── standard.css
│   ├── iphone.css
│   └── aqua.css
└── components/
    ├── albums.css
    └── photo_context_menu.css
```

Platform typography is separate from themes.

- Windows: Segoe UI
- Linux: native GTK/Adwaita typography
- macOS: native Apple-style font stack

Aqua CSS exists but is not currently wired as a selectable theme.

---

### Platform

```text
src/platform/
├── mod.rs
├── linux.rs
├── windows.rs
└── macos.rs
```

Platform-specific file/folder reveal helpers belong here.

---

### Window

```text
src/window/
├── build.rs
├── layout.rs
├── toolbar.rs
├── navigation.rs
├── dialogs.rs
├── shortcuts.rs
├── theme.rs
├── photo_actions.rs
├── availability.rs
└── library.rs
```

`build.rs` remains the coordinator but no longer owns all implementation details.

---

### Albums

```text
src/albums/
├── view.rs
├── card.rs
├── bookshelf.rs
└── actions.rs
```

---

### Editor

```text
src/edit/
├── editor.rs
├── controls.rs
├── zoom.rs
├── crop.rs
├── preview.rs
├── undo.rs
├── export.rs
└── render.rs
```

The editor is non-destructive.

Edits are stored in the library/database and applied during display/export.  
The original image must remain untouched.

---

### Grid / Gallery

```text
src/grid/
├── view.rs
├── tile.rs
├── grouping.rs
├── navigation.rs
├── selection.rs
└── virtualization.rs
```

The gallery must remain scalable for very large photo libraries.

The tested library currently contains roughly **70,000+ photos**.

---

## 4. Current Verified Baseline

### Automated tests

The refactor test suite previously passed:

```text
220 passed
0 failed
14 ignored
```

The ignored tests are mostly display-dependent GTK tests and one optional RAW/DNG fixture test.

---

### v4 runtime build

The v4 runtime log compiled and launched successfully.

Observed startup:

```text
photos=70637
folders=623
albums=25
```

The application launched successfully and processed the library.

---

## 5. Most Important v4 Fixes

These fixes must not be regressed.

### Fix A — Folder-mode Space / 1:1 now opens proper lightbox

Previously, Space / 1:1 could fail to open the full photo correctly from folder mode.

v4 now opens the lightbox using the active gallery collection.

Verified runtime example:

```text
UI TRACE lightbox_open photos=70637 index=62805
```

Then 1:1 successfully loads the original-size image:

```text
output=4016x6016
```

and toggles back to fit view correctly.

### Required behavior

When a photo is selected in Gallery / Folder / Album / Favorites:

- Space should open or toggle 1:1 appropriately.
- 1:1 should open the selected photo if the lightbox is closed.
- The viewer should know the full active collection, not just one photo.

---

### Fix B — Right-click → Open no longer creates a one-photo viewer

Previously the Open action effectively used:

```rust
lightbox.open(vec![photo], 0)
```

This created a one-photo viewer.

The result was:

- Right-click photo
- Open
- Mouse wheel
- Nothing changes
- Viewer remains stuck on index 0

v4 now opens the clicked photo inside the full active gallery/photo collection.

Verified runtime example:

```text
UI TRACE lightbox_open photos=70637 index=62805
```

not:

```text
photos=1 index=0
```

unless the real active view genuinely contains one photo.

---

### Fix C — Mouse-wheel navigation after Right-click Open

This is now verified working.

Example:

```text
index=62805
index=62806
index=62807
index=62808
```

and scrolling backward correctly reverses the indexes.

Focus remains:

```text
focus=true
```

This is important because the old bug showed repeated wheel events with:

```text
focus=false
```

after context-menu dismissal.

---

### Fix D — Lightbox focus restoration

After dismissing the photo context menu, focus is restored to the lightbox.

Do not remove this behavior.

Context-menu dismissal must not leave the viewer unable to:

- mouse-wheel to next/previous photo
- use keyboard navigation
- use Space / 1:1
- zoom

---

### Fix E — Context menu edge positioning

The old menu could clamp to:

```text
menu=(0, 0)
```

and look as if it jumped into the top-left corner.

v4 now keeps a small edge inset.

Examples:

```text
menu=(34, 4)
menu=(44, 4)
menu=(289, 4)
```

This behavior is intentional.

Do not “simplify” the clamp back to zero-edge coordinates.

---

## 6. Lightbox Behavior That Is Currently Working

The following were verified in v4 runtime traces.

### Folder mode

Lightbox opens against a large collection:

```text
photos=70637
```

Mouse-wheel navigation works.

1:1 works.

Fit mode works.

---

### Album mode

Example:

```text
photos=36
index=0
```

Wheel navigation progresses:

```text
0 -> 1 -> 2
```

---

### Favorites

Example:

```text
photos=929
```

Wheel navigation progresses through the active Favorites collection.

---

### All Photos

Example:

```text
photos=70637
```

Navigation works over the full gallery.

---

## 7. Current Zoom / Pan Behavior

Verified:

- Incremental Ctrl+wheel zoom
- Multiple zoom levels
- Drag-to-pan while zoomed
- 1:1 mode
- Fit mode
- Returning from 1:1 to fit

Example zoom progression:

```text
1.12
1.2544
1.404928
1.57351936
1.7623416832
1.973822685184
2.210681407406
```

Drag-to-pan traces show horizontal and vertical adjustments moving as expected.

Do not replace incremental zoom with a jump directly to 1:1.

---

## 8. Performance Notes — Not Yet a Confirmed Bug

The editor still produces many repeated preview render traces.

Example pattern:

```text
EDIT PREVIEW base_cache=hit ... total_ms=2 target=900x700
EDIT PREVIEW base_cache=hit ... total_ms=2 target=900x700
EDIT PREVIEW base_cache=hit ... total_ms=2 target=900x700
```

This may represent legitimate slider/UI events.

Most cached 900x700 previews are very fast.

Do not optimize this blindly.

Future performance testing should first determine:

- Which UI action triggers repeated preview work?
- Does it cause visible lag?
- Are identical recipes being rendered repeatedly?
- Is there already debounce/coalescing?
- Is the preview generation actually blocking GTK?
- Are repeated jobs cancelled properly?

Only optimize after measurement.

---

## 9. Collage Performance Notes

Earlier traces sometimes showed:

```text
canvas=0x0
```

during initial layout.

The collage later receives a valid canvas size and continues normally.

This is not currently classified as a confirmed bug.

Do not rewrite collage layout simply because `0x0` appears during initialization.

---

## 10. Lightbox Decode / Cache Notes

The lightbox uses:

- thumbnail preview
- decoded full texture
- display cache
- prefetch
- cancellation for obsolete requests

Fast scrolling may produce:

```text
lightbox_decode_cancelled
```

This is expected when the user scrolls faster than decode completion.

Do not treat cancelled obsolete decode jobs as failures.

The v4 log contains successful navigation even when older requests are cancelled.

---

## 11. Known Compiler Warnings

Current builds still contain warnings.

Examples:

- GTK deprecated APIs
- unused imports
- unused variables
- dead code
- unused Aqua theme constant
- unused helper functions

One example:

```text
unused import: gtk::prelude::*
src/css/mod.rs
```

Another:

```text
unused variable: search
src/window/build.rs
```

These are not current blockers.

### Important

Do not combine warning cleanup with bug fixes unless the warning directly relates to the bug.

GTK deprecation migration should be done separately.

---

## 12. GTK / Vulkan Note

On Fedora with AMD/RADV, the log may show:

```text
WARNING: radv is not a conformant Vulkan implementation, testing use only.
```

This message comes from the graphics stack / RADV and is not by itself a PIC crash or compile failure.

Do not attempt to “fix” PIC source solely because this warning appears.

---

## 13. Important UI Requirements

### Right-click menu

Must:

- open at/near pointer
- work in grid
- work in lightbox
- work when maximized
- close on outside click
- close with Escape
- restore focus to lightbox
- remain compact / Nautilus-like
- not jump to top-left
- not break double-click
- not block normal mouse navigation

---

### Lightbox

Must:

- preserve active collection
- support next/previous
- support wheel navigation
- support keyboard navigation
- support Space for 1:1
- support incremental zoom
- support drag-to-pan
- support right-click
- restore focus after context menus
- use cached previews when possible

---

### Grid

Must:

- support very large libraries
- avoid loading all full-resolution images
- preserve virtualization
- preserve selection
- preserve grouping
- preserve scroll position where applicable

---

## 14. Non-Destructive Editor Rules

The editor must NEVER overwrite the original image.

Edits are recipes/state.

Phase 1 tools include:

- Exposure
- Contrast
- Saturation
- Crop
- Rotate
- Reset
- Copy edits
- Paste edits
- Undo
- Export

Export should write a new image.

Reset must restore the edit recipe and view state correctly.

---

## 15. Useful Runtime Trace Commands

Basic runtime trace:

```bash
PICASA_TRACE=1 cargo run 2>&1 | tee pic-run.log
```

Standard verification:

```bash
cargo fmt --all -- --check
cargo check
cargo test
PICASA_TRACE=1 cargo run 2>&1 | tee pic-run.log
```

Look for:

```text
PANIC
CRITICAL
ERROR
lightbox_open
lightbox_scroll
lightbox_context_menu
photo_context_menu_show
photo_context_menu_dismiss
lightbox_one_to_one
lightbox_ctrl_zoom
lightbox_pan_begin
lightbox_pan_update
EDIT PREVIEW
UI PERF
```

---

## 16. When Debugging Lightbox Problems

Always inspect these fields:

```text
photos=
index=
visible=
focus=
path=
```

### Healthy example

```text
lightbox_open photos=70637 index=31
lightbox_scroll dy=1 visible=true focus=true index=31
lightbox_scroll dy=1 visible=true focus=true index=32
lightbox_scroll dy=1 visible=true focus=true index=33
```

### Broken one-photo navigation example

```text
lightbox_open photos=1 index=0
lightbox_scroll ... index=0
lightbox_scroll ... index=0
lightbox_scroll ... index=0
```

### Broken-focus example

```text
lightbox_scroll ... focus=false index=0
```

If either broken pattern returns, investigate the path used to open the lightbox and focus restoration after popup/context-menu dismissal.

---

## 17. Current Safe Baseline

The current safe working baseline is:

**PIC full split cleanup v4**

The important sequence was:

- Original monolithic code
- Full modular split
- v2 compiler fixes
- v3 test fix
- v4 lightbox stabilization

Do not go back to pre-v4 lightbox opening logic.

---

## 18. Suggested Workflow for Offline LLM Tweaks

For a small request:

1. Read this file.
2. Inspect only the relevant source files.
3. Identify the current behavior.
4. Write or update a focused regression test if practical.
5. Make the smallest code change.
6. Run:

```bash
cargo fmt --all -- --check
cargo check
cargo test
```

7. Run PIC with tracing.
8. Test the exact interaction manually.
9. Compare traces before/after.
10. Commit only if behavior is correct.

---

## 19. Git Safety

Before changing anything:

```bash
git status
git branch --show-current
git log --oneline -5
```

For experimental tweaks, create a branch:

```bash
git switch -c tweak/<short-name>
```

After a successful fix:

```bash
git add .
git commit -m "Fix <short description>"
```

Do not mix unrelated changes into the same commit.

---

## 20. Guidance for Local Models

The user is not a professional programmer and relies heavily on AI-assisted development.

When explaining changes:

- give exact file names
- give exact commands
- explain what will change
- explain what must not change
- avoid vague instructions
- avoid unnecessary jargon
- do not propose broad rewrites unless required
- preserve working behavior first
- prefer a small patch over architectural churn

When debugging:

- find the first real failure
- do not chase warning cascades
- distinguish compiler errors from warnings
- distinguish runtime bugs from performance traces
- distinguish expected decode cancellation from failures
- use trace evidence before guessing

---

## 21. Current Status

### Verified good

- Modular refactor compiles
- Automated tests previously pass
- Fedora startup works
- Large library loads
- Albums load
- Folder virtualization works
- Lightbox opens with full active collection
- Folder-mode Space / 1:1 works
- Right-click Open preserves collection
- Wheel navigation after Open works
- Lightbox focus stays active
- Context-menu edge placement improved
- Incremental zoom works
- Drag-to-pan works
- Favorites navigation works
- Album navigation works
- All Photos navigation works

### Still to evaluate later

- Editor repeated preview performance
- Collage repeated layout/render traces
- Broader performance profiling
- Windows build/runtime
- Visual regression testing
- GTK deprecation cleanup
- General warning cleanup

---

## 22. Final Instruction to Any LLM

Before making a change, ask:

> Is this tweak small enough to preserve the current v4 working behavior?

If yes, make the narrowest possible patch.

If the change affects:

- lightbox opening
- photo collection state
- focus
- context menus
- zoom
- selection
- gallery virtualization
- editor rendering

then test the affected interaction explicitly after `cargo test`.

**Preserve v4 behavior first. Optimize second.**
