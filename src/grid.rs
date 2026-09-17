use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;
use std::time::Instant;

use chrono::{Local, TimeZone};

use gio::prelude::*;
use glib::subclass::prelude::*;
use gtk::prelude::*;
use gtk4 as gtk;

use crate::db::{Folder, Photo};
use crate::photo_object::PhotoObject;

// EDIT THESE TWO VALUES to set your default thumbnail width and height.
// They are independent. Example: 180 x 120, 200 x 140, 220 x 160.
const DEFAULT_TILE_WIDTH: i32 = 180;
const DEFAULT_TILE_HEIGHT: i32 = 120;

// Existing +/- zoom remains enabled. Width changes by this step and height
// scales by the same factor, preserving the custom shape above.
const MIN_TILE_WIDTH: i32 = 100;
const MAX_TILE_WIDTH: i32 = 300;
const ZOOM_STEP_WIDTH: i32 = 24;

// Pointer travel (px) required before a folder press becomes a rubberband drag
// instead of a click. Keeps double-click detection intact.
const DRAG_CLAIM_THRESHOLD: f64 = 6.0;

const RAW_THUMBNAIL_CACHE_CAPACITY: usize = 256;
// Folder scrolling must never decode or stat thumbnails from the ListView
// bind callback. Keep a modest RAM LRU of paintables that were loaded by the
// viewport loaders so recycled rows can still show an instant thumbnail while
// the user scrolls through nearby photos. Motion prefetch fills this cache off
// the GTK thread; the ListView bind itself remains RAM-only.
const FOLDER_THUMBNAIL_CACHE_CAPACITY: usize = 512;
thread_local! {
    static RAW_THUMBNAIL_CACHE: RefCell<VecDeque<(String, i32, String, gtk::gdk::Paintable)>> =
        const { RefCell::new(VecDeque::new()) };
    static FOLDER_THUMBNAIL_CACHE: RefCell<VecDeque<(String, gtk::gdk::Paintable)>> =
        const { RefCell::new(VecDeque::new()) };
    static GRID_SCRUB_ACTIVE: Cell<bool> = const { Cell::new(false) };
}

// The grid stays one Rust module for private-state compatibility, while its
// implementation is split into focused source files for maintenance.
include!("grid/tile.rs");
include!("grid/grouping.rs");
include!("grid/virtualization.rs");
include!("grid/view.rs");
include!("grid/navigation.rs");
include!("grid/selection.rs");

#[cfg(test)]
mod folder_stream_tests {
    use super::{
        folder_chunk_size, folder_dragged_positions, folder_line_height, folder_section_plan,
        folder_selection_after_click, folder_virtual_row_matches, folder_virtual_rows,
        FolderCatalogEntry, FolderRowData, FolderRowKind, FolderTileBounds, FolderVirtualRow,
        GroupRange,
    };

    #[test]
    #[ignore = "requires a GTK display; run with --ignored --test-threads=1"]
    fn sidebar_width_changes_do_not_scroll_or_lose_the_visible_photo() {
        use super::{gtk, Gallery, GroupMode, PhotoObject};
        use gtk::prelude::*;

        fn settle() {
            let context = glib::MainContext::default();
            for _ in 0..30 {
                while context.pending() {
                    context.iteration(false);
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }

        gtk::init().unwrap();
        let gallery = Gallery::new(
            &[],
            148,
            |_| {},
            |_, _| {},
            |_, _, _, _| {},
            |_, _| {},
            |_| {},
        );
        gallery.group_mode.set(GroupMode::Folder);
        gallery.current_photos.replace(
            (1..=1200_i64)
                .map(|id| {
                    glib::Object::builder::<PhotoObject>()
                        .property("id", id)
                        .property("folder-id", 1_i64)
                        .property("original-available", true)
                        .build()
                })
                .collect(),
        );
        gallery.rebuild_group_ranges();
        gallery.update_width(1124);
        let scroll = gtk::ScrolledWindow::builder()
            .child(&gallery.folder_root)
            .build();
        let window = gtk::Window::builder()
            .default_width(1124)
            .default_height(600)
            .child(&scroll)
            .build();
        window.present();
        settle();
        scroll.vadjustment().set_value(8000.0);
        settle();

        // Six columns put the centre probe between tiles. The anchor must be
        // a visible photo, never the first photo of this long folder.
        let before = gallery.photo_for_visible_folder_row().unwrap().id();
        assert!(before > 100, "deep viewport anchored to photo {before}");
        let y = scroll.vadjustment().value();
        gallery.zoom_anchor.set(Some(before));
        for width in [1135, 1157, 1184, 1217, 1246, 1273, 1246, 1124] {
            gallery.update_width(width);
            settle();
            assert_eq!(
                scroll.vadjustment().value(),
                y,
                "sidebar width {width} scrolled"
            );
            assert_eq!(
                gallery.zoom_anchor.get(),
                Some(before),
                "width-only resize consumed zoom anchor"
            );
            assert_eq!(gallery.photo_for_visible_folder_row().unwrap().id(), before);
        }

        gallery.zoom_anchor.set(None);
        for width in [1440, 1124].into_iter().cycle().take(8) {
            gallery.update_width(width);
            window.set_default_size(width, 600);
            settle();
            let after = gallery.photo_for_visible_folder_row().unwrap().id();
            assert_eq!(after, before, "column change lost anchor at width {width}");
        }
        gallery.zoom_anchor.set(Some(before));

        // An actual zoom within the same columns still updates row geometry
        // and consumes its saved anchor, even when the width is unchanged.
        gallery.tile_height.set(gallery.tile_height.get() + 10);
        gallery.update_layout(1124, true);
        assert_eq!(gallery.zoom_anchor.get(), None);
        settle();
        let after = gallery.photo_for_visible_folder_row().unwrap().id();
        assert_eq!(after, before, "zoom lost anchor");

        // Ordinary scrolling must move the logical anchor too; retaining the
        // resize anchor must never pin the viewport to an old photo.
        scroll.vadjustment().set_value(16000.0);
        settle();
        let scrolled = gallery.photo_for_visible_folder_row().unwrap().id();
        assert_ne!(scrolled, before);
        gallery.update_width(1440);
        window.set_default_size(1440, 600);
        settle();
        assert_eq!(
            gallery.photo_for_visible_folder_row().unwrap().id(),
            scrolled
        );

        // A second column change can arrive before GTK has allocated the first
        // rebuild. Its adjustment may transiently reset to zero (as in the
        // reported 7→6→5 transition). Never capture that transient viewport.
        gallery.update_width(1124);
        scroll.vadjustment().set_value(0.0);
        gallery.update_width(970);
        window.set_default_size(970, 600);
        settle();
        assert_eq!(
            gallery.photo_for_visible_folder_row().unwrap().id(),
            scrolled,
            "overlapping column changes lost the original anchor"
        );
        window.close();
    }

    #[test]
    fn exact_folder_row_offset_uses_header_and_photo_heights() {
        let rows = vec![
            FolderVirtualRow {
                kind: FolderRowKind::Header,
                start: 0,
                end: 0,
            },
            FolderVirtualRow {
                kind: FolderRowKind::Photos,
                start: 0,
                end: 5,
            },
            FolderVirtualRow {
                kind: FolderRowKind::Photos,
                start: 5,
                end: 10,
            },
            FolderVirtualRow {
                kind: FolderRowKind::Header,
                start: 10,
                end: 10,
            },
            FolderVirtualRow {
                kind: FolderRowKind::Photos,
                start: 10,
                end: 15,
            },
        ];

        // At tile height 88 photo rows are 100 px while headers stay at 70 px.
        // Before row 4 there are two headers and two photo rows: 70+100+100+70.
        assert_eq!(super::folder_row_offset(&rows, 4, 88), 340.0);
    }

    #[test]
    fn folder_model_rows_use_compact_headers_and_zoomed_photo_lines() {
        assert_eq!(
            super::folder_model_row_height(FolderRowKind::Header, 88),
            super::FOLDER_HEADER_HEIGHT
        );
        assert_eq!(super::folder_model_row_height(FolderRowKind::Photos, 88), 100);
        assert_eq!(
            super::folder_model_row_height(FolderRowKind::Header, 155),
            super::FOLDER_HEADER_HEIGHT
        );
        assert_eq!(super::folder_model_row_height(FolderRowKind::Photos, 155), 167);
    }

    fn sample_ranges() -> Vec<GroupRange> {
        vec![
            GroupRange {
                start: 0,
                end: 5,
                label: "Pictures".to_string(),
                folder_id: 10,
            },
            GroupRange {
                start: 5,
                end: 18,
                label: "Drone".to_string(),
                folder_id: 11,
            },
        ]
    }

    #[test]
    fn parent_folder_with_recursive_photos_is_navigation_only() {
        let ranges = vec![GroupRange {
            start: 0,
            end: 5,
            label: "Drone Building".to_string(),
            folder_id: 274,
        }];
        let catalog = vec![
            FolderCatalogEntry {
                folder_id: 11,
                photo_count: 5,
            },
            FolderCatalogEntry {
                folder_id: 274,
                photo_count: 5,
            },
        ];

        let plan = folder_section_plan(&ranges, &catalog, &[11, 274]);
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].folder_id, 274);
        assert_eq!(plan[0].range_index, Some(0));
    }

    #[test]
    fn filesystem_ancestors_do_not_add_gallery_headers() {
        let ranges = vec![GroupRange {
            start: 0,
            end: 5,
            label: "Drone Building".to_string(),
            folder_id: 274,
        }];
        let catalog = vec![
            FolderCatalogEntry {
                folder_id: 1,
                photo_count: 5,
            },
            FolderCatalogEntry {
                folder_id: 11,
                photo_count: 5,
            },
            FolderCatalogEntry {
                folder_id: 274,
                photo_count: 5,
            },
        ];

        let plan = folder_section_plan(&ranges, &catalog, &[1, 11, 274]);
        assert_eq!(
            plan.iter()
                .map(|section| section.folder_id)
                .collect::<Vec<_>>(),
            vec![274]
        );
    }

    #[test]
    fn empty_parent_folder_does_not_add_blank_gallery_section() {
        let ranges = sample_ranges();
        let catalog = vec![FolderCatalogEntry {
            folder_id: 99,
            photo_count: 0,
        }];
        let plan = folder_section_plan(&ranges, &catalog, &[99, 10, 11]);
        assert_eq!(
            plan.iter()
                .map(|section| section.folder_id)
                .collect::<Vec<_>>(),
            vec![10, 11]
        );
    }

    #[test]
    fn folder_virtual_stream_uses_one_fixed_photo_line_per_column_width() {
        let rows = folder_virtual_rows(&sample_ranges(), 3);
        assert_eq!(rows.len(), 9);
        assert_eq!(rows[0].kind, FolderRowKind::Header);
        assert_eq!((rows[1].start, rows[1].end), (0, 3));
        assert_eq!((rows[2].start, rows[2].end), (3, 5));
        assert_eq!(rows[3].kind, FolderRowKind::Header);
        assert_eq!((rows[4].start, rows[4].end), (5, 8));
        assert_eq!((rows[5].start, rows[5].end), (8, 11));
        assert_eq!((rows[6].start, rows[6].end), (11, 14));
        assert_eq!((rows[7].start, rows[7].end), (14, 17));
        assert_eq!((rows[8].start, rows[8].end), (17, 18));
    }

    #[test]
    fn folder_chunk_size_equals_columns() {
        for columns in 1..=8u32 {
            assert_eq!(folder_chunk_size(columns), columns as usize);
        }
    }

    #[test]
    fn folder_photo_line_height_is_fixed() {
        assert_eq!(folder_line_height(100), 112);
        assert_eq!(folder_line_height(163), 175);
    }

    #[test]
    fn folder_virtual_photo_lines_cover_each_range_without_gaps() {
        for columns in 1..=8u32 {
            let rows = folder_virtual_rows(&sample_ranges(), folder_chunk_size(columns));
            for range in sample_ranges() {
                let covered = rows
                    .iter()
                    .filter(|row| row.kind == FolderRowKind::Photos)
                    .filter(|row| row.start >= range.start && row.end <= range.end)
                    .flat_map(|row| row.start..row.end)
                    .collect::<Vec<_>>();
                assert_eq!(covered, (range.start..range.end).collect::<Vec<_>>());
                assert!(rows
                    .iter()
                    .filter(|row| row.kind == FolderRowKind::Photos)
                    .filter(|row| row.start >= range.start && row.end <= range.end)
                    .all(|row| row.end - row.start <= columns as usize));
            }
        }
    }

    #[test]
    fn folder_click_selection_matches_grid_semantics() {
        assert_eq!(
            folder_selection_after_click(8, &[], None, 2, false, false),
            vec![2]
        );
        assert_eq!(
            folder_selection_after_click(8, &[2], Some(2), 5, true, false),
            vec![2, 5]
        );
        assert_eq!(
            folder_selection_after_click(8, &[2], Some(2), 5, false, true),
            vec![2, 3, 4, 5]
        );
        assert_eq!(
            folder_selection_after_click(8, &[1, 3], None, 5, false, true),
            vec![5]
        );
        assert_eq!(
            folder_selection_after_click(0, &[], None, 5, false, false),
            Vec::<u32>::new()
        );
    }

    #[test]
    fn folder_drag_selection_returns_intersecting_tiles_in_model_order() {
        let tiles = vec![
            FolderTileBounds {
                position: 0,
                x: 0.0,
                y: 0.0,
                width: 100.0,
                height: 100.0,
            },
            FolderTileBounds {
                position: 1,
                x: 110.0,
                y: 0.0,
                width: 100.0,
                height: 100.0,
            },
            FolderTileBounds {
                position: 2,
                x: 0.0,
                y: 110.0,
                width: 100.0,
                height: 100.0,
            },
        ];
        assert_eq!(
            folder_dragged_positions(&tiles, (50.0, 50.0), (160.0, 160.0)),
            vec![0, 1, 2]
        );
        assert_eq!(
            folder_dragged_positions(&tiles, (250.0, 250.0), (300.0, 300.0)),
            Vec::<u32>::new()
        );
        assert_eq!(
            folder_dragged_positions(&tiles, (160.0, 0.0), (50.0, 50.0)),
            vec![0, 1]
        );
    }

    #[test]
    fn folder_drag_rectangle_handles_reverse_pointer_direction() {
        assert_eq!(
            super::folder_drag_rectangle((120.0, 90.0), (20.0, 10.0)),
            (20.0, 10.0, 100.0, 80.0)
        );
    }

    #[test]
    fn folder_section_plan_changes_presentation_without_changing_source_ranges() {
        let original = sample_ranges();
        let plan = folder_section_plan(&original, &[], &[11, 10]);

        assert_eq!(
            plan.iter()
                .map(|section| section.folder_id)
                .collect::<Vec<_>>(),
            vec![11, 10]
        );
        assert_eq!(plan[0].range_index, Some(1));
        assert_eq!(plan[1].range_index, Some(0));
        assert_eq!(
            original
                .iter()
                .map(|range| range.folder_id)
                .collect::<Vec<_>>(),
            vec![10, 11]
        );
    }

    #[test]
    fn folder_virtual_row_match_compares_identity_fields() {
        let base = FolderRowData {
            kind: FolderRowKind::Header,
            folder_id: 7,
            folder_path: "/photos".to_string(),
            label: "photos".to_string(),
            count: 3,
            start: 0,
            end: 0,
            photo_ids: Vec::new(),
        };
        assert!(folder_virtual_row_matches(&base, &base.clone()));

        let mut different_folder = base.clone();
        different_folder.folder_id = 8;
        assert!(!folder_virtual_row_matches(&base, &different_folder));

        let mut different_count = base.clone();
        different_count.count = 4;
        assert!(!folder_virtual_row_matches(&base, &different_count));

        let mut different_photos = base.clone();
        different_photos.kind = FolderRowKind::Photos;
        different_photos.start = 0;
        different_photos.end = 2;
        different_photos.photo_ids = vec![10, 11];
        let mut same_geometry_new_photos = different_photos.clone();
        same_geometry_new_photos.photo_ids = vec![10, 12];
        assert!(!folder_virtual_row_matches(
            &different_photos,
            &same_geometry_new_photos
        ));
    }
}
