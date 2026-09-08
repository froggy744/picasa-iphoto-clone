use super::model::{CollageProject, LayoutKind};

pub fn apply(project: &mut CollageProject) {
    match project.layout {
        LayoutKind::Grid => grid(project),
        LayoutKind::Mosaic => mosaic(project),
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
    let usable_width = (canvas_ratio - gap * 2.0).max(0.01);
    let usable_height = (1.0 - gap * 2.0).max(0.01);
    let target_rows = ((count as f32 / canvas_ratio.max(0.5)).sqrt().round() as usize).max(1);
    let target_height = usable_height / target_rows as f32;

    let mut rows: Vec<Vec<usize>> = Vec::new();
    let mut current = Vec::new();
    let mut aspect_sum = 0.0;
    for index in 0..count {
        let next_sum = aspect_sum + project.items[index].photo.aspect_ratio.max(0.1);
        let next_height = usable_width / next_sum;
        let remaining = count - index - 1;
        let should_finish = !current.is_empty()
            && next_height < target_height * 0.72
            && (rows.len() + 1 < target_rows || remaining == 0);
        if should_finish {
            rows.push(current);
            current = Vec::new();
            aspect_sum = 0.0;
        }
        current.push(index);
        aspect_sum += project.items[index].photo.aspect_ratio.max(0.1);
    }
    if !current.is_empty() {
        rows.push(current);
    }

    let natural_heights: Vec<f32> = rows
        .iter()
        .map(|row| {
            usable_width
                / row
                    .iter()
                    .map(|&index| project.items[index].photo.aspect_ratio.max(0.1))
                    .sum::<f32>()
        })
        .collect();
    let height_scale = usable_height / natural_heights.iter().sum::<f32>().max(0.01);
    let mut y = gap;
    for (row_index, row) in rows.iter().enumerate() {
        let row_height = natural_heights[row_index] * height_scale;
        let total_gap = gap * (row.len().saturating_sub(1)) as f32;
        let available = (canvas_ratio - gap * 2.0 - total_gap).max(0.01);
        let aspect_sum = row
            .iter()
            .map(|&index| project.items[index].photo.aspect_ratio.max(0.1))
            .sum::<f32>();
        let mut x = gap;
        for (position, &index) in row.iter().enumerate() {
            let item_width = if position + 1 == row.len() {
                (canvas_ratio - gap - x).max(0.01)
            } else {
                available * project.items[index].photo.aspect_ratio.max(0.1) / aspect_sum
            };
            let item = &mut project.items[index];
            item.x = x / canvas_ratio;
            item.y = y;
            item.width = item_width / canvas_ratio;
            item.height = row_height;
            item.rotation = 0.0;
            item.z = index;
            x += item_width + gap;
        }
        y += row_height + gap;
    }
    // The final row reaches the exact bottom edge, avoiding accumulated float
    // error from row rounding while keeping all coordinates normalized.
    if let Some(last_row) = rows.last() {
        if let Some(&index) = last_row.first() {
            let last_y = project.items[index].y;
            let final_height = (1.0 - gap - last_y).max(0.01);
            for &index in last_row {
                project.items[index].height = final_height;
            }
        }
    }
}

pub(crate) fn next_unit(state: &mut u64) -> f32 {
    *state = state
        .wrapping_mul(2862933555777941757)
        .wrapping_add(3037000493);
    ((*state >> 11) as f64 / ((1u64 << 53) - 1) as f64) as f32
}

fn mixed_seed(seed: u64, index: u64) -> u64 {
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
}
