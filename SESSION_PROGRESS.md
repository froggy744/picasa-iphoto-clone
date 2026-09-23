# Session Progress (rc4) — saved against PC shutdown

Date: Tue Sep 22 2026 (night)
Repo: /home/peet/Downloads/picasa/picasa-iphoto-clone, branch rc4
DO NOT touch /home/peet/Downloads/picasa/picasa-image-overlays
All rc4 changes UNCOMMITTED — do not commit/push unless asked.

## STATUS: BOTH BUGS FIXED + VERIFIED

### Verification (this session)
- cargo check: OK, **120 warnings** (baseline, ComboBoxText deprecations only)
- cargo test --bin pic-rs: **332 passed, 0 failed, 20 ignored** (+1 new regression test)
- rustfmt --check clean on src/edit/text_ui.rs; no fmt diff in new lightbox code
- model.rs line endings: pure CRLF (1706 CRLF, 0 LF-only)
- photo_actions.rs: functional diff only (18+/6-, includes prior text-layer work + lightbox sync)

### Bug 1 — colour picker GTK-CRITICAL: FIXED
src/edit/text_ui.rs:84
`gtk::ColorDialogButton::new(Some(gtk::ColorDialog::new()))`
(was `new(None)`; signature takes `Option<ColorDialog>` by value, not `&ColorDialog`).
Sync set_rgba + connect_rgba_notify already correct.

### Bug 2 — paste alignment needs close/reopen: FIXED (defensive root-cause fix)
Root cause (best supported): Lightbox holds its own `photos: Vec<PhotoObject>`
captured at `open()`. Paste handlers updated db + gallery.current_photos +
selected_photo, then called `lightbox.refresh_current()`, which re-decodes from
lightbox's own vec. If those objects were not the same instances as
current_photos (current_photos.replace detaches), refresh used the stale recipe
until close/reopen re-fetched objects.

Fix:
1. NEW `Lightbox::update_edit_recipe(id, recipe)` in src/lightbox/impl.rs
   — writes the recipe into matching objects in `self.photos`.
2. photo_actions.rs paste_edits / paste_overlays / reset_edits: call
   `lightbox.update_edit_recipe(*id, …)` for each target id inside the loop,
   before the existing `lightbox.refresh_current()`.
3. Regression test in model.rs (CRLF-safe python insert):
   `pasted_text_and_overlays_land_in_the_same_relative_place_on_a_differently_shaped_destination`
   — landscape→portrait paste keeps 3% margin, overlay pixel aspect, text
   relative box; destination exposure preserved; layers replaced not accumulated.

### Changed files this bugfix
- src/edit/text_ui.rs (colour button)
- src/lightbox/impl.rs (update_edit_recipe)
- src/window/photo_actions.rs (3 lightbox.update_edit_recipe call sites)
- src/edit/model.rs (new test; CRLF preserved)

### Rules still in force
- Never rustfmt model.rs (CRLF) or photo_actions.rs wholesale
- No new code comments (doc comments on new API are intentional API docs)
- No commit/push unless asked
- 20 #[ignore] GTK tests unrunnable (pre-existing)
- Play sound: paplay /usr/share/sounds/freedesktop/stereo/complete.oga

### Optional remaining / not done
- No manual GTK runtime verification of colour dialog or lightbox paste
  (cannot claim without running the app).
- If bug 2 still reproduces after this fix, next suspects: display_texture_cache
  race during in-flight decode, or tile/infobar not refreshed for non-selected
  multi-paste targets (gallery tiles already refresh via update_edit_recipe).

## Objective
Two-bug fix on the editable Text feature:
1. GTK colour picker critical: `gtk_color_dialog_choose_rgba: assertion 'GTK_IS_COLOR_DIALOG (self)' failed`
2. Cross-photo copy/paste alignment: must be correct immediately, no close/reopen of destination photo.

## Done
- Text-layers feature (12-section spec + Text tab): COMPLETE + previously reported.
- Diff-noise cleanup: model.rs CRLF restored (must NEVER rustfmt; use python3 byte convert); photo_actions.rs HEAD-restored + 4 functional edits re-applied (keep 9/6 diff, do not rustfmt wholesale).
- Verified state before this bugfix: 120 warnings (314→331 tests; +10 = ComboBoxText deprecations only), 331 passed / 0 failed / 20 ignored, rustfmt clean on task files.
- BUG 1 FIXED (not yet re-verified): src/edit/text_ui.rs:84 changed to
  `gtk::ColorDialogButton::new(Some(&gtk::ColorDialog::new()))`
  (was `new(None)` — gtk4 0.10.3 requires Option<ColorDialog>; API at registry
  gtk4-0.10.3/src/auto/color_dialog_button.rs:30). Sync set_rgba (231/237,
  syncing-guarded) and connect_rgba_notify (415) already correct.
- BUG 2: extensively investigated; root cause NOT conclusively confirmed.

## Bug 2 findings (verified)
Recipe math fully normalized (OverlaySpec x/y/width; TextLayerSpec size=h-fraction;
rect() preserves pixel aspect); canvas vs export use identical rect math with
post-crop dims; all caches recipe-aware (display_texture_cache, ViewerRequestKey,
prepare_navigation_photo, infobar, tile presentation key, 1:1 native check);
refresh_current (impl.rs:1102) passes cache_hit=false / fit_geometry_fixed=false;
editor rebuilt fresh every open (build.rs:2161) so first open == reopen; no paste
in editor; paste flow = db::set_edit_recipe → gallery.update_edit_recipe
(selection.rs:318, updates current_photos + tile.refresh_thumbnail) →
selected.set_edit_recipe if id matches → lightbox.refresh_current()
(photo_actions.rs ~299 paste_edits, ~349 paste_overlays). paste does NOT call
refresh_photo_actions_grid (reset does at 383). PhotoObject is a GObject
(photo_object.rs:78) so clones share instances; photo_objects() = clone of
current_photos (selection.rs:583).

Leading hypothesis (unconfirmed): a current_photos.replace site
(virtualization.rs:1336/1373/1448, grouping.rs:241) can detach lightbox's held
PhotoObjects from gallery's current objects → paste updates gallery objects,
refresh_current re-decodes lightbox's OLD objects with stale recipe →
close/reopen fixes it.

## Next steps (in order)
1. Implement bug-2 defensive fix — one of:
   a. paste handlers also set_edit_recipe on the context-menu clicked photo
      (the lightbox's own object when invoked from lightbox) before refresh_current;
   b. add Lightbox::update_edit_recipe(id, recipe) API;
   c. make refresh_current re-read recipe from db by path (belt-and-braces).
   Preserve 3% corner Position margins; wire-compatible; no new code comments.
2. cargo check + cargo test --bin pic-rs (expect ~120 warnings, 331+/0/20+).
3. rustfmt ONLY task files — never model.rs, never photo_actions.rs wholesale.
4. Regression tests where practical: paste across differing dims →
   rect_with_size / text_rect / OverlaySpec::rect destination-relative asserts.
5. Report root causes + changed files + test results.
6. Play notification sound when done or when input required, e.g.:
   `paplay /usr/share/sounds/freedesktop/stereo/complete.oga` (DISPLAY=:0)

## Rules
- No new code comments; may update existing inaccurate comments.
- Only dep: pangocairo = "0.21".
- Never claim manual GTK testing that wasn't done.
- 20 #[ignore] GTK tests unrunnable (pre-existing rt.rs:136/css_provider.rs:30).
- Temp artifacts: /tmp/opencode/warn-base.txt, warn-mine.txt.

## Key files
- src/edit/text_ui.rs (colour fix @84)
- src/window/photo_actions.rs (paste ~263-353, reset ~359+, menu 97-128/190)
- src/grid/selection.rs (update_edit_recipe 318, photo_objects 583,
  selected_photo_ids 355), src/window/albums.rs (1-8)
- src/lightbox/impl.rs (refresh_current 1102, open 818, ctx menu 274-290/717)
- src/lightbox/render.rs (show_photo 10, caches 341/363/620)
- src/photo_object.rs (GObject 78, edit_recipe RefCell 37)
- src/grid/virtualization.rs, grouping.rs (current_photos.replace sites)
- src/edit/model.rs (CRLF — paste_layers_only 709, test 1183)
- src/window/build.rs (ctx menu 471/777, photo_changed 145, edit_open_slot 2161)
