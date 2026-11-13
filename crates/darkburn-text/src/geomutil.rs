//! Small path helpers: flattening to rings, union (weld) and synthetic emboldening.

use i_overlay::core::fill_rule::FillRule;
use i_overlay::float::simplify::SimplifyShape;
use kurbo::{BezPath, PathEl, Point, Rect, Shape, Vec2};

/// Flatten a path into polylines. Each entry is `(points, closed)`.
pub fn flatten_polylines(path: &BezPath, tol: f64) -> Vec<(Vec<Point>, bool)> {
    let mut out: Vec<(Vec<Point>, bool)> = Vec::new();
    let mut cur: Vec<Point> = Vec::new();
    kurbo::flatten(path.elements().iter().copied(), tol, |el| match el {
        PathEl::MoveTo(p) => {
            if cur.len() > 1 {
                out.push((std::mem::take(&mut cur), false));
            }
            cur.clear();
            cur.push(p);
        }
        PathEl::LineTo(p) => cur.push(p),
        PathEl::ClosePath if cur.len() > 1 => {
            let first = cur[0];
            out.push((std::mem::take(&mut cur), true));
            cur.push(first);
        }
        _ => {}
    });
    if cur.len() > 1 {
        out.push((cur, false));
    }
    out
}

/// Polylines back into a `BezPath`.
pub fn polylines_to_path(lines: &[(Vec<Point>, bool)]) -> BezPath {
    let mut p = BezPath::new();
    for (pts, closed) in lines {
        let mut it = pts.iter();
        if let Some(f) = it.next() {
            p.move_to(*f);
            for q in it {
                p.line_to(*q);
            }
            if *closed {
                p.close_path();
            }
        }
    }
    p
}

/// The farthest coordinate (mm) handed to the overlay library, which cannot take a point that is
/// not a number or absurdly far out.
const MAX_COORD: f64 = 1e12;

/// Union of all filled areas in `paths` (non-zero rule, which is how glyph outlines are wound).
/// The result is flattened to polygons within `tol` mm; one `BezPath` per outer shape, holes inside it.
/// An outline with a point that is not a number or absurdly far out has no area that could be
/// worked out and is left out.
pub fn weld(paths: &[BezPath], tol: f64) -> Vec<BezPath> {
    let mut rings: Vec<Vec<[f64; 2]>> = Vec::new();
    for p in paths {
        for (mut pts, _closed) in flatten_polylines(p, tol) {
            if pts.len() > 1 && pts.first() == pts.last() {
                pts.pop();
            }
            if pts.len() >= 3 && pts.iter().all(|q| q.x.abs() <= MAX_COORD && q.y.abs() <= MAX_COORD) {
                rings.push(pts.iter().map(|q| [q.x, q.y]).collect());
            }
        }
    }
    if rings.is_empty() {
        return Vec::new();
    }
    let shapes = rings.simplify_shape(FillRule::NonZero);
    shapes
        .into_iter()
        .map(|shape| {
            let mut bp = BezPath::new();
            for contour in shape {
                let mut it = contour.iter();
                if let Some(f) = it.next() {
                    bp.move_to((f[0], f[1]));
                    for q in it {
                        bp.line_to((q[0], q[1]));
                    }
                    bp.close_path();
                }
            }
            bp
        })
        .collect()
}

fn signed_area(pts: &[Point]) -> f64 {
    let mut a = 0.0;
    for i in 0..pts.len() {
        let p = pts[i];
        let q = pts[(i + 1) % pts.len()];
        a += p.x * q.y - q.x * p.y;
    }
    a / 2.0
}

fn norm(v: Vec2) -> Option<Vec2> {
    let l = v.hypot();
    if l < 1e-12 { None } else { Some(v / l) }
}

/// Grow (positive `d`) or shrink the filled area of a glyph path by moving every point along its
/// vertex normal (same idea as FreeType's emboldening). Counters (holes) shrink when the glyph grows.
pub fn embolden(path: &BezPath, d: f64) -> BezPath {
    // Split into subpaths of elements.
    let mut subs: Vec<Vec<PathEl>> = Vec::new();
    for el in path.elements() {
        match el {
            PathEl::MoveTo(_) => subs.push(vec![*el]),
            _ => {
                if let Some(s) = subs.last_mut() {
                    s.push(*el)
                }
            }
        }
    }
    // Fill side from the largest contour.
    let mut fill_left = false;
    let mut best = 0.0;
    for s in &subs {
        let pts = sub_points(s);
        let a = signed_area(&pts);
        if a.abs() > best {
            best = a.abs();
            fill_left = a > 0.0;
        }
    }
    let mut out = BezPath::new();
    for s in &subs {
        let pts = sub_points(s);
        let closed = s.iter().any(|e| matches!(e, PathEl::ClosePath));
        let n = pts.len();
        let mut moved = pts.clone();
        for i in 0..n {
            let prev = (0..n).map(|k| (i + n - 1 - k) % n).map(|j| pts[j]).find(|q| (*q - pts[i]).hypot() > 1e-9);
            let next = (1..=n).map(|k| (i + k) % n).map(|j| pts[j]).find(|q| (*q - pts[i]).hypot() > 1e-9);
            let (Some(pp), Some(nn)) = (prev, next) else { continue };
            let t1 = norm(pts[i] - pp);
            let t2 = norm(nn - pts[i]);
            let (Some(t1), Some(t2)) = (t1, t2) else { continue };
            // Away from the fill side.
            let nrm = |t: Vec2| if fill_left { Vec2::new(t.y, -t.x) } else { Vec2::new(-t.y, t.x) };
            let (n1, n2) = (nrm(t1), nrm(t2));
            let bis = norm(n1 + n2).unwrap_or(n1);
            let k = bis.dot(n1).max(0.5);
            let mut m = bis * (d / k);
            // The ends of an open path do not move sideways more than d.
            if !closed && (i == 0 || i == n - 1) {
                m = n1 * d;
            }
            moved[i] = pts[i] + m;
        }
        out.extend(rebuild(s, &moved));
    }
    out
}

fn sub_points(s: &[PathEl]) -> Vec<Point> {
    let mut v = Vec::new();
    for e in s {
        match e {
            PathEl::MoveTo(p) | PathEl::LineTo(p) => v.push(*p),
            PathEl::QuadTo(a, b) => {
                v.push(*a);
                v.push(*b)
            }
            PathEl::CurveTo(a, b, c) => {
                v.push(*a);
                v.push(*b);
                v.push(*c)
            }
            PathEl::ClosePath => {}
        }
    }
    // A closed contour that ends on its start point: keep it as is (the duplicate moves together).
    v
}

fn rebuild(s: &[PathEl], pts: &[Point]) -> Vec<PathEl> {
    let mut i = 0;
    let mut out = Vec::new();
    let mut take = || {
        let p = pts[i];
        i += 1;
        p
    };
    let mut first = None;
    for e in s {
        out.push(match e {
            PathEl::MoveTo(_) => {
                let p = take();
                first = Some(p);
                PathEl::MoveTo(p)
            }
            PathEl::LineTo(_) => PathEl::LineTo(take()),
            PathEl::QuadTo(..) => PathEl::QuadTo(take(), take()),
            PathEl::CurveTo(..) => PathEl::CurveTo(take(), take(), take()),
            PathEl::ClosePath => PathEl::ClosePath,
        });
    }
    // Duplicate start/end points must stay identical.
    if let (Some(f), Some(PathEl::ClosePath)) = (first, out.last().copied()) {
        let l = out.len();
        if l >= 2 {
            let last_pt = match out[l - 2] {
                PathEl::LineTo(p) => Some(p),
                PathEl::CurveTo(_, _, p) | PathEl::QuadTo(_, p) => Some(p),
                _ => None,
            };
            if let (Some(lp), Some(orig_first)) = (last_pt, s.first().and_then(|e| if let PathEl::MoveTo(p) = e { Some(*p) } else { None })) {
                let orig_last = match s[l - 2] {
                    PathEl::LineTo(p) => Some(p),
                    PathEl::CurveTo(_, _, p) | PathEl::QuadTo(_, p) => Some(p),
                    _ => None,
                };
                if orig_last == Some(orig_first) && (lp - f).hypot() > 0.0 {
                    out[l - 2] = match out[l - 2] {
                        PathEl::LineTo(_) => PathEl::LineTo(f),
                        PathEl::QuadTo(a, _) => PathEl::QuadTo(a, f),
                        PathEl::CurveTo(a, b, _) => PathEl::CurveTo(a, b, f),
                        o => o,
                    };
                }
            }
        }
    }
    out
}

/// Bounding box of a set of paths (None when all are empty).
pub fn union_bbox<'a>(paths: impl IntoIterator<Item = &'a BezPath>) -> Option<Rect> {
    let mut r: Option<Rect> = None;
    for p in paths {
        if p.elements().is_empty() {
            continue;
        }
        let b = p.bounding_box();
        r = Some(r.map_or(b, |a| a.union(b)));
    }
    r
}
