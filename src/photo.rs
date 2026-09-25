use glib::subclass::prelude::*;
use gtk4 as gtk;
use std::cell::RefCell;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct PhotoObject {
        pub path: RefCell<String>,
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
    pub fn new(path: impl Into<String>) -> Self {
        let object: Self = glib::Object::new();
        object.imp().path.replace(path.into());
        object
    }

    pub fn path(&self) -> String {
        self.imp().path.borrow().clone()
    }
}
