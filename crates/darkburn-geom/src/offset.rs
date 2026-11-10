//! Offsets: closed shapes inward/outward/both with round, bevel or miter joins; open
//! paths offset to both sides with end caps; "outer shapes only"; and kerf offsets that
//! keep each contour's start point and direction.
//!
//! Shape offsets use the i_overlay outline/stroke builders (curves flattened within `tol`,
//! results are straight-line contours, outer CCW and holes CW). Kerf offsets use
//! cavalier_contours, which offsets lines and true circular arcs, so circles stay circles.

use crate::boolean::{Ring, normalized_rings, path_to_rings, shapes_to_paths, usable_point};
use crate::path::{Contour, FillRule, contour_with_start, contours, flatten_contour, from_contours, nearest_on_contour};
use crate::query::{containment, measure_arc};
use cavalier_contours::polyline::{PlineCreation, PlineSource, PlineSourceMut, PlineVertex, Polyline as CPline, seg_arc_radius_and_center};
use i_overlay::core::fill_rule::FillRule as IFill;
use i_overlay::float::simplify::SimplifyShape;
use i_overlay::mesh::float::outline::offset::OutlineOffset;
use i_overlay::mesh::float::stroke::offset::StrokeOffset;
use i_overlay::mesh::float::style::{LineCap, LineJoin, OutlineStyle, StrokeStyle};
use kurbo::{Arc, BezPath, Line, ParamCurve, PathSeg, Point, Vec2};

/// Which side(s) of a closed shape to offset to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum OffsetSide {
    /// Grow the shape (holes shrink).
    #[default]
    Outward,
    /// Shrink the shape (holes grow).
    Inward,
    /// Both results.
    Both,
}

/// Corner style of an offset.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum Join {
    /// Circular arc around the corner.
    #[default]
    Round,
    /// Straight cut across the corner.
    Bevel,
    /// Sharp corner. Corners whose miter would be longer than `limit` times the offset
    /// distance are bevelled (same meaning as the SVG `stroke-miterlimit`).
    Miter { limit: f64 },
}

/// End style of an offset open path.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Cap {
    /// Half circle around the end.
    #[default]
    Round,
    /// Square end flush with the path end.
    Butt,
    /// Square end extended by the offset distance.
    Square,
}

/// Options for [`offset`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OffsetOptions {
    /// Offset distance in mm (must be positive; zero returns no geometry).
    pub distance: f64,
    pub side: OffsetSide,
    pub join: Join,
    /// End caps for open paths.
    pub cap: Cap,
    /// Ignore shapes that sit inside other shapes (holes and islands) and offset only the
    /// outermost contours.
    pub outer_only: bool,
    /// How each input path's inside is decided.
    pub fill_rule: FillRule,
    /// Flattening tolerance and round-join accuracy (mm).
    pub tol: f64,
}

impl Default for OffsetOptions {
    fn default() -> Self {
        Self {
            distance: 1.0,
            side: OffsetSide::Outward,
            join: Join::Round,
            cap: Cap::Round,
            outer_only: false,
            fill_rule: FillRule::EvenOdd,
            tol: 0.01,
        }
    }
}

/// Result of [`offset`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OffsetResult {
    /// Outward results (closed shapes) plus offsets of open paths.
    pub outward: Vec<BezPath>,
    /// Inward results (closed shapes).
    pub inward: Vec<BezPath>,
}

impl OffsetResult {
    /// All result shapes.
    pub fn all(&self) -> Vec<BezPath> {
        self.outward.iter().chain(self.inward.iter()).cloned().collect()
    }
}

/// Round-join step angle (radians) giving a chord error of `tol` at radius `r`.
fn round_step(r: f64, tol: f64) -> f64 {
    if r <= tol {
        return std::f64::consts::FRAC_PI_4;
    }
    2.0 * (1.0 - tol / r).clamp(-1.0, 1.0).acos()
}

fn line_join(j: Join, r: f64, tol: f64) -> LineJoin<f64> {
    match j {
        Join::Round => LineJoin::Round(round_step(r, tol)),
        Join::Bevel => LineJoin::Bevel,
        Join::Miter { limit } => {
            // Miter length / distance = 1 / sin(theta / 2), theta the interior angle.
            let limit = limit.max(1.0001);
            LineJoin::Miter(2.0 * (1.0 / limit).asin())
        }
    }
}

fn line_cap(c: Cap, r: f64, tol: f64) -> LineCap<[f64; 2]> {
    match c {
        Cap::Round => LineCap::Round(round_step(r, tol)),
        Cap::Butt => LineCap::Butt,
        Cap::Square => LineCap::Square,
    }
}

/// True when every point of the contour is a usable coordinate.
fn usable_contour(c: &Contour) -> bool {
    c.segs.iter().all(|s| match s {
        PathSeg::Line(l) => usable_point(l.p0) && usable_point(l.p1),
        PathSeg::Quad(q) => usable_point(q.p0) && usable_point(q.p1) && usable_point(q.p2),
        PathSeg::Cubic(cb) => usable_point(cb.p0) && usable_point(cb.p1) && usable_point(cb.p2) && usable_point(cb.p3),
    })
}

fn outline_rings(rings: &[Ring], signed: f64, join: Join, tol: f64) -> Vec<BezPath> {
    if rings.is_empty() || signed == 0.0 {
        return Vec::new();
    }
    let style = OutlineStyle::new(signed).line_join(line_join(join, signed.abs(), tol));
    let shapes = rings.to_vec().outline(&style);
    shapes_to_paths(&shapes)
}

/// Offsets open paths by `distance` to both sides with end caps (the outline of a stroke
/// of width `2 * distance`). Closed subpaths are offset as rings (both sides, giving a band).
pub fn offset_open(path: &BezPath, distance: f64, join: Join, cap: Cap, tol: f64) -> Vec<BezPath> {
    if distance <= 0.0 {
        return Vec::new();
    }
    let mut all = Vec::new();
    for c in contours(path) {
        let pl = flatten_contour(&c, tol);
        if pl.points.len() < 2 || !pl.points.iter().all(|p| usable_point(*p)) {
            continue;
        }
        let pts: Vec<[f64; 2]> = pl.points.iter().map(|p| [p.x, p.y]).collect();
        let style = StrokeStyle::new(2.0 * distance)
            .line_join(line_join(join, distance, tol))
            .start_cap(line_cap(cap, distance, tol))
            .end_cap(line_cap(cap, distance, tol));
        let shapes = pts.stroke(style, pl.closed);
        for s in shapes {
            all.extend(s);
        }
    }
    if all.is_empty() {
        return Vec::new();
    }
    shapes_to_paths(&all.simplify_shape(IFill::NonZero))
}

/// Offsets shapes. Closed subpaths are filled by `fill_rule` per input path, all inputs are
/// offset together (overlapping results merge), holes move the opposite way to their outer
/// contour. Open subpaths are offset to both sides with caps and merged into `outward`.
pub fn offset(paths: &[BezPath], opts: &OffsetOptions) -> OffsetResult {
    let d = opts.distance;
    if !(d > 0.0) || !d.is_finite() {
        return OffsetResult::default();
    }
    let tol = opts.tol.max(1e-6);
    // Split closed and open subpaths.
    let mut closed_paths: Vec<BezPath> = Vec::new();
    let mut open_path = BezPath::new();
    for p in paths {
        let cs = contours(p);
        let (closed, open): (Vec<Contour>, Vec<Contour>) = cs.into_iter().partition(|c| c.closed);
        if !closed.is_empty() {
            closed_paths.push(from_contours(closed.iter()));
        }
        for c in &open {
            crate::path::append_contour(&mut open_path, c);
        }
    }
    let rings: Vec<Ring> = if opts.outer_only {
        // Every closed contour on its own, keep only those not inside another one.
        let subs: Vec<BezPath> = closed_paths.iter().flat_map(crate::path::subpaths).collect();
        let tree = containment(&subs, tol);
        let outer: Vec<BezPath> = tree.roots().into_iter().map(|i| subs[i].clone()).collect();
        normalized_rings(&outer, FillRule::NonZero, tol)
    } else {
        normalized_rings(&closed_paths, opts.fill_rule, tol)
    };
    let mut res = OffsetResult::default();
    if matches!(opts.side, OffsetSide::Outward | OffsetSide::Both) {
        res.outward = outline_rings(&rings, d, opts.join, tol);
    }
    if matches!(opts.side, OffsetSide::Inward | OffsetSide::Both) {
        res.inward = outline_rings(&rings, -d, opts.join, tol);
    }
    if !open_path.is_empty() {
        let open_res = offset_open(&open_path, d, opts.join, opts.cap, tol);
        if res.outward.is_empty() {
            res.outward = open_res;
        } else {
            // Merge so overlapping open and closed results become one outline.
            let mut all: Vec<Ring> = Vec::new();
            for p in res.outward.iter().chain(open_res.iter()) {
                all.extend(path_to_rings(p, tol));
            }
            res.outward = shapes_to_paths(&all.simplify_shape(IFill::NonZero));
        }
    }
    res
}

fn bulge_for(sweep: f64) -> f64 {
    (sweep / 4.0).tan()
}

fn contour_to_pline(c: &Contour, tol: f64) -> CPline<f64> {
    let mut pl = CPline::with_capacity(c.segs.len() * 4, true);
    for seg in &c.segs {
        let start = seg.start();
        match seg {
            PathSeg::Line(_) => pl.add(start.x, start.y, 0.0),
            _ => {
                if let Some(arc) = measure_arc(seg, tol * 0.25)
                    && arc.sweep.abs() < std::f64::consts::PI * 1.01
                {
                    pl.add(start.x, start.y, bulge_for(arc.sweep));
                } else {
                    let mut pts = vec![start];
                    crate::path::flatten_seg(seg, tol, &mut pts);
                    for p in &pts[..pts.len() - 1] {
                        pl.add(p.x, p.y, 0.0);
                    }
                }
            }
        }
    }
    pl.remove_repeat_pos(1e-9).unwrap_or(pl)
}

fn pline_to_contour(pl: &CPline<f64>) -> Option<Contour> {
    let n = pl.vertex_count();
    if n < 2 {
        return None;
    }
    let mut segs = Vec::with_capacity(n);
    for i in 0..n {
        let v1 = pl.at(i);
        let v2 = pl.at((i + 1) % n);
        if i + 1 == n && !pl.is_closed() {
            break;
        }
        let p0 = Point::new(v1.x, v1.y);
        let p1 = Point::new(v2.x, v2.y);
        if v1.bulge.abs() < 1e-12 {
            if p0 != p1 {
                segs.push(PathSeg::Line(Line::new(p0, p1)));
            }
        } else {
            let (r, c) = seg_arc_radius_and_center(v1, v2);
            let center = Point::new(c.x, c.y);
            let start_angle = (p0 - center).atan2();
            let sweep = 4.0 * v1.bulge.atan();
            let arc = Arc::new(center, Vec2::new(r, r), start_angle, sweep, 0.0);
            let mut cur = p0;
            for el in arc.append_iter(1e-6) {
                if let kurbo::PathEl::CurveTo(a, b, e) = el {
                    segs.push(PathSeg::Cubic(kurbo::CubicBez::new(cur, a, b, e)));
                    cur = e;
                }
            }
            // Snap the arc end exactly onto the next vertex.
            if let Some(PathSeg::Cubic(last)) = segs.last_mut() {
                last.p3 = p1;
            }
        }
    }
    (!segs.is_empty()).then(|| Contour::new(segs, pl.is_closed()))
}

/// Kerf offset: moves every closed contour away from the material by `kerf` (mm; negative
/// moves into the material). Outer contours grow and holes shrink, decided by nesting
/// depth, whatever their winding. Each result contour keeps the start point (moved to the
/// nearest point on the offset) and the travel direction of its source. Open subpaths are
/// returned unchanged, and so are closed subpaths with coordinates that are not usable numbers.
/// Circular arcs stay exact arcs; other curves are flattened within `tol`.
pub fn kerf_offset(path: &BezPath, kerf: f64, tol: f64) -> BezPath {
    if kerf == 0.0 || !kerf.is_finite() {
        return path.clone();
    }
    let cs = contours(path);
    if cs.is_empty() {
        return path.clone();
    }
    let offsettable = |c: &Contour| c.closed && usable_contour(c);
    let closed_idx: Vec<usize> = (0..cs.len()).filter(|i| offsettable(&cs[*i])).collect();
    let closed_paths: Vec<BezPath> = closed_idx.iter().map(|i| cs[*i].to_bezpath()).collect();
    let tree = containment(&closed_paths, tol);
    let mut out: Vec<Contour> = Vec::new();
    for (k, &ci) in closed_idx.iter().enumerate() {
        let c = &cs[ci];
        let area = c.signed_area();
        if area.abs() < 1e-12 {
            continue;
        }
        let s = area.signum();
        let outer = tree.depth[k].is_multiple_of(2);
        let d = -kerf * s * if outer { 1.0 } else { -1.0 };
        let pl = contour_to_pline(c, tol);
        if pl.vertex_count() < 2 {
            continue;
        }
        let results = pl.parallel_offset(d);
        let start = c.start().unwrap_or_default();
        for r in &results {
            let Some(mut rc) = pline_to_contour(r) else { continue };
            if rc.signed_area().signum() != s {
                rc = rc.reversed();
            }
            if let Some((sp, _)) = nearest_on_contour(&rc, start) {
                rc = contour_with_start(&rc, sp);
            }
            out.push(rc);
        }
    }
    for c in cs.iter().filter(|c| !offsettable(c)) {
        out.push(c.clone());
    }
    from_contours(out.iter())
}

/// Builds a cavalier polyline from raw vertices; exposed for tests and callers that hold
/// arc data already.
pub fn pline_from_vertices(v: &[(f64, f64, f64)], closed: bool) -> BezPath {
    let mut pl = CPline::with_capacity(v.len(), closed);
    for &(x, y, b) in v {
        pl.add_vertex(PlineVertex::new(x, y, b));
    }
    pline_to_contour(&pl).map(|c| c.to_bezpath()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path::{signed_area, start_point, subpaths};
    use crate::query::distance_to_path;
    use kurbo::{Circle, Rect, Shape};
    use std::f64::consts::PI;

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> BezPath {
        Rect::new(x0, y0, x1, y1).to_path(0.1)
    }
    fn area(ps: &[BezPath]) -> f64 {
        ps.iter().map(signed_area).sum()
    }
    fn opts(d: f64, side: OffsetSide, join: Join) -> OffsetOptions {
        OffsetOptions { distance: d, side, join, tol: 0.001, ..Default::default() }
    }

    #[test]
    fn outward_miter_bevel_round_areas() {
        let r = [rect(0.0, 0.0, 10.0, 10.0)];
        let m = offset(&r, &opts(1.0, OffsetSide::Outward, Join::Miter { limit: 4.0 }));
        assert!((area(&m.outward) - 144.0).abs() < 1e-3, "{}", area(&m.outward));
        let b = offset(&r, &opts(1.0, OffsetSide::Outward, Join::Bevel));
        assert!((area(&b.outward) - (144.0 - 2.0)).abs() < 1e-3, "{}", area(&b.outward));
        let rd = offset(&r, &opts(1.0, OffsetSide::Outward, Join::Round));
        assert!((area(&rd.outward) - (140.0 + PI)).abs() < 0.01, "{}", area(&rd.outward));
        // A low miter limit bevels square corners (miter ratio sqrt(2) > 1.2).
        let low = offset(&r, &opts(1.0, OffsetSide::Outward, Join::Miter { limit: 1.2 }));
        assert!(area(&low.outward) < 143.9);
    }

    #[test]
    fn inward_and_both() {
        let r = [rect(0.0, 0.0, 10.0, 10.0)];
        let i = offset(&r, &opts(1.0, OffsetSide::Inward, Join::Miter { limit: 4.0 }));
        assert!(i.outward.is_empty());
        assert!((area(&i.inward) - 64.0).abs() < 1e-3);
        let b = offset(&r, &opts(1.0, OffsetSide::Both, Join::Miter { limit: 4.0 }));
        assert_eq!(b.outward.len(), 1);
        assert_eq!(b.inward.len(), 1);
        assert_eq!(b.all().len(), 2);
        // Shrinking past half the width removes the shape.
        let gone = offset(&r, &opts(6.0, OffsetSide::Inward, Join::Round));
        assert!(gone.inward.is_empty());
    }

    #[test]
    fn holes_move_opposite() {
        let mut p = rect(0.0, 0.0, 20.0, 20.0);
        p.extend(rect(5.0, 5.0, 15.0, 15.0));
        let o = offset(&[p], &opts(1.0, OffsetSide::Outward, Join::Miter { limit: 4.0 }));
        // Outer 22x22 minus hole 8x8.
        assert!((area(&o.outward) - (484.0 - 64.0)).abs() < 1e-3, "{}", area(&o.outward));
    }

    #[test]
    fn outer_only_ignores_inner_shapes() {
        let mut p = rect(0.0, 0.0, 20.0, 20.0);
        p.extend(rect(5.0, 5.0, 15.0, 15.0));
        let mut o = opts(1.0, OffsetSide::Outward, Join::Miter { limit: 4.0 });
        o.outer_only = true;
        let r = offset(&[p, rect(8.0, 8.0, 9.0, 9.0)], &o);
        assert_eq!(r.outward.len(), 1);
        assert_eq!(subpaths(&r.outward[0]).len(), 1);
        assert!((area(&r.outward) - 484.0).abs() < 1e-3);
    }

    #[test]
    fn overlapping_results_merge() {
        let r = offset(&[rect(0.0, 0.0, 10.0, 10.0), rect(11.0, 0.0, 21.0, 10.0)], &opts(1.0, OffsetSide::Outward, Join::Round));
        assert_eq!(r.outward.len(), 1);
    }

    #[test]
    fn open_path_both_sides_with_caps() {
        let mut l = BezPath::new();
        l.move_to((0.0, 0.0));
        l.line_to((10.0, 0.0));
        let butt = offset_open(&l, 1.0, Join::Round, Cap::Butt, 0.001);
        assert!((area(&butt) - 20.0).abs() < 1e-3);
        let sq = offset_open(&l, 1.0, Join::Round, Cap::Square, 0.001);
        assert!((area(&sq) - 24.0).abs() < 1e-3);
        let rd = offset_open(&l, 1.0, Join::Round, Cap::Round, 0.001);
        assert!((area(&rd) - (20.0 + PI)).abs() < 0.01);
        // Through `offset`, open paths land in `outward`.
        let o = offset(&[l], &opts(1.0, OffsetSide::Inward, Join::Round));
        assert_eq!(o.outward.len(), 1);
        assert!(o.inward.is_empty());
    }

    #[test]
    fn degenerate_inputs() {
        assert_eq!(offset(&[], &OffsetOptions::default()), OffsetResult::default());
        assert_eq!(offset(&[rect(0.0, 0.0, 1.0, 1.0)], &opts(0.0, OffsetSide::Both, Join::Round)), OffsetResult::default());
        assert_eq!(offset(&[rect(0.0, 0.0, 1.0, 1.0)], &opts(f64::NAN, OffsetSide::Both, Join::Round)), OffsetResult::default());
        let mut pt = BezPath::new();
        pt.move_to((1.0, 1.0));
        assert!(offset_open(&pt, 1.0, Join::Round, Cap::Round, 0.01).is_empty());
        assert!(kerf_offset(&BezPath::new(), 0.1, 0.01).is_empty());
    }

    #[test]
    fn self_intersecting_input_is_resolved_first() {
        let mut b = BezPath::new();
        b.move_to((0.0, 0.0));
        b.line_to((10.0, 10.0));
        b.line_to((10.0, 0.0));
        b.line_to((0.0, 10.0));
        b.close_path();
        let o = offset(&[b], &opts(0.5, OffsetSide::Outward, Join::Round));
        assert_eq!(o.outward.len(), 1);
        assert!(area(&o.outward) > 50.0);
    }

    #[test]
    fn kerf_keeps_start_and_direction() {
        // Clockwise rectangle starting at (10, 5) (the middle of the right edge).
        let mut p = BezPath::new();
        p.move_to((10.0, 5.0));
        p.line_to((10.0, 0.0));
        p.line_to((0.0, 0.0));
        p.line_to((0.0, 10.0));
        p.line_to((10.0, 10.0));
        p.close_path();
        let k = kerf_offset(&p, 0.1, 0.001);
        assert!(signed_area(&k) < 0.0, "direction kept");
        let s = start_point(&k).unwrap();
        assert!(s.distance(Point::new(10.1, 5.0)) < 1e-6, "{s:?}");
        assert!((signed_area(&k).abs() - 10.2 * 10.2).abs() < 0.01 + (4.0 - PI) * 0.01);
        // Negative kerf shrinks.
        let k2 = kerf_offset(&p, -0.1, 0.001);
        assert!((signed_area(&k2).abs() - 9.8 * 9.8).abs() < 1e-6);
    }

    #[test]
    fn kerf_circle_stays_circle_and_holes_shrink() {
        let c = Circle::new((0.0, 0.0), 10.0).to_path(1e-6);
        let k = kerf_offset(&c, 0.5, 0.001);
        for i in 0..50 {
            let a = i as f64 / 50.0 * std::f64::consts::TAU;
            let q = Point::new(10.5 * a.cos(), 10.5 * a.sin());
            assert!(distance_to_path(&k, q) < 1e-3);
        }
        // Ring: hole wound the same way as the outer still shrinks.
        let mut ring = rect(0.0, 0.0, 20.0, 20.0);
        ring.extend(rect(5.0, 5.0, 15.0, 15.0));
        let kr = kerf_offset(&ring, 1.0, 0.001);
        let subs = subpaths(&kr);
        assert_eq!(subs.len(), 2);
        let a0 = signed_area(&subs[0]).abs();
        let a1 = signed_area(&subs[1]).abs();
        assert!(a0 > 400.0 && a1 < 100.0, "{a0} {a1}");
    }

    #[test]
    fn kerf_leaves_open_paths() {
        let mut l = BezPath::new();
        l.move_to((0.0, 0.0));
        l.line_to((5.0, 0.0));
        assert_eq!(kerf_offset(&l, 0.2, 0.01), l);
    }

    #[test]
    fn coordinates_that_are_not_numbers_do_not_panic() {
        for bad in [f64::NAN, f64::INFINITY, 1e300] {
            let mut p = BezPath::new();
            p.move_to((0.0, 0.0));
            p.line_to((10.0, bad));
            p.line_to((10.0, 10.0));
            p.close_path();
            let good = rect(20.0, 0.0, 30.0, 10.0);
            // The shape that can be offset still is; the broken one gives nothing.
            let o = offset(&[p.clone(), good.clone()], &opts(1.0, OffsetSide::Both, Join::Miter { limit: 4.0 }));
            assert!((area(&o.outward) - 144.0).abs() < 1e-3, "{bad}");
            assert!((area(&o.inward) - 64.0).abs() < 1e-3, "{bad}");
            let mut l = BezPath::new();
            l.move_to((0.0, 0.0));
            l.line_to((bad, 0.0));
            l.line_to((5.0, 5.0));
            assert!(offset_open(&l, 1.0, Join::Round, Cap::Round, 0.001).is_empty());
            // Kerf leaves the broken contour alone and still offsets the good one.
            let mut both = p.clone();
            both.extend(good.clone());
            let k = kerf_offset(&both, 0.5, 0.001);
            let subs = subpaths(&k);
            assert_eq!(subs.len(), 2);
            assert!(subs.contains(&p) || bad.is_nan(), "the broken contour is passed through");
            assert!(subs.iter().any(|s| (signed_area(s) - (121.0 - (4.0 - PI) * 0.25)).abs() < 1e-3));
            let mut curve = BezPath::new();
            curve.move_to((0.0, 0.0));
            curve.curve_to((1.0, bad), (2.0, 2.0), (3.0, 0.0));
            curve.close_path();
            assert_eq!(subpaths(&kerf_offset(&curve, 0.1, 0.001)).len(), 1);
        }
        assert_eq!(kerf_offset(&rect(0.0, 0.0, 1.0, 1.0), f64::NAN, 0.01), rect(0.0, 0.0, 1.0, 1.0));
    }

    #[test]
    fn pline_bulge_round_trip() {
        // Two half circles = full circle radius 0.5 centred at (0.5, 0).
        let p = pline_from_vertices(&[(0.0, 0.0, 1.0), (1.0, 0.0, 1.0)], true);
        assert!((signed_area(&p) - PI * 0.25).abs() < 1e-4);
    }
}
