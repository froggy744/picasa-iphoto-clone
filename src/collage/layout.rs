use super::model::{CollageProject, LayoutKind};

pub fn apply(project: &mut CollageProject) {
    match project.layout {
        LayoutKind::Grid => grid(project),
        LayoutKind::Mosaic => mosaic(project),
        LayoutKind::SmartMosaic => super::smart_mosaic::apply(project),
    }
}

fn grid(project: &mut CollageProject) {
    let count = project.items.len().max(1);
    let columns = (count as f32).sqrt().ceil() as usize;
    let rows = (count + columns - 1) / columns;
    let gap = project.spacing.clamp(0.0, 0.12);
    let width = (1.0 - gap * (columns + 1) as f32) / columns as f32;
    let height = (1.0 - gap * (rows + 1) as f32) / rows as f32;
    for (index, item) in project.items.iter_mut().enumerate() {
        let column = index % columns;
        let row = index / columns;
        item.x = gap + column as f32 * (width + gap);
        item.y = gap + row as f32 * (height + gap);
        item.width = width;
        item.height = height;
        item.rotation = 0.0;
        item.z = index;
    }
}

fn mosaic(project: &mut CollageProject) {
    let count = project.items.len();
    if count == 0 {
        return;
    }
    let gap = project.spacing.clamp(0.0, 0.08);
    let canvas_ratio = project.effective_aspect_ratio().max(0.01);
    // Work in physical canvas coordinates (width = aspect, height = 1.0),
    // then normalize the finished rectangles for the shared preview/export
    // model. Splitting a region consumes the gap at the split, so every leaf
    // is packed into one rectangle without overlap or cumulative drift.
    let horizontal_gap = gap * canvas_ratio;
    let indices = (0..count).collect::<Vec<_>>();
    split_mosaic_region(
        project,
        &indices,
        horizontal_gap,
        gap,
        (canvas_ratio - horizontal_gap * 2.0).max(0.01),
        (1.0 - gap * 2.0).max(0.01),
        canvas_ratio,
        gap,
    );
}

fn split_mosaic_region(
    project: &mut CollageProject,
    indices: &[usize],
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    canvas_ratio: f32,
    gap: f32,
) {
    if indices.len() == 1 {
        let item = &mut project.items[indices[0]];
        item.x = (x / canvas_ratio).clamp(0.0, 1.0);
        item.y = y.clamp(0.0, 1.0);
        item.width = (width / canvas_ratio).min((1.0 - item.x).max(0.0));
        item.height = height.min((1.0 - item.y).max(0.0));
        item.rotation = 0.0;
        item.z = indices[0];
        return;
    }

    let target = if indices.len() >= 4 { 0.58 } else { 0.5 };
    let mut best = (f32::INFINITY, true, 1, 0.5);
    for split_vertical in [true, false] {
        let total_weight = indices
            .iter()
            .map(|&index| mosaic_weight(&project.items[index], split_vertical))
            .sum::<f32>();
        let mut accumulated = 0.0;
        for split in 1..indices.len() {
            accumulated += mosaic_weight(&project.items[indices[split - 1]], split_vertical);
            let weighted_fraction = accumulated / total_weight.max(0.01);
            let first_fraction = mosaic_split_fraction(
                weighted_fraction,
                target,
                indices.len(),
                project.keep_photo_aspect,
            );
            let (first_width, first_height, second_width, second_height) = if split_vertical {
                let split_gap = mosaic_split_gap(width, height, true, canvas_ratio, gap);
                let available = width - split_gap;
                let first_width = available * first_fraction;
                (first_width, height, width - first_width - split_gap, height)
            } else {
                let split_gap = mosaic_split_gap(width, height, false, canvas_ratio, gap);
                let available = height - split_gap;
                let first_height = available * first_fraction;
                (
                    width,
                    first_height,
                    width,
                    height - first_height - split_gap,
                )
            };
            let crop_cost =
                region_crop_cost(project, &indices[..split], first_width / first_height)
                    + region_crop_cost(project, &indices[split..], second_width / second_height);
            let balance_cost = (first_fraction - target).abs() * 0.12;
            let orientation_cost = if (width / height >= 1.15) == split_vertical {
                0.0
            } else {
                0.04
            };
            let score = crop_cost + balance_cost + orientation_cost;
            if score < best.0 {
                best = (score, split_vertical, split, first_fraction);
            }
        }
    }
    let (_, split_vertical, best_split, first_fraction) = best;
    let (first, second) = indices.split_at(best_split);

    if split_vertical {
        let split_gap = mosaic_split_gap(width, height, true, canvas_ratio, gap);
        let available = width - split_gap;
        let first_width = available * first_fraction;
        let second_x = x + first_width + split_gap;
        split_mosaic_region(project, first, x, y, first_width, height, canvas_ratio, gap);
        split_mosaic_region(
            project,
            second,
            second_x,
            y,
            width - first_width - split_gap,
            height,
            canvas_ratio,
            gap,
        );
    } else {
        let split_gap = mosaic_split_gap(width, height, false, canvas_ratio, gap);
        let available = height - split_gap;
        let first_height = available * first_fraction;
        let second_y = y + first_height + split_gap;
        split_mosaic_region(project, first, x, y, width, first_height, canvas_ratio, gap);
        split_mosaic_region(
            project,
            second,
            x,
            second_y,
            width,
            height - first_height - split_gap,
            canvas_ratio,
            gap,
        );
    }
}

fn mosaic_split_gap(width: f32, height: f32, vertical: bool, canvas_ratio: f32, gap: f32) -> f32 {
    let requested = if vertical { gap * canvas_ratio } else { gap };
    let available = if vertical { width } else { height };
    requested.min(available * 0.2)
}

fn mosaic_weight(item: &super::model::CollageItem, vertical: bool) -> f32 {
    let aspect = item.photo.aspect_ratio.max(0.1);
    if vertical {
        aspect
    } else {
        1.0 / aspect
    }
}

fn mosaic_split_fraction(
    weighted_fraction: f32,
    target: f32,
    item_count: usize,
    keep_photo_aspect: bool,
) -> f32 {
    let feature_target = if item_count == 2 && !keep_photo_aspect {
        0.65
    } else {
        target
    };
    let bias = if keep_photo_aspect {
        0.08
    } else if item_count == 2 {
        0.65
    } else {
        0.25
    };
    (weighted_fraction * (1.0 - bias) + feature_target * bias).clamp(0.28, 0.72)
}

fn region_crop_cost(project: &CollageProject, indices: &[usize], tile_ratio: f32) -> f32 {
    indices
        .iter()
        .map(|&index| {
            let source_ratio = project.items[index].photo.aspect_ratio.max(0.1);
            let mismatch = (tile_ratio.max(0.01) / source_ratio).ln().abs();
            let weight = if project.keep_photo_aspect { 5.0 } else { 1.0 };
            let extreme_penalty = if project.keep_photo_aspect && mismatch > 1.2 {
                100.0
            } else {
                0.0
            };
            weight * (mismatch * mismatch + (mismatch - 0.8).max(0.0) * 2.0) + extreme_penalty
        })
        .sum()
}

pub(crate) fn next_unit(state: &mut u64) -> f32 {
    *state = state
        .wrapping_mul(2862933555777941757)
        .wrapping_add(3037000493);
    ((*state >> 11) as f64 / ((1u64 << 53) - 1) as f64) as f32
}

pub(super) fn mixed_seed(seed: u64, index: u64) -> u64 {
    let mut value = seed ^ index.wrapping_mul(0x9e3779b97f4a7c15);
    value ^= value >> 30;
    value = value.wrapping_mul(0xbf58476d1ce4e5b9);
    value ^= value >> 27;
    value = value.wrapping_mul(0x94d049bb133111eb);
    value ^ (value >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collage::model::{CollageItem, CollageOrientation, CollagePhoto};

    fn project(count: usize, layout: LayoutKind, seed: u64) -> CollageProject {
        CollageProject {
            aspect: crate::collage::model::AspectRatio::SixteenNine,
            custom_aspect: 16.0 / 9.0,
            orientation: crate::collage::model::CollageOrientation::Landscape,
            background: crate::collage::model::Background::White,
            round_corners: false,
            corner_radius: 0.06,
            layout,
            keep_photo_aspect: true,
            spacing: 0.02,
            seed,
            items: (0..count)
                .map(|id| CollageItem {
                    photo: CollagePhoto {
                        id: id as i64,
                        path: String::new(),
                        filename: String::new(),
                        thumbnail_path: None,
                        library_rotation: 0,
                        edit_recipe: String::new(),
                        aspect_ratio: 1.5,
                    },
                    x: 0.0,
                    y: 0.0,
                    width: 0.0,
                    height: 0.0,
                    rotation: 0.0,
                    z: id,
                })
                .collect(),
        }
    }

    fn inside(project: &CollageProject) -> bool {
        project.items.iter().all(|item| {
            item.x >= 0.0
                && item.y >= 0.0
                && item.width >= 0.0
                && item.height >= 0.0
                && item.x + item.width <= 1.0 + f32::EPSILON
                && item.y + item.height <= 1.0 + f32::EPSILON
        })
    }

    fn overlaps(left: &CollageItem, right: &CollageItem) -> bool {
        left.x < right.x + right.width
            && right.x < left.x + left.width
            && left.y < right.y + right.height
            && right.y < left.y + left.height
    }

    #[test]
    fn grid_items_stay_inside_canvas() {
        let mut project = project(17, LayoutKind::Grid, 1);
        project.relayout();
        assert!(inside(&project));
    }

    #[test]
    fn shuffle_changes_grid_photo_order() {
        let mut project = project(8, LayoutKind::Grid, 1);
        let original = project
            .items
            .iter()
            .map(|item| item.photo.id)
            .collect::<Vec<_>>();
        project.shuffle();
        let shuffled = project
            .items
            .iter()
            .map(|item| item.photo.id)
            .collect::<Vec<_>>();
        assert_ne!(original, shuffled);
    }

    #[test]
    fn shuffle_changes_mosaic_photo_order() {
        let mut project = project(8, LayoutKind::Mosaic, 1);
        let original = project
            .items
            .iter()
            .map(|item| item.photo.id)
            .collect::<Vec<_>>();
        project.shuffle();
        let shuffled = project
            .items
            .iter()
            .map(|item| item.photo.id)
            .collect::<Vec<_>>();
        assert_ne!(original, shuffled);
    }

    #[test]
    fn mosaic_items_do_not_overlap_and_stay_inside_canvas() {
        let mut project = project(23, LayoutKind::Mosaic, 7);
        project.relayout();
        assert!(inside(&project));
        for (index, item) in project.items.iter().enumerate() {
            assert!(project.items[index + 1..]
                .iter()
                .all(|other| !overlaps(item, other)));
        }
    }

    #[test]
    fn mosaic_portrait_fills_the_usable_canvas() {
        let mut project = project(7, LayoutKind::Mosaic, 7);
        project.orientation = CollageOrientation::Portrait;
        project.relayout();
        assert!(inside(&project));
        assert!(
            project
                .items
                .iter()
                .map(|item| item.width * item.height)
                .sum::<f32>()
                > 0.80
        );
    }

    #[test]
    fn mosaic_landscape_has_packed_varied_tiles() {
        let mut project = project(6, LayoutKind::Mosaic, 7);
        project.keep_photo_aspect = false;
        project.relayout();
        assert!(inside(&project));
        assert!(
            project
                .items
                .iter()
                .map(|item| item.width * item.height)
                .sum::<f32>()
                > 0.80
        );
        let widths = project
            .items
            .iter()
            .map(|item| item.width)
            .collect::<Vec<_>>();
        let heights = project
            .items
            .iter()
            .map(|item| item.height)
            .collect::<Vec<_>>();
        assert!(
            widths.iter().copied().fold(0.0, f32::max)
                > widths.iter().copied().fold(f32::INFINITY, f32::min) * 1.4
                || heights.iter().copied().fold(0.0, f32::max)
                    > heights.iter().copied().fold(f32::INFINITY, f32::min) * 1.4
        );
    }

    #[test]
    fn mosaic_mixed_photo_aspects_stays_packed() {
        let mut project = project(8, LayoutKind::Mosaic, 7);
        for (index, item) in project.items.iter_mut().enumerate() {
            item.photo.aspect_ratio = [0.55, 1.9, 0.75, 1.4, 2.1, 0.65, 1.1, 1.8][index];
        }
        project.relayout();
        assert!(inside(&project));
        for (index, item) in project.items.iter().enumerate() {
            assert!(project.items[index + 1..]
                .iter()
                .all(|other| !overlaps(item, other)));
        }
    }

    #[test]
    fn mosaic_mixed_photo_crop_cost_is_bounded() {
        let mut project = project(8, LayoutKind::Mosaic, 7);
        for (index, item) in project.items.iter_mut().enumerate() {
            item.photo.aspect_ratio = [0.55, 1.9, 0.75, 1.4, 2.1, 0.65, 1.1, 1.8][index];
        }
        project.relayout();
        let canvas_ratio = project.effective_aspect_ratio();
        let costs = project.items.iter().map(|item| {
            let tile_ratio = item.width * canvas_ratio / item.height.max(0.01);
            let mismatch = (tile_ratio / item.photo.aspect_ratio.max(0.1)).ln().abs();
            mismatch * mismatch + (mismatch - 0.8).max(0.0) * 2.0
        });
        let costs = costs.collect::<Vec<_>>();
        assert!((costs.iter().sum::<f32>() / costs.len() as f32) < 1.0);
        assert!(costs.iter().all(|cost| *cost < 2.5));
    }

    #[test]
    fn keeping_photo_aspect_reduces_average_mismatch() {
        let mut normal = project(8, LayoutKind::Mosaic, 7);
        let aspects = [0.55, 1.9, 0.75, 1.4, 2.1, 0.65, 1.1, 1.8];
        for (item, aspect) in normal.items.iter_mut().zip(aspects) {
            item.photo.aspect_ratio = aspect;
        }
        normal.keep_photo_aspect = false;
        normal.relayout();

        let mut kept = normal.clone();
        kept.keep_photo_aspect = true;
        kept.relayout();

        let average_mismatch = |project: &CollageProject| {
            project
                .items
                .iter()
                .map(|item| {
                    let tile_ratio =
                        item.width * project.effective_aspect_ratio() / item.height.max(0.01);
                    (tile_ratio / item.photo.aspect_ratio.max(0.1)).ln().abs()
                })
                .sum::<f32>()
                / project.items.len() as f32
        };
        assert!(average_mismatch(&kept) < average_mismatch(&normal));
    }

    #[test]
    fn nine_photo_mosaic_has_variable_tile_sizes() {
        let mut project = project(9, LayoutKind::Mosaic, 7);
        project.relayout();
        let smallest_width = project
            .items
            .iter()
            .map(|item| item.width)
            .fold(f32::INFINITY, f32::min);
        let smallest_height = project
            .items
            .iter()
            .map(|item| item.height)
            .fold(f32::INFINITY, f32::min);
        assert!(project.items.iter().any(|item| {
            item.width > smallest_width * 1.5 || item.height > smallest_height * 1.5
        }));
    }

    #[test]
    fn layout_random_values_are_normalized() {
        let mut state = 42;
        for _ in 0..1000 {
            let value = next_unit(&mut state);
            assert!((0.0..=1.0).contains(&value));
        }
    }

    #[test]
    fn orientation_changes_effective_ratio_without_changing_base_ratio() {
        let mut project = project(3, LayoutKind::Mosaic, 7);
        project.aspect = crate::collage::model::AspectRatio::ThreeTwo;
        project.orientation = CollageOrientation::Landscape;
        assert!((project.effective_aspect_ratio() - 3.0 / 2.0).abs() < f32::EPSILON);

        project.orientation = CollageOrientation::Portrait;
        assert!((project.effective_aspect_ratio() - 2.0 / 3.0).abs() < f32::EPSILON);
        assert_eq!(project.aspect, crate::collage::model::AspectRatio::ThreeTwo);
    }

    fn smart_project(count: usize, portrait_canvas: bool) -> CollageProject {
        let mut project = project(count, LayoutKind::SmartMosaic, 42);
        let aspects = [0.48, 2.35, 0.62, 1.75, 0.78, 1.35, 2.05, 0.55, 1.1];
        for (index, item) in project.items.iter_mut().enumerate() {
            item.photo.aspect_ratio = aspects[index % aspects.len()];
        }
        if portrait_canvas {
            project.orientation = CollageOrientation::Portrait;
        }
        project
    }

    fn assert_valid_smart_layout(project: &CollageProject) {
        assert!(inside(project));
        for (index, item) in project.items.iter().enumerate() {
            assert!(project.items[index + 1..]
                .iter()
                .all(|other| !overlaps(item, other)));
        }
        let used = project
            .items
            .iter()
            .map(|item| item.width * item.height)
            .sum::<f32>();
        assert!(used > 0.78, "too much unused canvas: {used}");
        let mismatch = project
            .items
            .iter()
            .map(|item| {
                let tile = item.width * project.effective_aspect_ratio() / item.height.max(0.001);
                (tile / item.photo.aspect_ratio).ln().abs()
            })
            .sum::<f32>()
            / project.items.len() as f32;
        assert!(mismatch < 0.95, "average aspect mismatch: {mismatch}");
    }

    #[test]
    fn smart_mosaic_handles_mixed_sets_and_canvas_orientations() {
        for &(count, portrait) in &[(3, false), (7, false), (9, true), (16, false), (18, true)] {
            let mut project = smart_project(count, portrait);
            project.relayout();
            assert_valid_smart_layout(&project);
        }
    }

    #[test]
    fn smart_mosaic_is_deterministic_and_shuffle_changes_the_composition() {
        let mut first = smart_project(9, false);
        let mut second = first.clone();
        first.relayout();
        second.relayout();
        let geometry = |project: &CollageProject| {
            project
                .items
                .iter()
                .map(|i| {
                    (
                        i.photo.id,
                        i.x.to_bits(),
                        i.y.to_bits(),
                        i.width.to_bits(),
                        i.height.to_bits(),
                    )
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(geometry(&first), geometry(&second));
        let before = geometry(&first);
        first.shuffle();
        assert_valid_smart_layout(&first);
        assert_ne!(before, geometry(&first));
    }

    #[test]
    fn smart_mosaic_allows_limited_crop_when_no_crop_is_disabled() {
        let mut project = smart_project(8, false);
        project.keep_photo_aspect = false;
        project.relayout();
        assert_valid_smart_layout(&project);
        let worst = project
            .items
            .iter()
            .map(|item| {
                let tile = item.width * project.effective_aspect_ratio() / item.height.max(0.001);
                (tile / item.photo.aspect_ratio).ln().abs()
            })
            .fold(0.0, f32::max);
        assert!(worst < 1.75, "severe crop selected: {worst}");
    }
}
