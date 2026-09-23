const PROGRESS_THROTTLE: Duration = Duration::from_millis(100);
const FINISH_HIDE_AFTER: Duration = Duration::from_secs(4);

/// The one top-of-window notification bar (single row).
///
/// Shared by library refresh and long batch operations (multi-photo export,
/// bulk paste edits, bulk paste text & overlays). Refresh writes the label /
/// spinner / visibility directly; batch ops use `begin`/`update`/`finish`,
/// which also drive the inline progress bar and live `n / total` counts.
///
/// Visual refreshes are throttled so a 4000-item batch does not redraw the
/// GTK main loop thousands of times per second — except when the file counter
/// advances, which always repaints.
pub struct OperationProgressUi {
    root: gtk::Box,
    spinner: gtk::Spinner,
    label: gtk::Label,
    bar: gtk::ProgressBar,
    stop: gtk::Button,
    last_paint: Cell<Option<Instant>>,
    last_done: Cell<usize>,
    hide_source: Cell<Option<glib::SourceId>>,
    running: Cell<bool>,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl OperationProgressUi {
    pub fn new() -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        root.set_margin_start(12);
        root.set_margin_end(12);
        root.set_margin_top(8);
        root.set_margin_bottom(8);
        root.add_css_class("toolbar");
        root.add_css_class("card");
        root.set_visible(false);

        let spinner = gtk::Spinner::new();
        spinner.set_spinning(false);
        root.append(&spinner);

        let label = gtk::Label::new(Some("Refreshing library…"));
        label.set_xalign(0.0);
        label.set_hexpand(true);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        root.append(&label);

        // Inline fraction bar for batch ops only; hidden for plain refresh text.
        let bar = gtk::ProgressBar::new();
        bar.set_show_text(false);
        bar.set_valign(gtk::Align::Center);
        bar.set_width_request(160);
        bar.set_visible(false);
        root.append(&bar);

        let stop = gtk::Button::with_label("Stop");
        stop.set_tooltip_text(Some("Stop the current operation"));
        root.append(&stop);

        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        Rc::new(Self {
            root,
            spinner,
            label,
            bar,
            stop,
            last_paint: Cell::new(None),
            last_done: Cell::new(0),
            hide_source: Cell::new(None),
            running: Cell::new(false),
            cancel,
        })
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
