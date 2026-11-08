//! Basic path model shared by every module: contours (subpaths as segment lists),
//! polylines, flattening, closed/open detection, area, length, bounding box,
//! direction, reversing and start points.
//!
//! All geometry is in millimetres with Y up. A [`BezPath`] may hold several
//! subpaths (a compound shape: an outer contour and its holes, or several pieces).

use kurbo::{
    BezPath, CubicBez, Line, ParamCurve, ParamCurveArclen, ParamCurveArea, ParamCurveExtrema, ParamCurveNearest, PathEl, PathSeg, Point, Rect, Vec2,
};

/// Default accuracy for arc length and nearest-point queries (mm).
pub const DEFAULT_ACCURACY: f64 = 1e-6;

/// How the inside of a self-overlapping or compound shape is decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FillRule {
    /// Inside when a ray crosses the boundary an odd number of times. Laser software default.
    #[default]
    EvenOdd,
    /// Inside when the winding number is not zero.
    NonZero,
}

/// One subpath as a list of segments. Quadratic segments are promoted to cubics so
/// every segment is either a [`PathSeg::Line`] or a [`PathSeg::Cubic`].
///
/// Node `i` is the start point of segment `i`. A closed contour with `n` segments has
/// `n` nodes; an open contour has `n + 1` (the last node is the end of the last segment).
/// A lone `MoveTo` becomes a contour with no segments and is dropped by [`contours`].
#[derive(Clone, Debug, PartialEq)]
pub struct Contour {
    pub segs: Vec<PathSeg>,
    pub closed: bool,
}

impl Contour {
    pub fn new(segs: Vec<PathSeg>, closed: bool) -> Self {
        Self { segs, closed }
    }

    /// Start point (node 0).
    pub fn start(&self) -> Option<Point> {
        self.segs.first().map(|s| s.start())
    }

    /// End point of the last segment.
    pub fn end(&self) -> Option<Point> {
        self.segs.last().map(|s| s.end())
    }

    /// Number of nodes (see the type docs).
    pub fn node_count(&self) -> usize {
        if self.segs.is_empty() {
            0
        } else if self.closed {
            self.segs.len()
        } else {
            self.segs.len() + 1
        }
    }

    /// Position of node `i`.
    pub fn node(&self, i: usize) -> Option<Point> {
        if i < self.segs.len() {
            Some(self.segs[i].start())
        } else if !self.closed && i == self.segs.len() {
            self.end()
        } else {
            None
        }
    }

    /// Positions of all nodes.
    pub fn nodes(&self) -> Vec<Point> {
        (0..self.node_count()).filter_map(|i| self.node(i)).collect()
    }

    /// Signed area (positive = counter-clockwise with Y up). Open contours are treated as
    /// closed by a straight line from end to start.
    pub fn signed_area(&self) -> f64 {
        let mut a: f64 = self.segs.iter().map(|s| s.signed_area()).sum();
        if let (Some(s), Some(e)) = (self.start(), self.end())
            && !self.closed
        {
            a += PathSeg::Line(Line::new(e, s)).signed_area();
        }
        a
    }

    /// Length of the contour.
    pub fn length(&self) -> f64 {
        self.segs.iter().map(|s| s.arclen(DEFAULT_ACCURACY)).sum()
    }

    /// Tight bounding box, or `None` when empty.
    pub fn bounding_box(&self) -> Option<Rect> {
        self.segs.iter().map(|s| s.bounding_box()).reduce(|a, b| a.union(b))
    }

    /// The same contour traversed the other way. The start node stays the start node for
    /// closed contours (only the direction flips); open contours start at their old end.
    pub fn reversed(&self) -> Contour {
        let segs: Vec<PathSeg> = self.segs.iter().rev().map(|s| s.reverse()).collect();
        Contour { segs, closed: self.closed }
    }

    /// True when the contour is closed (explicitly or because its ends meet within `tol`).
    pub fn is_closed_within(&self, tol: f64) -> bool {
        self.closed || matches!((self.start(), self.end()), (Some(s), Some(e)) if s.distance(e) <= tol && self.segs.len() > 1)
    }

    /// Converts back to a single-subpath [`BezPath`].
    pub fn to_bezpath(&self) -> BezPath {
        let mut p = BezPath::new();
        append_contour(&mut p, self);
        p
    }
}

/// Promotes quads to cubics so downstream code sees only lines and cubics.
pub fn normalize_seg(seg: PathSeg) -> PathSeg {
    match seg {
        PathSeg::Quad(q) => PathSeg::Cubic(q.raise()),
        s => s,
    }
}

/// Splits a path into contours. Empty subpaths (a lone `MoveTo`) are dropped. A closed
/// subpath whose last point differs from its start gets an explicit closing line segment.
pub fn contours(path: &BezPath) -> Vec<Contour> {
    let mut out = Vec::new();
    let mut segs: Vec<PathSeg> = Vec::new();
    let mut start = Point::ZERO;
    let mut cur = Point::ZERO;
    let flush = |segs: &mut Vec<PathSeg>, closed: bool, out: &mut Vec<Contour>| {
        if !segs.is_empty() {
            out.push(Contour { segs: std::mem::take(segs), closed });
        }
    };
    for el in path.elements() {
        match *el {
            PathEl::MoveTo(p) => {
                flush(&mut segs, false, &mut out);
                start = p;
                cur = p;
            }
            PathEl::LineTo(p) => {
                segs.push(PathSeg::Line(Line::new(cur, p)));
                cur = p;
            }
            PathEl::QuadTo(p1, p2) => {
                segs.push(normalize_seg(PathSeg::Quad(kurbo::QuadBez::new(cur, p1, p2))));
                cur = p2;
            }
            PathEl::CurveTo(p1, p2, p3) => {
                segs.push(PathSeg::Cubic(CubicBez::new(cur, p1, p2, p3)));
                cur = p3;
            }
            PathEl::ClosePath => {
                if !segs.is_empty() {
                    if cur != start {
                        segs.push(PathSeg::Line(Line::new(cur, start)));
                    }
                    flush(&mut segs, true, &mut out);
                }
                cur = start;
            }
        }
    }
    flush(&mut segs, false, &mut out);
    out
}

/// Appends one contour to a path. A closed contour ending in a straight line uses
/// `ClosePath` for that line, so [`contours`] round-trips exactly.
pub fn append_contour(p: &mut BezPath, c: &Contour) {
    let Some(start) = c.start() else { return };
    p.move_to(start);
    let n = c.segs.len();
    for (i, s) in c.segs.iter().enumerate() {
        let last = i + 1 == n;
        match *s {
            PathSeg::Line(l) => {
                if !(last && c.closed && l.p1 == start) {
                    p.line_to(l.p1);
                }
            }
            PathSeg::Quad(q) => p.quad_to(q.p1, q.p2),
            PathSeg::Cubic(cb) => p.curve_to(cb.p1, cb.p2, cb.p3),
        }
    }
    if c.closed {
        p.close_path();
    }
}

/// Joins contours into one path.
pub fn from_contours<'a>(cs: impl IntoIterator<Item = &'a Contour>) -> BezPath {
    let mut p = BezPath::new();
    for c in cs {
        append_contour(&mut p, c);
    }
    p
}

/// Each subpath as its own [`BezPath`] (empty subpaths dropped).
pub fn subpaths(path: &BezPath) -> Vec<BezPath> {
    contours(path).iter().map(Contour::to_bezpath).collect()
}

/// A flattened subpath.
#[derive(Clone, Debug, PartialEq)]
pub struct Polyline {
    pub points: Vec<Point>,
    pub closed: bool,
}

impl Polyline {
    pub fn new(points: Vec<Point>, closed: bool) -> Self {
        Self { points, closed }
    }

    /// Signed area (positive = counter-clockwise, Y up); open polylines close implicitly.
    pub fn signed_area(&self) -> f64 {
        polygon_signed_area(&self.points)
    }

    pub fn length(&self) -> f64 {
        let mut l: f64 = self.points.windows(2).map(|w| w[0].distance(w[1])).sum();
        if self.closed && self.points.len() > 1 {
            l += self.points[self.points.len() - 1].distance(self.points[0]);
        }
        l
    }

    pub fn to_contour(&self) -> Option<Contour> {
        if self.points.len() < 2 {
            return None;
        }
        let mut segs: Vec<PathSeg> = self.points.windows(2).map(|w| PathSeg::Line(Line::new(w[0], w[1]))).collect();
        if self.closed {
            let (a, b) = (self.points[self.points.len() - 1], self.points[0]);
            if a != b {
                segs.push(PathSeg::Line(Line::new(a, b)));
            }
        }
        Some(Contour { segs, closed: self.closed })
    }

    pub fn to_bezpath(&self) -> BezPath {
        self.to_contour().map(|c| c.to_bezpath()).unwrap_or_default()
    }
}

/// Shoelace signed area of a closed point ring.
pub fn polygon_signed_area(pts: &[Point]) -> f64 {
    if pts.len() < 3 {
        return 0.0;
    }
    let mut a = 0.0;
    for i in 0..pts.len() {
        let p = pts[i];
        let q = pts[(i + 1) % pts.len()];
        a += p.x * q.y - q.x * p.y;
    }
    a * 0.5
}

/// Flattens one segment to points (excluding the start point) within `tol`.
pub fn flatten_seg(seg: &PathSeg, tol: f64, out: &mut Vec<Point>) {
    match seg {
        PathSeg::Line(l) => out.push(l.p1),
        _ => {
            let mut first = true;
            kurbo::flatten([PathEl::MoveTo(seg.start()), seg.as_path_el()], tol.max(1e-9), |el| {
                if first {
                    first = false;
                    return;
                }
                if let Some(p) = el.end_point() {
                    out.push(p);
                }
            });
        }
    }
}

/// Flattens a contour to a polyline. Closed polylines do not repeat the start point.
pub fn flatten_contour(c: &Contour, tol: f64) -> Polyline {
    let mut pts = Vec::with_capacity(c.segs.len() + 1);
    if let Some(s) = c.start() {
        pts.push(s);
    }
    for seg in &c.segs {
        flatten_seg(seg, tol, &mut pts);
    }
    pts.dedup();
    if c.closed && pts.len() > 1 && pts[0] == pts[pts.len() - 1] {
        pts.pop();
    }
    Polyline { points: pts, closed: c.closed }
}

/// Flattens every subpath of a path to polylines within `tol` (mm, maximum deviation).
pub fn flatten_to_polylines(path: &BezPath, tol: f64) -> Vec<Polyline> {
    contours(path).iter().map(|c| flatten_contour(c, tol)).filter(|p| !p.points.is_empty()).collect()
}

/// Flattens curves to straight lines within `tol`, keeping subpath structure.
pub fn flatten(path: &BezPath, tol: f64) -> BezPath {
    let mut out = BezPath::new();
    for pl in flatten_to_polylines(path, tol) {
        if let Some(c) = pl.to_contour() {
            append_contour(&mut out, &c);
        }
    }
    out
}

/// Builds a path from polylines.
pub fn polylines_to_bezpath(pls: &[Polyline]) -> BezPath {
    let mut out = BezPath::new();
    for pl in pls {
        if let Some(c) = pl.to_contour() {
            append_contour(&mut out, &c);
        }
    }
    out
}

/// True when the path has at least one subpath and every subpath is closed (explicitly
/// with `ClosePath`, or with end points within `tol`).
pub fn is_closed(path: &BezPath, tol: f64) -> bool {
    let cs = contours(path);
    !cs.is_empty() && cs.iter().all(|c| c.is_closed_within(tol))
}

/// True when any subpath is open.
pub fn is_open(path: &BezPath, tol: f64) -> bool {
    contours(path).iter().any(|c| !c.is_closed_within(tol))
}

/// Signed area of a path (sum over subpaths; positive = counter-clockwise, Y up).
/// With holes wound opposite to their outer contour this is the filled area.
pub fn signed_area(path: &BezPath) -> f64 {
    contours(path).iter().map(Contour::signed_area).sum()
}

/// Filled area of a path under the given fill rule, measured by flattening within `tol`
/// and resolving overlaps. Correct for holes regardless of their winding direction.
pub fn area(path: &BezPath, rule: FillRule, tol: f64) -> f64 {
    let shapes = crate::boolean::resolve(std::slice::from_ref(path), rule, tol);
    shapes.iter().map(signed_area).sum::<f64>().abs()
}

/// Total length of all subpaths.
pub fn length(path: &BezPath) -> f64 {
    contours(path).iter().map(Contour::length).sum()
}

/// Tight bounding box of a path, or `None` when empty.
pub fn bounding_box(path: &BezPath) -> Option<Rect> {
    contours(path).iter().filter_map(Contour::bounding_box).reduce(|a, b| a.union(b))
}

/// Bounding box of several paths.
pub fn bounding_box_all(paths: &[BezPath]) -> Option<Rect> {
    paths.iter().filter_map(bounding_box).reduce(|a, b| a.union(b))
}

/// Travel direction of a closed contour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// Counter-clockwise with Y up.
    CounterClockwise,
    Clockwise,
    /// Zero area (a line, a point, or a figure-eight that cancels out).
    Degenerate,
}

/// Direction of a contour by the sign of its area.
pub fn contour_direction(c: &Contour) -> Direction {
    let a = c.signed_area();
    if a > 1e-12 {
        Direction::CounterClockwise
    } else if a < -1e-12 {
        Direction::Clockwise
    } else {
        Direction::Degenerate
    }
}

/// Direction of the first subpath of `path`.
pub fn direction(path: &BezPath) -> Direction {
    contours(path).first().map(contour_direction).unwrap_or(Direction::Degenerate)
}

/// Reverses every subpath. Closed subpaths keep their start node; open ones start at
/// their old end.
pub fn reverse(path: &BezPath) -> BezPath {
    from_contours(contours(path).iter().map(Contour::reversed).collect::<Vec<_>>().iter())
}

/// Forces every closed subpath to the given direction (degenerate ones are left alone).
pub fn set_direction(path: &BezPath, dir: Direction) -> BezPath {
    let cs: Vec<Contour> = contours(path)
        .into_iter()
        .map(|c| {
            let d = contour_direction(&c);
            if d != Direction::Degenerate && dir != Direction::Degenerate && d != dir { c.reversed() } else { c }
        })
        .collect();
    from_contours(cs.iter())
}

/// Start point of the first subpath.
pub fn start_point(path: &BezPath) -> Option<Point> {
    contours(path).first().and_then(Contour::start)
}

/// Splits segment `seg` at parameter `t` into two halves.
pub fn split_seg(seg: &PathSeg, t: f64) -> (PathSeg, PathSeg) {
    (seg.subsegment(0.0..t), seg.subsegment(t..1.0))
}

/// Location on a contour: segment index and parameter in `0..=1`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SegParam {
    pub seg: usize,
    pub t: f64,
}

/// Nearest location on a contour to `p`, with squared distance.
pub fn nearest_on_contour(c: &Contour, p: Point) -> Option<(SegParam, f64)> {
    let mut best: Option<(SegParam, f64)> = None;
    for (i, s) in c.segs.iter().enumerate() {
        let n = s.nearest(p, DEFAULT_ACCURACY);
        if best.is_none_or(|(_, d)| n.distance_sq < d) {
            best = Some((SegParam { seg: i, t: n.t }, n.distance_sq));
        }
    }
    best
}

/// Moves the start of a closed contour to location `at`, splitting a segment there if
/// needed. Open contours are returned unchanged.
pub fn contour_with_start(c: &Contour, at: SegParam) -> Contour {
    if !c.closed || c.segs.is_empty() {
        return c.clone();
    }
    let eps = 1e-9;
    let mut segs = c.segs.clone();
    let mut idx = at.seg.min(segs.len() - 1);
    if at.t >= 1.0 - eps {
        idx = (idx + 1) % segs.len();
    } else if at.t > eps {
        let (a, b) = split_seg(&segs[idx], at.t);
        segs[idx] = a;
        segs.insert(idx + 1, b);
        idx += 1;
    }
    segs.rotate_left(idx);
    Contour { segs, closed: true }
}

/// Sets the start point of every closed subpath to the point nearest `p` (each subpath
/// independently, as the start-point tool does for compound shapes when `all` is true;
/// otherwise only the subpath nearest `p` is changed).
pub fn set_start_point(path: &BezPath, p: Point, all: bool) -> BezPath {
    let cs = contours(path);
    let nearest_idx =
        cs.iter().enumerate().filter_map(|(i, c)| nearest_on_contour(c, p).map(|(_, d)| (i, d))).min_by(|a, b| a.1.total_cmp(&b.1)).map(|(i, _)| i);
    let out: Vec<Contour> = cs
        .iter()
        .enumerate()
        .map(|(i, c)| {
            if all || Some(i) == nearest_idx {
                match nearest_on_contour(c, p) {
                    Some((sp, _)) => contour_with_start(c, sp),
                    None => c.clone(),
                }
            } else {
                c.clone()
            }
        })
        .collect();
    from_contours(out.iter())
}

/// Unit tangent of a segment at `t` (falls back to the chord for degenerate handles).
pub fn seg_tangent(seg: &PathSeg, t: f64) -> Vec2 {
    let d = match seg {
        PathSeg::Line(l) => l.p1 - l.p0,
        PathSeg::Quad(q) => kurbo::ParamCurveDeriv::deriv(q).eval(t).to_vec2(),
        PathSeg::Cubic(c) => {
            let d = kurbo::ParamCurveDeriv::deriv(c).eval(t).to_vec2();
            if d.hypot2() > 1e-24 {
                d
            } else {
                // Coincident handle: use the next handle direction.
                let probe = if t < 0.5 { seg.eval(t + 1e-4) - seg.eval(t) } else { seg.eval(t) - seg.eval(t - 1e-4) };
                probe
            }
        }
    };
    let d = if d.hypot2() > 1e-24 { d } else { seg.end() - seg.start() };
    if d.hypot2() > 0.0 { d.normalize() } else { Vec2::ZERO }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::Shape;

    pub(crate) fn rect_path(x0: f64, y0: f64, x1: f64, y1: f64) -> BezPath {
        Rect::new(x0, y0, x1, y1).to_path(0.1)
    }

    #[test]
    fn contour_round_trip_rect() {
        let p = rect_path(0.0, 0.0, 10.0, 5.0);
        let cs = contours(&p);
        assert_eq!(cs.len(), 1);
        assert!(cs[0].closed);
        assert_eq!(cs[0].node_count(), 4);
        let back = from_contours(cs.iter());
        assert_eq!(contours(&back), cs);
    }

    #[test]
    fn signed_area_ccw_positive() {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((10.0, 0.0));
        p.line_to((10.0, 10.0));
        p.line_to((0.0, 10.0));
        p.close_path();
        assert!((signed_area(&p) - 100.0).abs() < 1e-9);
        assert_eq!(direction(&p), Direction::CounterClockwise);
        let r = reverse(&p);
        assert!((signed_area(&r) + 100.0).abs() < 1e-9);
        assert_eq!(start_point(&r), Some(Point::new(0.0, 0.0)));
    }

    #[test]
    fn circle_area_and_length() {
        let c = kurbo::Circle::new((0.0, 0.0), 10.0).to_path(1e-4);
        assert!((signed_area(&c).abs() - std::f64::consts::PI * 100.0).abs() < 1e-2);
        assert!((length(&c) - std::f64::consts::TAU * 10.0).abs() < 1e-2);
        let bb = bounding_box(&c).unwrap();
        assert!((bb.width() - 20.0).abs() < 1e-6);
    }

    #[test]
    fn open_closed_detection() {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((10.0, 0.0));
        p.line_to((10.0, 10.0));
        assert!(is_open(&p, 0.01));
        p.line_to((0.0, 0.005));
        assert!(is_closed(&p, 0.01));
        assert!(!is_closed(&p, 0.001));
        assert!(!is_closed(&BezPath::new(), 0.1));
    }

    #[test]
    fn lone_moveto_dropped() {
        let mut p = BezPath::new();
        p.move_to((1.0, 1.0));
        p.move_to((2.0, 2.0));
        assert!(contours(&p).is_empty());
        assert_eq!(length(&p), 0.0);
        assert!(bounding_box(&p).is_none());
    }

    #[test]
    fn set_start_point_splits_segment() {
        let p = rect_path(0.0, 0.0, 10.0, 10.0);
        let q = set_start_point(&p, Point::new(5.0, -1.0), true);
        assert_eq!(start_point(&q), Some(Point::new(5.0, 0.0)));
        assert!((signed_area(&q) - signed_area(&p)).abs() < 1e-9);
        assert_eq!(contours(&q)[0].segs.len(), 5);
    }

    #[test]
    fn flatten_circle_within_tolerance() {
        let c = kurbo::Circle::new((0.0, 0.0), 10.0).to_path(1e-6);
        let f = flatten_to_polylines(&c, 0.01);
        assert_eq!(f.len(), 1);
        assert!(f[0].closed);
        for p in &f[0].points {
            assert!((p.to_vec2().hypot() - 10.0).abs() < 0.011);
        }
        // Midpoints of chords stay within tolerance too.
        for w in f[0].points.windows(2) {
            let m = w[0].midpoint(w[1]);
            assert!(10.0 - m.to_vec2().hypot() < 0.011);
        }
    }

    #[test]
    fn quads_promoted() {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.quad_to((5.0, 10.0), (10.0, 0.0));
        let c = contours(&p);
        assert!(matches!(c[0].segs[0], PathSeg::Cubic(_)));
    }
}
