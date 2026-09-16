# Lightbox Stabilization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove the post-refactor lightbox regressions around context-menu focus, Space/1:1 opening, and right-click Open navigation without changing photo rendering or gallery behavior.

**Architecture:** Keep the existing Gallery as the owner of the active photo collection and Lightbox as the viewer. All entry paths (double-click, Space, 1:1, and context-menu Open) must open the selected photo inside the Gallery collection, and context-menu teardown must never leave an open Lightbox without keyboard focus.

**Tech Stack:** Rust, GTK4/libadwaita.

**Spec:** Runtime evidence in `split-run.v1.log` plus user-reported bugs 4 and 5.

## Global Constraints

- Preserve the v3 modular source split.
- No redesign or unrelated warning cleanup.
- Do not change decode/render algorithms in this stabilization pass.
- Keep originals/non-destructive editing behavior unchanged.

---

### Task 1: Restore lightbox focus after context-menu dismissal

**Files:**
- Modify: `src/window/build.rs`
- Modify: `src/window/shortcuts.rs`

- [x] Restore lightbox root focus on the next GTK idle after outside-click menu dismissal.
- [x] Restore lightbox root focus after Escape dismisses the menu.
- [ ] Verify trace no longer shows repeated `lightbox_scroll ... focus=false` after dismissal.

### Task 2: Make Space open the selected photo in every gallery mode

**Files:**
- Modify: `src/window/build.rs`

- [x] Resolve the selected id from the shared selected photo when it belongs to the active collection.
- [x] Fall back to Gallery selection ids when the shared selected photo is absent/stale.
- [x] Open the full Gallery photo collection at that index.
- [ ] Verify in Library, Album, Favorites, and Folder modes.

### Task 3: Make the 1:1 toolbar button open the lightbox

**Files:**
- Modify: `src/window/build.rs`

- [x] If 1:1 is enabled while the lightbox is closed, invoke the same selected-photo open path as Space.
- [x] Apply 1:1 on the next GTK idle after the lightbox is visible.
- [x] Return focus to the lightbox.
- [ ] Verify the button both opens at 1:1 and toggles back to Fit normally.

### Task 4: Preserve navigation when using right-click → Open

**Files:**
- Modify: `src/window/photo_actions.rs`

- [x] Dismiss the context menu before opening the viewer.
- [x] Resolve the clicked photo inside `gallery.photo_objects()`.
- [x] Open the full active collection instead of `vec![photo]`.
- [x] Keep one-photo fallback only when the photo is genuinely outside the active Gallery model.
- [ ] Verify mouse wheel and Left/Right navigation immediately after Open.

### Task 5: Keep context menu off the exact window corner

**Files:**
- Modify: `src/window/photo_actions.rs`

- [x] Preserve pointer-centered placement where space permits.
- [x] Clamp with a 4 px edge inset rather than `(0, 0)`.
- [ ] Verify top-left, top-right, bottom-left, bottom-right and center clicks.

### Task 6: Verification gate

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo check`
- [ ] `cargo test`
- [ ] `PICASA_TRACE=1 cargo run 2>&1 | tee split-run.v4.log`
- [ ] Confirm no panic/GTK criticals.
- [ ] Confirm context-menu Open logs `lightbox_open photos=>1` when the active collection contains multiple photos.
- [ ] Confirm scroll after menu dismissal logs `focus=true`.
