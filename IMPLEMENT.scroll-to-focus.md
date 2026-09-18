# Implementation Guide — Scroll-Wheel "Scroll to Focus" for 1:1 / Space

Reproduces the **verified working** behavior (see `scroll.focus.v11.log`) on a clean
tree. Scope: **the scroll-wheel function only** — no hover-select, no network shares.

## Final behavior

| Action | Result |
|---|---|
| Wheel-scroll with pointer resting on a thumbnail | once the eased scroll settles (~220 ms), the photo under the pointer **becomes the selection** |
| Space / toolbar **1:1** afterwards (any delay) | opens that photo (it *is* the selection) |
| Space / 1:1 within 2.5 s of scrolling, pointer on a tile | selects + opens it (fallback, covers very fast presses) |
| Hover / key focus without scrolling | **never** changes anything — the selection always decides |
| Ctrl+wheel | grid zoom only — never counts as scroll input |
| Keyboard nav, scrollbar drags, programmatic `scroll_to` | never count as scroll input |

## Why the earlier attempts failed (do not reintroduce these)

1. **Scroll controller on the GridView/ListView** — the smooth scrollers
   (`install_smooth_gallery_scroll` / `install_folder_smooth_gallery_scroll` in
   `src/window/navigation.rs`) attach **capture-phase** scroll controllers to the
   `ScrolledWindow`s and return `Propagation::Stop`. Capture runs before
   descendants; a `Stop` kills propagation. Any scroll controller on the views
   **never fires**. → Stamping must happen inside those two capture handlers.
2. **Storing pointer position in widget space** — a scrolled child's widget-space
   coordinates shift with the content. After wheeling N px with a stationary
   pointer, the stored coordinate points N px back into the library → the pick
   resolves the **first rows of the first section** ("1:1 opens the first photo").
   → Store **window-space** coordinates; convert to view space only at activation.
3. **Grace-window-only (900 ms)** — humans press Space 1–18 s after the wheel
   stops. A grace alone can never cover that. → The scroll itself must move the
   selection to the photo under the pointer ("scroll to focus"), debounced until
   the eased spring settles. The grace stays only as a fast-press fallback
   (raised to 2.5 s).

---

## Step 1 — `src/grid/view.rs`: pointer tracking + resolution API

### 1a. Above `pub struct Gallery {`, add

```rust
/// Where the pointer last was over one of the two grid views. Coordinates are
/// stored in WINDOW space, not in the view's widget space: a scrolled child's
/// widget-space coordinates shift every time the content scrolls under a
/// stationary pointer (wheel scrolling would leave them stale by the whole
/// scroll distance - landing the pick on the first rows of the library).
/// Window coordinates stay constant; they are mapped back into the view's
/// current coordinate space only at activation time.
#[derive(Clone, Copy, Debug)]
pub enum PointerSpot {
    Grid(f64, f64),
    Folder(f64, f64),
}

impl PointerSpot {
    fn new(folder: bool, x: f64, y: f64) -> Self {
        if folder {
            PointerSpot::Folder(x, y)
        } else {
            PointerSpot::Grid(x, y)
        }
    }
}

/// Pointer position from a view-space event coordinate to window space.
fn pointer_window_position(view: &gtk::Widget, x: f64, y: f64) -> Option<(f64, f64)> {
    let native = view.native()?.upcast::<gtk::Widget>();
    let point = view
        .compute_point(&native, &gtk::graphene::Point::new(x as f32, y as f32))?;
    Some((point.x() as f64, point.y() as f64))
}

/// The reverse mapping at activation time: window space into the view's
/// CURRENT widget space, applying whatever scroll translation is live now.
fn window_point_in_view(view: &gtk::Widget, x: f64, y: f64) -> Option<(f64, f64)> {
    let native = view.native()?.upcast::<gtk::Widget>();
    let point = native
        .compute_point(view, &gtk::graphene::Point::new(x as f32, y as f32))?;
    Some((point.x() as f64, point.y() as f64))
}

/// How long after the last real wheel/touchpad scroll a Space/1:1 activation
/// may still resolve to the photo under the pointer. Scrolling also
/// continuously re-selects the photo under the pointer (scroll to focus), so
/// this is only a fallback for very fast activations.
pub const SCROLL_HOVER_OPEN_GRACE: std::time::Duration =
    std::time::Duration::from_millis(2500);
```

### 1b. In `pub struct Gallery`, after the last field (`on_zoom_changed`), add

```rust
    // Scroll-then-open support: the pointer position over whichever grid view
    // is visible (GridView or Folder ListView) in WINDOW space, plus the time
    // of the last real wheel/touchpad scroll. Hover alone never influences
    // which photo opens; only a Space/1:1 activation shortly after a scroll
    // resolves to the hovered thumbnail (and then also selects it).
    pointer_spot: Rc<Cell<Option<PointerSpot>>>,
    last_wheel_scroll: Rc<Cell<Option<Instant>>>,
```

### 1c. In `Gallery::new`, before the `for (view, is_folder) in [...]` loop that
attaches controllers, add the tracking controllers (place the two `let` bindings
before the loop; `root` and `folder_root` both exist at that point):

```rust
        let pointer_spot: Rc<Cell<Option<PointerSpot>>> = Rc::new(Cell::new(None));
        let last_wheel_scroll: Rc<Cell<Option<Instant>>> = Rc::new(Cell::new(None));
```

Inside the existing loop body (keep whatever else it does), for each of
`root` (GridView, `is_folder = false`) and `folder_root` (ListView,
`is_folder = true`) attach ONE `EventControllerMotion`:

```rust
            let spot = pointer_spot.clone();
            let motion = gtk::EventControllerMotion::new();
            {
                let spot = spot.clone();
                let view = view.clone();
                motion.connect_enter(move |_, x, y| {
                    if let Some((wx, wy)) = pointer_window_position(&view, x, y) {
                        spot.set(Some(PointerSpot::new(is_folder, x, y)));
                    }
                });
            }
            {
                let spot = spot.clone();
                let view = view.clone();
                motion.connect_motion(move |_, x, y| {
                    if let Some((wx, wy)) = pointer_window_position(&view, x, y) {
                        spot.set(Some(PointerSpot::new(is_folder, wx, wy)));
                    }
                });
            }
            {
                let spot = spot.clone();
                motion.connect_leave(move |_| spot.set(None));
            }
            view.add_controller(motion);
```

with the loop iterating **owned** widgets:

```rust
        for (view, is_folder) in [
            (root.clone().upcast::<gtk::Widget>(), false),
            (folder_root.clone().upcast::<gtk::Widget>(), true),
        ] {
```

### 1d. In the `Self { ... }` literal of `Gallery::new`, add the two fields

```rust
            pointer_spot,
            last_wheel_scroll,
```

### 1e. Add the four methods to `impl Gallery` (e.g. after `update_layout`)

```rust
    /// Stamp real user scroll input (wheel detent or touchpad surface scroll)
    /// for the scroll-then-open grace window. Called from the smooth scrollers'
    /// capture-phase handlers in window::navigation: they stop wheel events
    /// before those can reach the GridView/ListView, so nothing deeper can
    /// observe them. Ctrl+wheel (grid zoom input) must never call this.
    pub fn note_user_scroll(&self) {
        self.last_wheel_scroll.set(Some(Instant::now()));
    }

    /// True when a real wheel/touchpad scroll was stamped within `within`.
    /// Used by the smooth scrollers to keep the scroll-to-focus selection
    /// update running while the eased spring is still moving the content.
    pub fn recent_user_scroll(&self, within: std::time::Duration) -> bool {
        self.last_wheel_scroll
            .get()
            .map(|stamp| stamp.elapsed() <= within)
            .unwrap_or(false)
    }

    /// Photo under the pointer, ignoring the scroll grace window. The pointer
    /// spot must be on the currently mapped view.
    pub fn photo_under_pointer(&self) -> Option<PhotoObject> {
        let (view, x, y) = match self.pointer_spot.get()? {
            PointerSpot::Grid(x, y) => (self.root.clone().upcast::<gtk::Widget>(), x, y),
            PointerSpot::Folder(x, y) => {
                (self.folder_root.clone().upcast::<gtk::Widget>(), x, y)
            }
        };
        // Only the view that is actually presented (mapped - the active stack
        // child) can be under the pointer. Stale spots for the hidden view
        // must not resolve. `is_visible` is not enough: inactive GtkStack
        // pages keep their visible flag set.
        if !view.is_mapped() {
            return None;
        }
        // Map the window-space pointer position into the view's CURRENT
        // coordinate space. The stored window position is constant for a
        // stationary pointer; this conversion applies whatever scroll
        // translation is live right now.
        let (x, y) = window_point_in_view(&view, x, y)?;
        let picked = view.pick(x, y, gtk::PickFlags::DEFAULT)?;
        let tile = picked
            .ancestor(SquareTile::static_type())
            .and_downcast::<SquareTile>()?;
        // Recycled tiles can sit under the pointer unbound for a moment.
        let photo = tile.imp().photo.borrow().clone();
        photo
    }

    /// Scroll to focus: make the photo currently under the pointer the single
    /// selection. Called (debounced) while the user scrolls with the pointer
    /// resting on the grid, so the selection follows the browsed content and
    /// Space/1:1 simply opens the selection.
    /// Returns true when a photo is selected afterwards.
    pub fn select_photo_under_pointer(&self) -> bool {
        // Never fight multi-selection or collage checklist mode: those flows
        // own the selection explicitly.
        if self.collage_selection_mode.get() || self.selected_photo_ids(None).len() > 1 {
            return false;
        }
        let Some(photo) = self.photo_under_pointer() else {
            return false;
        };
        let selected = self.selected_photo_ids(None);
        if selected.len() == 1 && selected.contains(&photo.id()) {
            return true;
        }
        self.set_selected_photo_ids(&[photo.id()]);
        true
    }

    /// Photo under the pointer, but only within `grace` of the last real
    /// wheel/touchpad scroll. Fallback for very fast "scroll then Space"
    /// activations; the normal path is `select_photo_under_pointer`, which the
    /// smooth scrollers call once the eased scroll has settled.
    pub fn hovered_photo_after_scroll(
        &self,
        grace: std::time::Duration,
    ) -> Option<PhotoObject> {
        let scrolled_at = self.last_wheel_scroll.get()?;
        if scrolled_at.elapsed() > grace {
            return None;
        }
        self.photo_under_pointer()
    }
```

`Instant` is already imported at the top of `src/grid.rs`. All `grid/*.rs`
files are spliced into one module, so the private fields are accessible from
`select_photo_under_pointer` regardless of which file it lives in.

---

## Step 2 — `src/window/navigation.rs`: stamp + debounced scroll-to-focus

### 2a. `install_smooth_gallery_scroll` (used by the photo grid AND albums home)

After the existing scrub-cancel block and **before**
`let controller = gtk::EventControllerScroll::new(...)`, add:

```rust
    // Scroll to focus: while the user wheels with the pointer resting on the
    // grid, the photo under the pointer becomes the selection once the eased
    // scroll settles. The selection then simply follows the browsed content,
    // so Space/1:1 opens it no matter how much later they are pressed.
    let scroll_focus_source: Rc<RefCell<Option<glib::SourceId>>> = Rc::new(RefCell::new(None));
    let schedule_scroll_focus: Rc<dyn Fn()> = {
        let gallery = gallery.clone();
        let source_cell = scroll_focus_source.clone();
        Rc::new(move || {
            if let Some(old) = source_cell.borrow_mut().take() {
                old.remove();
            }
            let gallery = gallery.clone();
            let source_cell_for_timer = source_cell.clone();
            let id = glib::timeout_add_local_once(
                std::time::Duration::from_millis(220),
                move || {
                    source_cell_for_timer.borrow_mut().take();
                    gallery.select_photo_under_pointer();
                },
            );
            *source_cell.borrow_mut() = Some(id);
        })
    };
    {
        // The eased spring keeps moving content after the last wheel event;
        // keep re-arming the focus update while that tail is still running.
        let schedule = schedule_scroll_focus.clone();
        let gallery_for_focus = gallery.clone();
        adjustment.connect_value_changed(move |_| {
            if gallery_for_focus.recent_user_scroll(std::time::Duration::from_millis(600)) {
                schedule();
            }
        });
    }
```

Then inside `controller.connect_scroll(...)`, in the non-Ctrl path **after**
the `if dy == 0.0 { return ...; }` check and **before**
`match controller.unit()`, add:

```rust
        // Real user scroll input (wheel detent or touchpad surface scroll).
        // This capture-phase controller stops wheel events before they can
        // reach controllers deeper in the tree, so this is the only place the
        // scroll-then-open grace window (Gallery::note_user_scroll) can be
        // stamped from for this scroller.
        gallery.note_user_scroll();
        schedule_scroll_focus();
```

The Ctrl branch (grid zoom) stays **before** this and must **not** stamp.

### 2b. `install_folder_smooth_gallery_scroll` (Folder stream)

Same three pieces: the `scroll_focus_source` + `schedule_folder_focus` block,
the same `adjustment.connect_value_changed` re-arm hook, and
`gallery.note_user_scroll(); schedule_folder_focus();` at the same spot in its
scroll handler (after the Ctrl block and the `dy == 0.0` check, before the
`match controller.unit()`).

Both functions already receive `gallery: Rc<grid::Gallery>` and both handlers
capture it with `move` — no signature changes needed.

---

## Step 3 — `src/window/build.rs`: open the scrolled-to photo

In `space_open_slot.replace(Some(Rc::new(move || { ... })))` (search for
`space_open_slot.replace`), resolve the scrolled-to photo first, select it, and
prefer it over the stale selection:

```rust
        space_open_slot.replace(Some(Rc::new(move || {
            let photos = gallery.photo_objects();
            // Scroll-then-open: when 1:1/Space is activated within a short
            // grace after a wheel/touchpad scroll and the pointer rests on a
            // thumbnail, that photo becomes the selection and opens. This is
            // the fast "scroll, then Space through photos" flow. Normally the
            // scroll has already re-selected it (scroll to focus), so this is
            // only a fast-path fallback. Hover alone never changes anything:
            // the plain selection always wins.
            let scroll_hovered =
                gallery.hovered_photo_after_scroll(grid::SCROLL_HOVER_OPEN_GRACE);
            if let Some(hovered) = scroll_hovered.as_ref() {
                gallery.set_selected_photo_ids(&[hovered.id()]);
            }
            let selected_id = scroll_hovered
                .as_ref()
                .map(|photo| photo.id())
                .or_else(|| {
                    selected_photo
                        .borrow()
                        .as_ref()
                        .map(|photo| photo.id())
                })
                .filter(|id| photos.iter().any(|photo| photo.id() == *id))
                .or_else(|| gallery.selected_photo_ids(None).into_iter().next()));
```

Everything after this point (index lookup, `lightbox.open(photos, index)`,
availability refresh) stays exactly as it is. The Space key handler
(`src/window/shortcuts.rs`) and the 1:1 toggle button both call this slot, so
both get the behavior with no further changes.

---

## Step 4 — Optional debug logging (recommended, env-gated)

- `Gallery::note_user_scroll`: log `"[space-debug] note_user_scroll stamped"`
- `select_photo_under_pointer`: log `"[space-debug] scroll-to-focus selects: {filename}"`
- `photo_under_pointer`: log the view-space pointer, pick misses, and the
  resolved filename

all gated by `if std::env::var_os("PIC_DEBUG_SPACE").is_some() { ... }`.
Run with `PIC_DEBUG_SPACE=1 ./target/debug/pic-rs`. Expected healthy trace for
one scroll+Space:

```
[space-debug] note_user_scroll stamped        (× detents)
[space-debug] pointer resolves: <file>.jpg
[space-debug] scroll-to-focus selects: <file>.jpg
[space-debug] Space key: lightbox_visible=false edit_page=false
[space-debug] slot: scroll-hover wins -> <file>.jpg      (fast press)
   — or, if pressed much later —
[space-debug] slot: no scroll-hover -> selection decides  (selection already correct)
```

---

## Step 5 — Tests (`src/grid.rs`, module `scroll_hover_open_tests`)

Four gating tests (hover alone / expired grace / fresh scroll without pointer /
stale spot for hidden view all resolve to `None`) plus one rendered-grid
integration test (`scroll_then_space_resolves_photo_now_under_pointer`) that
builds a 240-photo gallery in a window, scrolls, anchors the pointer to a
visible tile's window-space position and asserts the hover resolves the
rendered photo, then scrolls again and asserts the selection follows. All are
`#[ignore = "requires a GTK display; run with --ignored --test-threads=1"]` and
must be run **one test per cargo invocation** (GTK cannot re-init across test
threads):

```bash
cargo test                                  # everything non-GTK
cargo test -- --ignored scroll_then_space_resolves_photo_now_under_pointer
cargo test -- --ignored hover_without_recent_scroll_never_resolves
cargo test -- --ignored expired_grace_falls_back_to_selection
cargo test -- --ignored fresh_scroll_without_pointer_resolves_to_nothing
cargo test -- --ignored stale_spot_for_hidden_view_never_resolves
```

Note: the frame-less test harness lags child allocations behind the adjustment
after a second scroll, so the integration test's second phase asserts that the
**selection follows** the content (`select_photo_under_pointer` changes the
selection away from the pre-scroll answer) instead of asserting absolute ids.

---

## Step 6 — Verify by hand

```bash
cargo build
PIC_DEBUG_SPACE=1 GDK_BACKEND=x11 ./target/debug/pic-rs
```

1. Wheel-scroll with the pointer parked on a thumbnail. Expect a burst of
   `note_user_scroll stamped`, then one `scroll-to-focus selects: <file>`.
2. Press Space at ANY delay. Log shows either `slot: scroll-hover wins -> <file>`
   or `slot: no scroll-hover -> selection decides` — both must open the same
   file that is under the pointer.
3. Move the mouse to another tile WITHOUT scrolling and press Space → the
   previously selected photo opens (hover alone never wins).
4. Ctrl+wheel → zoom only; then Space → selected photo (no hover capture).
5. Smooth scrolling must feel unchanged (the added code never touches
   `Propagation` values or the spring).

## Troubleshooting

| Symptom | Cause |
|---|---|
| `grace expired` immediately, `note_user_scroll` never logged | stamp not in the smooth scrollers' capture handlers |
| Opens the **first photo of the first section** | coordinates stored in view space instead of window space |
| Opens hovered photo even without scrolling | hover armed outside `note_user_scroll` (e.g. in the plain motion path) |
| Opens hovered photo after Ctrl+wheel zoom | stamp called in the Ctrl branch |
| Selection jumps while sweeping the grid | scroll-to-focus debounce missing / too short, or armed on every motion |
