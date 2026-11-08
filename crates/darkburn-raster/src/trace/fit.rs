//! Turning dense point chains into lines and cubic Béziers.
//!
//! Corners are found with a k-cosine test and refined by intersecting the straight runs on
//! either side. Between corners, cubics are fitted by least squares with Newton
//! reparameterisation and recursive splitting at the worst point, as described in Philip J.
//! Schneider, "An Algorithm for Automatically Fitting Digitized Curves", Graphics Gems (1990).
//! Written from the description.

use kurbo::{BezPath, CubicBez, Point, Vec2};

/// How a chain is fitted.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FitParams {
    /// Turning angle (degrees) above which a point becomes a corner. Above 180 there are none.
    pub corner_deg: f64,
    /// Maximum distance (px) between the fitted curve and the points.
    pub tolerance: f64,
    /// Straight lines only (smoothness 0).
    pub polygonal: bool,
}

fn turn_angle(a: Vec2, b: Vec2) -> f64 {
    let la = a.hypot();
    let lb = b.hypot();
    if la < 1e-12 || lb < 1e-12 {
        return 0.0;
    }
    (a.dot(b) / (la * lb)).clamp(-1.0, 1.0).acos().to_degrees()
}

fn normalize(v: Vec2) -> Vec2 {
    let l = v.hypot();
    if l < 1e-12 { Vec2::ZERO } else { v / l }
}

/// Intersection of lines `p + t·d` and `q + s·e`.
fn intersect(p: Point, d: Vec2, q: Point, e: Vec2) -> Option<Point> {
    let den = d.cross(e);
    if den.abs() < 1e-9 {
        return None;
    }
    let t = (q - p).cross(e) / den;
    Some(p + d * t)
}

/// Fits a closed loop of dense points (no repeated end point).
pub(crate) fn fit_closed(pts: &[Point], params: &FitParams) -> BezPath {
    let n = pts.len();
    let mut path = BezPath::new();
    if n < 3 {
        return path;
    }
    if n < 8 {
        path.move_to(pts[0]);
        for &p in &pts[1..] {
            path.line_to(p);
        }
        path.close_path();
        return path;
    }
    let k = (n / 8).clamp(1, 4);
    let at = |i: isize| pts[i.rem_euclid(n as isize) as usize];
    let angles: Vec<f64> = (0..n as isize).map(|i| turn_angle(at(i) - at(i - k as isize), at(i + k as isize) - at(i))).collect();
    let mut corners: Vec<usize> = Vec::new();
    for i in 0..n {
        let a = angles[i];
        if a <= params.corner_deg {
            continue;
        }
        let is_max = (1..=k as isize).all(|j| {
            let l = angles[(i as isize - j).rem_euclid(n as isize) as usize];
            let r = angles[(i as isize + j).rem_euclid(n as isize) as usize];
            a > l && a >= r
        });
        if is_max {
            corners.push(i);
        }
    }
    // Light smoothing of the staircase, corners excluded.
    let mut sm: Vec<Point> = (0..n as isize)
        .map(|i| {
            if corners.contains(&(i as usize)) {
                at(i)
            } else {
                let (a, b, c) = (at(i - 1), at(i), at(i + 1));
                Point::new((a.x + 2.0 * b.x + c.x) / 4.0, (a.y + 2.0 * b.y + c.y) / 4.0)
            }
        })
        .collect();
    // Refine corner positions: intersect the runs on either side.
    for &c in &corners {
        let (ci, k) = (c as isize, k as isize);
        let d = at(ci - 1) - at(ci - 1 - k);
        let e = at(ci + 1 + k) - at(ci + 1);
        if let Some(p) = intersect(at(ci - 1), d, at(ci + 1), e)
            && (p - at(ci)).hypot() < 1.5
        {
            sm[c] = p;
        }
    }
    let sat = |i: isize| sm[i.rem_euclid(n as isize) as usize];
    // Break points: corners, or two smooth joins on a loop without corners.
    let (breaks, smooth_breaks) = if corners.is_empty() { (vec![0, n / 2], true) } else { (corners.clone(), false) };
    let central = |i: usize| normalize(sat(i as isize + 2) - sat(i as isize - 2));
    path.move_to(sm[breaks[0]]);
    for bi in 0..breaks.len() {
        let s = breaks[bi];
        let e = breaks[(bi + 1) % breaks.len()];
        let len = if e > s { e - s } else { e + n - s };
        let seg: Vec<Point> = (0..=len).map(|j| sm[(s + j) % n]).collect();
        let (t1, t2) = if smooth_breaks { (central(s), -central(e)) } else { end_tangents(&seg) };
        emit_segment(&mut path, &seg, t1, t2, params);
    }
    path.close_path();
    path
}

/// Fits an open chain.
pub(crate) fn fit_open(pts: &[Point], params: &FitParams) -> BezPath {
    let n = pts.len();
    let mut path = BezPath::new();
    if n < 2 {
        return path;
    }
    path.move_to(pts[0]);
    if n < 4 {
        for &p in &pts[1..] {
            path.line_to(p);
        }
        return path;
    }
    let k = (n / 8).clamp(1, 4);
    let mut breaks = vec![0usize];
    let angles: Vec<f64> = (0..n).map(|i| if i < k || i + k >= n { 0.0 } else { turn_angle(pts[i] - pts[i - k], pts[i + k] - pts[i]) }).collect();
    for i in k..n.saturating_sub(k) {
        let a = angles[i];
        if a > params.corner_deg && (1..=k).all(|j| a > angles[i - j] && a >= angles[(i + j).min(n - 1)]) {
            breaks.push(i);
        }
    }
    breaks.push(n - 1);
    let mut sm = pts.to_vec();
    for i in 1..n - 1 {
        if !breaks.contains(&i) {
            let (a, b, c) = (pts[i - 1], pts[i], pts[i + 1]);
            sm[i] = Point::new((a.x + 2.0 * b.x + c.x) / 4.0, (a.y + 2.0 * b.y + c.y) / 4.0);
        }
    }
    for w in breaks.windows(2) {
        let seg = &sm[w[0]..=w[1]];
        let (t1, t2) = end_tangents(seg);
        emit_segment(&mut path, seg, t1, t2, params);
    }
    path
}

fn end_tangents(seg: &[Point]) -> (Vec2, Vec2) {
    let m = seg.len() - 1;
    let k = m.clamp(1, 3);
    (normalize(seg[k] - seg[0]), normalize(seg[m - k] - seg[m]))
}

fn emit_segment(path: &mut BezPath, seg: &[Point], t1: Vec2, t2: Vec2, params: &FitParams) {
    if seg.len() < 2 {
        return;
    }
    if params.polygonal || seg.len() == 2 {
        for p in simplify_polyline(seg, params.tolerance.max(0.05)).into_iter().skip(1) {
            path.line_to(p);
        }
        return;
    }
    // A segment that is straight within tolerance stays a line.
    let (a, b) = (seg[0], seg[seg.len() - 1]);
    if (b - a).hypot() > 1e-9 && seg.iter().all(|&p| point_line_distance(p, a, b) <= params.tolerance) {
        path.line_to(b);
        return;
    }
    let mut out = Vec::new();
    fit_cubic(seg, t1, t2, params.tolerance.max(0.05), &mut out, 0);
    for c in out {
        path.curve_to(c.p1, c.p2, c.p3);
    }
}

fn point_line_distance(p: Point, a: Point, b: Point) -> f64 {
    let d = b - a;
    let l = d.hypot();
    if l < 1e-12 {
        return (p - a).hypot();
    }
    let t = ((p - a).dot(d) / (l * l)).clamp(0.0, 1.0);
    (p - (a + d * t)).hypot()
}

/// Ramer–Douglas–Peucker.
pub(crate) fn simplify_polyline(pts: &[Point], tol: f64) -> Vec<Point> {
    if pts.len() <= 2 {
        return pts.to_vec();
    }
    let mut keep = vec![false; pts.len()];
    keep[0] = true;
    keep[pts.len() - 1] = true;
    let mut stack = vec![(0usize, pts.len() - 1)];
    while let Some((s, e)) = stack.pop() {
        let mut worst = (0.0, 0usize);
        for i in s + 1..e {
            let d = point_line_distance(pts[i], pts[s], pts[e]);
            if d > worst.0 {
                worst = (d, i);
            }
        }
        if worst.0 > tol {
            keep[worst.1] = true;
            stack.push((s, worst.1));
            stack.push((worst.1, e));
        }
    }
    pts.iter().zip(keep).filter(|(_, k)| *k).map(|(p, _)| *p).collect()
}

#[inline]
fn bez_eval(b: &[Point; 4], t: f64) -> Point {
    let mt = 1.0 - t;
    let (a, bb, c, d) = (mt * mt * mt, 3.0 * mt * mt * t, 3.0 * mt * t * t, t * t * t);
    Point::new(a * b[0].x + bb * b[1].x + c * b[2].x + d * b[3].x, a * b[0].y + bb * b[1].y + c * b[2].y + d * b[3].y)
}

fn chord_params(pts: &[Point]) -> Vec<f64> {
    let mut u = vec![0.0; pts.len()];
    for i in 1..pts.len() {
        u[i] = u[i - 1] + (pts[i] - pts[i - 1]).hypot();
    }
    let total = u[pts.len() - 1];
    if total > 0.0 {
        u.iter_mut().for_each(|v| *v /= total);
    }
    u
}

fn generate(pts: &[Point], u: &[f64], t1: Vec2, t2: Vec2) -> [Point; 4] {
    let (p0, p3) = (pts[0], pts[pts.len() - 1]);
    let (mut c00, mut c01, mut c11, mut x0, mut x1) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for (p, &t) in pts.iter().zip(u) {
        let mt = 1.0 - t;
        let (b0, b1, b2, b3) = (mt * mt * mt, 3.0 * mt * mt * t, 3.0 * mt * t * t, t * t * t);
        let a0 = t1 * b1;
        let a1 = t2 * b2;
        c00 += a0.dot(a0);
        c01 += a0.dot(a1);
        c11 += a1.dot(a1);
        let base = p0.to_vec2() * (b0 + b1) + p3.to_vec2() * (b2 + b3);
        let tmp = p.to_vec2() - base;
        x0 += a0.dot(tmp);
        x1 += a1.dot(tmp);
    }
    let det = c00 * c11 - c01 * c01;
    let seg_len = (p3 - p0).hypot();
    let eps = 1e-6 * seg_len;
    let (mut al, mut ar) = if det.abs() > 1e-12 { ((x0 * c11 - x1 * c01) / det, (c00 * x1 - c01 * x0) / det) } else { (0.0, 0.0) };
    if al < eps || ar < eps || al > seg_len * 2.0 || ar > seg_len * 2.0 {
        al = seg_len / 3.0;
        ar = seg_len / 3.0;
    }
    [p0, p0 + t1 * al, p3 + t2 * ar, p3]
}

fn max_error(pts: &[Point], b: &[Point; 4], u: &[f64]) -> (f64, usize) {
    let mut worst = (0.0, pts.len() / 2);
    for i in 1..pts.len() - 1 {
        let d = (bez_eval(b, u[i]) - pts[i]).hypot();
        if d > worst.0 {
            worst = (d, i);
        }
    }
    worst
}

fn reparameterize(pts: &[Point], u: &[f64], b: &[Point; 4]) -> Vec<f64> {
    let d1 = [(b[1] - b[0]) * 3.0, (b[2] - b[1]) * 3.0, (b[3] - b[2]) * 3.0];
    let d2 = [(d1[1] - d1[0]) * 2.0, (d1[2] - d1[1]) * 2.0];
    pts.iter()
        .zip(u)
        .map(|(&p, &t)| {
            let mt = 1.0 - t;
            let q = bez_eval(b, t);
            let q1 = d1[0] * (mt * mt) + d1[1] * (2.0 * mt * t) + d1[2] * (t * t);
            let q2 = d2[0] * mt + d2[1] * t;
            let diff = q - p;
            let den = q1.dot(q1) + diff.dot(q2);
            if den.abs() < 1e-12 { t } else { (t - diff.dot(q1) / den).clamp(0.0, 1.0) }
        })
        .collect()
}

fn fit_cubic(pts: &[Point], t1: Vec2, t2: Vec2, tol: f64, out: &mut Vec<CubicBez>, depth: u32) {
    let n = pts.len();
    let to_cubic = |b: [Point; 4]| CubicBez::new(b[0], b[1], b[2], b[3]);
    if n == 2 {
        let d = (pts[1] - pts[0]).hypot() / 3.0;
        out.push(to_cubic([pts[0], pts[0] + t1 * d, pts[1] + t2 * d, pts[1]]));
        return;
    }
    let mut u = chord_params(pts);
    let mut b = generate(pts, &u, t1, t2);
    let (mut err, mut split) = max_error(pts, &b, &u);
    if err <= tol {
        out.push(to_cubic(b));
        return;
    }
    if err <= tol * 4.0 {
        for _ in 0..6 {
            u = reparameterize(pts, &u, &b);
            b = generate(pts, &u, t1, t2);
            (err, split) = max_error(pts, &b, &u);
            if err <= tol {
                out.push(to_cubic(b));
                return;
            }
        }
    }
    if depth > 40 {
        out.push(to_cubic(b));
        return;
    }
    let split = split.clamp(1, n - 2);
    let mut tc = normalize(pts[split - 1] - pts[split + 1]);
    if tc == Vec2::ZERO {
        tc = normalize(pts[split - 1] - pts[split]);
    }
    fit_cubic(&pts[..=split], t1, tc, tol, out, depth + 1);
    fit_cubic(&pts[split..], -tc, t2, tol, out, depth + 1);
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::{ParamCurveNearest, PathEl, Shape};

    fn params() -> FitParams {
        FitParams { corner_deg: 70.0, tolerance: 0.3, polygonal: false }
    }

    fn max_dev(path: &BezPath, pts: &[Point]) -> f64 {
        pts.iter().map(|&p| path.segments().map(|s| s.nearest(p, 1e-6).distance_sq.sqrt()).fold(f64::MAX, f64::min)).fold(0.0, f64::max)
    }

    #[test]
    fn circle_fits_with_few_curves() {
        let pts: Vec<Point> = (0..200).map(|i| Point::new(50.0, 50.0) + Vec2::from_angle(i as f64 * 0.0314159) * 30.0).collect();
        let path = fit_closed(&pts, &params());
        let curves = path.elements().iter().filter(|e| matches!(e, PathEl::CurveTo(..))).count();
        assert!((2..=8).contains(&curves), "{curves}");
        assert!(max_dev(&path, &pts) < 0.35);
        assert!((path.area().abs() - std::f64::consts::PI * 900.0).abs() < 10.0);
    }

    #[test]
    fn square_keeps_corners() {
        let mut pts = Vec::new();
        for i in 0..20 {
            pts.push(Point::new(i as f64 + 0.5, 0.0));
        }
        for i in 0..20 {
            pts.push(Point::new(20.0, i as f64 + 0.5));
        }
        for i in 0..20 {
            pts.push(Point::new(19.5 - i as f64, 20.0));
        }
        for i in 0..20 {
            pts.push(Point::new(0.0, 19.5 - i as f64));
        }
        let path = fit_closed(&pts, &params());
        let lines = path.elements().iter().filter(|e| matches!(e, PathEl::LineTo(..))).count();
        assert_eq!(lines, 4, "{path:?}");
        assert!((path.area().abs() - 400.0).abs() < 1e-6, "{}", path.area());
    }

    #[test]
    fn rdp_simplifies_line() {
        let pts: Vec<Point> = (0..10).map(|i| Point::new(i as f64, if i % 2 == 0 { 0.0 } else { 0.1 })).collect();
        assert_eq!(simplify_polyline(&pts, 0.2).len(), 2);
        assert!(simplify_polyline(&pts, 0.05).len() > 2);
    }

    #[test]
    fn open_curve_fits() {
        let pts: Vec<Point> = (0..60).map(|i| Point::new(i as f64, (i as f64 / 10.0).sin() * 10.0)).collect();
        let path = fit_open(&pts, &params());
        assert!(max_dev(&path, &pts[2..58]) < 0.5);
        assert_eq!(path.elements()[0], PathEl::MoveTo(pts[0]));
    }
}
