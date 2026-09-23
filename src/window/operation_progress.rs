const PROGRESS_THROTTLE: Duration = Duration::from_millis(100);
const FINISH_HIDE_AFTER: Duration = Duration::from_secs(4);
/// Search entry floor in the header; the status row yields before this shrinks.
const SEARCH_MIN_WIDTH: i32 = 220;
/// `search_area` spacing between the entry and the status row.
const SEARCH_GAP: i32 = 6;

/// The one status indicator (single row), embedded in the header title area
/// beside the search entry.
///
/// Shared by library refresh and long batch operations (multi-photo export,
/// bulk paste edits, bulk paste text & overlays). Refresh writes the label /
/// spinner / visibility directly; batch ops use `begin`/`update`/`finish`,
/// which also drive the inline progress bar and live `n / total` counts.
///
/// It lives in the header rather than above the grid so appearing/disappearing
/// never resizes the photo grid. Call sites toggle `root()`'s visible flag;
/// a separate `slot` wrapper is what sits in `search_area`, so a narrow-window
/// guard can hide the whole row without fighting those call sites.
///
/// Narrow windows degrade the status row before the search entry drops below
/// its 220px floor: full row → no progress bar → spinner + Stop only → hide
/// the slot (the operation keeps running). See `watch_space`.
///
/// Visual refreshes are throttled so a 4000-item batch does not redraw the
/// GTK main loop thousands of times per second — except when the file counter
/// advances, which always repaints.
pub struct OperationProgressUi {
    /// Parent of `root`; mounted in `search_area`. Hidden only by the space
    /// guard, never by operation call sites.
    slot: gtk::Box,
    root: gtk::Box,
    spinner: gtk::Spinner,
    label: gtk::Label,
    bar: gtk::ProgressBar,
    stop: gtk::Button,
    last_paint: Cell<Option<Instant>>,
    last_done: Cell<usize>,
    hide_source: Cell<Option<glib::SourceId>>,
    running: Cell<bool>,
    /// Space guard hid the bar; restore it when room returns if still running.
    bar_suppressed: Cell<bool>,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl OperationProgressUi {
    pub fn new() -> Rc<Self> {
        let slot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        slot.set_valign(gtk::Align::Center);
        slot.set_visible(true);

        let root = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        root.set_margin_start(6);
        root.set_margin_end(6);
        root.set_margin_top(2);
        root.set_margin_bottom(2);
        root.set_valign(gtk::Align::Center);
        root.set_visible(false);

        let spinner = gtk::Spinner::new();
        spinner.set_spinning(false);
        root.append(&spinner);

        let label = gtk::Label::new(Some("Refreshing library…"));
        label.set_xalign(0.0);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        // Cap the natural width so a long "n / total · filename" line never
        // claims header space the search entry needs.
        label.set_max_width_chars(48);
        root.append(&label);

        // Inline fraction bar for batch ops only; hidden for plain refresh
        // text. The space guard also hides it before the search entry shrinks.
        let bar = gtk::ProgressBar::new();
        bar.set_show_text(false);
        bar.set_valign(gtk::Align::Center);
        bar.set_visible(false);
        root.append(&bar);

        let stop = gtk::Button::with_label("Stop");
        stop.set_tooltip_text(Some("Stop the current operation"));
        stop.add_css_class("flat");
        root.append(&stop);

        slot.append(&root);

        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        Rc::new(Self {
            slot,
            root,
            spinner,
            label,
            bar,
            stop,
            last_paint: Cell::new(None),
            last_done: Cell::new(0),
            hide_source: Cell::new(None),
            running: Cell::new(false),
            bar_suppressed: Cell::new(false),
            cancel,
        })
    }

    /// Outer wrapper mounted in the header; hide/show `root()` for operations.
    pub fn slot(&self) -> &gtk::Box {
        &self.slot
    }

    pub fn root(&self) -> &gtk::Box {
        &self.root
    }

    pub fn spinner(&self) -> &gtk::Spinner {
        &self.spinner
    }

    pub fn label(&self) -> &gtk::Label {
        &self.label
    }

    pub fn stop(&self) -> &gtk::Button {
        &self.stop
    }

    pub fn is_running(&self) -> bool {
        self.running.get()
    }

    /// Watch header space next to the search entry and degrade the status row
    /// before the entry would drop below its 220px floor. The operation itself
    /// is unaffected: hiding `slot` only changes what is drawn.
    pub fn watch_space(self: &Rc<Self>, search_area: &gtk::Box) {
        let weak = Rc::downgrade(self);
        search_area.add_tick_callback(move |area, _| {
            let Some(this) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            this.apply_space_guard(area.allocated_width());
            glib::ControlFlow::Continue
        });
    }

    /// Degradation ladder, re-evaluated every frame while the header lays out:
    /// full row → no bar → spinner + Stop only → hide the slot.
    fn apply_space_guard(&self, available: i32) {
        // Idle: reset so the next operation starts with the full row.
        // Read the `visible` property (not `is_visible`) — the latter is false
        // whenever an ancestor such as `slot` is hidden.
        if !self.root.property::<bool>("visible") {
            if !self.slot.is_visible() {
                self.slot.set_visible(true);
            }
            if !self.label.is_visible() {
                self.label.set_visible(true);
            }
            if self.bar_suppressed.get() {
                self.bar_suppressed.set(false);
                if self.running.get() {
                    self.bar.set_visible(true);
                }
            }
            return;
        }

        let budget = available.saturating_sub(SEARCH_MIN_WIDTH + SEARCH_GAP);

        // What the operation wants right now (bar may have been suppressed).
        let bar_wanted = if self.bar_suppressed.get() {
            self.running.get()
        } else {
            self.bar.is_visible()
        };

        // Stage 1: full row (label + bar as the operation left it).
        if !self.label.is_visible() {
            self.label.set_visible(true);
        }
        if bar_wanted != self.bar.is_visible() {
            self.bar.set_visible(bar_wanted);
        }
        self.bar_suppressed.set(false);
        if self.fits(budget) {
            if !self.slot.is_visible() {
                self.slot.set_visible(true);
            }
            return;
        }

        // Stage 2: drop the progress bar (largest fixed min-width).
        if self.bar.is_visible() {
            self.bar.set_visible(false);
        }
        self.bar_suppressed.set(true);
        if self.fits(budget) {
            if !self.slot.is_visible() {
                self.slot.set_visible(true);
            }
            return;
        }

        // Stage 3: spinner + Stop only.
        if self.label.is_visible() {
            self.label.set_visible(false);
        }
        if self.fits(budget) {
            if !self.slot.is_visible() {
                self.slot.set_visible(true);
            }
            return;
        }

        // Stage 4: no room even for spinner + Stop; hide the row only.
        if self.slot.is_visible() {
            self.slot.set_visible(false);
        }
    }

    fn fits(&self, budget: i32) -> bool {
        let (min, ..) = self.root.measure(gtk::Orientation::Horizontal, -1);
        min <= budget
    }

    /// Fire `callback` when the user hits Stop on this notification bar.
    pub fn connect_stop<C: Fn() + 'static>(&self, callback: C) {
        self.stop.connect_clicked(move |_| callback());
    }

    /// Shared flag polled by background workers. Cleared by `begin`.
    pub fn cancel_flag(&self) -> std::sync::Arc<std::sync::atomic::AtomicBool> {
        self.cancel.clone()
    }

    pub fn request_cancel(&self) {
        self.cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
        if self.running.get() {
            self.stop.set_sensitive(false);
            self.label
                .set_text("Stopping…");
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn clear_cancel(&self) {
        self.cancel
            .store(false, std::sync::atomic::Ordering::Relaxed);
        self.stop.set_sensitive(true);
    }

    fn cancel_hide(&self) {
        if let Some(source) = self.hide_source.take() {
            // The auto-hide source may already have fired; glib panics if we
            // remove a dead SourceId. That was the LayersOnly paste crash.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                source.remove();
            }));
        }
    }

    pub fn begin(&self, name: &str, total: usize) {
        self.cancel_hide();
        self.clear_cancel();
        self.running.set(true);
        let total = total.max(1);
        self.label
            .set_text(&format!("{name}    0 / {total}    0%"));
        self.bar.set_fraction(0.0);
        self.bar.set_visible(true);
        self.spinner.set_spinning(true);
        self.stop.set_visible(true);
        self.stop.set_sensitive(true);
        self.root.set_visible(true);
        self.last_paint.set(Some(Instant::now()));
        self.last_done.set(0);
    }

    pub fn update(&self, name: &str, done: usize, total: usize, filename: &str, failed: usize) {
        let total = total.max(1);
        let done = done.min(total);
        let finished = done >= total;
        // Always repaint when the file counter advances so multi-export shows
        // a live `n / total`. Time-throttle only identical-count spam.
        let count_changed = self.last_done.get() != done;
        if !finished && !count_changed {
            if let Some(last) = self.last_paint.get() {
                if last.elapsed() < PROGRESS_THROTTLE {
                    return;
                }
            }
        }
        self.last_done.set(done);
        self.last_paint.set(Some(Instant::now()));
        if !self.root.is_visible() {
            self.label.set_text(name);
            self.root.set_visible(true);
        }
        let percent = done * 100 / total;
        let mut text = format!("{name}    {done} / {total}    {percent}%");
        if !filename.is_empty() {
            text.push_str("    ");
            text.push_str(filename);
        }
        if failed > 0 {
            text.push_str(&format!("    ({failed} failed)"));
        }
        self.label.set_text(&text);
        self.bar.set_fraction(done as f64 / total as f64);
        self.bar.set_visible(true);
    }

    pub fn finish(&self, name: &str, summary: &str) {
        self.cancel_hide();
        self.running.set(false);
        self.clear_cancel();
        // Summary already carries the outcome ("Export complete — 3 exported").
        if summary.is_empty() {
            self.label.set_text(name);
        } else {
            self.label.set_text(summary);
        }
        self.bar.set_fraction(1.0);
        self.bar.set_visible(false);
        self.spinner.set_spinning(false);
        self.stop.set_sensitive(false);
        self.root.set_visible(true);
        self.last_paint.set(Some(Instant::now()));
        self.last_done.set(0);
        let weak_root = self.root.downgrade();
        let source = glib::timeout_add_local_once(FINISH_HIDE_AFTER, move || {
            if let Some(root) = weak_root.upgrade() {
                root.set_visible(false);
            }
        });
        self.hide_source.set(Some(source));
    }
}
