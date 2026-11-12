//! Small polyline and polygon helpers.

use kurbo::{BezPath, PathEl, Point, Rect, Vec2};

/// Points closer than this are treated as the same point, mm.
pub const EPS: f64 = 1e-6;

/// A flattened subpath.
#[derive(Clone, Debug, PartialEq)]
pub struct Poly {
    /// Vertices. Closed polygons do not repeat the first vertex at the end.
    pub pts: Vec<Point>,
    pub closed: bool,
}

/// Flatten a path into polylines with the given tolerance (mm).
pub fn flatten(path: &BezPath, tolerance: f64) -> Vec<Poly> {
    let mut out = Vec::new();
    let mut cur: Vec<Point> = Vec::new();
    let finish = |cur: &mut Vec<Point>, closed: bool, out: &mut Vec<Poly>| {
        let mut pts = std::mem::take(cur);
        pts.dedup_by(|a, b| a.distance(*b) < EPS);
        let mut closed = closed;
        if pts.len() > 2 && pts[0].distance(*pts.last().unwrap()) < EPS {
            pts.pop();
            closed = true;
        } else if closed && pts.len() > 1 && pts[0].distance(*pts.last().unwrap()) < EPS {
            pts.pop();
        }
        if closed && pts.len() < 3 {
            closed = false;
        }
        if pts.len() >= 2 {
            out.push(Poly { pts, closed });
        }
    };
    kurbo::flatten(path.iter(), tolerance.max(1e-4), |el| match el {
        PathEl::MoveTo(p) => {
            finish(&mut cur, false, &mut out);
            cur.push(p);
        }
        PathEl::LineTo(p) => cur.push(p),
        PathEl::ClosePath => finish(&mut cur, true, &mut out),
        _ => {}
    });
    finish(&mut cur, false, &mut out);
    out
}

/// Signed area (positive = counter-clockwise).
pub fn signed_area(pts: &[Point]) -> f64 {
    let n = pts.len();
    if n < 3 {
        return 0.0;
    }
    let mut a = 0.0;
    for i in 0..n {
        let p = pts[i];
        let q = pts[(i + 1) % n];
        a += p.x * q.y - q.x * p.y;
    }
    a * 0.5
}

/// Even-odd point in polygon test.
pub fn point_in_polygon(p: Point, pts: &[Point]) -> bool {
    let n = pts.len();
    let mut inside = false;
    let mut j = n.wrapping_sub(1);
    for i in 0..n {
        let a = pts[i];
        let b = pts[j];
        if (a.y > p.y) != (b.y > p.y) {
            let x = a.x + (p.y - a.y) * (b.x - a.x) / (b.y - a.y);
            if p.x < x {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

/// Bounding box of points (zero rect for an empty slice).
pub fn bbox(pts: &[Point]) -> Rect {
    let mut it = pts.iter();
    let Some(first) = it.next() else { return Rect::ZERO };
    let mut r = Rect::from_points(*first, *first);
    for p in it {
        r = r.union_pt(*p);
    }
    r
}

/// Rect `a` fully inside rect `b` (with a small tolerance).
pub fn rect_inside(a: Rect, b: Rect) -> bool {
    a.x0 >= b.x0 - EPS && a.y0 >= b.y0 - EPS && a.x1 <= b.x1 + EPS && a.y1 <= b.y1 + EPS
}

/// Total length of a polyline.
pub fn polyline_length(pts: &[Point]) -> f64 {
    pts.windows(2).map(|w| w[0].distance(w[1])).sum()
}

/// Distance from `p` to segment `a`-`b`.
pub fn dist_point_segment(p: Point, a: Point, b: Point) -> f64 {
    let ab = b - a;
    let len2 = ab.hypot2();
    if len2 < EPS * EPS {
        return p.distance(a);
    }
    let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

/// Parameter (arc length) of the point on a polyline nearest to `p`.
pub fn nearest_arclen(pts: &[Point], p: Point) -> (f64, f64) {
    let mut best = (f64::INFINITY, 0.0);
    let mut s = 0.0;
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let ab = b - a;
        let len = ab.hypot();
        let t = if len < EPS { 0.0 } else { ((p - a).dot(ab) / (len * len)).clamp(0.0, 1.0) };
        let d = p.distance(a + ab * t);
        if d < best.0 {
            best = (d, s + t * len);
        }
        s += len;
    }
    (best.1, best.0)
}

/// Split a polyline at the given increasing arc lengths. Returns `pieces.len() + 1` polylines.
pub fn split_at(pts: &[Point], cuts: &[f64]) -> Vec<Vec<Point>> {
    let mut out = vec![vec![pts[0]]];
    let mut ci = 0;
    let mut s = 0.0;
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let len = a.distance(b);
        while ci < cuts.len() && cuts[ci] <= s + len + EPS && cuts[ci] >= s - EPS {
            let t = if len < EPS { 0.0 } else { ((cuts[ci] - s) / len).clamp(0.0, 1.0) };
            let q = a.lerp(b, t);
            let last = out.last_mut().unwrap();
            if last.last().is_none_or(|l| l.distance(q) > EPS) {
                last.push(q);
            }
            out.push(vec![q]);
            ci += 1;
        }
        let last = out.last_mut().unwrap();
        if last.last().is_none_or(|l| l.distance(b) > EPS) {
            last.push(b);
        }
        s += len;
    }
    while out.len() < cuts.len() + 1 {
        out.push(vec![*pts.last().unwrap()]);
    }
    out
}

/// Point at arc length `s` along a polyline, and the unit tangent there.
pub fn point_at(pts: &[Point], s: f64) -> (Point, Vec2) {
    let mut acc = 0.0;
    for w in pts.windows(2) {
        let len = w[0].distance(w[1]);
        if len < EPS {
            continue;
        }
        if acc + len >= s {
            let t = ((s - acc) / len).clamp(0.0, 1.0);
            return (w[0].lerp(w[1], t), (w[1] - w[0]) / len);
        }
        acc += len;
    }
    let n = pts.len();
    let dir = if n >= 2 { unit(pts[n - 1] - pts[n - 2]) } else { Vec2::new(1.0, 0.0) };
    (pts[n - 1], dir)
}

/// Unit vector (or zero).
pub fn unit(v: Vec2) -> Vec2 {
    let l = v.hypot();
    if l < EPS { Vec2::ZERO } else { v / l }
}

/// Rotate a vector by `angle` radians (counter-clockwise).
pub fn rotate(v: Vec2, angle: f64) -> Vec2 {
    let (s, c) = angle.sin_cos();
    Vec2::new(v.x * c - v.y * s, v.x * s + v.y * c)
}

/// Turning angle at vertex `i` of a closed polygon, radians in 0..=PI (0 = straight).
pub fn corner_angle(pts: &[Point], i: usize) -> f64 {
    let n = pts.len();
    let prev = pts[(i + n - 1) % n];
    let next = pts[(i + 1) % n];
    let a = unit(pts[i] - prev);
    let b = unit(next - pts[i]);
    if a == Vec2::ZERO || b == Vec2::ZERO {
        return 0.0;
    }
    a.dot(b).clamp(-1.0, 1.0).acos()
}

/// Rotate a point about the origin.
pub fn rotate_point(p: Point, angle: f64) -> Point {
    rotate(p.to_vec2(), angle).to_point()
}
