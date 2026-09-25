use glib::subclass::prelude::*;
use gtk4 as gtk;
use std::cell::{Cell, RefCell};

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct PhotoObject {
        pub path: RefCell<String>,
        pub favorite: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PhotoObject {
        const NAME: &'static str = "PicPrototypePhotoObject";
        type Type = super::PhotoObject;
    }

    impl ObjectImpl for PhotoObject {}
}

glib::wrapper! {
    pub struct PhotoObject(ObjectSubclass<imp::PhotoObject>);
}

impl PhotoObject {
    pub fn new(path: impl Into<String>, favorite: bool) -> Self {
        let object: Self = glib::Object::new();
        object.imp().path.replace(path.into());
        object.imp().favorite.set(favorite);
        object
    }

    pub fn path(&self) -> String {
        self.imp().path.borrow().clone()
    }

    pub fn favorite(&self) -> bool {
        self.imp().favorite.get()
    }

    pub fn set_favorite(&self, favorite: bool) {
        self.imp().favorite.set(favorite);
    }
}
