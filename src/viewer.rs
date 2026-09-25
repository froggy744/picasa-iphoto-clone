use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gio::prelude::*;
use gtk::prelude::*;
use gtk4 as gtk;

use crate::photo::PhotoObject;

pub struct Viewer {
    pub root: gtk::Overlay,
    picture: gtk::Picture,
    photos: Rc<RefCell<Vec<PhotoObject>>>,
    index: Rc<Cell<usize>>,
    one_to_one: Rc<Cell<bool>>,
    refresh: Rc<dyn Fn()>,
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
        picture.set_hexpand(true);
        picture.set_vexpand(true);
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
        let one_to_one = Rc::new(Cell::new(false));

        let refresh: Rc<dyn Fn()> = {
            let picture = picture.clone();
            let photos = photos.clone();
            let index = index.clone();
            let one_to_one = one_to_one.clone();
            let viewport = viewport.clone();
            Rc::new(move || {
                let Some(photo) = photos.borrow().get(index.get()).cloned() else {
                    picture.set_paintable(gtk::gdk::Paintable::NONE);
                    return;
                };
                let path = photo.path();
                let file = gio::File::for_path(&path);
                picture.set_file(Some(&file));
                picture.set_tooltip_text(Some(&path));

                if one_to_one.get() {
                    if let Ok((width, height)) = image::image_dimensions(&path) {
                        picture.set_size_request(
                            i32::try_from(width).unwrap_or(i32::MAX),
                            i32::try_from(height).unwrap_or(i32::MAX),
                        );
                    }
                    picture.set_can_shrink(false);
                    picture.set_hexpand(false);
                    picture.set_vexpand(false);
                } else {
                    picture.set_size_request(-1, -1);
                    picture.set_can_shrink(true);
                    picture.set_hexpand(true);
                    picture.set_vexpand(true);
                }
                viewport.hadjustment().set_value(0.0);
                viewport.vadjustment().set_value(0.0);
            })
        };

        {
            let root = root.clone();
            close.connect_clicked(move |_| root.set_visible(false));
        }

        {
            let photos = photos.clone();
            let index = index.clone();
            let refresh = refresh.clone();
            previous.connect_clicked(move |_| {
                if photos.borrow().is_empty() {
                    return;
                }
                let current = index.get();
                if current > 0 {
                    index.set(current - 1);
                    refresh();
                }
            });
        }

        {
            let photos = photos.clone();
            let index = index.clone();
            let refresh = refresh.clone();
            next.connect_clicked(move |_| {
                let len = photos.borrow().len();
                if len == 0 {
                    return;
                }
                let current = index.get();
                if current + 1 < len {
                    index.set(current + 1);
                    refresh();
                }
            });
        }

        let double_click = gtk::GestureClick::new();
        double_click.set_button(1);
        {
            let root = root.clone();
            double_click.connect_pressed(move |gesture, presses, _, _| {
                if presses == 2 {
                    root.set_visible(false);
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                }
            });
        }
        picture.add_controller(double_click);

        let scroll = gtk::EventControllerScroll::new(
            gtk::EventControllerScrollFlags::VERTICAL
                | gtk::EventControllerScrollFlags::DISCRETE,
        );
        {
            let photos = photos.clone();
            let index = index.clone();
            let refresh = refresh.clone();
            scroll.connect_scroll(move |_, _, dy| {
                let len = photos.borrow().len();
                if len == 0 || dy == 0.0 {
                    return glib::Propagation::Proceed;
                }
                let current = index.get();
                let target = if dy < 0.0 {
                    current.saturating_sub(1)
                } else {
                    (current + 1).min(len - 1)
                };
                if target != current {
                    index.set(target);
                    refresh();
                }
                glib::Propagation::Stop
            });
        }
        root.add_controller(scroll);

        let key = gtk::EventControllerKey::new();
        key.set_propagation_phase(gtk::PropagationPhase::Capture);
        {
            let root = root.clone();
            let photos = photos.clone();
            let index = index.clone();
            let one_to_one = one_to_one.clone();
            let refresh = refresh.clone();
            key.connect_key_pressed(move |_, key, _, _| {
                if key == gtk::gdk::Key::Escape {
                    root.set_visible(false);
                    return glib::Propagation::Stop;
                }
                if key == gtk::gdk::Key::space {
                    one_to_one.set(!one_to_one.get());
                    refresh();
                    return glib::Propagation::Stop;
                }

                let len = photos.borrow().len();
                if len == 0 {
                    return glib::Propagation::Proceed;
                }
                let current = index.get();
                let target = if key == gtk::gdk::Key::Left {
                    Some(current.saturating_sub(1))
                } else if key == gtk::gdk::Key::Right {
                    Some((current + 1).min(len - 1))
                } else {
                    None
                };
                if let Some(target) = target {
                    if target != current {
                        index.set(target);
                        refresh();
                    }
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            });
        }
        root.add_controller(key);

        Self {
            root,
            picture,
            photos,
            index,
            one_to_one,
            refresh,
        }
    }

    pub fn open(&self, photos: Vec<PhotoObject>, selected_index: usize) {
        if photos.is_empty() {
            return;
        }
        let index = selected_index.min(photos.len() - 1);
        self.photos.replace(photos);
        self.index.set(index);
        self.one_to_one.set(false);
        (self.refresh)();
        self.root.set_visible(true);
        self.root.grab_focus();
    }

    pub fn close(&self) {
        self.root.set_visible(false);
    }

    pub fn set_one_to_one(&self, enabled: bool) {
        self.one_to_one.set(enabled);
        if self.root.is_visible() {
            (self.refresh)();
        }
    }

    pub fn one_to_one(&self) -> bool {
        self.one_to_one.get()
    }

    pub fn current_photo(&self) -> Option<PhotoObject> {
        self.photos.borrow().get(self.index.get()).cloned()
    }
}
