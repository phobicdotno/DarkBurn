//! Boolean operations (union, difference, intersection, xor), weld and cut shapes.
//!
//! Curves are flattened within `tol` first; results are straight-line contours with outer
//! boundaries counter-clockwise and holes clockwise (Y up). Each input path is filled by
//! its own fill rule before it is combined with the others, so the result does not depend
//! on how the inputs overlap each other. Open subpaths are closed implicitly.
//!
//! Precision: the overlay engine snaps to an integer grid of 2^29 steps across the
//! combined bounding box, i.e. finer than 1/4096 mm for anything up to 131 m across.

use crate::path::{FillRule, contours, flatten_contour};
use i_overlay::core::fill_rule::FillRule as IFill;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::float::simplify::SimplifyShape;
use i_overlay::float::single::SingleFloatOverlay;
use kurbo::{BezPath, Point};

/// A boolean operation between a subject set and a clip set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoolOp {
    Union,
    /// Subject minus clip.
    Difference,
    Intersection,
    /// Areas covered by exactly one of the two sets.
    Xor,
}

pub(crate) type Ring = Vec<[f64; 2]>;
pub(crate) type IShape = Vec<Ring>;

fn ifill(rule: FillRule) -> IFill {
    match rule {
        FillRule::EvenOdd => IFill::EvenOdd,
        FillRule::NonZero => IFill::NonZero,
    }
}

/// Largest coordinate (mm) the overlay and offset engines are given. Far beyond any machine,
/// and well inside the range where they still work.
pub(crate) const MAX_COORD: f64 = 1e12;

/// True when `p` is a usable coordinate: a number no further out than [`MAX_COORD`].
pub(crate) fn usable_point(p: Point) -> bool {
    p.x.abs() <= MAX_COORD && p.y.abs() <= MAX_COORD
}

/// Flattens a path to rings (every subpath closed). A subpath with a coordinate that is not
/// a number, infinite or absurdly large has no area that could be worked out and is left out.
pub(crate) fn path_to_rings(path: &BezPath, tol: f64) -> Vec<Ring> {
    contours(path)
        .iter()
        .map(|c| flatten_contour(c, tol))
        .filter(|pl| pl.points.len() >= 3 && pl.points.iter().all(|p| usable_point(*p)))
        .map(|pl| pl.points.iter().map(|p| [p.x, p.y]).collect())
        .collect()
}

/// Converts one overlay shape (outer + holes) to a path.
pub(crate) fn shape_to_path(shape: &[Ring]) -> BezPath {
    let mut p = BezPath::new();
    for ring in shape {
        if ring.len() < 3 {
            continue;
        }
        p.move_to(Point::new(ring[0][0], ring[0][1]));
        for q in &ring[1..] {
            p.line_to(Point::new(q[0], q[1]));
        }
        p.close_path();
    }
    p
}

pub(crate) fn shapes_to_paths(shapes: &[IShape]) -> Vec<BezPath> {
    shapes.iter().map(|s| shape_to_path(s)).filter(|p| !p.is_empty()).collect()
}

/// Fills each path by `rule` on its own and returns properly oriented rings
/// (outer CCW, holes CW) so they can be combined with the nonzero rule.
pub(crate) fn normalized_rings(paths: &[BezPath], rule: FillRule, tol: f64) -> Vec<Ring> {
    let mut out = Vec::new();
    for p in paths {
        let rings = path_to_rings(p, tol);
        if rings.is_empty() {
            continue;
        }
        let shapes: Vec<IShape> = rings.simplify_shape(ifill(rule));
        for s in shapes {
            out.extend(s);
        }
    }
    out
}

/// Resolves overlaps and self-intersections of `paths` treated as one compound shape
/// under `rule`. Returns one path per resulting shape (outer contour plus its holes).
pub fn resolve(paths: &[BezPath], rule: FillRule, tol: f64) -> Vec<BezPath> {
    let rings: Vec<Ring> = paths.iter().flat_map(|p| path_to_rings(p, tol)).collect();
    if rings.is_empty() {
        return Vec::new();
    }
    shapes_to_paths(&rings.simplify_shape(ifill(rule)))
}

/// Each path filled by `rule` on its own, as properly oriented rings: one group per path.
fn normalized_groups(paths: &[BezPath], rule: FillRule, tol: f64) -> Vec<Vec<Ring>> {
    paths
        .iter()
        .map(|p| {
            let rings = path_to_rings(p, tol);
            if rings.is_empty() {
                return Vec::new();
            }
            let shapes: Vec<IShape> = rings.simplify_shape(ifill(rule));
            shapes.into_iter().flatten().collect()
        })
        .filter(|g: &Vec<Ring>| !g.is_empty())
        .collect()
}

/// The union of `groups`, merged two by two. Done in one step, every crossing of every shape is
/// worked through at once: a few hundred shapes stacked on one spot (Duplicate, then Align
/// centres) took minutes that way. Merged in pairs, each step works on what the shapes cover so
/// far, which stays small when they overlap.
fn union_groups(mut groups: Vec<Vec<Ring>>) -> Vec<Ring> {
    while groups.len() > 1 {
        let mut next = Vec::with_capacity(groups.len().div_ceil(2));
        let mut it = groups.into_iter();
        while let Some(a) = it.next() {
            match it.next() {
                Some(b) => {
                    let shapes: Vec<IShape> = a.overlay(&b, OverlayRule::Union, IFill::NonZero);
                    next.push(shapes.into_iter().flatten().collect());
                }
                None => next.push(a),
            }
        }
        groups = next;
    }
    groups.pop().unwrap_or_default()
}

/// General boolean between two sets of paths. Each input path is filled by `rule`; each set is
/// the union of its paths.
pub fn boolean(subject: &[BezPath], clip: &[BezPath], op: BoolOp, rule: FillRule, tol: f64) -> Vec<BezPath> {
    let s = union_groups(normalized_groups(subject, rule, tol));
    let c = union_groups(normalized_groups(clip, rule, tol));
    boolean_rings(&s, &c, op)
}

pub(crate) fn boolean_rings(s: &[Ring], c: &[Ring], op: BoolOp) -> Vec<BezPath> {
    let rule = match op {
        BoolOp::Union => OverlayRule::Union,
        BoolOp::Difference => OverlayRule::Difference,
        BoolOp::Intersection => OverlayRule::Intersect,
        BoolOp::Xor => OverlayRule::Xor,
    };
    if s.is_empty() && c.is_empty() {
        return Vec::new();
    }
    let s = s.to_vec();
    let c = c.to_vec();
    let shapes: Vec<IShape> = s.overlay(&c, rule, IFill::NonZero);
    shapes_to_paths(&shapes)
}

/// Union of all `paths`. Returns one path per separate piece (with its holes).
pub fn union(paths: &[BezPath], rule: FillRule, tol: f64) -> Vec<BezPath> {
    let rings = union_groups(normalized_groups(paths, rule, tol));
    if rings.is_empty() {
        return Vec::new();
    }
    shapes_to_paths(&rings.simplify_shape(IFill::NonZero))
}

/// `subject` minus `clip`.
pub fn difference(subject: &[BezPath], clip: &[BezPath], rule: FillRule, tol: f64) -> Vec<BezPath> {
    boolean(subject, clip, BoolOp::Difference, rule, tol)
}

/// Area common to `a` and `b`.
pub fn intersection(a: &[BezPath], b: &[BezPath], rule: FillRule, tol: f64) -> Vec<BezPath> {
    boolean(a, b, BoolOp::Intersection, rule, tol)
}

/// Area covered by exactly one of `a` and `b`.
pub fn xor(a: &[BezPath], b: &[BezPath], rule: FillRule, tol: f64) -> Vec<BezPath> {
    boolean(a, b, BoolOp::Xor, rule, tol)
}

/// Weld: the union of every path as a single compound path (all pieces and holes as
/// subpaths of one path). Empty input gives an empty path.
pub fn weld(paths: &[BezPath], rule: FillRule, tol: f64) -> BezPath {
    let mut out = BezPath::new();
    for p in union(paths, rule, tol) {
        out.extend(p);
    }
    out
}

/// Cut shapes: splits all shapes by each other so that every resulting piece is a region
/// covered by one fixed set of the inputs. Overlaps become their own pieces.
pub fn cut_shapes(paths: &[BezPath], rule: FillRule, tol: f64) -> Vec<BezPath> {
    // Each piece with its bounding box. A shape only cuts the pieces its box touches; the
    // others stay as they are without being worked through again (with a few hundred shapes
    // that is nearly all of them, and the difference between a moment and a minute).
    let mut pieces: Vec<(Vec<Ring>, Bounds)> = Vec::new();
    // What the shapes so far cover, each ring with its box.
    let mut covered: Vec<(Ring, Bounds)> = Vec::new();
    for p in paths {
        let s = normalized_rings(std::slice::from_ref(p), rule, tol);
        if s.is_empty() {
            continue;
        }
        let sb = bounds(&s);
        let mut next: Vec<(Vec<Ring>, Bounds)> = Vec::with_capacity(pieces.len() * 2 + 1);
        for (piece, pb) in pieces {
            if !touch(pb, sb) {
                next.push((piece, pb));
                continue;
            }
            for r in [OverlayRule::Intersect, OverlayRule::Difference] {
                let shapes: Vec<IShape> = piece.overlay(&s, r, IFill::NonZero);
                for sh in shapes {
                    let b = bounds(&sh);
                    next.push((sh, b));
                }
            }
        }
        // Rings of the covered area away from this shape's box do not change what is covered
        // inside it.
        let near: Vec<Ring> = covered.iter().filter(|(_, b)| touch(*b, sb)).map(|(r, _)| r.clone()).collect();
        let fresh: Vec<IShape> = s.overlay(&near, OverlayRule::Difference, IFill::NonZero);
        for sh in fresh {
            let b = bounds(&sh);
            next.push((sh, b));
        }
        // The new shape joins the covered area: only the rings near it can merge with it.
        let (near, mut far): (Vec<_>, Vec<_>) = covered.into_iter().partition(|(_, b)| touch(*b, sb));
        let mut merged: Vec<Ring> = near.into_iter().map(|(r, _)| r).collect();
        merged.extend(s);
        for ring in merged.simplify_shape(IFill::NonZero).into_iter().flatten() {
            let b = bounds(std::slice::from_ref(&ring));
            far.push((ring, b));
        }
        covered = far;
        pieces = next;
    }
    pieces.iter().map(|(s, _)| shape_to_path(s)).filter(|p| !p.is_empty()).collect()
}

/// Bounding box of rings: (x0, y0, x1, y1).
type Bounds = (f64, f64, f64, f64);

fn bounds(rings: &[Ring]) -> Bounds {
    let mut b = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for p in rings.iter().flatten() {
        b = (b.0.min(p[0]), b.1.min(p[1]), b.2.max(p[0]), b.3.max(p[1]));
    }
    b
}

/// Whether two boxes overlap or touch (with a hair to spare, so that shapes sharing an edge
/// are still worked against each other).
fn touch(a: Bounds, b: Bounds) -> bool {
    const HAIR: f64 = 1e-6;
    a.0 <= b.2 + HAIR && b.0 <= a.2 + HAIR && a.1 <= b.3 + HAIR && b.1 <= a.3 + HAIR
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path::{signed_area, subpaths};
    use kurbo::{Circle, Rect, Shape};

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> BezPath {
        Rect::new(x0, y0, x1, y1).to_path(0.1)
    }
    fn total_area(ps: &[BezPath]) -> f64 {
        ps.iter().map(signed_area).sum()
    }
    const T: f64 = 0.001;

    #[test]
    fn cut_shapes_of_many_separate_shapes_is_quick() {
        // Found by the monkey (seeds 1015 and 2028): Cut Shapes on a few hundred shapes took
        // 40 to 50 seconds, because every shape was worked against every piece so far, also
        // the ones nowhere near it.
        let mut shapes: Vec<BezPath> = Vec::new();
        for i in 0..400 {
            let c = Point::new(f64::from(i % 20) * 30.0, f64::from(i / 20) * 30.0);
            shapes.push(Circle::new(c, 10.0).to_path(0.01));
        }
        // And a square inside the first circle, so there is something to cut.
        shapes.push(rect(-5.0, -5.0, 5.0, 5.0));
        let started = std::time::Instant::now();
        let pieces = cut_shapes(&shapes, FillRule::EvenOdd, T);
        let took = started.elapsed();
        assert_eq!(pieces.len(), 401, "399 circles, the first circle around the square, and the square");
        let area = 400.0 * std::f64::consts::PI * 100.0;
        assert!((total_area(&pieces) - area).abs() < 10.0, "{} against {area}", total_area(&pieces));
        assert!(took < std::time::Duration::from_secs(4), "took {took:?}");
    }

    #[test]
    fn union_two_squares() {
        let u = union(&[rect(0.0, 0.0, 10.0, 10.0), rect(5.0, 5.0, 15.0, 15.0)], FillRule::EvenOdd, T);
        assert_eq!(u.len(), 1);
        assert!((total_area(&u) - 175.0).abs() < 1e-6);
        assert!(signed_area(&u[0]) > 0.0, "outer is counter-clockwise");
    }

    #[test]
    fn difference_makes_hole_and_orientation() {
        let d = difference(&[rect(0.0, 0.0, 10.0, 10.0)], &[rect(3.0, 3.0, 7.0, 7.0)], FillRule::EvenOdd, T);
        assert_eq!(d.len(), 1);
        let subs = subpaths(&d[0]);
        assert_eq!(subs.len(), 2);
        assert!(signed_area(&subs[0]) > 0.0);
        assert!(signed_area(&subs[1]) < 0.0);
        assert!((total_area(&d) - 84.0).abs() < 1e-6);
    }

    #[test]
    fn intersection_and_xor() {
        let a = [rect(0.0, 0.0, 10.0, 10.0)];
        let b = [rect(5.0, 0.0, 15.0, 10.0)];
        assert!((total_area(&intersection(&a, &b, FillRule::EvenOdd, T)) - 50.0).abs() < 1e-6);
        let x = xor(&a, &b, FillRule::EvenOdd, T);
        assert_eq!(x.len(), 2);
        assert!((total_area(&x) - 100.0).abs() < 1e-6);
        assert!(intersection(&a, &[rect(20.0, 0.0, 30.0, 10.0)], FillRule::EvenOdd, T).is_empty());
    }

    #[test]
    fn empty_and_degenerate_inputs() {
        assert!(union(&[], FillRule::EvenOdd, T).is_empty());
        assert!(union(&[BezPath::new()], FillRule::EvenOdd, T).is_empty());
        let mut line = BezPath::new();
        line.move_to((0.0, 0.0));
        line.line_to((10.0, 0.0));
        assert!(union(&[line.clone()], FillRule::EvenOdd, T).is_empty());
        let d = difference(&[rect(0.0, 0.0, 1.0, 1.0)], &[line], FillRule::EvenOdd, T);
        assert!((total_area(&d) - 1.0).abs() < 1e-9);
        assert!(difference(&[], &[rect(0.0, 0.0, 1.0, 1.0)], FillRule::EvenOdd, T).is_empty());
        assert!(weld(&[], FillRule::EvenOdd, T).is_empty());
    }

    #[test]
    fn coordinates_that_are_not_numbers_are_left_out() {
        let good = rect(0.0, 0.0, 5.0, 5.0);
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 1e300] {
            let mut p = BezPath::new();
            p.move_to((0.0, 0.0));
            p.line_to((10.0, bad));
            p.line_to((10.0, 10.0));
            p.close_path();
            for op in [BoolOp::Union, BoolOp::Difference, BoolOp::Intersection, BoolOp::Xor] {
                let r = boolean(std::slice::from_ref(&good), std::slice::from_ref(&p), op, FillRule::EvenOdd, T);
                let expected = if op == BoolOp::Intersection { 0.0 } else { 25.0 };
                assert!((total_area(&r) - expected).abs() < 1e-9, "{op:?} with {bad}");
            }
            assert!((total_area(&union(&[p.clone(), good.clone()], FillRule::EvenOdd, T)) - 25.0).abs() < 1e-9);
            assert!((signed_area(&weld(&[p.clone(), good.clone()], FillRule::EvenOdd, T)) - 25.0).abs() < 1e-9);
            assert_eq!(cut_shapes(&[good.clone(), p.clone()], FillRule::EvenOdd, T).len(), 1);
            assert!(resolve(std::slice::from_ref(&p), FillRule::EvenOdd, T).is_empty());
            assert_eq!(crate::path::area(&p, FillRule::EvenOdd, T), 0.0);
        }
    }

    #[test]
    fn self_intersecting_bowtie() {
        let mut b = BezPath::new();
        b.move_to((0.0, 0.0));
        b.line_to((10.0, 10.0));
        b.line_to((10.0, 0.0));
        b.line_to((0.0, 10.0));
        b.close_path();
        // Signed area cancels to zero but the filled area is two triangles of 25.
        assert!(signed_area(&b).abs() < 1e-9);
        let u = union(&[b.clone()], FillRule::EvenOdd, T);
        assert_eq!(u.len(), 2);
        assert!((total_area(&u) - 50.0).abs() < 1e-6);
        assert!((crate::path::area(&b, FillRule::NonZero, T) - 50.0).abs() < 1e-6);
    }

    #[test]
    fn holes_follow_fill_rule() {
        // Same-direction hole: even-odd keeps the hole, nonzero fills it.
        let mut p = rect(0.0, 0.0, 10.0, 10.0);
        p.extend(rect(2.0, 2.0, 8.0, 8.0));
        assert!((total_area(&union(&[p.clone()], FillRule::EvenOdd, T)) - 64.0).abs() < 1e-6);
        assert!((total_area(&union(&[p.clone()], FillRule::NonZero, T)) - 100.0).abs() < 1e-6);
        // Welding a shape with a hole to a square inside that hole keeps the ring hole partly.
        let w = weld(&[p, rect(4.0, 4.0, 6.0, 6.0)], FillRule::EvenOdd, T);
        assert!((signed_area(&w) - 68.0).abs() < 1e-6);
        assert_eq!(subpaths(&w).len(), 3);
    }

    #[test]
    fn precision_quarter_4096th_mm() {
        let e = 1.0 / 4096.0;
        // Spread over a 600 mm bed so the grid scale is realistic.
        let a = rect(0.0, 0.0, 10.0, 10.0);
        let far = rect(590.0, 590.0, 600.0, 600.0);
        let b = rect(e, 0.0, 10.0 + e, 10.0);
        let d = difference(&[a, far], &[b], FillRule::EvenOdd, T);
        let area: f64 = total_area(&d);
        let expected = 10.0 * e + 100.0;
        assert!((area - expected).abs() < 10.0 * e * 0.05, "area {area} expected {expected}");
    }

    #[test]
    fn curves_flatten_within_tolerance() {
        let c = Circle::new((0.0, 0.0), 10.0).to_path(1e-6);
        let u = union(&[c], FillRule::EvenOdd, 0.001);
        let a = total_area(&u);
        assert!((a - std::f64::consts::PI * 100.0).abs() < 0.1, "{a}");
    }

    #[test]
    fn cut_shapes_splits_overlap() {
        let pieces = cut_shapes(&[rect(0.0, 0.0, 10.0, 10.0), rect(5.0, 0.0, 15.0, 10.0)], FillRule::EvenOdd, T);
        assert_eq!(pieces.len(), 3);
        let mut areas: Vec<f64> = pieces.iter().map(signed_area).collect();
        areas.sort_by(f64::total_cmp);
        for a in areas {
            assert!((a - 50.0).abs() < 1e-6);
        }
        // Three overlapping shapes: centre region separated.
        let p3 = cut_shapes(&[rect(0.0, 0.0, 10.0, 10.0), rect(5.0, 0.0, 15.0, 10.0), rect(0.0, 5.0, 15.0, 6.0)], FillRule::EvenOdd, T);
        assert!((total_area(&p3) - 150.0).abs() < 1e-6);
        assert!(p3.len() >= 6);
    }

    #[test]
    fn duplicate_inputs_union_cleanly() {
        let r = rect(0.0, 0.0, 10.0, 10.0);
        let u = union(&[r.clone(), r.clone(), r], FillRule::EvenOdd, T);
        assert_eq!(u.len(), 1);
        assert!((total_area(&u) - 100.0).abs() < 1e-9);
    }
}
