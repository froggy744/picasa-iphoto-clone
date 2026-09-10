//! Complete-composition search, independent of the legacy Mosaic algorithm.
use super::layout::{mixed_seed, next_unit};
use super::model::CollageProject;

const CANDIDATES: usize = 40;
const REFINEMENTS: usize = 180;

#[derive(Clone)]
enum Node {
    Photo(usize),
    Split {
        first: usize,
        second: usize,
        vertical: bool,
        bias: f32,
    },
}

#[derive(Clone, Copy, Debug, Default)]
struct Rect {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

#[derive(Debug, Default)]
struct Score {
    crop: f32,
    empty: f32,
    repeated_shape: f32,
    repeated_size: f32,
    tiny: f32,
    balance: f32,
    edge_bias: f32,
    mismatch: f32,
    hierarchy: f32,
}

impl Score {
    fn total(&self, contain: bool) -> f32 {
        self.crop * 12.0
            + self.empty * if contain { 32.0 } else { 12.0 }
            + self.repeated_shape * 1.0
            + self.repeated_size * 1.4
            + self.tiny * 14.0
            + self.balance * 3.0
            + self.edge_bias * 1.2
            + self.mismatch * 3.0
            + self.hierarchy * 3.0
    }
}

pub(super) fn apply(project: &mut CollageProject) {
    if project.items.is_empty() {
        return;
    }
    let mut best = (f32::INFINITY, Vec::new());
    for number in 0..CANDIDATES {
        let (score, rectangles) = candidate(project, number);
        if score < best.0 {
            best = (score, rectangles);
        }
    }
    let ratio = project.effective_aspect_ratio();
    for (index, (item, rect)) in project.items.iter_mut().zip(best.1).enumerate() {
        item.x = rect.x / ratio;
        item.y = rect.y;
        item.width = (rect.w / ratio).min(1.0 - item.x);
        item.height = rect.h.min(1.0 - item.y);
        item.rotation = 0.0;
        item.z = index;
    }
}

fn random_index(state: &mut u64, count: usize) -> usize {
    ((next_unit(state) * count as f32) as usize).min(count - 1)
}

fn candidate(project: &CollageProject, number: usize) -> (f32, Vec<Rect>) {
    let count = project.items.len();
    let mut state = mixed_seed(project.seed, number as u64 + 1);
    let mut order = (0..count).collect::<Vec<_>>();
    for i in (1..count).rev() {
        order.swap(i, random_index(&mut state, i + 1));
    }
    let mut tree = Vec::with_capacity(count * 2 - 1);
    build_tree(&order, &mut tree, &mut state);
    let mut rectangles = geometry(project, &tree);
    let mut current = score(project, &rectangles).total(project.keep_photo_aspect);
    let mut best = (current, rectangles.clone());
    if count == 1 {
        return best;
    }
    // Each proposal is materialized in full. No branch chooses its geometry
    // using a local crop heuristic: acceptance uses only the whole canvas.
    for step in 0..REFINEMENTS {
        let mut proposal = tree.clone();
        match step % 3 {
            0 => {
                let leaves = proposal
                    .iter()
                    .enumerate()
                    .filter_map(|(i, n)| matches!(n, Node::Photo(_)).then_some(i))
                    .collect::<Vec<_>>();
                let a = leaves[random_index(&mut state, count)];
                let b = leaves[random_index(&mut state, count)];
                proposal.swap(a, b);
            }
            _ => {
                let branches = proposal
                    .iter()
                    .enumerate()
                    .filter_map(|(i, n)| matches!(n, Node::Split { .. }).then_some(i))
                    .collect::<Vec<_>>();
                let branch = branches[random_index(&mut state, branches.len())];
                if let Node::Split { vertical, bias, .. } = &mut proposal[branch] {
                    if step % 3 == 1 {
                        *vertical = !*vertical;
                    } else {
                        *bias = (*bias + (next_unit(&mut state) - 0.5) * 0.32).clamp(-0.6, 0.6);
                    }
                }
            }
        }
        rectangles = geometry(project, &proposal);
        let value = score(project, &rectangles).total(project.keep_photo_aspect);
        if value < best.0 {
            best = (value, rectangles.clone());
        }
        // Seeded annealing escapes repeated rows/columns; always retain the
        // lowest score visited, never the last accepted (possibly worse) one.
        let temperature = 0.12 * (1.0 - step as f32 / REFINEMENTS as f32).powi(2);
        if value < current
            || next_unit(&mut state) < ((current - value) / temperature.max(0.0001)).exp()
        {
            tree = proposal;
            current = value;
        }
    }
    best
}

fn build_tree(order: &[usize], tree: &mut Vec<Node>, state: &mut u64) -> usize {
    let node = if order.len() == 1 {
        Node::Photo(order[0])
    } else {
        let fraction = 0.25 + next_unit(state) * 0.5;
        let split = ((order.len() as f32 * fraction).round() as usize).clamp(1, order.len() - 1);
        let first = build_tree(&order[..split], tree, state);
        let second = build_tree(&order[split..], tree, state);
        Node::Split {
            first,
            second,
            vertical: next_unit(state) > 0.5,
            bias: 0.0,
        }
    };
    tree.push(node);
    tree.len() - 1
}

fn geometry(project: &CollageProject, tree: &[Node]) -> Vec<Rect> {
    // Natural subtree aspects provide a no-crop starting point. Random tree
    // topology, photo assignment and whole-layout refinement determine the
    // composition; no fixed rows, columns or preferred split fraction.
    let mut aspects = vec![0.0; tree.len()];
    for (i, node) in tree.iter().enumerate() {
        aspects[i] = match *node {
            Node::Photo(photo) => project.items[photo].photo.aspect_ratio.max(0.01),
            Node::Split {
                first,
                second,
                vertical,
                ..
            } => {
                if vertical {
                    aspects[first] + aspects[second]
                } else {
                    1.0 / (1.0 / aspects[first] + 1.0 / aspects[second])
                }
            }
        };
    }
    let ratio = project.effective_aspect_ratio();
    let gap = project.spacing.clamp(0.0, 0.06) * 0.65;
    let mut result = vec![Rect::default(); project.items.len()];
    let mut pending = vec![(
        tree.len() - 1,
        Rect {
            x: gap * ratio,
            y: gap,
            w: ratio * (1.0 - 2.0 * gap),
            h: 1.0 - 2.0 * gap,
        },
    )];
    while let Some((index, rect)) = pending.pop() {
        match tree[index] {
            Node::Photo(photo) => result[photo] = rect,
            Node::Split {
                first,
                second,
                vertical,
                bias,
            } => {
                let natural = if vertical {
                    aspects[first] / (aspects[first] + aspects[second])
                } else {
                    aspects[second] / (aspects[first] + aspects[second])
                };
                let odds = (natural / (1.0 - natural).max(0.0001)) * bias.exp();
                let fraction = (odds / (1.0 + odds)).clamp(0.04, 0.96);
                let mut a = rect;
                let mut b = rect;
                if vertical {
                    let gutter = (gap * ratio).min(rect.w * 0.15);
                    a.w = (rect.w - gutter) * fraction;
                    b.x = rect.x + a.w + gutter;
                    b.w = rect.w - a.w - gutter;
                } else {
                    let gutter = gap.min(rect.h * 0.15);
                    a.h = (rect.h - gutter) * fraction;
                    b.y = rect.y + a.h + gutter;
                    b.h = rect.h - a.h - gutter;
                }
                pending.push((first, a));
                pending.push((second, b));
            }
        }
    }
    result
}

fn score(project: &CollageProject, rectangles: &[Rect]) -> Score {
    let mut result = Score::default();
    let count = rectangles.len() as f32;
    let ratio = project.effective_aspect_ratio();
    let area = rectangles
        .iter()
        .map(|r| r.w * r.h / ratio)
        .collect::<Vec<_>>();
    let total = area.iter().sum::<f32>();
    let average = total / count;
    let mut visible = 0.0;
    let mut focal_mass = 0.0;
    let mut focal_center = (0.0, 0.0);
    let mut center = (0.0, 0.0);
    let mut adjacent: f32 = 0.0;
    let mut pairs: f32 = 0.0;
    let mut worst_tiny: f32 = 0.0;
    for (i, rect) in rectangles.iter().enumerate() {
        let aspect = rect.w / rect.h;
        let mismatch = (aspect / project.items[i].photo.aspect_ratio.max(0.01))
            .ln()
            .abs();
        let loss = 1.0 - (-mismatch).exp();
        let visible_area = area[i]
            * if project.keep_photo_aspect {
                1.0 - loss
            } else {
                1.0
            };
        visible += visible_area;
        if !project.keep_photo_aspect {
            result.crop += (loss + 8.0 * (loss - 0.25).max(0.0).powi(2)) / count;
        }
        result.mismatch += (mismatch.powi(2) + (mismatch - 0.5).max(0.0).powi(2)) / count;
        let relative = visible_area / average;
        result.tiny += (0.35 - relative).max(0.0).powi(2) * 1.15 / count;
        worst_tiny = worst_tiny.max((0.25 - relative).max(0.0).powi(2));
        let short = (rect.w / ratio.sqrt()).min(rect.h * ratio.sqrt());
        result.tiny += (0.32 - short * count.sqrt()).max(0.0).powi(2) * 1.2 / count;
        worst_tiny = worst_tiny.max((0.25 - short * count.sqrt()).max(0.0).powi(2));
        let x = (rect.x + rect.w * 0.5) / ratio;
        let y = rect.y + rect.h * 0.5;
        center.0 += x * visible_area;
        center.1 += y * visible_area;
        let mass = (area[i] / average - 1.0).max(0.0).powi(2);
        focal_mass += mass;
        focal_center.0 += x * mass;
        focal_center.1 += y * mass;
        // Large tiles near the perimeter incur more cost than supporting ones.
        result.edge_bias += mass * ((x - 0.5).abs().max((y - 0.5).abs()) - 0.25).max(0.0);
        for (j, other) in rectangles[..i].iter().enumerate() {
            pairs += 1.0;
            result.repeated_size += (1.0 - (area[i] / area[j]).ln().abs() / 0.18).max(0.0);
            let gap_x = (rect.x - other.x - other.w)
                .max(other.x - rect.x - rect.w)
                .max(0.0)
                / ratio;
            let gap_y = (rect.y - other.y - other.h)
                .max(other.y - rect.y - rect.h)
                .max(0.0);
            let overlap_x = (rect.x + rect.w).min(other.x + other.w) > rect.x.max(other.x);
            let overlap_y = (rect.y + rect.h).min(other.y + other.h) > rect.y.max(other.y);
            let tolerance = project.spacing.clamp(0.0, 0.06) * 0.66 + 0.001;
            if (gap_x <= tolerance && overlap_y) || (gap_y <= tolerance && overlap_x) {
                adjacent += 1.0;
                result.repeated_shape +=
                    (1.0 - (aspect / (other.w / other.h)).ln().abs() / 0.22).max(0.0);
            }
        }
    }
    result.empty = (1.0 - visible).max(0.0); // Includes ALL contain letterboxing.
                                             // A single nearly invisible photo must not disappear into a large-set mean.
    result.tiny += worst_tiny * 8.0;
    result.repeated_shape /= adjacent.max(1.0);
    result.repeated_size /= pairs.max(1.0);
    result.balance = (center.0 / visible - 0.5).powi(2) + (center.1 / visible - 0.5).powi(2);
    if focal_mass > 0.0 {
        result.balance += (focal_center.0 / focal_mass - 0.5).powi(2)
            + (focal_center.1 / focal_mass - 0.5).powi(2);
        result.edge_bias /= focal_mass;
    }
    if rectangles.len() >= 3 {
        let mut sizes = area.iter().map(|a| a / average).collect::<Vec<_>>();
        sizes.sort_by(|a, b| b.total_cmp(a));
        // One or two focal tiles, followed by distinctly smaller supporting
        // tiles. Penalize both an equal grid and a single overwhelming tile.
        result.hierarchy = (1.65 - sizes[0]).max(0.0).powi(2)
            + (sizes[0] - 3.2).max(0.0).powi(2)
            + (sizes[2] - 1.35).max(0.0).powi(2)
            + (1.25 - sizes[0] / sizes[2]).max(0.0).powi(2);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collage::model::{CollageItem, CollageOrientation, CollagePhoto};

    fn fixture(count: usize, portrait: bool, contain: bool) -> CollageProject {
        let mut project = CollageProject::new(Vec::new());
        project.keep_photo_aspect = contain;
        project.seed = 42;
        if portrait {
            project.orientation = CollageOrientation::Portrait;
        }
        let aspects = [0.48, 2.35, 0.62, 1.75, 0.78, 1.35, 2.05, 0.55, 1.1];
        project.items = (0..count)
            .map(|i| CollageItem {
                photo: CollagePhoto {
                    id: i as i64,
                    path: String::new(),
                    filename: String::new(),
                    thumbnail_path: None,
                    library_rotation: 0,
                    edit_recipe: String::new(),
                    aspect_ratio: aspects[i % aspects.len()],
                },
                x: 0.0,
                y: 0.0,
                width: 1.0,
                height: 1.0,
                rotation: 0.0,
                z: i,
            })
            .collect();
        project
    }

    fn selected_rectangles(project: &CollageProject) -> Vec<Rect> {
        let ratio = project.effective_aspect_ratio();
        project
            .items
            .iter()
            .map(|i| Rect {
                x: i.x * ratio,
                y: i.y,
                w: i.width * ratio,
                h: i.height,
            })
            .collect()
    }

    fn assert_valid(project: &CollageProject, rects: &[Rect]) {
        let ratio = project.effective_aspect_ratio();
        for (i, r) in rects.iter().enumerate() {
            assert!(r.x.is_finite() && r.y.is_finite() && r.w > 0.0 && r.h > 0.0);
            assert!(
                r.x >= 0.0 && r.y >= 0.0 && r.x + r.w <= ratio + 0.00001 && r.y + r.h <= 1.00001
            );
            for other in &rects[..i] {
                let overlap_x = (r.x + r.w).min(other.x + other.w) - r.x.max(other.x);
                let overlap_y = (r.y + r.h).min(other.y + other.h) - r.y.max(other.y);
                assert!(overlap_x <= 0.00001 || overlap_y <= 0.00001);
            }
        }
    }

    #[test]
    fn all_candidates_are_valid_varied_and_lowest_score_wins() {
        let mut project = fixture(9, false, true);
        let mut minimum = f32::INFINITY;
        let mut unique = std::collections::HashSet::new();
        for number in 0..CANDIDATES {
            let (value, rects) = candidate(&project, number);
            assert_valid(&project, &rects);
            minimum = minimum.min(value);
            unique.insert(
                rects
                    .iter()
                    .flat_map(|r| [r.x.to_bits(), r.y.to_bits(), r.w.to_bits(), r.h.to_bits()])
                    .collect::<Vec<_>>(),
            );
        }
        assert!(unique.len() >= 20);
        apply(&mut project);
        let selected = score(&project, &selected_rectangles(&project)).total(true);
        assert!((selected - minimum).abs() < 0.0001);
    }

    #[test]
    fn completed_compositions_have_hierarchy_and_little_dead_space() {
        for count in [3, 7, 8, 9, 16, 24] {
            for portrait in [false, true] {
                for contain in [false, true] {
                    let mut project = fixture(count, portrait, contain);
                    apply(&mut project);
                    let rects = selected_rectangles(&project);
                    assert_valid(&project, &rects);
                    let s = score(&project, &rects);
                    // Three extreme aspects can require unavoidable letterbox
                    // space; larger sets offer many more packing alternatives.
                    assert!(s.empty < if count == 3 { 0.38 } else { 0.30 }, "{s:?}");
                    assert!(s.mismatch < 0.25, "{s:?}");
                    assert!(s.hierarchy < 0.25, "{s:?}");
                    assert!(s.repeated_size < 0.45, "{s:?}");
                    assert!(s.repeated_shape < 0.45, "{s:?}");
                    assert!(s.balance < 0.16, "{s:?}");
                    if count >= 7 {
                        let mean = rects.iter().map(|r| r.w * r.h).sum::<f32>() / count as f32;
                        assert!(
                            rects.iter().all(|r| r.w * r.h > mean * 0.12),
                            "tiny tile in {rects:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn scoring_detects_letterboxing_and_equal_grid() {
        let mut project = fixture(4, false, true);
        let ratio = project.effective_aspect_ratio();
        let grid = (0..4)
            .map(|i| Rect {
                x: (i % 2) as f32 * ratio / 2.0,
                y: (i / 2) as f32 / 2.0,
                w: ratio / 2.0,
                h: 0.5,
            })
            .collect::<Vec<_>>();
        for item in &mut project.items {
            item.photo.aspect_ratio = ratio / 2.0;
        }
        let contained = score(&project, &grid);
        assert!((contained.empty - 0.5).abs() < 0.00001);
        assert!(contained.repeated_shape > 0.99);
        assert!(contained.repeated_size > 0.99);
        assert!(contained.hierarchy > 0.4);
        assert_eq!(contained.crop, 0.0);
        project.keep_photo_aspect = false;
        let covered = score(&project, &grid);
        assert_eq!(covered.empty, 0.0);
        assert!(covered.crop > 0.5);
    }

    #[test]
    fn focal_balance_checks_both_axes_and_edges() {
        let mut project = fixture(4, false, false);
        project.aspect = crate::collage::model::AspectRatio::Square;
        let one_side = vec![
            Rect {
                x: 0.0,
                y: 0.0,
                w: 0.7,
                h: 0.5,
            },
            Rect {
                x: 0.0,
                y: 0.5,
                w: 0.7,
                h: 0.5,
            },
            Rect {
                x: 0.7,
                y: 0.0,
                w: 0.3,
                h: 0.5,
            },
            Rect {
                x: 0.7,
                y: 0.5,
                w: 0.3,
                h: 0.5,
            },
        ];
        let mut balanced = one_side.clone();
        balanced[1].x = 0.3;
        balanced[3].x = 0.0;
        for transpose in [false, true] {
            let transform = |rs: &[Rect]| {
                rs.iter()
                    .map(|r| {
                        if transpose {
                            Rect {
                                x: r.y,
                                y: r.x,
                                w: r.h,
                                h: r.w,
                            }
                        } else {
                            *r
                        }
                    })
                    .collect::<Vec<_>>()
            };
            let clustered = score(&project, &transform(&one_side));
            let distributed = score(&project, &transform(&balanced));
            assert!(distributed.balance < clustered.balance);
        }
        let mut edge = balanced.clone();
        edge[0].w = 0.4;
        edge[1].w = 0.4;
        edge[1].x = 0.6;
        let at_edge = score(&project, &edge);
        edge[0].x = 0.3;
        edge[1].x = 0.3;
        // Isolate the score component with the same sizes and inward centers.
        assert!(score(&project, &edge).edge_bias < at_edge.edge_bias);
    }

    #[test]
    fn identical_sources_still_get_a_size_hierarchy() {
        for portrait in [false, true] {
            let mut project = fixture(9, portrait, true);
            for item in &mut project.items {
                item.photo.aspect_ratio = 1.5;
            }
            apply(&mut project);
            let rects = selected_rectangles(&project);
            assert_valid(&project, &rects);
            let s = score(&project, &rects);
            assert!(s.hierarchy < 0.2, "{s:?}");
            assert!(s.repeated_size < 0.4, "{s:?}");
            assert!(s.empty < 0.3, "{s:?}");
        }
    }
}
