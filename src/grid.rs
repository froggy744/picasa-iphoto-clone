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

#[cfg(test)]
#[path = "grid/history_tests.rs"]
mod history_tests;

// EDIT THESE TWO VALUES to set your default thumbnail width and height.
// They are independent. Example: 180 x 120, 200 x 140, 220 x 160.
const DEFAULT_TILE_WIDTH: i32 = 180;
const DEFAULT_TILE_HEIGHT: i32 = 120;

// Existing +/- zoom remains enabled. Height scales by the same factor as the
// width change, preserving the custom shape above.
const MIN_TILE_WIDTH: i32 = 100;
const MAX_TILE_WIDTH: i32 = 300;

// Perceptually uniform zoom ladder. Every level is ~17% wider than the one
// before (a fixed pixel step feels huge on small thumbnails and invisible on
// large ones; ratio steps feel equal at every size). The first and last
// levels are pinned to MIN/MAX_TILE_WIDTH. All zoom input snaps to these
// levels so +/-, Ctrl+wheel and Reset always land on the same canonical
// sizes instead of drifting with the starting width.
pub(crate) const ZOOM_LEVELS: [i32; 8] = [100, 117, 137, 160, 187, 219, 256, 300];

pub(crate) fn nearest_zoom_level(width: i32) -> i32 {
    let mut best = ZOOM_LEVELS[0];
    for level in ZOOM_LEVELS {
        if (level - width).abs() < (best - width).abs() {
            best = level;
        }
    }
    best
}

fn zoom_level_index(level: i32) -> usize {
    ZOOM_LEVELS
        .iter()
        .position(|&candidate| candidate == level)
        .unwrap_or(0)
}

pub(crate) fn next_zoom_level(width: i32) -> i32 {
    let index = zoom_level_index(nearest_zoom_level(width));
    ZOOM_LEVELS[(index + 1).min(ZOOM_LEVELS.len() - 1)]
}

pub(crate) fn prev_zoom_level(width: i32) -> i32 {
    let index = zoom_level_index(nearest_zoom_level(width));
    ZOOM_LEVELS[index.saturating_sub(1)]
}

/// Tile width that shows ~4 thumbnails per row at `surface_width`, snapped to
/// the largest ladder level that still fits 4 columns. Mirrors the column
/// math of `columns_for_width` (48px gutters, +30px per-tile allowance).
pub(crate) fn zoom_level_for_four_columns(surface_width: i32) -> i32 {
    let available = (surface_width - 48).max(200);
    let ideal = (available as f64 / 4.0 - 30.0).round() as i32;
    let ideal = ideal.clamp(MIN_TILE_WIDTH, MAX_TILE_WIDTH);
    ZOOM_LEVELS
        .iter()
        .rev()
        .copied()
        .find(|&level| level <= ideal)
        .unwrap_or(MIN_TILE_WIDTH)
}

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
mod zoom_ladder_tests {
    use super::{
        nearest_zoom_level, next_zoom_level, prev_zoom_level, zoom_level_for_four_columns,
        ZOOM_LEVELS,
    };

    #[test]
    fn ladder_steps_feel_uniform() {
        for pair in ZOOM_LEVELS.windows(2) {
            let ratio = pair[1] as f64 / pair[0] as f64;
            assert!(
                (1.15..=1.20).contains(&ratio),
                "step {pair:?} ratio {ratio} outside natural range"
            );
        }
    }

    #[test]
    fn stepping_from_legacy_defaults_stays_on_ladder() {
        // The old fixed default 136 and the old initial width 180 sit between
        // levels; stepping must snap onto the ladder, never reproduce them.
        assert_eq!(next_zoom_level(136), 160);
        assert_eq!(prev_zoom_level(136), 117);
        assert_eq!(next_zoom_level(180), 219);
        assert_eq!(prev_zoom_level(180), 160);
        // Ends clamp.
        assert_eq!(next_zoom_level(300), 300);
        assert_eq!(prev_zoom_level(100), 100);
        assert_eq!(nearest_zoom_level(136), 137);
    }

    #[test]
    fn four_column_default_fits_narrow_and_clamps_wide() {
        // 1080p with the default sidebar open (surface ~1498): exactly the
        // 4-thumbnails-per-row view at the ladder maximum.
        assert_eq!(zoom_level_for_four_columns(1498), 300);
        // 1366x768 laptop (surface ~1066): 4 columns at a smaller level.
        assert_eq!(zoom_level_for_four_columns(1066), 219);
        // Very wide surfaces clamp to the ladder maximum; the column count
        // then grows with width instead of the tile size.
        assert_eq!(zoom_level_for_four_columns(2560), 300);
        // Tiny surfaces clamp to the minimum.
        assert_eq!(zoom_level_for_four_columns(320), 100);
    }
}
