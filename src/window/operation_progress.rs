const PROGRESS_THROTTLE: Duration = Duration::from_millis(100);
const FINISH_HIDE_AFTER: Duration = Duration::from_secs(4);

/// One reusable top-of-window notification for long batch operations
/// (multi-photo export, bulk paste edits, bulk paste text & overlays).
///
/// A single instance updates in place — never one notification per photo.
/// Visual refreshes are throttled so a 4000-item batch does not redraw the
/// GTK main loop thousands of times per second.
pub struct OperationProgressUi {
    root: gtk::Box,
    title: gtk::Label,
    detail: gtk::Label,
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
        let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
        root.set_margin_start(12);
        root.set_margin_end(12);
        root.set_margin_top(8);
        root.set_margin_bottom(4);
        root.add_css_class("toolbar");
        root.add_css_class("card");
        root.set_visible(false);

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let title = gtk::Label::new(None);
        title.set_xalign(0.0);
        title.set_hexpand(true);
        title.add_css_class("heading");
        header.append(&title);
        let stop = gtk::Button::with_label("Stop");
        stop.add_css_class("flat");
        stop.set_visible(false);
        stop.set_tooltip_text(Some("Stop this operation"));
        header.append(&stop);
        root.append(&header);

        let detail = gtk::Label::new(None);
        detail.set_xalign(0.0);
        detail.set_ellipsize(gtk::pango::EllipsizeMode::End);
        detail.add_css_class("dim-label");
        root.append(&detail);

        let bar = gtk::ProgressBar::new();
        bar.set_show_text(true);
        bar.set_hexpand(true);
        root.append(&bar);

        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        Rc::new(Self {
            root,
            title,
            detail,
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
            self.detail.set_text("Stopping…");
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
        self.title.set_text(name);
        let total = total.max(1);
        self.detail.set_text(&format!("0 / {total}    0%"));
        self.bar.set_fraction(0.0);
        self.bar.set_text(Some(&format!("0 / {total}")));
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
            self.title.set_text(name);
            self.root.set_visible(true);
        }
        let percent = done * 100 / total;
        let mut detail = format!("{done} / {total}    {percent}%");
        if !filename.is_empty() {
            detail.push_str("    ");
            detail.push_str(filename);
        }
        if failed > 0 {
            detail.push_str(&format!("    ({failed} failed)"));
        }
        self.detail.set_text(&detail);
        self.bar.set_fraction(done as f64 / total as f64);
        self.bar
            .set_text(Some(&format!("{done} / {total}    {percent}%")));
    }

    pub fn finish(&self, name: &str, summary: &str) {
        self.cancel_hide();
        self.running.set(false);
        self.clear_cancel();
        self.title.set_text(name);
        self.detail.set_text(summary);
        self.bar.set_fraction(1.0);
        self.bar.set_text(Some(summary));
        self.stop.set_visible(false);
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
