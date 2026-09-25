use std::collections::BTreeMap;

use gio::prelude::*;
use gtk4 as gtk;

use crate::db::Photo;
use crate::photo_object::PhotoObject;

#[derive(Clone)]
pub(crate) struct FolderSection {
    pub folder_id: i64,
    pub folder_path: String,
    pub label: String,
    pub photos: gio::ListStore,
}

pub(crate) struct GalleryV2Model {
    all_photos: gio::ListStore,
    folder_sections: Vec<FolderSection>,
}

impl GalleryV2Model {
    pub(crate) fn from_photos(photos: &[Photo]) -> Self {
        let all_photos = gio::ListStore::new::<PhotoObject>();
        let mut by_folder = BTreeMap::<(i64, String), Vec<PhotoObject>>::new();

        for photo in photos {
            let object = PhotoObject::from_photo(photo);
            all_photos.append(&object);

            let folder_id = photo.folder_id.unwrap_or_default();
            let folder_path = photo.folder_path.clone().unwrap_or_default();
            by_folder
                .entry((folder_id, folder_path))
                .or_default()
                .push(object);
        }

        let folder_sections = by_folder
            .into_iter()
            .map(|((folder_id, folder_path), objects)| {
                let photos = gio::ListStore::new::<PhotoObject>();
                for photo in objects {
                    photos.append(&photo);
                }

                let label = if folder_path.is_empty() {
                    "Photos".to_string()
                } else {
                    crate::source::filename(&folder_path)
                };

                FolderSection {
                    folder_id,
                    folder_path,
                    label,
                    photos,
                }
            })
            .collect();

        Self {
            all_photos,
            folder_sections,
        }
    }

    pub(crate) fn all_photos(&self) -> gio::ListStore {
        self.all_photos.clone()
    }

    pub(crate) fn folder_sections(&self) -> &[FolderSection] {
        &self.folder_sections
    }

    pub(crate) fn len(&self) -> u32 {
        self.all_photos.n_items()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.all_photos.n_items() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn photo(id: i64, folder_id: i64, folder_path: &str, path: &str) -> Photo {
        Photo {
            id,
            path: path.into(),
            folder_id: Some(folder_id),
            folder_path: Some(folder_path.into()),
            taken_at: None,
            camera: None,
            width: Some(16),
            height: Some(16),
            size_bytes: Some(1),
            mtime: Some(1),
            added_at: id,
            rotation: 0,
            edit_recipe: String::new(),
            favorite: false,
            trashed: false,
            history_caption: None,
            edited_at: 0,
        }
    }

    #[test]
    fn gallery_v2_keeps_one_persistent_photo_object_per_input_photo() {
        let model = GalleryV2Model::from_photos(&[
            photo(1, 10, "/photos/a", "/photos/a/1.jpg"),
            photo(2, 10, "/photos/a", "/photos/a/2.jpg"),
            photo(3, 20, "/photos/b", "/photos/b/3.jpg"),
        ]);

        assert_eq!(model.len(), 3);
        assert_eq!(model.folder_sections().len(), 2);
        assert_eq!(model.folder_sections()[0].photos.n_items(), 2);
        assert_eq!(model.folder_sections()[1].photos.n_items(), 1);

        let all_first = model
            .all_photos()
            .item(0)
            .and_downcast::<PhotoObject>()
            .unwrap();
        let folder_first = model.folder_sections()[0]
            .photos
            .item(0)
            .and_downcast::<PhotoObject>()
            .unwrap();

        assert_eq!(all_first.id(), folder_first.id());
        assert_eq!(all_first, folder_first);
    }

    #[test]
    fn gallery_v2_grouping_is_independent_of_zoom_geometry() {
        let model = GalleryV2Model::from_photos(&[
            photo(1, 10, "/photos/a", "/photos/a/1.jpg"),
            photo(2, 10, "/photos/a", "/photos/a/2.jpg"),
            photo(3, 10, "/photos/a", "/photos/a/3.jpg"),
        ]);

        // Geometry never appears in the model API. Changing columns belongs to
        // the view only, so the section membership remains stable by design.
        let ids = (0..model.folder_sections()[0].photos.n_items())
            .filter_map(|position| {
                model.folder_sections()[0]
                    .photos
                    .item(position)
                    .and_downcast::<PhotoObject>()
                    .map(|photo| photo.id())
            })
            .collect::<Vec<_>>();

        assert_eq!(ids, vec![1, 2, 3]);
    }
}
