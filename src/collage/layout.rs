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
    let count = project.items.len().max(1);
    // A slightly roomier logical grid leaves enough free cells for occasional
    // 2x1 and 1x2 tiles instead of forcing the packing into a square grid.
    let columns = ((count as f32 * 1.5).sqrt().ceil() as usize).max(2);
    let rows = (count + columns - 1) / columns;
    let gap = project.spacing.clamp(0.0, 0.08);
    let cell_width = (1.0 - gap * (columns + 1) as f32) / columns as f32;
    let cell_height = (1.0 - gap * (rows + 1) as f32) / rows as f32;
    let seed = project.seed;
    let item_count = project.items.len();

    let mut occupied = vec![vec![false; columns]; rows];
    for (index, item) in project.items.iter_mut().enumerate() {
        let prefer_wide = mixed_seed(seed, index as u64) & 1 == 0;
        let prefer_tall = mixed_seed(seed, index as u64) & 2 == 0;
        let remaining = item_count.saturating_sub(index + 1);
        let free_cells = occupied.iter().flatten().filter(|cell| !**cell).count();
        let mut placement = None;

        let larger_tiles = count >= 3;
        let candidates = if larger_tiles && prefer_wide {
            [(2, 1), (1, 2), (1, 1)]
        } else if larger_tiles && prefer_tall {
            [(1, 2), (2, 1), (1, 1)]
        } else {
            [(1, 1), (2, 1), (1, 2)]
        };
        for (cell_widths, cell_heights) in candidates {
            if cell_widths * cell_heights > free_cells.saturating_sub(remaining) {
                continue;
            }
            'search: for row in 0..rows {
                for column in 0..columns {
                    if column + cell_widths > columns || row + cell_heights > rows {
                        continue;
                    }
                    let mut region_occupied = false;
                    for cell_row in row..row + cell_heights {
                        for cell_column in column..column + cell_widths {
                            if occupied[cell_row][cell_column] {
                                region_occupied = true;
                            }
                        }
                    }
                    if region_occupied {
                        continue;
                    }
                    placement = Some((column, row, cell_widths, cell_heights));
                    break 'search;
                }
            }
            if placement.is_some() {
                break;
            }
        }

        let (column, row, cell_widths, cell_heights) = placement.unwrap_or((0, 0, 1, 1));
        for cell_row in row..row + cell_heights {
            for cell_column in column..column + cell_widths {
                occupied[cell_row][cell_column] = true;
            }
        }
        item.x = gap + column as f32 * (cell_width + gap);
        item.y = gap + row as f32 * (cell_height + gap);
        item.width = cell_width * cell_widths as f32 + gap * (cell_widths - 1) as f32;
        item.height = cell_height * cell_heights as f32 + gap * (cell_heights - 1) as f32;
        item.rotation = 0.0;
        item.z = index;
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
    use crate::collage::model::{CollageItem, CollagePhoto};

    fn project(count: usize, layout: LayoutKind, seed: u64) -> CollageProject {
        CollageProject {
            aspect: crate::collage::model::AspectRatio::SixteenNine,
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
}
