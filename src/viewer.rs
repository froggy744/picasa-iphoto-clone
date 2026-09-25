use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use gtk::prelude::*;
use gtk4 as gtk;

use crate::photo::PhotoObject;

#[derive(Clone)]
struct ViewerJob {
    generation: u64,
    path: String,
    rotation: i32,
    native: bool,
    target_width: u32,
    target_height: u32,
}

struct ViewerResult {
    generation: u64,
    width: i32,
    height: i32,
    rgba: Vec<u8>,
}

type PhotoChanged = Rc<RefCell<Option<Box<dyn Fn(PhotoObject)>>>>;
type OneToOneChanged = Rc<RefCell<Option<Box<dyn Fn(bool)>>>>;

pub struct Viewer {
    pub root: gtk::Overlay,
    picture: gtk::Picture,
    viewport: gtk::ScrolledWindow,
    photos: Rc<RefCell<Vec<PhotoObject>>>,
    index: Rc<Cell<usize>>,
    zoom: Rc<Cell<f64>>,
    one_to_one: Rc<Cell<bool>>,
    generation: Rc<Cell<u64>>,
    latest_generation: Arc<AtomicU64>,
    jobs: mpsc::Sender<ViewerJob>,
    refresh: Rc<dyn Fn()>,
    photo_changed: PhotoChanged,
    one_to_one_changed: OneToOneChanged,
}

impl Viewer {
    pub fn new() -> Self {
        let root = gtk::Overlay::new();
        root.set_hexpand(true);
        root.set_vexpand(true);
        root.set_focusable(true);
        root.set_visible(false);

        let backdrop = gtk::Box::new(gtk::Orientation::Vertical, 0);
        backdrop.set_hexpand(true);
        backdrop.set_vexpand(true);
        backdrop.add_css_class("lightbox-backdrop");
        root.set_child(Some(&backdrop));

        let picture = gtk::Picture::new();
        picture.set_can_shrink(true);
        picture.set_content_fit(gtk::ContentFit::Contain);
        picture.set_hexpand(false);
        picture.set_vexpand(false);
        picture.set_halign(gtk::Align::Center);
        picture.set_valign(gtk::Align::Center);
        picture.add_css_class("lightbox-picture");

        let viewport = gtk::ScrolledWindow::new();
        viewport.set_hexpand(true);
        viewport.set_vexpand(true);
        viewport.set_policy(gtk::PolicyType::External, gtk::PolicyType::External);
        viewport.set_child(Some(&picture));
        root.add_overlay(&viewport);

        let close = gtk::Button::from_icon_name("window-close-symbolic");
        close.set_tooltip_text(Some("Close"));
        close.add_css_class("circular");
        close.set_halign(gtk::Align::End);
        close.set_valign(gtk::Align::Start);
        close.set_margin_top(14);
        close.set_margin_end(14);
        root.add_overlay(&close);

        let previous = gtk::Button::from_icon_name("go-previous-symbolic");
        previous.set_tooltip_text(Some("Previous photo"));
        previous.add_css_class("circular");
        previous.set_halign(gtk::Align::Start);
        previous.set_valign(gtk::Align::Center);
        previous.set_margin_start(14);
        root.add_overlay(&previous);

        let next = gtk::Button::from_icon_name("go-next-symbolic");
        next.set_tooltip_text(Some("Next photo"));
        next.add_css_class("circular");
        next.set_halign(gtk::Align::End);
        next.set_valign(gtk::Align::Center);
        next.set_margin_end(14);
        root.add_overlay(&next);

        let photos = Rc::new(RefCell::new(Vec::<PhotoObject>::new()));
        let index = Rc::new(Cell::new(0usize));
        let zoom = Rc::new(Cell::new(1.0f64));
        let one_to_one = Rc::new(Cell::new(false));
        let generation = Rc::new(Cell::new(0u64));
        let latest_generation = Arc::new(AtomicU64::new(0));
        let photo_changed: PhotoChanged = Rc::new(RefCell::new(None));
        let one_to_one_changed: OneToOneChanged = Rc::new(RefCell::new(None));

        let (job_tx, job_rx) = mpsc::channel::<ViewerJob>();
        let (result_tx, result_rx) = mpsc::channel::<ViewerResult>();
        let shared_jobs = Arc::new(Mutex::new(job_rx));

        for _ in 0..2 {
            let jobs = shared_jobs.clone();
            let results = result_tx.clone();
            let latest = latest_generation.clone();
            std::thread::spawn(move || loop {
                let job = {
                    let Ok(receiver) = jobs.lock() else {
                        return;
                    };
                    match receiver.recv() {
                        Ok(job) => job,
                        Err(_) => return,
                    }
                };
                if latest.load(Ordering::Acquire) != job.generation {
                    continue;
                }

                let decoded = image::ImageReader::open(&job.path)
                    .ok()
                    .and_then(|reader| reader.with_guessed_format().ok())
                    .and_then(|reader| reader.decode().ok());

                let Some(image) = decoded else {
                    continue;
                };
                if latest.load(Ordering::Acquire) != job.generation {
                    continue;
                }

                let image = match job.rotation.rem_euclid(360) {
                    90 => image.rotate90(),
                    180 => image.rotate180(),
                    270 => image.rotate270(),
                    _ => image,
                };
                let image = if job.native {
                    image
                } else {
                    image.thumbnail(job.target_width.max(1), job.target_height.max(1))
                };
                let rgba = image.to_rgba8();
                let result = ViewerResult {
                    generation: job.generation,
                    width: rgba.width() as i32,
                    height: rgba.height() as i32,
                    rgba: rgba.into_raw(),
                };
                if latest.load(Ordering::Acquire) == job.generation {
                    let _ = results.send(result);
                }
            });
        }

        {
            let picture = picture.clone();
            let viewport = viewport.clone();
            let generation = generation.clone();
            let one_to_one = one_to_one.clone();
            let zoom = zoom.clone();
            glib::timeout_add_local(Duration::from_millis(16), move || {
                while let Ok(result) = result_rx.try_recv() {
                    if result.generation != generation.get()
                        || result.width <= 0
                        || result.height <= 0
                    {
                        continue;
                    }

                    let stride = result.width.max(1) as usize * 4;
                    let bytes = glib::Bytes::from_owned(result.rgba);
                    let texture = gtk::gdk::MemoryTexture::new(
                        result.width,
                        result.height,
                        gtk::gdk::MemoryFormat::R8g8b8a8,
                        &bytes,
                        stride,
                    );
                    picture.set_paintable(Some(&texture));
                    picture.set_size_request(result.width, result.height);
                    picture.set_can_shrink(!(one_to_one.get() || zoom.get() > 1.0));
                    picture.queue_resize();

                    let viewport = viewport.clone();
                    glib::idle_add_local_once(move || center_viewport(&viewport));
                }
                glib::ControlFlow::Continue
            });
        }

        let refresh: Rc<dyn Fn()> = {
            let photos = photos.clone();
            let index = index.clone();
            let root = root.clone();
            let zoom = zoom.clone();
            let one_to_one = one_to_one.clone();
            let generation = generation.clone();
            let latest_generation = latest_generation.clone();
            let jobs = job_tx.clone();
            Rc::new(move || {
                let Some(photo) = photos.borrow().get(index.get()).cloned() else {
                    return;
                };
                let next_generation = generation.get().wrapping_add(1);
                generation.set(next_generation);
                latest_generation.store(next_generation, Ordering::Release);

                let native = one_to_one.get();
                let scale = zoom.get().clamp(0.25, 4.0);
                let target_width = ((root.width().max(320) as f64) * scale)
                    .round()
                    .clamp(1.0, u32::MAX as f64) as u32;
                let target_height = ((root.height().max(240) as f64) * scale)
                    .round()
                    .clamp(1.0, u32::MAX as f64) as u32;

                let _ = jobs.send(ViewerJob {
                    generation: next_generation,
                    path: photo.path(),
                    rotation: photo.rotation(),
                    native,
                    target_width,
                    target_height,
                });
            })
        };

        {
            let root = root.clone();
            let generation = generation.clone();
            let latest = latest_generation.clone();
            close.connect_clicked(move |_| {
                let next = generation.get().wrapping_add(1);
                generation.set(next);
                latest.store(next, Ordering::Release);
                root.set_visible(false);
            });
        }

        let navigate = {
            let photos = photos.clone();
            let index = index.clone();
            let zoom = zoom.clone();
            let one_to_one = one_to_one.clone();
            let refresh = refresh.clone();
            let photo_changed = photo_changed.clone();
            Rc::new(move |delta: i32| {
                let len = photos.borrow().len();
                if len == 0 {
                    return;
                }
                let current = index.get();
                let target = if delta < 0 {
                    current.saturating_sub(delta.unsigned_abs() as usize)
                } else {
                    (current + delta as usize).min(len - 1)
                };
                if target == current {
                    return;
                }
                index.set(target);
                zoom.set(1.0);
                one_to_one.set(false);
                if let Some(photo) = photos.borrow().get(target).cloned() {
                    if let Some(handler) = photo_changed.borrow().as_ref() {
                        handler(photo);
                    }
                }
                refresh();
            })
        };

        {
            let navigate = navigate.clone();
            previous.connect_clicked(move |_| navigate(-1));
        }
        {
            let navigate = navigate.clone();
            next.connect_clicked(move |_| navigate(1));
        }

        let apply_zoom = {
            let zoom = zoom.clone();
            let one_to_one = one_to_one.clone();
            let one_to_one_changed = one_to_one_changed.clone();
            let refresh = refresh.clone();
            Rc::new(move |factor: f64| {
                if one_to_one.replace(false) {
                    if let Some(handler) = one_to_one_changed.borrow().as_ref() {
                        handler(false);
                    }
                }
                zoom.set((zoom.get() * factor).clamp(0.25, 4.0));
                refresh();
            })
        };

        let fit = {
            let zoom = zoom.clone();
            let one_to_one = one_to_one.clone();
            let one_to_one_changed = one_to_one_changed.clone();
            let refresh = refresh.clone();
            Rc::new(move || {
                zoom.set(1.0);
                if one_to_one.replace(false) {
                    if let Some(handler) = one_to_one_changed.borrow().as_ref() {
                        handler(false);
                    }
                }
                refresh();
            })
        };

        let toggle_one_to_one = {
            let zoom = zoom.clone();
            let one_to_one = one_to_one.clone();
            let one_to_one_changed = one_to_one_changed.clone();
            let refresh = refresh.clone();
            Rc::new(move || {
                let enabled = !one_to_one.get();
                one_to_one.set(enabled);
                if !enabled {
                    zoom.set(1.0);
                }
                if let Some(handler) = one_to_one_changed.borrow().as_ref() {
                    handler(enabled);
                }
                refresh();
            })
        };

        let double_click = gtk::GestureClick::new();
        double_click.set_button(1);
        {
            let root = root.clone();
            let generation = generation.clone();
            let latest = latest_generation.clone();
            double_click.connect_pressed(move |gesture, presses, _, _| {
                if presses == 2 {
                    let next = generation.get().wrapping_add(1);
                    generation.set(next);
                    latest.store(next, Ordering::Release);
                    root.set_visible(false);
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                }
            });
        }
        picture.add_controller(double_click);

        let drag_start_h = Rc::new(Cell::new(0.0));
        let drag_start_v = Rc::new(Cell::new(0.0));
        let pan = gtk::GestureDrag::new();
        pan.set_button(1);
        {
            let viewport = viewport.clone();
            let drag_start_h = drag_start_h.clone();
            let drag_start_v = drag_start_v.clone();
            pan.connect_drag_begin(move |gesture, _, _| {
                let hadj = viewport.hadjustment();
                let vadj = viewport.vadjustment();
                let can_pan = hadj.upper() > hadj.page_size() + 1.0
                    || vadj.upper() > vadj.page_size() + 1.0;
                if !can_pan {
                    gesture.set_state(gtk::EventSequenceState::Denied);
                    return;
                }
                drag_start_h.set(hadj.value());
                drag_start_v.set(vadj.value());
                viewport.set_cursor_from_name(Some("grabbing"));
            });
        }
        {
            let viewport = viewport.clone();
            let drag_start_h = drag_start_h.clone();
            let drag_start_v = drag_start_v.clone();
            pan.connect_drag_update(move |_, dx, dy| {
                let hadj = viewport.hadjustment();
                let vadj = viewport.vadjustment();
                let max_h = (hadj.upper() - hadj.page_size()).max(hadj.lower());
                let max_v = (vadj.upper() - vadj.page_size()).max(vadj.lower());
                hadj.set_value((drag_start_h.get() - dx).clamp(hadj.lower(), max_h));
                vadj.set_value((drag_start_v.get() - dy).clamp(vadj.lower(), max_v));
            });
        }
        {
            let viewport = viewport.clone();
            pan.connect_drag_end(move |_, _, _| {
                viewport.set_cursor_from_name(Some("grab"));
            });
        }
        root.add_controller(pan);

        let scroll = gtk::EventControllerScroll::new(
            gtk::EventControllerScrollFlags::VERTICAL
                | gtk::EventControllerScrollFlags::DISCRETE,
        );
        scroll.set_propagation_phase(gtk::PropagationPhase::Capture);
        {
            let navigate = navigate.clone();
            let apply_zoom = apply_zoom.clone();
            scroll.connect_scroll(move |controller, _, dy| {
                if controller
                    .current_event_state()
                    .contains(gtk::gdk::ModifierType::CONTROL_MASK)
                {
                    if dy < 0.0 {
                        apply_zoom(1.15);
                    } else if dy > 0.0 {
                        apply_zoom(0.87);
                    }
                    return glib::Propagation::Stop;
                }
                if dy < 0.0 {
                    navigate(-1);
                } else if dy > 0.0 {
                    navigate(1);
                }
                glib::Propagation::Stop
            });
        }
        root.add_controller(scroll);

        let key = gtk::EventControllerKey::new();
        key.set_propagation_phase(gtk::PropagationPhase::Capture);
        {
            let root = root.clone();
            let generation = generation.clone();
            let latest = latest_generation.clone();
            let navigate = navigate.clone();
            let apply_zoom = apply_zoom.clone();
            let fit = fit.clone();
            let toggle_one_to_one = toggle_one_to_one.clone();
            key.connect_key_pressed(move |_, key, _, _| {
                if key == gtk::gdk::Key::Escape {
                    let next = generation.get().wrapping_add(1);
                    generation.set(next);
                    latest.store(next, Ordering::Release);
                    root.set_visible(false);
                    glib::Propagation::Stop
                } else if key == gtk::gdk::Key::Left {
                    navigate(-1);
                    glib::Propagation::Stop
                } else if key == gtk::gdk::Key::Right {
                    navigate(1);
                    glib::Propagation::Stop
                } else if key == gtk::gdk::Key::plus || key == gtk::gdk::Key::equal {
                    apply_zoom(1.15);
                    glib::Propagation::Stop
                } else if key == gtk::gdk::Key::minus {
                    apply_zoom(0.87);
                    glib::Propagation::Stop
                } else if key == gtk::gdk::Key::_0 {
                    fit();
                    glib::Propagation::Stop
                } else if key == gtk::gdk::Key::_1 || key == gtk::gdk::Key::space {
                    toggle_one_to_one();
                    glib::Propagation::Stop
                } else {
                    glib::Propagation::Proceed
                }
            });
        }
        root.add_controller(key);

        {
            let root = root.clone();
            let one_to_one = one_to_one.clone();
            let zoom = zoom.clone();
            let refresh = refresh.clone();
            let last_width = Rc::new(Cell::new(0));
            let last_height = Rc::new(Cell::new(0));
            glib::timeout_add_local(Duration::from_millis(160), move || {
                if root.is_visible()
                    && !one_to_one.get()
                    && (zoom.get() - 1.0).abs() < f64::EPSILON
                {
                    let width = root.width();
                    let height = root.height();
                    if width > 0
                        && height > 0
                        && (width != last_width.replace(width)
                            || height != last_height.replace(height))
                    {
                        refresh();
                    }
                }
                glib::ControlFlow::Continue
            });
        }

        Self {
            root,
            picture,
            viewport,
            photos,
            index,
            zoom,
            one_to_one,
            generation,
            latest_generation,
            jobs: job_tx,
            refresh,
            photo_changed,
            one_to_one_changed,
        }
    }

    pub fn open(&self, photos: Vec<PhotoObject>, selected_index: usize) {
        if photos.is_empty() {
            return;
        }
        let index = selected_index.min(photos.len() - 1);
        self.photos.replace(photos);
        self.index.set(index);
        self.zoom.set(1.0);
        self.one_to_one.set(false);
        self.picture.set_paintable(gtk::gdk::Paintable::NONE);
        self.root.set_visible(true);
        self.root.grab_focus();
        if let Some(photo) = self.photos.borrow().get(index).cloned() {
            if let Some(handler) = self.photo_changed.borrow().as_ref() {
                handler(photo);
            }
        }
        (self.refresh)();
    }

    pub fn close(&self) {
        let next = self.generation.get().wrapping_add(1);
        self.generation.set(next);
        self.latest_generation.store(next, Ordering::Release);
        self.root.set_visible(false);
    }

    pub fn set_one_to_one(&self, enabled: bool) {
        if self.one_to_one.get() == enabled {
            return;
        }
        self.one_to_one.set(enabled);
        if !enabled {
            self.zoom.set(1.0);
        }
        if let Some(handler) = self.one_to_one_changed.borrow().as_ref() {
            handler(enabled);
        }
        if self.root.is_visible() {
            (self.refresh)();
        }
    }

    pub fn set_photo_changed_handler(&self, handler: impl Fn(PhotoObject) + 'static) {
        self.photo_changed.replace(Some(Box::new(handler)));
    }

    pub fn set_one_to_one_changed_handler(&self, handler: impl Fn(bool) + 'static) {
        self.one_to_one_changed.replace(Some(Box::new(handler)));
    }

    pub fn refresh_current(&self) {
        if self.root.is_visible() {
            (self.refresh)();
        }
    }

    pub fn current_photo(&self) -> Option<PhotoObject> {
        self.photos.borrow().get(self.index.get()).cloned()
    }
}

fn center_viewport(viewport: &gtk::ScrolledWindow) {
    let hadj = viewport.hadjustment();
    let vadj = viewport.vadjustment();
    let max_h = (hadj.upper() - hadj.page_size()).max(hadj.lower());
    let max_v = (vadj.upper() - vadj.page_size()).max(vadj.lower());
    hadj.set_value((max_h / 2.0).clamp(hadj.lower(), max_h));
    vadj.set_value((max_v / 2.0).clamp(vadj.lower(), max_v));
}
