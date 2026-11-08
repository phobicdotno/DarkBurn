//! Simple nesting: packs shapes by their bounding boxes into a sheet with a shelf packer,
//! trying several rotations (and optionally mirroring) per shape.
//!
//! This is a bounding-box packer, not a true-shape nester: parts never interlock.

use crate::path::{bounding_box, signed_area};
use kurbo::{Affine, BezPath, Rect, Vec2};

/// Options for [`nest`].
#[derive(Clone, Debug, PartialEq)]
pub struct NestOptions {
    /// Minimum gap between the bounding boxes of neighbouring parts (mm).
    pub spacing: f64,
    /// Empty border kept inside the sheet edge (mm).
    pub margin: f64,
    /// Rotations to try, degrees. Empty means no rotation.
    pub rotations: Vec<f64>,
    /// Also try each rotation mirrored left-right.
    pub allow_mirror: bool,
}

impl Default for NestOptions {
    fn default() -> Self {
        Self { spacing: 2.0, margin: 0.0, rotations: vec![0.0, 90.0], allow_mirror: false }
    }
}

/// Rotation angles `0, step, 2·step, …` below 360°. A step ≤ 0 gives just `[0]`.
pub fn rotation_steps(step_deg: f64) -> Vec<f64> {
    if step_deg <= 1e-9 {
        return vec![0.0];
    }
    let n = (360.0 / step_deg - 1e-9).ceil().max(1.0) as usize;
    (0..n).map(|i| i as f64 * step_deg).collect()
}

/// Result of [`nest`].
#[derive(Clone, Debug, PartialEq)]
pub struct NestResult {
    /// Transform per input shape, or `None` when it did not fit (or is empty).
    pub placements: Vec<Option<Affine>>,
    /// Number of shapes that were not placed.
    pub unplaced: usize,
    /// Filled area of placed parts divided by the usable sheet area (0..=1).
    pub utilization: f64,
}

struct Shelf {
    y: f64,
    height: f64,
    x: f64,
}

struct Variant {
    xf: Affine,
    bbox: Rect,
}

/// Nests `shapes` into `sheet`. Larger parts go first; each part takes the lowest, then
/// leftmost, spot over all allowed rotations.
pub fn nest(shapes: &[BezPath], sheet: Rect, o: &NestOptions) -> NestResult {
    let m = o.margin.max(0.0);
    let usable = Rect::new(sheet.x0 + m, sheet.y0 + m, sheet.x1 - m, sheet.y1 - m);
    let mut placements = vec![None; shapes.len()];
    if usable.width() <= 0.0 || usable.height() <= 0.0 {
        let unplaced = shapes.len();
        return NestResult { placements, unplaced, utilization: 0.0 };
    }
    let rotations = if o.rotations.is_empty() { vec![0.0] } else { o.rotations.clone() };
    let sp = o.spacing.max(0.0);
    // Parts are padded by the spacing; the far sheet edges get the same slack back.
    let (w_avail, h_avail) = (usable.width() + sp, usable.height() + sp);

    let mut order: Vec<(usize, f64, f64)> =
        shapes.iter().enumerate().filter_map(|(i, p)| bounding_box(p).map(|b| (i, b.width().max(b.height()), b.area()))).collect();
    order.sort_by(|a, b| b.1.total_cmp(&a.1).then(b.2.total_cmp(&a.2)));

    let mut shelves: Vec<Shelf> = Vec::new();
    let mut top = 0.0;
    let mut placed_area = 0.0;
    for (i, _, _) in order {
        let path = &shapes[i];
        let mut variants: Vec<Variant> = Vec::new();
        for &deg in &rotations {
            for mirror in [false, true] {
                if mirror && !o.allow_mirror {
                    continue;
                }
                let m = if mirror { Affine::scale_non_uniform(-1.0, 1.0) } else { Affine::IDENTITY };
                let xf = Affine::rotate(deg.to_radians()) * m;
                if let Some(bbox) = bounding_box(&(xf * path.clone())) {
                    variants.push(Variant { xf, bbox });
                }
            }
        }
        // (y, x, shelf index or None for a new shelf, variant)
        let mut best: Option<(f64, f64, Option<usize>, usize)> = None;
        for (vi, v) in variants.iter().enumerate() {
            let (w, h) = (v.bbox.width() + sp, v.bbox.height() + sp);
            if w > w_avail + 1e-9 {
                continue;
            }
            let mut cand = None;
            for (si, s) in shelves.iter().enumerate() {
                if h <= s.height + 1e-9 && s.x + w <= w_avail + 1e-9 {
                    cand = Some((s.y, s.x, Some(si)));
                    break;
                }
            }
            if cand.is_none() && top + h <= h_avail + 1e-9 {
                cand = Some((top, 0.0, None));
            }
            if let Some((y, x, si)) = cand {
                let better = match best {
                    None => true,
                    Some((by, bx, bsi, bvi)) => {
                        let bh = variants[bvi].bbox.height();
                        // Prefer the lower spot; for a new shelf prefer the flatter variant.
                        y < by - 1e-9
                            || ((y - by).abs() <= 1e-9 && si.is_none() && bsi.is_none() && v.bbox.height() < bh - 1e-9)
                            || ((y - by).abs() <= 1e-9 && si.is_some() && x < bx - 1e-9)
                    }
                };
                if better {
                    best = Some((y, x, si, vi));
                }
            }
        }
        let Some((y, x, si, vi)) = best else { continue };
        let v = &variants[vi];
        let (w, h) = (v.bbox.width() + sp, v.bbox.height() + sp);
        match si {
            Some(si) => shelves[si].x += w,
            None => {
                shelves.push(Shelf { y: top, height: h, x: w });
                top += h;
            }
        }
        let target = Vec2::new(usable.x0 + x - v.bbox.x0, usable.y0 + y - v.bbox.y0);
        placements[i] = Some(Affine::translate(target) * v.xf);
        placed_area += signed_area(path).abs();
    }
    let unplaced = placements.iter().filter(|p| p.is_none()).count();
    let utilization = (placed_area / usable.area()).clamp(0.0, 1.0);
    NestResult { placements, unplaced, utilization }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::Shape;
    use std::time::Instant;

    fn rect(w: f64, h: f64) -> BezPath {
        Rect::new(0.0, 0.0, w, h).to_path(0.1)
    }

    fn placed_boxes(shapes: &[BezPath], r: &NestResult) -> Vec<Rect> {
        shapes.iter().zip(&r.placements).filter_map(|(s, p)| p.map(|a| bounding_box(&(a * s.clone())).unwrap())).collect()
    }

    fn check_no_overlap(boxes: &[Rect], sheet: Rect, spacing: f64) {
        for b in boxes {
            assert!(b.x0 >= sheet.x0 - 1e-6 && b.y0 >= sheet.y0 - 1e-6 && b.x1 <= sheet.x1 + 1e-6 && b.y1 <= sheet.y1 + 1e-6, "{b:?}");
        }
        for (i, a) in boxes.iter().enumerate() {
            for b in &boxes[i + 1..] {
                let gap_x = (b.x0 - a.x1).max(a.x0 - b.x1);
                let gap_y = (b.y0 - a.y1).max(a.y0 - b.y1);
                assert!(gap_x.max(gap_y) >= spacing - 1e-6, "{a:?} {b:?}");
            }
        }
    }

    #[test]
    fn places_inside_without_overlap() {
        let shapes: Vec<BezPath> = (0..30).map(|i| rect(5.0 + (i % 7) as f64 * 3.0, 4.0 + (i % 5) as f64 * 2.0)).collect();
        let sheet = Rect::new(10.0, 10.0, 210.0, 160.0);
        let o = NestOptions { spacing: 2.0, margin: 3.0, ..Default::default() };
        let r = nest(&shapes, sheet, &o);
        assert_eq!(r.unplaced, 0);
        check_no_overlap(&placed_boxes(&shapes, &r), sheet.inset(-3.0), 2.0);
        assert!(r.utilization > 0.0 && r.utilization <= 1.0);
    }

    #[test]
    fn too_big_and_empty_not_placed() {
        let shapes = vec![rect(500.0, 500.0), BezPath::new(), rect(10.0, 10.0)];
        let r = nest(&shapes, Rect::new(0.0, 0.0, 100.0, 100.0), &NestOptions::default());
        assert!(r.placements[0].is_none());
        assert!(r.placements[1].is_none());
        assert!(r.placements[2].is_some());
        assert_eq!(r.unplaced, 2);
        let none = nest(&shapes, Rect::new(0.0, 0.0, 1.0, 1.0), &NestOptions { margin: 1.0, ..Default::default() });
        assert_eq!(none.unplaced, 3);
    }

    #[test]
    fn rotation_lets_long_part_fit() {
        let long = vec![rect(10.0, 50.0)];
        let sheet = Rect::new(0.0, 0.0, 100.0, 20.0);
        let no_rot = nest(&long, sheet, &NestOptions { rotations: vec![0.0], ..Default::default() });
        assert!(no_rot.placements[0].is_none());
        let rot = nest(&long, sheet, &NestOptions { rotations: rotation_steps(90.0), ..Default::default() });
        let b = placed_boxes(&long, &rot)[0];
        assert!((b.width() - 50.0).abs() < 1e-9);
        check_no_overlap(&[b], sheet, 0.0);
        assert_eq!(rotation_steps(90.0), vec![0.0, 90.0, 180.0, 270.0]);
        assert_eq!(rotation_steps(0.0), vec![0.0]);
    }

    #[test]
    fn mirror_variant_allowed() {
        let mut tri = BezPath::new();
        tri.move_to((0.0, 0.0));
        tri.line_to((10.0, 0.0));
        tri.line_to((0.0, 10.0));
        tri.close_path();
        let r = nest(&[tri.clone()], Rect::new(0.0, 0.0, 50.0, 50.0), &NestOptions { allow_mirror: true, ..Default::default() });
        assert!(r.placements[0].is_some());
    }

    #[test]
    fn nests_2000_small_rects_quickly() {
        let shapes: Vec<BezPath> = (0..2000).map(|i| rect(3.0 + (i % 4) as f64, 2.0 + (i % 3) as f64)).collect();
        let sheet = Rect::new(0.0, 0.0, 600.0, 400.0);
        let t = Instant::now();
        let o = NestOptions { spacing: 1.0, ..Default::default() };
        let r = nest(&shapes, sheet, &o);
        let el = t.elapsed();
        assert_eq!(r.unplaced, 0);
        assert!(el.as_secs_f64() < 5.0, "{el:?}");
        check_no_overlap(&placed_boxes(&shapes, &r), sheet, 1.0);
    }
}
