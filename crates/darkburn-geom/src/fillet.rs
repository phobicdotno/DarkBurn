//! Round corners (fillets, including negative-radius notches) and chamfers.
//!
//! Only corners between two straight segments are changed; curve segments and the end
//! nodes of open subpaths are left alone. The amount cut back along each edge is limited
//! to half of the shorter neighbouring edge, so neighbouring corners never overlap: a
//! radius that would need more is reduced to fit (the corner is still rounded).

use crate::node::NodeRef;
use crate::path::{Contour, contours, from_contours};
use kurbo::{Arc, BezPath, CubicBez, Line, PathEl, PathSeg, Point, Vec2};

#[derive(Clone, Copy, Debug)]
enum Kind {
    /// Positive: tangent arc; negative: concave notch centred on the corner.
    Round(f64),
    Chamfer(f64),
}

/// Corner replacement: cut-back distances along the incoming and outgoing edges and the
/// segments going from the incoming cut point to the outgoing cut point.
struct Corner {
    d: f64,
    segs: Vec<PathSeg>,
}

fn arc_segs(center: Point, radius: f64, from: Point, sweep: f64, to: Point) -> Vec<PathSeg> {
    let start_angle = (from - center).atan2();
    let arc = Arc::new(center, Vec2::new(radius, radius), start_angle, sweep, 0.0);
    let mut segs = Vec::new();
    let mut cur = from;
    for el in arc.append_iter(1e-7) {
        if let PathEl::CurveTo(p1, p2, p3) = el {
            segs.push(PathSeg::Cubic(CubicBez::new(cur, p1, p2, p3)));
            cur = p3;
        }
    }
    // Snap the ends exactly onto the cut points.
    if let Some(PathSeg::Cubic(c)) = segs.last_mut() {
        let d = to - c.p3;
        c.p2 += d;
        c.p3 = to;
    }
    segs
}

fn corner(v: Point, u_in: Vec2, u_out: Vec2, len_in: f64, len_out: f64, kind: Kind, tol: f64) -> Option<Corner> {
    let cross = u_in.cross(u_out);
    let dot = u_in.dot(u_out);
    let turn = cross.atan2(dot); // signed exterior turn angle
    let max_d = 0.5 * len_in.min(len_out);
    // Collinear or a full reversal (spike): not a corner we can round.
    if turn.abs() < 1e-9 || std::f64::consts::PI - turn.abs() < 1e-9 || cross.abs() * len_in.min(len_out) < tol {
        return None;
    }
    let interior = std::f64::consts::PI - turn.abs();
    match kind {
        Kind::Round(r) if r > 0.0 => {
            let half_tan = (interior * 0.5).tan();
            let d = (r / half_tan).min(max_d);
            let r = d * half_tan;
            if d < tol || r < tol * 1e-3 {
                return None;
            }
            let a = v - u_in * d;
            let b = v + u_out * d;
            let n = if turn > 0.0 { Vec2::new(-u_in.y, u_in.x) } else { Vec2::new(u_in.y, -u_in.x) };
            let center = a + n * r;
            Some(Corner { d, segs: arc_segs(center, r, a, turn, b) })
        }
        Kind::Round(r) if r < 0.0 => {
            let d = (-r).min(max_d);
            if d < tol {
                return None;
            }
            let a = v - u_in * d;
            let b = v + u_out * d;
            let sweep = -turn.signum() * interior;
            Some(Corner { d, segs: arc_segs(v, d, a, sweep, b) })
        }
        Kind::Chamfer(dist) if dist > 0.0 => {
            let d = dist.min(max_d);
            if d < tol {
                return None;
            }
            let a = v - u_in * d;
            let b = v + u_out * d;
            Some(Corner { d, segs: vec![PathSeg::Line(Line::new(a, b))] })
        }
        _ => None,
    }
}

fn process_contour(c: &Contour, kind: Kind, tol: f64, selected: impl Fn(usize) -> bool) -> Contour {
    let n = c.segs.len();
    if n < 2 {
        return c.clone();
    }
    // corners[i] = replacement at node i.
    let mut corners: Vec<Option<Corner>> = (0..n).map(|_| None).collect();
    for (i, slot) in corners.iter_mut().enumerate() {
        if !selected(i) {
            continue;
        }
        let si = if c.closed {
            (i + n - 1) % n
        } else if i == 0 {
            continue;
        } else {
            i - 1
        };
        let (PathSeg::Line(lin), PathSeg::Line(lout)) = (c.segs[si], c.segs[i]) else { continue };
        let (vin, vout) = (lin.p1 - lin.p0, lout.p1 - lout.p0);
        let (len_in, len_out) = (vin.hypot(), vout.hypot());
        if len_in < 1e-12 || len_out < 1e-12 {
            continue;
        }
        *slot = corner(lout.p0, vin / len_in, vout / len_out, len_in, len_out, kind, tol);
    }
    let mut segs = Vec::with_capacity(n * 2);
    for k in 0..n {
        let next = (k + 1) % n;
        let seg = match c.segs[k] {
            PathSeg::Line(l) => {
                let len = (l.p1 - l.p0).hypot();
                let dir = if len > 0.0 { (l.p1 - l.p0) / len } else { Vec2::ZERO };
                let d0 = corners[k].as_ref().map_or(0.0, |c| c.d);
                let d1 = if c.closed || next != 0 { corners[next].as_ref().map_or(0.0, |c| c.d) } else { 0.0 };
                let (p0, p1) = (l.p0 + dir * d0, l.p1 - dir * d1);
                // A fully consumed edge (both ends cut by half) disappears.
                if p0.distance(p1) > 1e-12 { Some(PathSeg::Line(Line::new(p0, p1))) } else { None }
            }
            s => Some(s),
        };
        segs.extend(seg);
        if (c.closed || next != 0)
            && let Some(cn) = &corners[next]
        {
            segs.extend(cn.segs.iter().copied());
        }
    }
    Contour::new(segs, c.closed)
}

fn apply(path: &BezPath, kind: Kind, tol: f64, nodes: Option<&[NodeRef]>) -> BezPath {
    let cs: Vec<Contour> = contours(path)
        .iter()
        .enumerate()
        .map(|(ci, c)| match nodes {
            None => process_contour(c, kind, tol, |_| true),
            Some(sel) => process_contour(c, kind, tol, |i| sel.contains(&NodeRef::new(ci, i))),
        })
        .collect();
    from_contours(cs.iter())
}

/// Rounds every corner between two straight segments with a tangent arc of `radius`.
/// A negative radius cuts an inward notch instead: a concave arc of `|radius|` centred on
/// the corner. Corners whose cut-back or turn is below `tol` (mm) are skipped; the radius
/// is reduced where it would use more than half of either neighbouring edge.
pub fn round_corners(path: &BezPath, radius: f64, tol: f64) -> BezPath {
    apply(path, Kind::Round(radius), tol, None)
}

/// Like [`round_corners`] but only at the given nodes.
pub fn round_corners_at(path: &BezPath, nodes: &[NodeRef], radius: f64, tol: f64) -> BezPath {
    apply(path, Kind::Round(radius), tol, Some(nodes))
}

/// Cuts every corner between two straight segments with a straight bevel `distance`
/// along each edge (limited to half of the shorter edge).
pub fn chamfer(path: &BezPath, distance: f64, tol: f64) -> BezPath {
    apply(path, Kind::Chamfer(distance), tol, None)
}

/// Like [`chamfer`] but only at the given nodes.
pub fn chamfer_at(path: &BezPath, nodes: &[NodeRef], distance: f64, tol: f64) -> BezPath {
    apply(path, Kind::Chamfer(distance), tol, Some(nodes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path::{bounding_box, is_closed, signed_area};
    use kurbo::{Rect, Shape};
    use std::f64::consts::PI;

    const T: f64 = 1e-6;

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> BezPath {
        Rect::new(x0, y0, x1, y1).to_path(0.1)
    }
    fn polyline(pts: &[(f64, f64)], close: bool) -> BezPath {
        let mut p = BezPath::new();
        p.move_to(pts[0]);
        for q in &pts[1..] {
            p.line_to(*q);
        }
        if close {
            p.close_path();
        }
        p
    }

    #[test]
    fn fillet_rectangle_area() {
        let r = rect(0.0, 0.0, 40.0, 20.0);
        let f = round_corners(&r, 3.0, T);
        let expected = 800.0 - (4.0 - PI) * 9.0;
        assert!((signed_area(&f).abs() - expected).abs() < 1e-4, "{}", signed_area(&f));
        assert!(is_closed(&f, 0.0));
        let bb = bounding_box(&f).unwrap();
        assert!((bb.width() - 40.0).abs() < 1e-6 && (bb.height() - 20.0).abs() < 1e-6);
        // Same result in the other winding direction.
        let rev = crate::path::reverse(&r);
        assert!((signed_area(&round_corners(&rev, 3.0, T)).abs() - expected).abs() < 1e-4);
    }

    #[test]
    fn negative_radius_notch() {
        let r = rect(0.0, 0.0, 40.0, 20.0);
        let f = round_corners(&r, -3.0, T);
        let expected = 800.0 - PI * 9.0;
        assert!((signed_area(&f).abs() - expected).abs() < 1e-4, "{}", signed_area(&f));
        // Notches stay inside the original outline.
        let bb = bounding_box(&f).unwrap();
        assert!(bb.x0 >= -1e-9 && bb.y0 >= -1e-9 && bb.x1 <= 40.0 + 1e-9 && bb.y1 <= 20.0 + 1e-9);
    }

    #[test]
    fn radius_too_large_is_clamped() {
        let r = rect(0.0, 0.0, 10.0, 10.0);
        let f = round_corners(&r, 100.0, T);
        // Clamped to 5 mm: the square becomes a circle of radius 5.
        assert!((signed_area(&f).abs() - PI * 25.0).abs() < 1e-3);
        let bb = bounding_box(&f).unwrap();
        assert!((bb.width() - 10.0).abs() < 1e-6);
    }

    #[test]
    fn collinear_nodes_skipped_and_curves_untouched() {
        let p = polyline(&[(0.0, 0.0), (5.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)], true);
        let f = round_corners(&p, 1.0, T);
        // 4 corners get arcs; the collinear node at (5,0) does not.
        let expected = 100.0 - (4.0 - PI);
        assert!((signed_area(&f).abs() - expected).abs() < 1e-4);
        let circle = kurbo::Circle::new((0.0, 0.0), 5.0).to_path(1e-6);
        assert_eq!(round_corners(&circle, 1.0, T), circle);
        assert_eq!(chamfer(&circle, 1.0, T), circle);
    }

    #[test]
    fn open_polyline_ends_not_rounded() {
        let p = polyline(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)], false);
        let f = round_corners(&p, 2.0, T);
        let c = &contours(&f)[0];
        assert!(!c.closed);
        assert_eq!(c.start(), Some(Point::new(0.0, 0.0)));
        assert_eq!(c.end(), Some(Point::new(10.0, 10.0)));
        let n = c.segs.len();
        assert!(n >= 3);
        assert!(matches!(c.segs[0], PathSeg::Line(_)) && matches!(c.segs[n - 1], PathSeg::Line(_)));
        // The arc is tangent: it starts at (8,0) and ends at (10,2).
        assert_eq!(kurbo::ParamCurve::end(&c.segs[0]), Point::new(8.0, 0.0));
        assert!(kurbo::ParamCurve::start(&c.segs[n - 1]).distance(Point::new(10.0, 2.0)) < 1e-9);
        // Arc points stay at distance r from the centre (8,2).
        for s in &c.segs[1..n - 1] {
            assert!(matches!(s, PathSeg::Cubic(_)));
            let mid = kurbo::ParamCurve::eval(s, 0.5);
            assert!((mid.distance(Point::new(8.0, 2.0)) - 2.0).abs() < 1e-6);
        }
    }

    #[test]
    fn chamfer_rectangle() {
        let r = rect(0.0, 0.0, 40.0, 20.0);
        let f = chamfer(&r, 2.0, T);
        assert!((signed_area(&f).abs() - (800.0 - 4.0 * 2.0)).abs() < 1e-9);
        assert_eq!(contours(&f)[0].segs.len(), 8);
    }

    #[test]
    fn only_selected_nodes() {
        let r = rect(0.0, 0.0, 40.0, 20.0);
        let f = round_corners_at(&r, &[NodeRef::new(0, 0)], 3.0, T);
        assert!((signed_area(&f).abs() - (800.0 - (4.0 - PI) * 9.0 / 4.0)).abs() < 1e-4);
        let c = chamfer_at(&r, &[NodeRef::new(0, 1), NodeRef::new(0, 2)], 2.0, T);
        assert!((signed_area(&c).abs() - (800.0 - 4.0)).abs() < 1e-9);
        assert_eq!(round_corners_at(&r, &[NodeRef::new(3, 0)], 3.0, T), from_contours(contours(&r).iter()));
    }

    #[test]
    fn degenerate_inputs() {
        assert!(round_corners(&BezPath::new(), 1.0, T).is_empty());
        let l = polyline(&[(0.0, 0.0), (1.0, 0.0)], false);
        assert_eq!(round_corners(&l, 1.0, T), l);
        // Zero radius / distance changes nothing.
        let r = rect(0.0, 0.0, 10.0, 10.0);
        assert!((signed_area(&round_corners(&r, 0.0, T)) - signed_area(&r)).abs() < 1e-12);
        assert!((signed_area(&chamfer(&r, 0.0, T)) - signed_area(&r)).abs() < 1e-12);
        // A spike (full reversal) is left alone.
        let spike = polyline(&[(0.0, 0.0), (10.0, 0.0), (5.0, 0.0)], false);
        assert_eq!(contours(&round_corners(&spike, 1.0, T))[0].segs.len(), 2);
    }

    #[test]
    fn triangle_fillet_tangent_points() {
        let tri = polyline(&[(0.0, 0.0), (20.0, 0.0), (10.0, 17.320508075688775)], true);
        let f = round_corners(&tri, 1.0, T);
        // Equilateral: each corner removes r^2 (cot(30°) - (π - 60°)/2 ... ) = r²(√3 - π/3).
        let removed = 3.0 * (3f64.sqrt() - PI / 3.0);
        let a0 = signed_area(&tri).abs();
        assert!((signed_area(&f).abs() - (a0 - removed)).abs() < 1e-4);
    }
}
