//! Geometry helpers for the job compiler: flattening, hatch fill, wobble, clipping, ordering.
//! All coordinates are millimetres.

use kurbo::{BezPath, PathEl, Point, Rect};

/// One flattened sub-path.
#[derive(Clone, Debug, PartialEq)]
pub struct Poly {
    pub pts: Vec<Point>,
    pub closed: bool,
}

/// Flattens a path into polylines. Zero-length repeats are removed.
pub fn flatten_path(path: &BezPath, tolerance: f64) -> Vec<Poly> {
    let mut out: Vec<Poly> = Vec::new();
    let mut cur: Vec<Point> = Vec::new();
    let flush = |cur: &mut Vec<Point>, closed: bool, out: &mut Vec<Poly>| {
        if !cur.is_empty() {
            let mut pts = std::mem::take(cur);
            pts.dedup_by(|a, b| (a.x - b.x).abs() < 1e-9 && (a.y - b.y).abs() < 1e-9);
            if closed && pts.len() > 1 && pts.first() == pts.last() {
                pts.pop();
            }
            out.push(Poly { pts, closed });
        }
    };
    kurbo::flatten(path.elements().iter().copied(), tolerance.max(1e-6), |el| match el {
        PathEl::MoveTo(p) => {
            flush(&mut cur, false, &mut out);
            cur.push(p);
        }
        PathEl::LineTo(p) => cur.push(p),
        PathEl::ClosePath => flush(&mut cur, true, &mut out),
        _ => {}
    });
    flush(&mut cur, false, &mut out);
    out
}

/// Bounding box of polylines.
pub fn polys_bounds(polys: &[Poly]) -> Option<Rect> {
    let mut r: Option<Rect> = None;
    for p in polys.iter().flat_map(|p| p.pts.iter()) {
        r = Some(match r {
            None => Rect::new(p.x, p.y, p.x, p.y),
            Some(r) => Rect::new(r.x0.min(p.x), r.y0.min(p.y), r.x1.max(p.x), r.y1.max(p.y)),
        });
    }
    r
}

fn rot(p: Point, c: f64, s: f64) -> Point {
    Point::new(p.x * c - p.y * s, p.x * s + p.y * c)
}

/// Even-odd scan-line hatch of closed shapes.
///
/// Lines run along `angle_deg` and are `interval` apart. Every other line is reversed when
/// `bidirectional` is set. The result is grouped per scan line: `lines[i]` holds the segments of
/// line `i` in travel order.
pub fn hatch(polys: &[Poly], interval: f64, angle_deg: f64, bidirectional: bool) -> Vec<Vec<(Point, Point)>> {
    let interval = interval.max(1e-4);
    let (s, c) = (-angle_deg.to_radians()).sin_cos();
    // rotate by -angle so hatch lines become horizontal
    let rp: Vec<Vec<Point>> = polys.iter().filter(|p| p.pts.len() >= 3).map(|p| p.pts.iter().map(|&q| rot(q, c, s)).collect()).collect();
    let mut ymin = f64::MAX;
    let mut ymax = f64::MIN;
    for p in rp.iter().flatten() {
        ymin = ymin.min(p.y);
        ymax = ymax.max(p.y);
    }
    let mut lines = Vec::new();
    if rp.is_empty() || ymin > ymax {
        return lines;
    }
    let (bs, bc) = (angle_deg.to_radians().sin(), angle_deg.to_radians().cos());
    let mut y = ymin + interval / 2.0;
    let mut idx = 0usize;
    while y < ymax {
        let mut xs: Vec<f64> = Vec::new();
        for poly in &rp {
            for i in 0..poly.len() {
                let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
                if (a.y <= y && b.y > y) || (b.y <= y && a.y > y) {
                    xs.push(a.x + (y - a.y) / (b.y - a.y) * (b.x - a.x));
                }
            }
        }
        xs.sort_by(|a, b| a.total_cmp(b));
        let mut segs: Vec<(Point, Point)> =
            xs.chunks_exact(2).filter(|w| w[1] - w[0] > 1e-9).map(|w| (rot(Point::new(w[0], y), bc, bs), rot(Point::new(w[1], y), bc, bs))).collect();
        if bidirectional && idx % 2 == 1 {
            segs.reverse();
            for sgm in &mut segs {
                *sgm = (sgm.1, sgm.0);
            }
        }
        if !segs.is_empty() {
            lines.push(segs);
            idx += 1;
        }
        y += interval;
    }
    lines
}

/// Wobble settings: the path is resampled every `step_mm` and each sample is displaced on a
/// circle of diameter `size_mm`; one revolution takes `points_per_rev` samples.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Wobble {
    pub size_mm: f64,
    pub step_mm: f64,
    pub points_per_rev: u32,
}

impl Default for Wobble {
    fn default() -> Self {
        Self { size_mm: 0.2, step_mm: 0.05, points_per_rev: 8 }
    }
}

/// Applies wobble to an open polyline. The first and last points are kept exact so paths still
/// start and end where they should.
pub fn wobble_polyline(pts: &[Point], w: &Wobble) -> Vec<Point> {
    if pts.len() < 2 || w.size_mm <= 0.0 || w.step_mm <= 0.0 {
        return pts.to_vec();
    }
    let mut out = vec![pts[0]];
    let mut k = 0u32;
    let r = w.size_mm / 2.0;
    let n = w.points_per_rev.max(3) as f64;
    let mut carry = 0.0; // distance already travelled since the last sample
    for seg in pts.windows(2) {
        let (a, b) = (seg[0], seg[1]);
        let len = a.distance(b);
        if len < 1e-12 {
            continue;
        }
        let mut d = w.step_mm - carry;
        while d < len {
            let t = d / len;
            let base = Point::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t);
            k += 1;
            let th = std::f64::consts::TAU * (k as f64) / n;
            out.push(Point::new(base.x + r * th.cos(), base.y + r * th.sin()));
            d += w.step_mm;
        }
        carry = len - (d - w.step_mm);
    }
    out.push(*pts.last().expect("len >= 2"));
    out
}

/// Clips an open polyline to the horizontal band `y0..=y1`, returning the pieces inside.
pub fn clip_polyline_y(pts: &[Point], y0: f64, y1: f64) -> Vec<Vec<Point>> {
    let mut out: Vec<Vec<Point>> = Vec::new();
    let mut cur: Vec<Point> = Vec::new();
    let inside = |p: Point| p.y >= y0 && p.y <= y1;
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        // parameter range of the segment inside the band
        let (mut t0, mut t1) = (0.0f64, 1.0f64);
        let dy = b.y - a.y;
        if dy.abs() < 1e-15 {
            if !inside(a) {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                continue;
            }
        } else {
            let (ta, tb) = ((y0 - a.y) / dy, (y1 - a.y) / dy);
            let (lo, hi) = if ta < tb { (ta, tb) } else { (tb, ta) };
            t0 = t0.max(lo);
            t1 = t1.min(hi);
        }
        if t0 > t1 {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            continue;
        }
        let pa = Point::new(a.x + (b.x - a.x) * t0, a.y + dy * t0);
        let pb = Point::new(a.x + (b.x - a.x) * t1, a.y + dy * t1);
        if cur.last().is_none_or(|l| l.distance(pa) > 1e-9) {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            cur.push(pa);
        }
        cur.push(pb);
        if t1 < 1.0 {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out.retain(|p| p.len() >= 2 && p.first().zip(p.last()).is_some_and(|(a, b)| p.len() > 2 || a.distance(*b) > 1e-9));
    out
}

/// Convex hull (Andrew's monotone chain), counter-clockwise.
pub fn convex_hull(points: &[Point]) -> Vec<Point> {
    let mut p: Vec<Point> = points.to_vec();
    p.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    p.dedup_by(|a, b| a.x == b.x && a.y == b.y);
    if p.len() < 3 {
        return p;
    }
    let cross = |o: Point, a: Point, b: Point| (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x);
    let mut lower: Vec<Point> = Vec::new();
    for &q in &p {
        while lower.len() >= 2 && cross(lower[lower.len() - 2], lower[lower.len() - 1], q) <= 0.0 {
            lower.pop();
        }
        lower.push(q);
    }
    let mut upper: Vec<Point> = Vec::new();
    for &q in p.iter().rev() {
        while upper.len() >= 2 && cross(upper[upper.len() - 2], upper[upper.len() - 1], q) <= 0.0 {
            upper.pop();
        }
        upper.push(q);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

/// Greedy nearest-neighbour order of open polylines, reversing where that is shorter.
/// Closed paths are kept in direction. Returns indices with a reverse flag.
pub fn order_paths(starts_ends: &[(Point, Point, bool)], from: Point) -> Vec<(usize, bool)> {
    let n = starts_ends.len();
    let mut used = vec![false; n];
    let mut cur = from;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let mut best = (f64::MAX, 0usize, false);
        for (i, &(s, e, closed)) in starts_ends.iter().enumerate() {
            if used[i] {
                continue;
            }
            let ds = cur.distance(s);
            if ds < best.0 {
                best = (ds, i, false);
            }
            if !closed {
                let de = cur.distance(e);
                if de < best.0 {
                    best = (de, i, true);
                }
            }
        }
        used[best.1] = true;
        let (s, e, _) = starts_ends[best.1];
        cur = if best.2 { s } else { e };
        out.push((best.1, best.2));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(x: f64, y: f64, s: f64) -> Poly {
        Poly { pts: vec![Point::new(x, y), Point::new(x + s, y), Point::new(x + s, y + s), Point::new(x, y + s)], closed: true }
    }

    #[test]
    fn flatten_square_and_curve() {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((10.0, 0.0));
        p.line_to((10.0, 10.0));
        p.close_path();
        let polys = flatten_path(&p, 0.01);
        assert_eq!(polys.len(), 1);
        assert!(polys[0].closed && polys[0].pts.len() == 3);
        let mut c = BezPath::new();
        c.move_to((0.0, 0.0));
        c.curve_to((0.0, 10.0), (10.0, 10.0), (10.0, 0.0));
        let polys = flatten_path(&c, 0.01);
        assert!(polys[0].pts.len() > 8 && !polys[0].closed);
    }

    #[test]
    fn hatch_square_covers_area() {
        let lines = hatch(&[square(0.0, 0.0, 10.0)], 1.0, 0.0, false);
        assert_eq!(lines.len(), 10);
        let total: f64 = lines.iter().flatten().map(|(a, b)| a.distance(*b)).sum();
        assert!((total - 100.0).abs() < 1e-6);
    }

    #[test]
    fn hatch_bidirectional_alternates() {
        let lines = hatch(&[square(0.0, 0.0, 10.0)], 2.0, 0.0, true);
        assert!(lines[0][0].0.x < lines[0][0].1.x);
        assert!(lines[1][0].0.x > lines[1][0].1.x);
    }

    #[test]
    fn hatch_respects_holes_even_odd() {
        let lines = hatch(&[square(0.0, 0.0, 10.0), square(3.0, 3.0, 4.0)], 1.0, 0.0, false);
        let total: f64 = lines.iter().flatten().map(|(a, b)| a.distance(*b)).sum();
        assert!((total - 84.0).abs() < 1e-6, "{total}");
        // a line through the hole has two segments
        assert!(lines.iter().any(|l| l.len() == 2));
    }

    #[test]
    fn hatch_rotated_stays_inside() {
        let lines = hatch(&[square(0.0, 0.0, 10.0)], 0.5, 45.0, false);
        assert!(!lines.is_empty());
        for (a, b) in lines.iter().flatten() {
            for p in [a, b] {
                assert!(p.x > -1e-6 && p.x < 10.0 + 1e-6 && p.y > -1e-6 && p.y < 10.0 + 1e-6, "{p:?}");
            }
            // direction is 45 degrees
            assert!(((b.y - a.y).abs() - (b.x - a.x).abs()).abs() < 1e-6);
        }
    }

    #[test]
    fn wobble_keeps_ends_and_adds_points() {
        let pts = [Point::new(0.0, 0.0), Point::new(10.0, 0.0)];
        let w = wobble_polyline(&pts, &Wobble { size_mm: 0.4, step_mm: 0.5, points_per_rev: 8 });
        assert_eq!(w.first(), Some(&pts[0]));
        assert_eq!(w.last(), Some(&pts[1]));
        assert!(w.len() >= 20);
        for p in &w[1..w.len() - 1] {
            assert!(p.y.abs() <= 0.2 + 1e-9);
        }
        assert_eq!(wobble_polyline(&pts, &Wobble { size_mm: 0.0, ..Default::default() }), pts.to_vec());
    }

    #[test]
    fn clip_band() {
        let pts = [Point::new(0.0, -5.0), Point::new(0.0, 15.0)];
        let pieces = clip_polyline_y(&pts, 0.0, 10.0);
        assert_eq!(pieces.len(), 1);
        assert_eq!((pieces[0][0].y, pieces[0][1].y), (0.0, 10.0));
        let out = clip_polyline_y(&pts, 20.0, 30.0);
        assert!(out.is_empty());
        // zig-zag leaves and re-enters the band
        let z = [Point::new(0.0, 5.0), Point::new(1.0, 15.0), Point::new(2.0, 5.0)];
        assert_eq!(clip_polyline_y(&z, 0.0, 10.0).len(), 2);
    }

    #[test]
    fn hull_of_points() {
        let pts =
            [Point::new(0.0, 0.0), Point::new(4.0, 0.0), Point::new(4.0, 4.0), Point::new(0.0, 4.0), Point::new(2.0, 2.0), Point::new(1.0, 3.0)];
        assert_eq!(convex_hull(&pts).len(), 4);
    }

    #[test]
    fn ordering_prefers_near_and_reverses() {
        let items = [(Point::new(10.0, 0.0), Point::new(20.0, 0.0), false), (Point::new(0.0, 0.0), Point::new(5.0, 0.0), false)];
        let o = order_paths(&items, Point::new(6.0, 0.0));
        assert_eq!(o[0], (1, true));
        assert_eq!(o[1], (0, false));
    }
}
