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
        pub edit_recipe: RefCell<String>,
        #[property(get, set)]
        pub favorite: Cell<bool>,
        #[property(get, set)]
        pub folder_id: Cell<i64>,
        #[property(get, set)]
        pub folder_path: RefCell<Option<String>>,
        #[property(get, set)]
        pub original_available: Cell<bool>,
        // When the original was last probed for availability. Not a GObject
        // property; used to re-probe on rebind after a TTL so a drive that goes
        // offline without a mount event still gets its offline badge.
        pub original_checked_at: Cell<Option<std::time::Instant>>,
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
    /// When the original was last probed, if ever.
    pub fn original_checked_at(&self) -> Option<std::time::Instant> {
        self.imp().original_checked_at.get()
    }

    pub fn set_original_checked_at(&self, value: Option<std::time::Instant>) {
        self.imp().original_checked_at.set(value);
    }

    pub fn from_photo(photo: &Photo) -> Self {
        let object: Self = glib::Object::new();
        object.set_from_photo(photo);
        object
    }

    /// Populate a fresh object directly rather than through the GObject property
    /// system. Constructing a library-sized model sets ~15 properties for each
    /// of tens of thousands of photos; going through `set_property` (ParamSpec
    /// lookup, `Value` boxing, notifications) dominated folder-open time. No
    /// consumer connects to these properties' notify signals, so writing the
    /// subclass fields directly is equivalent and several times faster.
    pub fn set_from_photo(&self, photo: &Photo) {
        let imp = self.imp();
        imp.id.set(photo.id);
        *imp.path.borrow_mut() = photo.path.clone();
        *imp.filename.borrow_mut() = crate::source::filename(&photo.path);
        *imp.taken_at.borrow_mut() = photo.taken_at.clone();
        *imp.camera.borrow_mut() = photo.camera.clone();
        imp.width.set(photo.width.unwrap_or_default());
        imp.height.set(photo.height.unwrap_or_default());
        imp.size_bytes.set(photo.size_bytes.unwrap_or_default());
        imp.mtime.set(photo.mtime.unwrap_or_default());
        imp.rotation.set(photo.rotation);
        *imp.edit_recipe.borrow_mut() = photo.edit_recipe.clone();
        imp.favorite.set(photo.favorite);
        imp.folder_id.set(photo.folder_id.unwrap_or_default());
        *imp.folder_path.borrow_mut() = photo.folder_path.clone();
        // Availability is probed asynchronously when a tile is bound, and
        // re-probed after a TTL. Probing every original while constructing a
        // library-sized model, or synchronously while scrolling, blocks GTK.
        imp.original_available.set(true);
        *imp.cached_thumbnail_path.borrow_mut() = thumbnail::cache_path(
            &photo.path,
            photo.mtime,
            photo.size_bytes,
        )
        .ok()
        .map(|path| path.to_string_lossy().into_owned());
        // The visible tile performs this inexpensive cache check lazily.
        imp.thumbnail_available.set(false);
        imp.original_checked_at.set(None);
    }
}
