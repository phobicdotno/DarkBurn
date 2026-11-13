//! Tangent circles: the circles of a given radius that touch two lines, arcs or circles, and
//! the circles that touch three of them (the problem of Apollonius).
//!
//! The solvers work on the full line or circle each element lies on (its [`Carrier`]), so a
//! solution may touch a segment's extension or the missing part of an arc; every
//! [`Solution`] says whether all of its touch points lie on the elements themselves.

use std::f64::consts::{PI, TAU};
use std::fmt;

use kurbo::{BezPath, Circle, Line, ParamCurve, PathEl, PathSeg, Point, Shape, Vec2};

/// A straight segment, arc or full circle taken from a drawing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Element {
    Segment(Line),
    /// The part of the circle `center`/`radius` that starts at angle `start` (radians) and
    /// turns by `sweep` (counter-clockwise when positive, with Y up).
    Arc {
        center: Point,
        radius: f64,
        start: f64,
        sweep: f64,
    },
    Circle(Circle),
}

/// The full line or circle an element lies on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Carrier {
    /// The points `p` with `normal.dot(p.to_vec2()) == offset`; `normal` has unit length.
    Line {
        normal: Vec2,
        offset: f64,
    },
    Circle(Circle),
}

/// One tangent circle and where it touches each input (in input order).
#[derive(Clone, Debug, PartialEq)]
pub struct Solution {
    pub circle: Circle,
    pub touches: Vec<Point>,
    /// Every touch point lies on the segment or arc itself, not on its extension.
    pub on_elements: bool,
}

/// Why no tangent circle could be given.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TangentError {
    /// The radius is not a positive number.
    BadRadius,
    /// An input is not a usable line, arc or circle (zero length or radius, or not finite).
    BadInput,
    /// Endless circles fit (for example two inputs on the same line or circle, or parallel
    /// lines exactly two radii apart), so there is no list to pick from.
    Endless,
    /// No circle touches all the inputs.
    NoSolution,
}

impl fmt::Display for TangentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            TangentError::BadRadius => "The radius must be a positive number.",
            TangentError::BadInput => "Pick straight lines, arcs or circles.",
            TangentError::Endless => "Endless circles fit these objects, so there is none to pick.",
            TangentError::NoSolution => "No circle touches all of these objects.",
        })
    }
}

impl std::error::Error for TangentError {}

fn arc_point(center: Point, radius: f64, angle: f64) -> Point {
    center + Vec2::from_angle(angle) * radius
}

/// Whether `angle` lies within the sweep from `start`.
fn in_sweep(start: f64, sweep: f64, angle: f64) -> bool {
    let d = if sweep >= 0.0 { (angle - start).rem_euclid(TAU) } else { (start - angle).rem_euclid(TAU) };
    d <= sweep.abs() + 1e-12
}

/// Wraps an angle into (-PI, PI].
fn wrap(a: f64) -> f64 {
    let w = a.rem_euclid(TAU);
    if w > PI { w - TAU } else { w }
}

fn finite(p: Point) -> bool {
    p.x.is_finite() && p.y.is_finite()
}

impl Element {
    /// The full line or circle the element lies on, or `None` for a zero-length segment, a
    /// zero radius or a value that is not finite.
    pub fn carrier(&self) -> Option<Carrier> {
        match *self {
            Element::Segment(l) => {
                let d = l.p1 - l.p0;
                let len = d.hypot();
                if !(finite(l.p0) && finite(l.p1)) || len <= 1e-12 {
                    return None;
                }
                let normal = Vec2::new(-d.y, d.x) / len;
                Some(Carrier::Line { normal, offset: normal.dot(l.p0.to_vec2()) })
            }
            Element::Arc { center, radius, start, sweep } => {
                let ok = finite(center) && radius.is_finite() && radius > 1e-12;
                (ok && start.is_finite() && sweep.is_finite()).then_some(Carrier::Circle(Circle::new(center, radius)))
            }
            Element::Circle(c) => (finite(c.center) && c.radius.is_finite() && c.radius > 1e-12).then_some(Carrier::Circle(c)),
        }
    }

    /// The point of the element nearest to `p`.
    pub fn nearest(&self, p: Point) -> Point {
        match *self {
            Element::Segment(l) => {
                let d = l.p1 - l.p0;
                let len2 = d.hypot2();
                if len2 <= 0.0 {
                    return l.p0;
                }
                let t = ((p - l.p0).dot(d) / len2).clamp(0.0, 1.0);
                l.p0 + d * t
            }
            Element::Arc { center, radius, start, sweep } => {
                let v = p - center;
                let a = if v.hypot2() > 0.0 { v.atan2() } else { start };
                if in_sweep(start, sweep, a) {
                    return arc_point(center, radius, a);
                }
                let (s, e) = (arc_point(center, radius, start), arc_point(center, radius, start + sweep));
                if s.distance(p) <= e.distance(p) { s } else { e }
            }
            Element::Circle(c) => {
                let v = p - c.center;
                let u = if v.hypot2() > 0.0 { v.normalize() } else { Vec2::new(1.0, 0.0) };
                c.center + u * c.radius
            }
        }
    }

    /// Distance from `p` to the element.
    pub fn distance(&self, p: Point) -> f64 {
        self.nearest(p).distance(p)
    }

    /// Whether `p` lies on the element (not just on its carrier) within `tol`.
    pub fn holds(&self, p: Point, tol: f64) -> bool {
        self.distance(p) <= tol
    }

    /// The element as a path, for drawing it.
    pub fn path(&self, tolerance: f64) -> BezPath {
        match *self {
            Element::Segment(l) => {
                let mut b = BezPath::new();
                b.move_to(l.p0);
                b.line_to(l.p1);
                b
            }
            Element::Arc { center, radius, start, sweep } => kurbo::Arc::new(center, Vec2::new(radius, radius), start, sweep, 0.0).to_path(tolerance),
            Element::Circle(c) => c.to_path(tolerance),
        }
    }
}

/// Where `circle` touches `carrier`, or `None` when it does not touch it within `tol`. A
/// circle that coincides with the carrier does not count as touching it.
pub fn tangent_point(carrier: &Carrier, circle: &Circle, tol: f64) -> Option<Point> {
    let (x, rho) = (circle.center, circle.radius);
    match *carrier {
        Carrier::Line { normal, offset } => {
            let d = normal.dot(x.to_vec2()) - offset;
            ((d.abs() - rho).abs() <= tol).then(|| x - normal * d)
        }
        Carrier::Circle(c) => {
            let v = x - c.center;
            let dist = v.hypot();
            if dist <= 1e-12 {
                return None;
            }
            let u = v / dist;
            if (dist - (c.radius + rho)).abs() <= tol || (rho < c.radius && (dist - (c.radius - rho)).abs() <= tol) {
                Some(c.center + u * c.radius)
            } else if rho > c.radius && (dist - (rho - c.radius)).abs() <= tol {
                Some(c.center - u * c.radius)
            } else {
                None
            }
        }
    }
}

fn carriers<const N: usize>(els: [&Element; N]) -> Result<[Carrier; N], TangentError> {
    let mut out = [Carrier::Circle(Circle::new(Point::ZERO, 1.0)); N];
    for (o, e) in out.iter_mut().zip(els) {
        *o = e.carrier().ok_or(TangentError::BadInput)?;
    }
    Ok(out)
}

fn clean_tol(tol: f64) -> f64 {
    if tol.is_finite() && tol > 0.0 { tol } else { 1e-9 }
}

/// Lines and circles a tangent circle's centre must lie on.
#[derive(Clone, Copy, Debug)]
enum Locus {
    Line { normal: Vec2, offset: f64 },
    Circle { center: Point, radius: f64 },
}

fn loci(c: &Carrier, radius: f64, tol: f64) -> Vec<Locus> {
    match *c {
        Carrier::Line { normal, offset } => {
            vec![Locus::Line { normal, offset: offset + radius }, Locus::Line { normal, offset: offset - radius }]
        }
        Carrier::Circle(c) => {
            let mut v = vec![Locus::Circle { center: c.center, radius: c.radius + radius }];
            let inner = (c.radius - radius).abs();
            if inner > tol {
                v.push(Locus::Circle { center: c.center, radius: inner });
            }
            v
        }
    }
}

enum Meet {
    Points(Vec<Point>),
    /// The two loci are the same line or circle.
    Same,
}

fn line_circle(normal: Vec2, offset: f64, center: Point, radius: f64, tol: f64) -> Vec<Point> {
    let d = normal.dot(center.to_vec2()) - offset;
    let foot = center - normal * d;
    if d.abs() > radius {
        return if d.abs() - radius <= tol { vec![foot] } else { vec![] };
    }
    let s = (radius * radius - d * d).max(0.0).sqrt();
    if s <= tol {
        return vec![foot];
    }
    let along = Vec2::new(-normal.y, normal.x);
    vec![foot + along * s, foot - along * s]
}

fn meet(a: &Locus, b: &Locus, tol: f64) -> Meet {
    match (*a, *b) {
        (Locus::Line { normal: n1, offset: h1 }, Locus::Line { normal: n2, offset: h2 }) => {
            let det = n1.cross(n2);
            if det.abs() <= 1e-12 {
                let h2 = if n1.dot(n2) < 0.0 { -h2 } else { h2 };
                return if (h1 - h2).abs() <= tol { Meet::Same } else { Meet::Points(vec![]) };
            }
            let x = (h1 * n2.y - h2 * n1.y) / det;
            let y = (n1.x * h2 - n2.x * h1) / det;
            Meet::Points(vec![Point::new(x, y)])
        }
        (Locus::Line { normal, offset }, Locus::Circle { center, radius }) | (Locus::Circle { center, radius }, Locus::Line { normal, offset }) => {
            Meet::Points(line_circle(normal, offset, center, radius, tol))
        }
        (Locus::Circle { center: c1, radius: r1 }, Locus::Circle { center: c2, radius: r2 }) => {
            let v = c2 - c1;
            let d = v.hypot();
            if d <= tol {
                return if (r1 - r2).abs() <= tol { Meet::Same } else { Meet::Points(vec![]) };
            }
            if d > r1 + r2 + tol || d < (r1 - r2).abs() - tol {
                return Meet::Points(vec![]);
            }
            let u = v / d;
            let a = (d * d + r1 * r1 - r2 * r2) / (2.0 * d);
            let h = (r1 * r1 - a * a).max(0.0).sqrt();
            let base = c1 + u * a;
            if h <= tol {
                return Meet::Points(vec![base]);
            }
            let perp = Vec2::new(-u.y, u.x);
            Meet::Points(vec![base + perp * h, base - perp * h])
        }
    }
}

/// Keeps the candidates that touch every element, once each, sorted by radius, then x, then y.
fn finish(candidates: impl IntoIterator<Item = Circle>, els: &[&Element], cars: &[Carrier], tol: f64) -> Vec<Solution> {
    let mut out: Vec<Solution> = Vec::new();
    for circle in candidates {
        if !(finite(circle.center) && circle.radius.is_finite() && circle.radius > tol) {
            continue;
        }
        let Some(touches) = cars.iter().map(|c| tangent_point(c, &circle, tol)).collect::<Option<Vec<_>>>() else {
            continue;
        };
        let dup = out.iter().any(|s| s.circle.center.distance(circle.center) <= tol && (s.circle.radius - circle.radius).abs() <= tol);
        if dup {
            continue;
        }
        let on_elements = els.iter().zip(&touches).all(|(e, p)| e.holds(*p, tol));
        out.push(Solution { circle, touches, on_elements });
    }
    out.sort_by(|a, b| {
        let key = |s: &Solution| (s.circle.radius, s.circle.center.x, s.circle.center.y);
        key(a).partial_cmp(&key(b)).unwrap_or(std::cmp::Ordering::Equal)
    });
    out
}

/// Every circle of `radius` that touches both `a` and `b` (on their full lines or circles),
/// checked to within `tol` (mm).
pub fn with_radius(a: &Element, b: &Element, radius: f64, tol: f64) -> Result<Vec<Solution>, TangentError> {
    let tol = clean_tol(tol);
    if !(radius.is_finite() && radius > tol) {
        return Err(TangentError::BadRadius);
    }
    let cars = carriers([a, b])?;
    let mut centres = Vec::new();
    let mut endless = false;
    for la in loci(&cars[0], radius, tol) {
        for lb in loci(&cars[1], radius, tol) {
            match meet(&la, &lb, tol) {
                Meet::Points(p) => centres.extend(p),
                Meet::Same => endless = true,
            }
        }
    }
    if endless {
        return Err(TangentError::Endless);
    }
    let sols = finish(centres.into_iter().map(|c| Circle::new(c, radius)), &[a, b], &cars, tol);
    if sols.is_empty() { Err(TangentError::NoSolution) } else { Ok(sols) }
}

fn same_carrier(a: &Carrier, b: &Carrier, tol: f64) -> bool {
    match (*a, *b) {
        (Carrier::Line { normal: n1, offset: h1 }, Carrier::Line { normal: n2, offset: h2 }) => {
            n1.cross(n2).abs() <= 1e-9 && {
                let h2 = if n1.dot(n2) < 0.0 { -h2 } else { h2 };
                (h1 - h2).abs() <= tol
            }
        }
        (Carrier::Circle(c1), Carrier::Circle(c2)) => c1.center.distance(c2.center) <= tol && (c1.radius - c2.radius).abs() <= tol,
        _ => false,
    }
}

fn dot3(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross3(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn det3(m: [[f64; 3]; 3]) -> f64 {
    dot3(m[0], cross3(m[1], m[2]))
}

/// Solves `m * x = b` by Cramer's rule; `None` when `m` is (nearly) singular.
fn solve3(m: [[f64; 3]; 3], b: [f64; 3]) -> Option<[f64; 3]> {
    let det = det3(m);
    let scale: f64 = m.iter().map(|r| dot3(*r, *r).sqrt()).product();
    if !det.is_finite() || det.abs() <= 1e-12 * scale.max(1e-300) {
        return None;
    }
    let mut x = [0.0; 3];
    for (i, xi) in x.iter_mut().enumerate() {
        let mut mi = m;
        for (row, bv) in mi.iter_mut().zip(b) {
            row[i] = bv;
        }
        *xi = det3(mi) / det;
    }
    x.iter().all(|v| v.is_finite()).then_some(x)
}

/// Candidate circles for one choice of sides: `signs[i]` is +1 for touching carrier `i` from
/// the side its normal points to (lines) or from outside (circles), -1 otherwise. The
/// carriers are already moved so the origin lies among them.
fn apollonius_signed(cars: &[Carrier; 3], signs: [f64; 3], tol: f64) -> Vec<Circle> {
    // Unknowns v = (x, y, rho). A line gives n.X - s*rho = h. A circle gives
    // |X|^2 - rho^2 - 2c.X - 2*s*r*rho + |c|^2 - r^2 = 0; differences of two circle
    // equations are linear.
    let mut rows: Vec<([f64; 3], f64)> = Vec::new();
    let mut first: Option<(Circle, f64)> = None;
    for (c, s) in cars.iter().zip(signs) {
        match *c {
            Carrier::Line { normal, offset } => rows.push(([normal.x, normal.y, -s], offset)),
            Carrier::Circle(ci) => {
                let k = ci.center.to_vec2().hypot2() - ci.radius * ci.radius;
                match first {
                    None => first = Some((ci, s)),
                    Some((c0, s0)) => {
                        let k0 = c0.center.to_vec2().hypot2() - c0.radius * c0.radius;
                        rows.push((
                            [2.0 * (ci.center.x - c0.center.x), 2.0 * (ci.center.y - c0.center.y), 2.0 * (s * ci.radius - s0 * c0.radius)],
                            k - k0,
                        ));
                    }
                }
            }
        }
    }
    let Some((c0, s0)) = first else {
        let m = [rows[0].0, rows[1].0, rows[2].0];
        return solve3(m, [rows[0].1, rows[1].1, rows[2].1])
            .filter(|v| v[2] > tol)
            .map(|v| Circle::new(Point::new(v[0], v[1]), v[2]))
            .into_iter()
            .collect();
    };
    // Two linear rows: the solutions lie on the line P0 + t*D.
    let mut lin = [[0.0; 3]; 2];
    let mut rhs = [0.0; 2];
    for (i, (r, h)) in rows.iter().enumerate().take(2) {
        let n = dot3(*r, *r).sqrt();
        if n <= 1e-12 {
            return vec![];
        }
        lin[i] = [r[0] / n, r[1] / n, r[2] / n];
        rhs[i] = h / n;
    }
    let d = cross3(lin[0], lin[1]);
    let dn = dot3(d, d).sqrt();
    if dn <= 1e-9 {
        return vec![];
    }
    let d = [d[0] / dn, d[1] / dn, d[2] / dn];
    // Minimum-norm particular solution P0 = A^T (A A^T)^-1 b.
    let g = dot3(lin[0], lin[1]);
    let det = 1.0 - g * g;
    let w0 = (rhs[0] - g * rhs[1]) / det;
    let w1 = (rhs[1] - g * rhs[0]) / det;
    let p0 = [0, 1, 2].map(|i| lin[0][i] * w0 + lin[1][i] * w1);
    let m = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] - a[2] * b[2];
    let l = [-2.0 * c0.center.x, -2.0 * c0.center.y, -2.0 * s0 * c0.radius];
    let k0 = c0.center.to_vec2().hypot2() - c0.radius * c0.radius;
    let qa = m(d, d);
    let qb = 2.0 * m(p0, d) + dot3(l, d);
    let qc = m(p0, p0) + dot3(l, p0) + k0;
    let ts: Vec<f64> = if qa.abs() <= 1e-12 {
        if qb.abs() <= 1e-12 { vec![] } else { vec![-qc / qb] }
    } else {
        let mut disc = qb * qb - 4.0 * qa * qc;
        if disc < 0.0 && disc > -1e-9 * (qb * qb + (4.0 * qa * qc).abs()) {
            disc = 0.0;
        }
        if disc < 0.0 {
            vec![]
        } else {
            let sq = disc.sqrt();
            vec![(-qb + sq) / (2.0 * qa), (-qb - sq) / (2.0 * qa)]
        }
    };
    ts.into_iter()
        .map(|t| [0, 1, 2].map(|i| p0[i] + t * d[i]))
        .filter(|v| v[2] > tol && v.iter().all(|x| x.is_finite()))
        .map(|v| Circle::new(Point::new(v[0], v[1]), v[2]))
        .collect()
}

fn shifted(c: &Carrier, o: Vec2) -> Carrier {
    match *c {
        Carrier::Line { normal, offset } => Carrier::Line { normal, offset: offset - normal.dot(o) },
        Carrier::Circle(ci) => Carrier::Circle(Circle::new(ci.center - o, ci.radius)),
    }
}

fn anchor(e: &Element) -> Point {
    match *e {
        Element::Segment(l) => l.p0.midpoint(l.p1),
        Element::Arc { center, .. } => center,
        Element::Circle(c) => c.center,
    }
}

/// Every circle that touches all three elements (on their full lines or circles), up to
/// eight, checked to within `tol` (mm).
pub fn apollonius(a: &Element, b: &Element, c: &Element, tol: f64) -> Result<Vec<Solution>, TangentError> {
    let tol = clean_tol(tol);
    let cars = carriers([a, b, c])?;
    for (i, j) in [(0, 1), (0, 2), (1, 2)] {
        if same_carrier(&cars[i], &cars[j], tol) {
            return Err(TangentError::Endless);
        }
    }
    // Work around the inputs' middle so the squared terms stay small.
    let o = (anchor(a).to_vec2() + anchor(b).to_vec2() + anchor(c).to_vec2()) / 3.0;
    let local = [shifted(&cars[0], o), shifted(&cars[1], o), shifted(&cars[2], o)];
    let mut cands = Vec::new();
    for mask in 0..8u32 {
        let signs = [0, 1, 2].map(|i| if mask & (1 << i) == 0 { 1.0 } else { -1.0 });
        cands.extend(apollonius_signed(&local, signs, tol).into_iter().map(|ci| Circle::new(ci.center + o, ci.radius)));
    }
    let sols = finish(cands, &[a, b, c], &cars, tol);
    if sols.is_empty() { Err(TangentError::NoSolution) } else { Ok(sols) }
}

/// The circle through three points, or `None` when they are (nearly) on one line.
fn circle_through(a: Point, b: Point, c: Point) -> Option<Circle> {
    let (b1, c1) = (b - a, c - a);
    let d = 2.0 * b1.cross(c1);
    let scale = b1.hypot2().max(c1.hypot2());
    if d.abs() <= 1e-12 * scale.max(1e-300) {
        return None;
    }
    let (bb, cc) = (b1.hypot2(), c1.hypot2());
    let ux = (c1.y * bb - b1.y * cc) / d;
    let uy = (b1.x * cc - c1.x * bb) / d;
    let center = a + Vec2::new(ux, uy);
    let r = center.distance(a);
    (finite(center) && r.is_finite()).then_some(Circle::new(center, r))
}

/// Least-squares circle through points (the algebraic fit): `None` for fewer than three
/// points or points on one line.
fn fit_circle(pts: &[Point]) -> Option<Circle> {
    if pts.len() < 3 {
        return None;
    }
    let mean = pts.iter().fold(Vec2::ZERO, |s, p| s + p.to_vec2()) / pts.len() as f64;
    let mut m = [[0.0; 3]; 3];
    let mut b = [0.0; 3];
    for p in pts {
        let v = p.to_vec2() - mean;
        let row = [v.x, v.y, 1.0];
        let z = v.hypot2();
        for (mi, (bi, ri)) in m.iter_mut().zip(b.iter_mut().zip(row)) {
            for (mij, rj) in mi.iter_mut().zip(row) {
                *mij += ri * rj;
            }
            *bi -= z * ri;
        }
    }
    let [d, e, f] = solve3(m, b)?;
    let r2 = (d * d + e * e) / 4.0 - f;
    if r2.is_nan() || r2 <= 0.0 {
        return None;
    }
    Some(Circle::new(Point::new(-d / 2.0, -e / 2.0) + mean, r2.sqrt()))
}

#[derive(Clone, Debug)]
enum Piece {
    Line(Line),
    Arc {
        center: Point,
        radius: f64,
        sweep: f64,
        from: Point,
        to: Point,
        /// Points known to lie on the circle (segment ends) and curve midpoints for fitting.
        ends: Vec<Point>,
        mids: Vec<Point>,
    },
}

fn dist_to_line(p: Point, a: Point, b: Point) -> f64 {
    let d = b - a;
    let len = d.hypot();
    if len <= 0.0 { p.distance(a) } else { (p - a).cross(d).abs() / len }
}

/// How far a curve may stray from a circle of radius `r` and still count as part of it: `tol`,
/// or a thousandth of the radius when that is more. One cubic per quarter circle (as SVG
/// import and many drawing programs write circles) strays by about 0.03 % of the radius, so a
/// fixed tolerance alone would refuse large circles.
fn round_tol(tol: f64, r: f64) -> f64 {
    if r.is_finite() { tol.max(ROUND_REL * r) } else { tol }
}

/// The share of the radius [`round_tol`] allows.
const ROUND_REL: f64 = 1e-3;

fn piece(seg: PathSeg, tol: f64) -> Option<Piece> {
    let (p0, p1) = (seg.start(), seg.end());
    if let PathSeg::Line(l) = seg {
        return (p0.distance(p1) > tol).then_some(Piece::Line(l));
    }
    let samples: Vec<Point> = (1..10).map(|i| seg.eval(i as f64 / 10.0)).collect();
    if p0.distance(p1) > tol && samples.iter().all(|p| dist_to_line(*p, p0, p1) <= tol) {
        return Some(Piece::Line(Line::new(p0, p1)));
    }
    let pm = seg.eval(0.5);
    let c = circle_through(p0, pm, p1)?;
    let round = round_tol(tol, c.radius);
    if !samples.iter().all(|p| (p.distance(c.center) - c.radius).abs() <= round) {
        return None;
    }
    let (a0, am, a1) = ((p0 - c.center).atan2(), (pm - c.center).atan2(), (p1 - c.center).atan2());
    let sweep = wrap(am - a0) + wrap(a1 - am);
    Some(Piece::Arc { center: c.center, radius: c.radius, sweep, from: p0, to: p1, ends: vec![p0, p1], mids: vec![pm] })
}

/// Joins `b` onto the end of `a` when both are pieces of one arc turning the same way.
fn join(a: &Piece, b: &Piece, tol: f64) -> Option<Piece> {
    match (a, b) {
        (
            Piece::Arc { center: c1, radius: r1, sweep: s1, from, to: t1, ends: e1, mids: m1 },
            Piece::Arc { center: c2, radius: r2, sweep: s2, from: f2, to, ends: e2, mids: m2 },
        ) if t1.distance(*f2) <= tol
            && c1.distance(*c2) <= round_tol(tol, r1.max(*r2))
            && (r1 - r2).abs() <= round_tol(tol, r1.max(*r2))
            && s1.signum() == s2.signum()
            && (s1 + s2).abs() <= TAU + 1e-6 =>
        {
            Some(Piece::Arc {
                center: *c1,
                radius: *r1,
                sweep: s1 + s2,
                from: *from,
                to: *to,
                ends: e1.iter().chain(e2).copied().collect(),
                mids: m1.iter().chain(m2).copied().collect(),
            })
        }
        _ => None,
    }
}

fn to_element(p: Piece, tol: f64) -> Element {
    match p {
        Piece::Line(l) => Element::Segment(l),
        Piece::Arc { center, radius, sweep, from, to, ends, mids } => {
            let mut pts: Vec<Point> = Vec::new();
            for e in ends {
                if pts.iter().all(|q| q.distance(e) > tol) {
                    pts.push(e);
                }
            }
            if pts.len() < 3 {
                pts.extend(mids);
            }
            let c = fit_circle(&pts).unwrap_or(Circle::new(center, radius));
            if from.distance(to) <= tol && sweep.abs() > PI {
                Element::Circle(c)
            } else {
                Element::Arc { center: c.center, radius: c.radius, start: (from - c.center).atan2(), sweep }
            }
        }
    }
}

/// The straight segments, arcs and circles a path is made of. A curve counts as an arc when
/// it stays within `tol` (mm) of a circle, or within a thousandth of the radius for large
/// circles, consecutive pieces of one arc are joined, and a
/// closed run of arcs becomes a circle. Curves that are neither straight nor round are left
/// out, as are zero-length pieces.
pub fn elements(path: &BezPath, tol: f64) -> Vec<Element> {
    let tol = clean_tol(tol);
    let mut subpaths: Vec<(Vec<PathSeg>, bool)> = Vec::new();
    let mut cur: Vec<PathSeg> = Vec::new();
    let (mut start, mut last) = (Point::ZERO, Point::ZERO);
    for el in path.elements() {
        match *el {
            PathEl::MoveTo(p) => {
                if !cur.is_empty() {
                    subpaths.push((std::mem::take(&mut cur), false));
                }
                (start, last) = (p, p);
            }
            PathEl::LineTo(p) => {
                cur.push(PathSeg::Line(Line::new(last, p)));
                last = p;
            }
            PathEl::QuadTo(a, p) => {
                cur.push(PathSeg::Quad(kurbo::QuadBez::new(last, a, p)));
                last = p;
            }
            PathEl::CurveTo(a, b, p) => {
                cur.push(PathSeg::Cubic(kurbo::CubicBez::new(last, a, b, p)));
                last = p;
            }
            PathEl::ClosePath => {
                if last != start {
                    cur.push(PathSeg::Line(Line::new(last, start)));
                }
                last = start;
                subpaths.push((std::mem::take(&mut cur), true));
            }
        }
    }
    if !cur.is_empty() {
        subpaths.push((cur, false));
    }
    let mut out = Vec::new();
    for (segs, closed) in subpaths {
        let mut pieces: Vec<Piece> = Vec::new();
        for s in segs {
            let Some(p) = piece(s, tol) else { continue };
            match pieces.last().and_then(|q| join(q, &p, tol)) {
                Some(j) => *pieces.last_mut().unwrap() = j,
                None => pieces.push(p),
            }
        }
        if closed
            && pieces.len() > 1
            && let Some(j) = join(&pieces[pieces.len() - 1], &pieces[0], tol)
        {
            pieces.pop();
            pieces[0] = j;
        }
        out.extend(pieces.into_iter().map(|p| to_element(p, tol)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::{Rect, RoundedRect};

    const TOL: f64 = 1e-6;

    fn seg(x0: f64, y0: f64, x1: f64, y1: f64) -> Element {
        Element::Segment(Line::new((x0, y0), (x1, y1)))
    }

    fn circ(x: f64, y: f64, r: f64) -> Element {
        Element::Circle(Circle::new((x, y), r))
    }

    /// Every solution touches every input: distance to the carrier equals the radius (outside
    /// or inside), and the touch point lies on both the circle and the carrier.
    fn check(sols: &[Solution], els: &[&Element]) {
        for s in sols {
            assert!(s.circle.radius > 0.0 && s.circle.radius.is_finite(), "{s:?}");
            assert_eq!(s.touches.len(), els.len());
            for (e, p) in els.iter().zip(&s.touches) {
                let c = e.carrier().unwrap();
                assert!((p.distance(s.circle.center) - s.circle.radius).abs() < 2e-6, "touch on the circle: {s:?}");
                let on_carrier = match c {
                    Carrier::Line { normal, offset } => (normal.dot(p.to_vec2()) - offset).abs(),
                    Carrier::Circle(ci) => (p.distance(ci.center) - ci.radius).abs(),
                };
                assert!(on_carrier < 2e-6, "touch on the input: {s:?}");
                // Tangency: the distance from the centre to the carrier equals the radius.
                let gap = match c {
                    Carrier::Line { normal, offset } => ((normal.dot(s.circle.center.to_vec2()) - offset).abs() - s.circle.radius).abs(),
                    Carrier::Circle(ci) => {
                        let d = s.circle.center.distance(ci.center);
                        (d - (ci.radius + s.circle.radius)).abs().min((d - (ci.radius - s.circle.radius).abs()).abs())
                    }
                };
                assert!(gap < 2e-6, "tangent: {s:?}");
            }
        }
    }

    fn has(sols: &[Solution], x: f64, y: f64, r: f64) -> bool {
        sols.iter().any(|s| s.circle.center.distance(Point::new(x, y)) < 1e-6 && (s.circle.radius - r).abs() < 1e-6)
    }

    #[test]
    fn three_lines_give_the_incircle_and_the_three_excircles() {
        // The 3-4-5 right triangle: inradius 1 at (1, 1), exradii 2, 3 and 6.
        let (a, b, c) = (seg(0.0, 0.0, 4.0, 0.0), seg(4.0, 0.0, 0.0, 3.0), seg(0.0, 3.0, 0.0, 0.0));
        let sols = apollonius(&a, &b, &c, TOL).unwrap();
        check(&sols, &[&a, &b, &c]);
        assert_eq!(sols.len(), 4, "{sols:?}");
        let radii: Vec<f64> = sols.iter().map(|s| s.circle.radius).collect();
        for (r, want) in radii.iter().zip([1.0, 2.0, 3.0, 6.0]) {
            assert!((r - want).abs() < 1e-9, "{radii:?}");
        }
        assert!(has(&sols, 1.0, 1.0, 1.0));
        // Only the incircle touches the sides themselves; the others touch their extensions.
        let inside: Vec<_> = sols.iter().filter(|s| s.on_elements).collect();
        assert_eq!(inside.len(), 1);
        assert!((inside[0].circle.radius - 1.0).abs() < 1e-9);
    }

    #[test]
    fn three_touching_unit_circles_give_the_soddy_circles() {
        let h = 3f64.sqrt();
        let (a, b, c) = (circ(0.0, 0.0, 1.0), circ(2.0, 0.0, 1.0), circ(1.0, h, 1.0));
        let sols = apollonius(&a, &b, &c, TOL).unwrap();
        check(&sols, &[&a, &b, &c]);
        let centre = (1.0, h / 3.0);
        assert!(has(&sols, centre.0, centre.1, 1.0 / (3.0 + 2.0 * h)), "{sols:?}");
        assert!(has(&sols, centre.0, centre.1, 1.0 / (2.0 * h - 3.0)), "{sols:?}");
    }

    #[test]
    fn three_separate_circles_give_eight_circles() {
        let (a, b, c) = (circ(0.0, 0.0, 1.0), circ(10.0, 0.0, 2.0), circ(4.0, 8.0, 1.5));
        let sols = apollonius(&a, &b, &c, TOL).unwrap();
        check(&sols, &[&a, &b, &c]);
        assert_eq!(sols.len(), 8, "{sols:?}");
        assert!(sols.iter().all(|s| s.on_elements), "full circles hold every touch point");
    }

    #[test]
    fn mixed_inputs_solve_and_check() {
        // Two lines and a circle, and a line with two circles.
        let (a, b, c) = (seg(0.0, 0.0, 20.0, 0.0), seg(0.0, 0.0, 0.0, 20.0), circ(10.0, 10.0, 2.0));
        let sols = apollonius(&a, &b, &c, TOL).unwrap();
        check(&sols, &[&a, &b, &c]);
        assert!(!sols.is_empty() && sols.len() <= 8, "{sols:?}");
        let (a, b, c) = (seg(-20.0, 0.0, 20.0, 0.0), circ(-5.0, 4.0, 2.0), circ(6.0, 5.0, 3.0));
        let sols = apollonius(&a, &b, &c, TOL).unwrap();
        check(&sols, &[&a, &b, &c]);
        assert!(!sols.is_empty() && sols.len() <= 8, "{sols:?}");
        // The circle touching y = 0 and both unit circles at (+-2, 1) from below the line is
        // not there; one from above with centre on x = 0 is.
        let (a, b, c) = (seg(-10.0, 0.0, 10.0, 0.0), circ(-2.0, 1.0, 1.0), circ(2.0, 1.0, 1.0));
        let sols = apollonius(&a, &b, &c, TOL).unwrap();
        check(&sols, &[&a, &b, &c]);
        // Centre (0, r) with distance to (2, 1) equal to r + 1: 4 + (r - 1)^2 = (r + 1)^2, r = 1.
        assert!(has(&sols, 0.0, 1.0, 1.0), "{sols:?}");
    }

    #[test]
    fn two_lines_at_right_angles_give_four_corner_circles() {
        let (a, b) = (seg(-10.0, 0.0, 10.0, 0.0), seg(0.0, -10.0, 0.0, 10.0));
        let sols = with_radius(&a, &b, 2.0, TOL).unwrap();
        check(&sols, &[&a, &b]);
        assert_eq!(sols.len(), 4);
        for (x, y) in [(2.0, 2.0), (-2.0, 2.0), (2.0, -2.0), (-2.0, -2.0)] {
            assert!(has(&sols, x, y, 2.0), "{sols:?}");
        }
    }

    #[test]
    fn a_line_and_a_circle_with_a_radius() {
        // The circle sits 2 above the line with radius 1: only one circle of radius 1 fits
        // between them, touching the line at the origin.
        let (a, b) = (seg(-10.0, 0.0, 10.0, 0.0), circ(0.0, 3.0, 1.0));
        let sols = with_radius(&a, &b, 1.0, TOL).unwrap();
        check(&sols, &[&a, &b]);
        assert_eq!(sols.len(), 1, "{sols:?}");
        assert!(has(&sols, 0.0, 1.0, 1.0));
        assert!(sols[0].touches[0].distance(Point::ZERO) < 1e-9);
        assert!(sols[0].touches[1].distance(Point::new(0.0, 2.0)) < 1e-9);
        // A larger radius gives circles beside the given one (outside) and around it (inside).
        let sols = with_radius(&a, &b, 3.0, TOL).unwrap();
        check(&sols, &[&a, &b]);
        assert_eq!(sols.len(), 4, "{sols:?}");
    }

    #[test]
    fn two_circles_with_a_radius() {
        let (a, b) = (circ(0.0, 0.0, 2.0), circ(6.0, 0.0, 2.0));
        let sols = with_radius(&a, &b, 1.0, TOL).unwrap();
        check(&sols, &[&a, &b]);
        assert_eq!(sols.len(), 1, "{sols:?}");
        assert!(has(&sols, 3.0, 0.0, 1.0));
        let sols = with_radius(&a, &b, 2.0, TOL).unwrap();
        check(&sols, &[&a, &b]);
        assert_eq!(sols.len(), 2, "{sols:?}");
        assert!(has(&sols, 3.0, 7f64.sqrt(), 2.0) && has(&sols, 3.0, -(7f64.sqrt()), 2.0));
        // A big radius also gives circles that hold both.
        let sols = with_radius(&a, &b, 6.0, TOL).unwrap();
        check(&sols, &[&a, &b]);
        assert!(sols.iter().any(|s| s.touches[0].x < 0.0 && s.touches[1].x > 6.0), "{sols:?}");
    }

    #[test]
    fn degenerate_inputs_give_plain_errors() {
        let (a, far, near) = (seg(0.0, 0.0, 10.0, 0.0), seg(0.0, 10.0, 10.0, 10.0), seg(0.0, 4.0, 10.0, 4.0));
        assert_eq!(with_radius(&a, &far, 2.0, TOL), Err(TangentError::NoSolution));
        assert_eq!(with_radius(&a, &near, 2.0, TOL), Err(TangentError::Endless));
        let (c2, c6) = (circ(0.0, 0.0, 2.0), circ(0.0, 0.0, 6.0));
        assert_eq!(with_radius(&c2, &c6, 2.0, TOL), Err(TangentError::Endless));
        assert_eq!(with_radius(&c2, &c6, 1.0, TOL), Err(TangentError::NoSolution));
        for r in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(with_radius(&a, &c2, r, TOL), Err(TangentError::BadRadius));
        }
        let dot = seg(1.0, 1.0, 1.0, 1.0);
        assert_eq!(with_radius(&a, &dot, 1.0, TOL), Err(TangentError::BadInput));
        assert_eq!(with_radius(&a, &circ(f64::NAN, 0.0, 1.0), 1.0, TOL), Err(TangentError::BadInput));
        assert_eq!(apollonius(&a, &dot, &c2, TOL), Err(TangentError::BadInput));
        // The same line twice: endless circles touch it and the third.
        assert_eq!(apollonius(&a, &seg(20.0, 0.0, 30.0, 0.0), &c2, TOL), Err(TangentError::Endless));
        // Three parallel lines: no circle touches all of them.
        assert_eq!(apollonius(&a, &near, &far, TOL), Err(TangentError::NoSolution));
        // Concentric circles and a third circle between them that is concentric too.
        assert_eq!(apollonius(&c2, &c6, &circ(0.0, 0.0, 4.0), TOL), Err(TangentError::NoSolution));
        // A circle inside another with a line far away.
        assert_eq!(apollonius(&c2, &c6, &seg(0.0, 50.0, 10.0, 50.0), TOL), Err(TangentError::NoSolution));
    }

    #[test]
    fn no_result_is_ever_nan() {
        // A sweep of awkward inputs: whatever comes back is finite and checked.
        let inputs = [
            seg(0.0, 0.0, 1.0, 0.0),
            seg(0.0, 0.0, 0.0, 1.0),
            seg(0.0, 1e-9, 1.0, 1e-9),
            circ(0.0, 0.0, 1.0),
            circ(1e-9, 0.0, 1.0),
            circ(2.0, 0.0, 1.0),
            circ(0.5, 0.0, 0.5),
            circ(1e6, 1e6, 3.0),
        ];
        for a in &inputs {
            for b in &inputs {
                for r in [1e-3, 0.5, 1.0, 7.0] {
                    if let Ok(s) = with_radius(a, b, r, TOL) {
                        check(&s, &[a, b]);
                    }
                }
                for c in &inputs {
                    if let Ok(s) = apollonius(a, b, c, TOL) {
                        assert!(s.len() <= 8);
                        check(&s, &[a, b, c]);
                    }
                }
            }
        }
    }

    #[test]
    fn arcs_hold_only_their_own_part() {
        let arc = Element::Arc { center: Point::ZERO, radius: 2.0, start: 0.0, sweep: PI / 2.0 };
        assert!(arc.holds(Point::new(0.0, 2.0), 1e-9));
        assert!(arc.holds(Point::new(2f64.sqrt(), 2f64.sqrt()), 1e-9));
        assert!(!arc.holds(Point::new(-2.0, 0.0), 1e-6));
        assert!(arc.nearest(Point::new(-5.0, 0.1)).distance(Point::new(0.0, 2.0)) < 1e-9);
        let cw = Element::Arc { center: Point::ZERO, radius: 2.0, start: 0.0, sweep: -PI / 2.0 };
        assert!(cw.holds(Point::new(0.0, -2.0), 1e-9) && !cw.holds(Point::new(0.0, 2.0), 1e-6));
        // A tangent circle on the missing part of an arc is still found, but marked.
        let line = seg(-10.0, -3.0, 10.0, -3.0);
        let sols = with_radius(&arc, &line, 0.5, TOL).unwrap();
        check(&sols, &[&arc, &line]);
        assert!(sols.iter().any(|s| !s.on_elements));
    }

    #[test]
    fn paths_split_into_lines_arcs_and_circles() {
        let circle = Circle::new((5.0, 7.0), 12.5).to_path(0.01);
        let els = elements(&circle, 0.05);
        assert_eq!(els.len(), 1, "{els:?}");
        let Element::Circle(c) = els[0] else { panic!("{els:?}") };
        assert!(c.center.distance(Point::new(5.0, 7.0)) < 1e-6 && (c.radius - 12.5).abs() < 1e-6, "{c:?}");

        let rr = RoundedRect::from_rect(Rect::new(0.0, 0.0, 40.0, 20.0), 5.0).to_path(0.01);
        let els = elements(&rr, 0.05);
        let lines = els.iter().filter(|e| matches!(e, Element::Segment(_))).count();
        let arcs: Vec<_> = els.iter().filter(|e| matches!(e, Element::Arc { .. })).collect();
        assert_eq!((lines, arcs.len()), (4, 4), "{els:?}");
        for a in arcs {
            let Element::Arc { radius, sweep, .. } = *a else { unreachable!() };
            assert!((radius - 5.0).abs() < 1e-3 && (sweep.abs() - PI / 2.0).abs() < 1e-3, "{a:?}");
        }

        let square = Rect::new(0.0, 0.0, 10.0, 10.0).to_path(0.01);
        let els = elements(&square, 0.05);
        assert_eq!(els.len(), 4);
        assert!(els.iter().all(|e| matches!(e, Element::Segment(_))));

        // A straight cubic is a segment; an S-curve is neither and is left out.
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.curve_to((1.0, 0.0), (2.0, 0.0), (3.0, 0.0));
        p.move_to((0.0, 10.0));
        p.curve_to((5.0, 20.0), (5.0, 0.0), (10.0, 10.0));
        let els = elements(&p, 0.05);
        assert_eq!(els, vec![seg(0.0, 0.0, 3.0, 0.0)]);
    }

    /// A circle of four cubics, one per quarter, with the usual control arm (0.5523 of the
    /// radius), as SVG import and many drawing programs write it.
    fn four_cubic_circle(c: Point, r: f64, quarters: usize) -> BezPath {
        let k = 4.0 / 3.0 * (2f64.sqrt() - 1.0) * r;
        let mut p = BezPath::new();
        let at = |a: f64| c + Vec2::new(a.cos(), a.sin()) * r;
        let dir = |a: f64| Vec2::new(-a.sin(), a.cos());
        p.move_to(at(0.0));
        for q in 0..quarters {
            let (a0, a1) = (q as f64 * PI / 2.0, (q + 1) as f64 * PI / 2.0);
            p.curve_to(at(a0) + dir(a0) * k, at(a1) - dir(a1) * k, at(a1));
        }
        if quarters == 4 {
            p.close_path();
        }
        p
    }

    #[test]
    fn large_circles_and_arcs_made_of_quarter_cubics_are_recognised() {
        for r in [5.0, 50.0, 200.0, 300.0, 2000.0] {
            let els = elements(&four_cubic_circle(Point::new(10.0, -20.0), r, 4), 0.05);
            assert_eq!(els.len(), 1, "radius {r}: {els:?}");
            let Element::Circle(c) = els[0] else { panic!("radius {r}: {els:?}") };
            assert!(c.center.distance(Point::new(10.0, -20.0)) < 1e-6 && (c.radius - r).abs() < 1e-6, "radius {r}: {c:?}");
            // Half of it is one arc of half a turn.
            let els = elements(&four_cubic_circle(Point::new(10.0, -20.0), r, 2), 0.05);
            assert_eq!(els.len(), 1, "radius {r}: {els:?}");
            let Element::Arc { center, radius, sweep, .. } = els[0] else { panic!("radius {r}: {els:?}") };
            assert!(center.distance(Point::new(10.0, -20.0)) < 1e-6 && (radius - r).abs() < 1e-6 && (sweep - PI).abs() < 1e-6);
        }
        // A large ellipse that is clearly not round is still left out.
        let ellipse = kurbo::Ellipse::new((0.0, 0.0), (300.0, 290.0), 0.0).to_path(0.01);
        let els = elements(&ellipse, 0.05);
        assert!(els.iter().all(|e| !matches!(e, Element::Circle(_))), "{els:?}");
    }

    #[test]
    fn a_half_circle_path_is_an_arc_and_solves() {
        let half = kurbo::Arc::new((0.0, 0.0), (10.0, 10.0), 0.0, PI, 0.0).to_path(0.01);
        let els = elements(&half, 0.05);
        assert_eq!(els.len(), 1, "{els:?}");
        let Element::Arc { center, radius, sweep, .. } = els[0] else { panic!("{els:?}") };
        assert!(center.distance(Point::ZERO) < 1e-6 && (radius - 10.0).abs() < 1e-6 && (sweep - PI).abs() < 1e-6);
        let floor = seg(-20.0, 0.0, 20.0, 0.0);
        let sols = with_radius(&els[0], &floor, 2.0, TOL).unwrap();
        check(&sols, &[&els[0], &floor]);
        // Inside the dome, touching the floor: centre at height 2, 8 from the middle.
        assert!(sols.iter().any(|s| s.on_elements && (s.circle.center.y - 2.0).abs() < 1e-6), "{sols:?}");
    }
}
