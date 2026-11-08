//! Queries: point in shape, winding, hit testing, nearest point, containment and nesting
//! of shapes (for inner-first cutting and fill grouping), segment intersections and
//! measuring helpers.

use crate::path::{Contour, DEFAULT_ACCURACY, FillRule, SegParam, contours, flatten_contour};
use kurbo::{BezPath, ParamCurve, ParamCurveArclen, ParamCurveExtrema, PathSeg, Point, Rect, Shape, Vec2};

/// Winding number of `p` with respect to `path`. Open subpaths are treated as closed by
/// a straight line from their end back to their start.
pub fn winding_number(path: &BezPath, p: Point) -> i32 {
    contours(path).iter().map(|c| contour_winding(c, p)).sum()
}

fn contour_winding(c: &Contour, p: Point) -> i32 {
    let mut cc = c.clone();
    cc.closed = true;
    if let (Some(s), Some(e)) = (cc.start(), cc.end())
        && s != e
    {
        cc.segs.push(PathSeg::Line(kurbo::Line::new(e, s)));
    }
    cc.to_bezpath().winding(p)
}

/// True when `p` is inside the filled shape under `rule`.
pub fn point_in_shape(path: &BezPath, p: Point, rule: FillRule) -> bool {
    let w = winding_number(path, p);
    match rule {
        FillRule::EvenOdd => w % 2 != 0,
        FillRule::NonZero => w != 0,
    }
}

/// Result of a nearest-point query.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PathHit {
    /// Index of the subpath (in [`contours`] order).
    pub subpath: usize,
    /// Segment index and parameter within that subpath.
    pub at: SegParam,
    /// The nearest point on the path.
    pub point: Point,
    /// Distance from the query point.
    pub distance: f64,
}

/// Nearest point on the outline of `path` to `p`.
pub fn nearest_point(path: &BezPath, p: Point) -> Option<PathHit> {
    let mut best: Option<PathHit> = None;
    for (ci, c) in contours(path).iter().enumerate() {
        for (si, s) in c.segs.iter().enumerate() {
            let n = kurbo::ParamCurveNearest::nearest(s, p, DEFAULT_ACCURACY);
            let d = n.distance_sq.sqrt();
            if best.is_none_or(|b| d < b.distance) {
                best = Some(PathHit { subpath: ci, at: SegParam { seg: si, t: n.t }, point: s.eval(n.t), distance: d });
            }
        }
    }
    best
}

/// Distance from `p` to the outline of `path` (hit testing). `f64::INFINITY` for an empty path.
pub fn distance_to_path(path: &BezPath, p: Point) -> f64 {
    nearest_point(path, p).map(|h| h.distance).unwrap_or(f64::INFINITY)
}

/// True when `p` is within `tol` of the outline, or inside the filled shape when `filled`.
pub fn hit_test(path: &BezPath, p: Point, tol: f64, filled: bool) -> bool {
    if filled && point_in_shape(path, p, FillRule::EvenOdd) {
        return true;
    }
    distance_to_path(path, p) <= tol
}

/// Which shapes sit inside which. Built by [`containment`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ContainmentTree {
    /// Smallest closed shape that contains shape `i`, if any.
    pub parent: Vec<Option<usize>>,
    /// Number of shapes around shape `i` (0 = outermost).
    pub depth: Vec<usize>,
    /// Direct children of shape `i`.
    pub children: Vec<Vec<usize>>,
}

impl ContainmentTree {
    /// Shape indices ordered so that every shape comes before the shapes containing it
    /// (inner-first cutting). Ties keep input order.
    pub fn inner_first_order(&self) -> Vec<usize> {
        let mut idx: Vec<usize> = (0..self.parent.len()).collect();
        idx.sort_by(|a, b| self.depth[*b].cmp(&self.depth[*a]));
        idx
    }

    /// Groups for filling: each shape at even depth together with its direct children
    /// (the holes in it). Shapes at odd depth that sit inside a hole start new groups only
    /// at even depth, so islands inside holes become their own group.
    pub fn fill_groups(&self) -> Vec<Vec<usize>> {
        let mut groups = Vec::new();
        for i in 0..self.parent.len() {
            if self.depth[i] % 2 == 0 {
                let mut g = vec![i];
                g.extend(self.children[i].iter().copied());
                groups.push(g);
            }
        }
        groups
    }

    /// Outermost shapes (depth 0).
    pub fn roots(&self) -> Vec<usize> {
        (0..self.parent.len()).filter(|i| self.parent[*i].is_none()).collect()
    }
}

struct Prepared {
    bbox: Option<Rect>,
    area: f64,
    closed: bool,
    samples: Vec<Point>,
}

fn prepare(path: &BezPath, tol: f64) -> Prepared {
    let cs = contours(path);
    let closed = !cs.is_empty() && cs.iter().all(|c| c.is_closed_within(tol));
    let mut samples = Vec::new();
    for c in &cs {
        let pl = flatten_contour(c, tol);
        samples.extend(pl.points.iter().copied());
        // Midpoints help when every vertex touches the outer boundary.
        for w in pl.points.windows(2) {
            samples.push(w[0].midpoint(w[1]));
        }
    }
    let area = cs.iter().map(|c| c.signed_area()).sum::<f64>().abs();
    Prepared { bbox: crate::path::bounding_box(path), area, closed, samples }
}

fn inside_prepared(outer_path: &BezPath, outer: &Prepared, inner: &Prepared, tol: f64) -> bool {
    if !outer.closed || inner.samples.is_empty() {
        return false;
    }
    let (Some(ob), Some(ib)) = (outer.bbox, inner.bbox) else { return false };
    if ib.x0 < ob.x0 - tol || ib.y0 < ob.y0 - tol || ib.x1 > ob.x1 + tol || ib.y1 > ob.y1 + tol {
        return false;
    }
    if inner.closed && inner.area >= outer.area {
        return false;
    }
    // Inside when no sample is clearly outside: samples on the boundary are tolerated.
    let mut inside_hits = 0;
    for &p in &inner.samples {
        if point_in_shape(outer_path, p, FillRule::NonZero) {
            inside_hits += 1;
        } else if distance_to_path(outer_path, p) > tol {
            return false;
        }
    }
    inside_hits > 0
}

/// True when shape `inner` lies within closed shape `outer` (boundary contact within
/// `tol` allowed). Open `inner` paths count when all their points are inside.
pub fn contains(outer: &BezPath, inner: &BezPath, tol: f64) -> bool {
    inside_prepared(outer, &prepare(outer, tol), &prepare(inner, tol), tol)
}

/// Computes which shapes are inside which. Each path is treated as one shape (its
/// subpaths together). Open shapes can be children but never parents.
pub fn containment(paths: &[BezPath], tol: f64) -> ContainmentTree {
    let prep: Vec<Prepared> = paths.iter().map(|p| prepare(p, tol)).collect();
    let n = paths.len();
    let mut parent = vec![None; n];
    // Sort candidates by area so the first container found is the smallest.
    let mut by_area: Vec<usize> = (0..n).filter(|i| prep[*i].closed).collect();
    by_area.sort_by(|a, b| prep[*a].area.total_cmp(&prep[*b].area));
    for i in 0..n {
        for &j in &by_area {
            if j == i || prep[j].area <= prep[i].area && prep[i].closed {
                continue;
            }
            if inside_prepared(&paths[j], &prep[j], &prep[i], tol) {
                parent[i] = Some(j);
                break;
            }
        }
    }
    let mut depth = vec![0; n];
    for i in 0..n {
        let mut d = 0;
        let mut cur = parent[i];
        while let Some(p) = cur {
            d += 1;
            cur = parent[p];
            if d > n {
                break;
            }
        }
        depth[i] = d;
    }
    let mut children = vec![Vec::new(); n];
    for i in 0..n {
        if let Some(p) = parent[i] {
            children[p].push(i);
        }
    }
    ContainmentTree { parent, depth, children }
}

/// Intersection of two segments as pairs of parameters `(t_a, t_b)`.
pub fn seg_intersections(a: &PathSeg, b: &PathSeg) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    match (a, b) {
        (PathSeg::Line(la), _) => {
            for i in b.intersect_line(*la) {
                out.push((i.line_t, i.segment_t));
            }
        }
        (_, PathSeg::Line(lb)) => {
            for i in a.intersect_line(*lb) {
                out.push((i.segment_t, i.line_t));
            }
        }
        _ => curve_curve(a, b, 0.0, 1.0, 0.0, 1.0, 0, &mut out),
    }
    out.sort_by(|x, y| x.0.total_cmp(&y.0));
    out.dedup_by(|x, y| (x.0 - y.0).abs() < 1e-7 && (x.1 - y.1).abs() < 1e-7);
    out
}

fn curve_curve(a: &PathSeg, b: &PathSeg, a0: f64, a1: f64, b0: f64, b1: f64, depth: u32, out: &mut Vec<(f64, f64)>) {
    let sa = a.subsegment(a0..a1);
    let sb = b.subsegment(b0..b1);
    let ba = ParamCurveExtrema::bounding_box(&sa);
    let bb = ParamCurveExtrema::bounding_box(&sb);
    if ba.x1 < bb.x0 || bb.x1 < ba.x0 || ba.y1 < bb.y0 || bb.y1 < ba.y0 {
        return;
    }
    let small = 1e-9;
    if depth > 48 || (ba.width().max(ba.height()) < small && bb.width().max(bb.height()) < small) {
        out.push(((a0 + a1) * 0.5, (b0 + b1) * 0.5));
        return;
    }
    // Once both pieces are nearly straight, intersect their chords.
    if depth > 6 && is_flat(&sa) && is_flat(&sb) {
        let la = kurbo::Line::new(sa.start(), sa.end());
        for i in PathSeg::Line(kurbo::Line::new(sb.start(), sb.end())).intersect_line(la) {
            out.push((a0 + i.line_t * (a1 - a0), b0 + i.segment_t * (b1 - b0)));
        }
        return;
    }
    let am = (a0 + a1) * 0.5;
    let bm = (b0 + b1) * 0.5;
    curve_curve(a, b, a0, am, b0, bm, depth + 1, out);
    curve_curve(a, b, a0, am, bm, b1, depth + 1, out);
    curve_curve(a, b, am, a1, b0, bm, depth + 1, out);
    curve_curve(a, b, am, a1, bm, b1, depth + 1, out);
}

fn is_flat(s: &PathSeg) -> bool {
    match s {
        PathSeg::Line(_) => true,
        PathSeg::Quad(q) => kurbo::Line::new(q.p0, q.p2).nearest_dist(q.p1) < 1e-10,
        PathSeg::Cubic(c) => {
            let l = kurbo::Line::new(c.p0, c.p3);
            l.nearest_dist(c.p1) < 1e-10 && l.nearest_dist(c.p2) < 1e-10
        }
    }
}

trait NearestDist {
    fn nearest_dist(&self, p: Point) -> f64;
}

impl NearestDist for kurbo::Line {
    fn nearest_dist(&self, p: Point) -> f64 {
        kurbo::ParamCurveNearest::nearest(self, p, 1e-12).distance_sq.sqrt()
    }
}

/// All crossing points between the outlines of two paths.
pub fn path_intersections(a: &BezPath, b: &BezPath) -> Vec<Point> {
    let ca = contours(a);
    let cb = contours(b);
    let mut out: Vec<Point> = Vec::new();
    for x in &ca {
        for sa in &x.segs {
            let ba = ParamCurveExtrema::bounding_box(sa);
            for y in &cb {
                for sb in &y.segs {
                    if !ba.inflate(1e-9, 1e-9).overlaps(ParamCurveExtrema::bounding_box(sb)) {
                        continue;
                    }
                    for (ta, _) in seg_intersections(sa, sb) {
                        let p = sa.eval(ta);
                        if !out.iter().any(|q| q.distance(p) < 1e-7) {
                            out.push(p);
                        }
                    }
                }
            }
        }
    }
    out
}

/// A circle arc found by measuring.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArcMeasure {
    pub center: Point,
    pub radius: f64,
    /// Signed sweep in radians (positive = counter-clockwise).
    pub sweep: f64,
    /// Arc length.
    pub length: f64,
}

/// Circle through three points, if they are not collinear.
pub fn circle_through(a: Point, b: Point, c: Point) -> Option<(Point, f64)> {
    let d = 2.0 * (a.x * (b.y - c.y) + b.x * (c.y - a.y) + c.x * (a.y - b.y));
    if d.abs() < 1e-12 {
        return None;
    }
    let a2 = a.to_vec2().hypot2();
    let b2 = b.to_vec2().hypot2();
    let c2 = c.to_vec2().hypot2();
    let ux = (a2 * (b.y - c.y) + b2 * (c.y - a.y) + c2 * (a.y - b.y)) / d;
    let uy = (a2 * (c.x - b.x) + b2 * (a.x - c.x) + c2 * (b.x - a.x)) / d;
    let center = Point::new(ux, uy);
    Some((center, center.distance(a)))
}

/// Arc from `start` through `mid` to `end`: centre, radius, signed sweep and length.
pub fn arc_through(start: Point, mid: Point, end: Point) -> Option<ArcMeasure> {
    let (center, radius) = circle_through(start, mid, end)?;
    let ang = |p: Point| (p - center).atan2();
    let a0 = ang(start);
    let am = ang(mid);
    let a1 = ang(end);
    let norm = |x: f64| x.rem_euclid(std::f64::consts::TAU);
    let ccw_to_mid = norm(am - a0);
    let ccw_to_end = norm(a1 - a0);
    let sweep = if ccw_to_mid <= ccw_to_end { ccw_to_end } else { ccw_to_end - std::f64::consts::TAU };
    Some(ArcMeasure { center, radius, sweep, length: radius * sweep.abs() })
}

/// Measures a segment as a circular arc when it is one within `tol` (sampled at 9 points);
/// returns `None` for straight lines and curves that are not arcs.
pub fn measure_arc(seg: &PathSeg, tol: f64) -> Option<ArcMeasure> {
    if matches!(seg, PathSeg::Line(_)) {
        return None;
    }
    let m = arc_through(seg.start(), seg.eval(0.5), seg.end())?;
    for i in 1..9 {
        let p = seg.eval(i as f64 / 9.0);
        if (p.distance(m.center) - m.radius).abs() > tol {
            return None;
        }
    }
    Some(ArcMeasure { length: seg.arclen(DEFAULT_ACCURACY), ..m })
}

/// Angle at `vertex` between rays to `a` and `b`, in radians `0..=π`.
pub fn angle_at(a: Point, vertex: Point, b: Point) -> f64 {
    let u = a - vertex;
    let v = b - vertex;
    let c = u.cross(v);
    let d = u.dot(v);
    c.atan2(d).abs()
}

/// Width and height of the bounding box of a set of paths.
pub fn size(paths: &[BezPath]) -> Option<Vec2> {
    crate::path::bounding_box_all(paths).map(|r| Vec2::new(r.width(), r.height()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::{Circle, CubicBez, Line};

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> BezPath {
        Rect::new(x0, y0, x1, y1).to_path(0.1)
    }

    #[test]
    fn point_in_with_hole_even_odd_vs_nonzero() {
        // Outer and hole wound the same way: even-odd sees a hole, nonzero does not.
        let mut p = rect(0.0, 0.0, 10.0, 10.0);
        p.extend(rect(3.0, 3.0, 7.0, 7.0));
        let centre = Point::new(5.0, 5.0);
        assert!(!point_in_shape(&p, centre, FillRule::EvenOdd));
        assert!(point_in_shape(&p, centre, FillRule::NonZero));
        assert!(point_in_shape(&p, Point::new(1.0, 1.0), FillRule::EvenOdd));
        assert!(!point_in_shape(&p, Point::new(11.0, 1.0), FillRule::NonZero));
    }

    #[test]
    fn open_path_point_in_closes_implicitly() {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((10.0, 0.0));
        p.line_to((10.0, 10.0));
        assert!(point_in_shape(&p, Point::new(8.0, 2.0), FillRule::EvenOdd));
        assert!(!point_in_shape(&p, Point::new(2.0, 8.0), FillRule::EvenOdd));
    }

    #[test]
    fn nearest_and_distance() {
        let c = Circle::new((0.0, 0.0), 10.0).to_path(1e-6);
        let h = nearest_point(&c, Point::new(20.0, 0.0)).unwrap();
        assert!((h.distance - 10.0).abs() < 1e-4);
        assert!((h.point.x - 10.0).abs() < 1e-4);
        assert!(hit_test(&c, Point::new(10.05, 0.0), 0.1, false));
        assert!(!hit_test(&c, Point::new(0.0, 0.0), 0.1, false));
        assert!(hit_test(&c, Point::new(0.0, 0.0), 0.1, true));
        assert_eq!(distance_to_path(&BezPath::new(), Point::ZERO), f64::INFINITY);
    }

    #[test]
    fn containment_tree_and_order() {
        let paths = vec![
            rect(0.0, 0.0, 100.0, 100.0),  // 0 outer
            rect(10.0, 10.0, 50.0, 50.0),  // 1 hole in 0
            rect(20.0, 20.0, 30.0, 30.0),  // 2 island in 1
            rect(200.0, 0.0, 210.0, 10.0), // 3 alone
            rect(60.0, 60.0, 70.0, 70.0),  // 4 hole in 0
        ];
        let t = containment(&paths, 1e-3);
        assert_eq!(t.parent, vec![None, Some(0), Some(1), None, Some(0)]);
        assert_eq!(t.depth, vec![0, 1, 2, 0, 1]);
        let order = t.inner_first_order();
        assert_eq!(order[0], 2);
        assert!(order.iter().position(|&i| i == 1) < order.iter().position(|&i| i == 0));
        let g = t.fill_groups();
        assert!(g.contains(&vec![0, 1, 4]));
        assert!(g.contains(&vec![2]));
        assert!(g.contains(&vec![3]));
        assert_eq!(t.roots(), vec![0, 3]);
    }

    #[test]
    fn containment_open_child_and_touching() {
        let mut line = BezPath::new();
        line.move_to((2.0, 2.0));
        line.line_to((8.0, 8.0));
        let outer = rect(0.0, 0.0, 10.0, 10.0);
        let touching = rect(0.0, 0.0, 5.0, 5.0);
        let t = containment(&[outer.clone(), line.clone(), touching.clone()], 1e-6);
        assert_eq!(t.parent[1], Some(0));
        assert_eq!(t.parent[2], Some(0));
        // Overlapping but not inside.
        assert!(!contains(&outer, &rect(5.0, 5.0, 15.0, 15.0), 1e-6));
        // Open shapes are never parents.
        assert!(!contains(&line, &rect(4.0, 4.0, 4.1, 4.1), 1e-6));
    }

    #[test]
    fn identical_shapes_do_not_nest() {
        let t = containment(&[rect(0.0, 0.0, 1.0, 1.0), rect(0.0, 0.0, 1.0, 1.0)], 1e-6);
        assert_eq!(t.parent, vec![None, None]);
    }

    #[test]
    fn intersections_line_line_and_curve_curve() {
        let a = PathSeg::Line(Line::new((0.0, 0.0), (10.0, 10.0)));
        let b = PathSeg::Line(Line::new((0.0, 10.0), (10.0, 0.0)));
        let i = seg_intersections(&a, &b);
        assert_eq!(i.len(), 1);
        assert!((i[0].0 - 0.5).abs() < 1e-9);
        let c1 = PathSeg::Cubic(CubicBez::new((0.0, 0.0), (3.0, 10.0), (7.0, 10.0), (10.0, 0.0)));
        let c2 = PathSeg::Cubic(CubicBez::new((0.0, 5.0), (3.0, -5.0), (7.0, -5.0), (10.0, 5.0)));
        let i = seg_intersections(&c1, &c2);
        assert_eq!(i.len(), 2, "{i:?}");
        for (ta, tb) in i {
            assert!(c1.eval(ta).distance(c2.eval(tb)) < 1e-6);
        }
        let pts = path_intersections(&Circle::new((0.0, 0.0), 5.0).to_path(1e-6), &rect(0.0, -10.0, 20.0, 10.0));
        assert_eq!(pts.len(), 2);
    }

    #[test]
    fn arc_measures() {
        let m = arc_through(Point::new(10.0, 0.0), Point::new(0.0, 10.0), Point::new(-10.0, 0.0)).unwrap();
        assert!((m.radius - 10.0).abs() < 1e-9);
        assert!((m.sweep - std::f64::consts::PI).abs() < 1e-9);
        let cw = arc_through(Point::new(-10.0, 0.0), Point::new(0.0, 10.0), Point::new(10.0, 0.0)).unwrap();
        assert!((cw.sweep + std::f64::consts::PI).abs() < 1e-9);
        assert!(arc_through(Point::ZERO, Point::new(1.0, 1.0), Point::new(2.0, 2.0)).is_none());
        let circle = Circle::new((0.0, 0.0), 10.0).to_path(1e-6);
        let seg = crate::path::contours(&circle)[0].segs[0];
        let a = measure_arc(&seg, 0.01).unwrap();
        assert!((a.radius - 10.0).abs() < 0.01);
        assert!((a.sweep.abs() * a.radius - a.length).abs() < 1e-3);
        assert!(measure_arc(&PathSeg::Line(Line::new((0.0, 0.0), (1.0, 0.0))), 0.01).is_none());
        assert!((angle_at(Point::new(1.0, 0.0), Point::ZERO, Point::new(0.0, 1.0)) - std::f64::consts::FRAC_PI_2).abs() < 1e-12);
        assert_eq!(size(&[rect(0.0, 0.0, 3.0, 4.0)]), Some(Vec2::new(3.0, 4.0)));
    }
}
