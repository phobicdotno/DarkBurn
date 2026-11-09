//! Remove rounded corners: replace each tangent arc between two straight lines by the sharp
//! corner where the two lines meet.
//!
//! A run of curve segments counts as a rounded corner when it starts and ends tangent to the
//! straight segments around it, the extended lines meet at one point, the two cut points are
//! the same distance from that point (the signature of a tangent arc) and the run's samples lie
//! on one circle. Anything else (free-form curves, stadium ends where the lines are parallel,
//! whole circles) is left alone.

use crate::path::{Contour, contours, from_contours};
use kurbo::{BezPath, Line, ParamCurve, ParamCurveDeriv, PathSeg, Point, Vec2};

fn is_line(s: &PathSeg) -> bool {
    matches!(s, PathSeg::Line(_))
}

fn tangent_at(s: &PathSeg, t: f64) -> Vec2 {
    let d = match s {
        PathSeg::Line(l) => l.p1 - l.p0,
        PathSeg::Quad(q) => q.deriv().eval(t).to_vec2(),
        PathSeg::Cubic(c) => c.deriv().eval(t).to_vec2(),
    };
    let h = d.hypot();
    if h < 1e-12 { Vec2::ZERO } else { d / h }
}

/// Intersection of the line through `a` along `da` and the line through `b` along `db`.
fn meet(a: Point, da: Vec2, b: Point, db: Vec2) -> Option<Point> {
    let cross = da.cross(db);
    if cross.abs() < 1e-6 {
        return None;
    }
    let t = (b - a).cross(db) / cross;
    Some(a + da * t)
}

/// The sharp corner for the arc run `run` between lines `lin` and `lout`, if it is a rounded corner.
fn sharp_corner(lin: &Line, lout: &Line, run: &[PathSeg], tol: f64) -> Option<Point> {
    let a = run.first()?.start();
    let b = run.last()?.end();
    let din = (lin.p1 - lin.p0).normalize();
    let dout = (lout.p1 - lout.p0).normalize();
    if !(din.x.is_finite() && dout.x.is_finite()) {
        return None;
    }
    // Tangent continuity at both ends.
    let t0 = tangent_at(run.first()?, 0.0);
    let t1 = tangent_at(run.last()?, 1.0);
    if t0.dot(din) < 1.0 - 1e-3 || t1.dot(dout) < 1.0 - 1e-3 {
        return None;
    }
    let v = meet(a, din, b, dout)?;
    // The corner must lie ahead of the incoming line and behind the outgoing one.
    if (v - a).dot(din) < -tol || (b - v).dot(dout) < -tol {
        return None;
    }
    let (da, db) = (a.distance(v), b.distance(v));
    if da < tol || (da - db).abs() > tol.max(da * 1e-3) {
        return None;
    }
    // One circle: centre where the normals at a and b meet.
    let na = Vec2::new(-din.y, din.x);
    let nb = Vec2::new(-dout.y, dout.x);
    let c = meet(a, na, b, nb)?;
    let r = c.distance(a);
    for s in run {
        for k in 0..=4 {
            let p = s.eval(f64::from(k) / 4.0);
            if (c.distance(p) - r).abs() > tol.max(r * 2e-3) {
                return None;
            }
        }
    }
    Some(v)
}

fn process(c: &Contour, tol: f64) -> Contour {
    // Zero-length lines (a closing segment of a path that already ends on its start) are noise.
    let c = &Contour::new(c.segs.iter().copied().filter(|s| !matches!(s, PathSeg::Line(l) if l.p0.distance(l.p1) < 1e-9)).collect(), c.closed);
    let n = c.segs.len();
    if n < 3 {
        return c.clone();
    }
    let mut drop = vec![false; n];
    // New end of a line i (index of line, point) and new start.
    let mut new_end: Vec<Option<Point>> = vec![None; n];
    let mut new_start: Vec<Option<Point>> = vec![None; n];
    let mut i = 0;
    let limit = if c.closed { n } else { n - 1 };
    while i < limit {
        if !is_line(&c.segs[i]) {
            i += 1;
            continue;
        }
        // Collect the curve run after line i.
        let mut j = i + 1;
        let mut run: Vec<usize> = Vec::new();
        while (c.closed || j < n) && !is_line(&c.segs[j % n]) {
            run.push(j % n);
            j += 1;
            if run.len() > n {
                break;
            }
        }
        let jn = j % n;
        if run.is_empty() || run.len() >= n || (!c.closed && j >= n) {
            i += 1;
            continue;
        }
        if let (PathSeg::Line(lin), PathSeg::Line(lout)) = (c.segs[i], c.segs[jn]) {
            let segs: Vec<PathSeg> = run.iter().map(|&k| c.segs[k]).collect();
            if let Some(v) = sharp_corner(&lin, &lout, &segs, tol) {
                for &k in &run {
                    drop[k] = true;
                }
                new_end[i] = Some(v);
                new_start[jn] = Some(v);
            }
        }
        i = if c.closed || j > i + 1 { j.max(i + 1) } else { i + 1 };
        if !c.closed && i >= limit {
            break;
        }
    }
    let mut out = Vec::with_capacity(n);
    for k in 0..n {
        if drop[k] {
            continue;
        }
        match c.segs[k] {
            PathSeg::Line(l) => {
                let l2 = Line::new(new_start[k].unwrap_or(l.p0), new_end[k].unwrap_or(l.p1));
                out.push(PathSeg::Line(l2));
            }
            s => out.push(s),
        }
    }
    Contour::new(out, c.closed)
}

/// Replaces every rounded corner (a tangent arc between two straight segments) by a sharp one.
/// `tol` is the distance in millimetres within which the arc must match a circle.
pub fn remove_rounded_corners(path: &BezPath, tol: f64) -> BezPath {
    let cs: Vec<Contour> = contours(path).iter().map(|c| process(c, tol)).collect();
    from_contours(cs.iter())
}

/// How many rounded corners `remove_rounded_corners` would remove.
pub fn count_rounded_corners(path: &BezPath, tol: f64) -> usize {
    contours(path).iter().map(|c| c.segs.len() - process(c, tol).segs.len()).filter(|d| *d > 0).count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fillet::round_corners;
    use crate::path::bounding_box;
    use kurbo::{Rect, Shape};

    fn rect(w: f64, h: f64) -> BezPath {
        Rect::new(0.0, 0.0, w, h).to_path(0.1)
    }

    #[test]
    fn rounded_rectangle_gets_its_corners_back() {
        let r = round_corners(&rect(20.0, 10.0), 3.0, 1e-6);
        assert!(bounding_box(&r).is_some());
        let sharp = remove_rounded_corners(&r, 1e-3);
        let bb = bounding_box(&sharp).unwrap();
        assert!((bb.width() - 20.0).abs() < 1e-6 && (bb.height() - 10.0).abs() < 1e-6);
        assert_eq!(contours(&sharp)[0].segs.len(), 4, "four lines");
        assert!((kurbo::Shape::area(&sharp).abs() - 200.0).abs() < 1e-6);
    }

    #[test]
    fn unequal_rounds_and_open_paths() {
        // An L shape with one rounded corner in the middle of an open polyline.
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((10.0, 0.0));
        p.line_to((10.0, 10.0));
        let r = round_corners(&p, 2.0, 1e-6);
        assert!(contours(&r)[0].segs.len() >= 3);
        let s = remove_rounded_corners(&r, 1e-3);
        let c = &contours(&s)[0];
        assert_eq!(c.segs.len(), 2);
        assert!(c.segs[0].end().distance(Point::new(10.0, 0.0)) < 1e-6);
    }

    #[test]
    fn circles_stadiums_and_free_curves_are_left_alone() {
        let circle = kurbo::Circle::new((5.0, 5.0), 4.0).to_path(0.01);
        assert_eq!(remove_rounded_corners(&circle, 1e-3), circle);
        // A stadium: parallel lines joined by half circles.
        let mut st = BezPath::new();
        st.move_to((0.0, 0.0));
        st.line_to((10.0, 0.0));
        st.extend(kurbo::Arc::new((10.0, 2.0), (2.0, 2.0), -std::f64::consts::FRAC_PI_2, std::f64::consts::PI, 0.0).append_iter(0.01).skip(1));
        st.line_to((0.0, 4.0));
        st.close_path();
        assert_eq!(count_rounded_corners(&st, 1e-3), 0);
    }
}
