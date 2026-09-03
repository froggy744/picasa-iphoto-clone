use std::cell::{Cell, RefCell};

use glib::prelude::ObjectExt;
use glib::subclass::prelude::*;
use glib::Properties;

use crate::db::Photo;
use crate::thumbnail;

mod imp {
    use super::*;

    #[derive(Default, Properties)]
    #[properties(wrapper_type = super::PhotoObject)]
    pub struct PhotoObject {
        #[property(get, set)]
        pub id: Cell<i64>,
        #[property(get, set)]
        pub path: RefCell<String>,
        #[property(get, set)]
        pub filename: RefCell<String>,
        #[property(get, set)]
        pub taken_at: RefCell<Option<String>>,
        #[property(get, set)]
        pub camera: RefCell<Option<String>>,
        #[property(get, set)]
        pub width: Cell<i64>,
        #[property(get, set)]
        pub height: Cell<i64>,
        #[property(get, set)]
        pub size_bytes: Cell<i64>,
        #[property(get, set)]
        pub mtime: Cell<i64>,
        #[property(get, set)]
        pub rotation: Cell<i32>,
        #[property(get, set)]
        pub favorite: Cell<bool>,
        #[property(get, set)]
        pub folder_id: Cell<i64>,
        #[property(get, set)]
        pub folder_path: RefCell<Option<String>>,
        #[property(get, set)]
        pub original_available: Cell<bool>,
        #[property(get, set)]
        pub cached_thumbnail_path: RefCell<Option<String>>,
        #[property(get, set)]
        pub thumbnail_available: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PhotoObject {
        const NAME: &'static str = "PicasaPhotoObject";
        type Type = super::PhotoObject;
    }

    impl ObjectImpl for PhotoObject {
        fn properties() -> &'static [glib::ParamSpec] {
            Self::derived_properties()
        }

        fn set_property(&self, id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            self.derived_set_property(id, value, pspec);
        }

        fn property(&self, id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            self.derived_property(id, pspec)
        }
    }
}

glib::wrapper! {
    pub struct PhotoObject(ObjectSubclass<imp::PhotoObject>);
}

impl PhotoObject {
    pub fn from_photo(photo: &Photo) -> Self {
        let cached_thumbnail_path =
            thumbnail::cache_path(&photo.path, photo.mtime, photo.size_bytes)
                .ok()
                .map(|path| path.to_string_lossy().into_owned());
        let object = glib::Object::builder::<Self>()
            .property("id", photo.id)
            .property("path", &photo.path)
            .property("filename", crate::source::filename(&photo.path))
            .property("taken-at", photo.taken_at.clone())
            .property("camera", photo.camera.clone())
            .property("width", photo.width.unwrap_or_default())
            .property("height", photo.height.unwrap_or_default())
            .property("size-bytes", photo.size_bytes.unwrap_or_default())
            .property("mtime", photo.mtime.unwrap_or_default())
            .property("rotation", photo.rotation)
            .property("favorite", photo.favorite)
            .property("folder-id", photo.folder_id.unwrap_or_default())
            .property("folder-path", photo.folder_path.clone())
            .property(
                "original-available",
                photo
                    .folder_path
                    .as_deref()
                    .map(crate::source::cached_source_available)
                    .unwrap_or(true)
                    && crate::source::cached_file_available(&photo.path),
            )
            .property("cached-thumbnail-path", cached_thumbnail_path.clone())
            .property(
                "thumbnail-available",
                cached_thumbnail_path
                    .as_deref()
                    .map(|path| std::path::Path::new(path).is_file())
                    .unwrap_or(false),
            )
            .build();
        object
    }
}
