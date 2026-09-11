# Responsive Bookshelf Layout Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Keep albums fixed to repeating shelf surfaces during window resizing while preserving the no-theme layout.

**Architecture:** Model bookshelf row geometry as tested constants and pure calculations in `albums_view.rs`. Apply the normalized background to the FlowBox at a fixed vertical size and allocate every responsive FlowBox child one matching row slot. Preserve the two legacy multi-shelf backgrounds until matching one-row redesigns are supplied.

**Tech Stack:** Rust, GTK4, GTK CSS, SVG/librsvg, Cargo tests

**Spec:** `docs/superpowers/specs/2026-09-11-responsive-bookshelf-layout-design.md`

## Global Constraints

- The bookshelf row height is exactly 288 logical pixels.
- The shelf surface is exactly y=235 within every rendered row.
- Horizontal background scaling is allowed; vertical sizing must not depend on window width.
- Bookshelf-disabled presentation must retain its current geometry.
- Existing legacy raster backgrounds are not modified.

---

### Task 1: Fixed bookshelf geometry and background selection

**Files:**
- Modify: `src/albums_view.rs`
- Test: `src/albums_view.rs`

**Interfaces:**
- Produces: `BOOKSHELF_ROW_HEIGHT: i32`, `BOOKSHELF_SURFACE_Y: i32`
- Produces: `bookshelf_card_margin_top(cover_height: i32) -> i32`
- Produces: `bookshelf_background_rules(directory: &Path) -> String`
- Consumes: `AlbumAppearance::bookshelf_enabled` and the existing background index

- [x] **Step 1: Write failing pure geometry and CSS tests**

Add tests asserting that cover heights 91, 137, and 186 all satisfy `margin_top + cover_height == 235`, that row height remains 288, that index zero resolves to `bookshelf.png`, and that generated CSS gives index zero `background-size: 100% 288px` while retaining `100% auto` for the two legacy backgrounds.

- [x] **Step 2: Run tests and verify the expected failures**

Run: `cargo test albums_view::tests::responsive_bookshelf_geometry_keeps_cover_bottoms_on_the_surface -- --exact`

Run: `cargo test albums_view::tests::bookshelf_background_rules_use_fixed_row_height -- --exact`

Expected: compilation failure because the geometry helper and CSS rule helper do not exist yet, or assertion failure against the old background name and `100% auto` behavior.

- [x] **Step 3: Implement the minimal geometry and CSS helpers**

Define the two constants, calculate `235 - cover_height` with a non-negative clamp, change background index zero to `bookshelf.png`, and generate a direct FlowBox CSS rule with a fixed 288-pixel height.

- [x] **Step 4: Preserve legacy background behavior**

Keep `bookshelf2.png` and `bookshel3f.png` as background indexes one and two with their existing width-relative CSS. Restrict responsive row geometry to an enabled bookshelf at index zero.

- [x] **Step 5: Run focused tests**

Run: `cargo test albums_view::tests::responsive_bookshelf_geometry_keeps_cover_bottoms_on_the_surface -- --exact`

Run: `cargo test albums_view::tests::bookshelf_background_rules_use_fixed_row_height -- --exact`

Expected: both tests pass.

### Task 2: Apply row slots only in bookshelf mode

**Files:**
- Modify: `src/albums_view.rs`
- Test: `src/albums_view.rs`

**Interfaces:**
- Consumes: `BOOKSHELF_ROW_HEIGHT`, `BOOKSHELF_SURFACE_Y`, and `bookshelf_card_margin_top`
- Produces: themed FlowBox children with a 288-pixel height request and cover bottoms at y=235

- [x] **Step 1: Extend the display-backed regression test first**

In `album_view_modes_preserve_albums_and_click_navigation`, assert responsive bookshelf mode has row spacing zero, expands vertically, owns the responsive background class, every FlowBox child requests height 288, and the card top margin plus its cover height equals 235. After returning to default mode, assert row spacing is 20 and the child no longer requests themed height, margin, expansion, or background class.

- [x] **Step 2: Run the ignored GTK test and verify failure**

Run: `cargo test albums_view::tests::album_view_modes_preserve_albums_and_click_navigation -- --ignored --exact --test-threads=1`

Expected: failure because bookshelf children still use the natural card height, do not expand, and use 28-pixel row spacing.

- [x] **Step 3: Apply minimal themed row layout**

Read `AlbumAppearance` once in `populate`. In responsive bookshelf mode set FlowBox row spacing to zero, enable vertical expansion, set each FlowBoxChild height request to 288, set each card top margin to `bookshelf_card_margin_top(actual_cover_height)`, and darken count labels. In non-bookshelf modes explicitly reset expansion, card margin, and child height request while preserving current 20/28 spacing.

- [x] **Step 4: Run the display-backed test again**

Run: `cargo test albums_view::tests::album_view_modes_preserve_albums_and_click_navigation -- --ignored --exact --test-threads=1`

Expected: pass.

- [x] **Step 5: Run the complete verification set**

Run: `cargo test`

Run: `cargo test albums_view::tests::context_menu_renders_and_applies_the_next_background_action -- --ignored --exact --test-threads=1`

Run: `cargo check`

Run: `git diff --check`

Expected: all normal tests pass, both relevant display-backed tests pass individually, compilation succeeds, and no whitespace errors are reported.

### Task 3: Review and commit the responsive bookshelf change

**Files:**
- Add: `images/bookshelf.png`
- Add: `images/bookshelf-row-template.svg`
- Add: `images/bookshelf-row-template.md`
- Add: `docs/superpowers/specs/2026-09-11-responsive-bookshelf-layout-design.md`
- Add: `docs/superpowers/plans/2026-09-11-responsive-bookshelf-layout.md`
- Modify/Add: files from Tasks 1 and 2

**Interfaces:**
- Consumes: verified implementation and assets from Tasks 1 and 2
- Produces: one independently revertible Git commit after user-visible inspection

- [x] **Step 1: Review the complete diff against the specification**

Confirm the title/count remain outside the responsive background, index zero uses one-row geometry, legacy backgrounds remain unchanged, no-theme behavior is unchanged, and no unrelated files are included.

- [x] **Step 2: Stage the exact feature files**

Run: `git add src/albums_view.rs images/bookshelf.png images/bookshelf-row-template.svg images/bookshelf-row-template.md`

Run: `git add -f docs/superpowers/specs/2026-09-11-responsive-bookshelf-layout-design.md docs/superpowers/plans/2026-09-11-responsive-bookshelf-layout.md`

- [x] **Step 3: Commit after verification and visual approval**

Run: `git commit -m "fix: keep albums aligned with bookshelf rows"`

Do not push until the themed layout has been visually reviewed at narrow, medium, and wide window sizes.
