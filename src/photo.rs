use glib::subclass::prelude::*;
use gtk4 as gtk;
use std::cell::{Cell, RefCell};

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct PhotoObject {
        pub id: Cell<i64>,
        pub path: RefCell<String>,
        pub favorite: Cell<bool>,
        pub rotation: Cell<i32>,
        pub mtime: Cell<i64>,
        pub size_bytes: Cell<i64>,
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
    pub fn new(
        id: i64,
        path: impl Into<String>,
        favorite: bool,
        rotation: i32,
        mtime: Option<i64>,
        size_bytes: Option<i64>,
    ) -> Self {
        let object: Self = glib::Object::new();
        object.imp().id.set(id);
        object.imp().path.replace(path.into());
        object.imp().favorite.set(favorite);
        object.imp().rotation.set(rotation.rem_euclid(360));
        object.imp().mtime.set(mtime.unwrap_or_default());
        object.imp().size_bytes.set(size_bytes.unwrap_or_default());
        object
    }

    pub fn id(&self) -> i64 {
        self.imp().id.get()
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

    pub fn rotation(&self) -> i32 {
        self.imp().rotation.get()
    }

    pub fn set_rotation(&self, rotation: i32) {
        self.imp().rotation.set(rotation.rem_euclid(360));
    }

    pub fn mtime(&self) -> i64 {
        self.imp().mtime.get()
    }

    pub fn size_bytes(&self) -> i64 {
        self.imp().size_bytes.get()
    }
}
