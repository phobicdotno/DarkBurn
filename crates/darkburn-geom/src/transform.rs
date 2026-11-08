//! Transforms: 4-point warp (bilinear or perspective), 16-point deform (bicubic grid),
//! mirror across a line, two-point rotate/scale and small affine helpers.
//!
//! Non-affine maps cannot keep Bézier curves exact, so warped paths are flattened in
//! source space within `tol` and every straight piece is subdivided until its mapped
//! midpoint lies within `tol` of the mapped chord. The result is made of straight lines.

use crate::path::{Polyline, flatten_to_polylines, polylines_to_bezpath};
use kurbo::{Affine, BezPath, Point, Rect, Vec2};

/// Maps every point of `path` through `f`, flattening curves and subdividing lines so the
/// result follows the mapped shape within `tol` (mm). Subpaths keep their closed flag.
pub fn map_path(path: &BezPath, tol: f64, f: impl Fn(Point) -> Point) -> BezPath {
    let tol = tol.max(1e-6);
    let pls: Vec<Polyline> = flatten_to_polylines(path, tol)
        .into_iter()
        .map(|pl| {
            let n = pl.points.len();
            let mut out = Vec::with_capacity(n * 2);
            if n == 0 {
                return Polyline::new(out, pl.closed);
            }
            let edges = if pl.closed { n } else { n - 1 };
            out.push(f(pl.points[0]));
            for i in 0..edges {
                let p = pl.points[i];
                let q = pl.points[(i + 1) % n];
                let fp = f(p);
                let fq = f(q);
                subdivide(p, q, fp, fq, &f, tol, 0, &mut out);
                if !(pl.closed && i + 1 == n) {
                    out.push(fq);
                }
            }
            out.dedup();
            Polyline::new(out, pl.closed)
        })
        .collect();
    polylines_to_bezpath(&pls)
}

#[allow(clippy::too_many_arguments)]
fn subdivide(p: Point, q: Point, fp: Point, fq: Point, f: &impl Fn(Point) -> Point, tol: f64, depth: u32, out: &mut Vec<Point>) {
    if depth >= 16 {
        return;
    }
    let m = p.midpoint(q);
    let fm = f(m);
    // Flat enough when the mapped midpoint lies on the mapped chord (projective maps keep
    // lines straight but move the midpoint along them).
    if seg_dist(fm, fp, fq) <= tol {
        return;
    }
    subdivide(p, m, fp, fm, f, tol, depth + 1, out);
    out.push(fm);
    subdivide(m, q, fm, fq, f, tol, depth + 1, out);
}

fn seg_dist(p: Point, a: Point, b: Point) -> f64 {
    let ab = b - a;
    let l2 = ab.hypot2();
    if l2 < 1e-24 {
        return p.distance(a);
    }
    let t = ((p - a).dot(ab) / l2).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

/// Corners of a rectangle in warp order: bottom-left, bottom-right, top-right, top-left (Y up).
pub fn rect_corners(r: Rect) -> [Point; 4] {
    [Point::new(r.x0, r.y0), Point::new(r.x1, r.y0), Point::new(r.x1, r.y1), Point::new(r.x0, r.y1)]
}

/// True when the four points form a convex quad of non-zero area (either winding).
pub fn is_convex_quad(q: &[Point; 4]) -> bool {
    let mut sign = 0.0;
    for i in 0..4 {
        let a = q[i];
        let b = q[(i + 1) % 4];
        let c = q[(i + 2) % 4];
        let cr = (b - a).cross(c - b);
        if cr.abs() < 1e-12 {
            return false;
        }
        if sign == 0.0 {
            sign = cr.signum();
        } else if cr.signum() != sign {
            return false;
        }
    }
    true
}

/// A projective (perspective) map of the plane, stored as a 3×3 matrix.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Homography {
    /// Row-major matrix: `x' = (m[0][0]x + m[0][1]y + m[0][2]) / w`, `w = m[2][0]x + m[2][1]y + m[2][2]`.
    pub m: [[f64; 3]; 3],
}

impl Homography {
    /// Solves the homography mapping each `src[i]` onto `dst[i]`. `None` when either quad
    /// is not convex or has zero area.
    pub fn from_quads(src: [Point; 4], dst: [Point; 4]) -> Option<Homography> {
        if !is_convex_quad(&src) || !is_convex_quad(&dst) {
            return None;
        }
        let mut a = [[0.0f64; 9]; 8];
        for i in 0..4 {
            let (x, y) = (src[i].x, src[i].y);
            let (u, v) = (dst[i].x, dst[i].y);
            a[2 * i] = [x, y, 1.0, 0.0, 0.0, 0.0, -x * u, -y * u, u];
            a[2 * i + 1] = [0.0, 0.0, 0.0, x, y, 1.0, -x * v, -y * v, v];
        }
        let h = solve8(&mut a)?;
        let hm = Homography { m: [[h[0], h[1], h[2]], [h[3], h[4], h[5]], [h[6], h[7], 1.0]] };
        if hm.m.iter().flatten().all(|v| v.is_finite()) { Some(hm) } else { None }
    }

    /// Maps a point. Points on the vanishing line map to infinity.
    pub fn apply(&self, p: Point) -> Point {
        let m = &self.m;
        let w = m[2][0] * p.x + m[2][1] * p.y + m[2][2];
        Point::new((m[0][0] * p.x + m[0][1] * p.y + m[0][2]) / w, (m[1][0] * p.x + m[1][1] * p.y + m[1][2]) / w)
    }

    /// The inverse map, or `None` when singular.
    pub fn inverse(&self) -> Option<Homography> {
        let m = &self.m;
        let c00 = m[1][1] * m[2][2] - m[1][2] * m[2][1];
        let c01 = m[1][2] * m[2][0] - m[1][0] * m[2][2];
        let c02 = m[1][0] * m[2][1] - m[1][1] * m[2][0];
        let det = m[0][0] * c00 + m[0][1] * c01 + m[0][2] * c02;
        if det.abs() < 1e-15 {
            return None;
        }
        let inv = [
            [c00 / det, (m[0][2] * m[2][1] - m[0][1] * m[2][2]) / det, (m[0][1] * m[1][2] - m[0][2] * m[1][1]) / det],
            [c01 / det, (m[0][0] * m[2][2] - m[0][2] * m[2][0]) / det, (m[0][2] * m[1][0] - m[0][0] * m[1][2]) / det],
            [c02 / det, (m[0][1] * m[2][0] - m[0][0] * m[2][1]) / det, (m[0][0] * m[1][1] - m[0][1] * m[1][0]) / det],
        ];
        Some(Homography { m: inv })
    }
}

/// Gaussian elimination with partial pivoting on an 8×9 augmented matrix.
fn solve8(a: &mut [[f64; 9]; 8]) -> Option<[f64; 8]> {
    for col in 0..8 {
        let piv = (col..8).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if a[piv][col].abs() < 1e-12 {
            return None;
        }
        a.swap(col, piv);
        for r in 0..8 {
            if r != col {
                let f = a[r][col] / a[col][col];
                if f != 0.0 {
                    let pivot_row = a[col];
                    for (dst, src) in a[r][col..].iter_mut().zip(&pivot_row[col..]) {
                        *dst -= f * src;
                    }
                }
            }
        }
    }
    Some(std::array::from_fn(|i| a[i][8] / a[i][i]))
}

fn unit_coords(src: Rect, p: Point) -> (f64, f64) {
    ((p.x - src.x0) / src.width(), (p.y - src.y0) / src.height())
}

fn rect_ok(src: Rect) -> bool {
    src.width().abs() > 1e-12 && src.height().abs() > 1e-12 && src.is_finite()
}

/// Bilinear 4-point warp: the rectangle `src` is mapped onto the quad `dst`
/// (bottom-left, bottom-right, top-right, top-left). Straight lines become curves, so
/// the result is flattened within `tol`. `None` for an empty `src`.
pub fn warp_bilinear(path: &BezPath, src: Rect, dst: [Point; 4], tol: f64) -> Option<BezPath> {
    if !rect_ok(src) {
        return None;
    }
    let [bl, br, tr, tl] = dst.map(|p| p.to_vec2());
    Some(map_path(path, tol, |p| {
        let (u, v) = unit_coords(src, p);
        ((1.0 - u) * (1.0 - v) * bl + u * (1.0 - v) * br + u * v * tr + (1.0 - u) * v * tl).to_point()
    }))
}

/// Perspective 4-point warp: the corners of `src` go exactly onto `dst` (bottom-left,
/// bottom-right, top-right, top-left) and straight lines stay straight. `None` when `dst`
/// is not a convex quad or `src` is empty.
pub fn warp_perspective(path: &BezPath, src: Rect, dst: [Point; 4], tol: f64) -> Option<BezPath> {
    if !rect_ok(src) {
        return None;
    }
    let h = Homography::from_quads(rect_corners(src), dst)?;
    Some(map_path(path, tol, |p| h.apply(p)))
}

fn bernstein3(t: f64) -> [f64; 4] {
    let s = 1.0 - t;
    [s * s * s, 3.0 * s * s * t, 3.0 * s * t * t, t * t * t]
}

/// Evaluates the bicubic tensor-product Bézier surface of a 4×4 grid at `(u, v)` in `0..=1`.
/// Row 0 is the bottom row; column 0 is the left column.
pub fn bicubic_eval(grid: &[[Point; 4]; 4], u: f64, v: f64) -> Point {
    let bu = bernstein3(u);
    let bv = bernstein3(v);
    let mut acc = Vec2::ZERO;
    for (i, row) in grid.iter().enumerate() {
        for (j, p) in row.iter().enumerate() {
            acc += bv[i] * bu[j] * p.to_vec2();
        }
    }
    acc.to_point()
}

/// The evenly spaced 4×4 grid over `src`; deforming with it leaves paths unchanged.
pub fn identity_grid(src: Rect) -> [[Point; 4]; 4] {
    let mut g = [[Point::ZERO; 4]; 4];
    for (i, row) in g.iter_mut().enumerate() {
        for (j, p) in row.iter_mut().enumerate() {
            *p = Point::new(src.x0 + src.width() * j as f64 / 3.0, src.y0 + src.height() * i as f64 / 3.0);
        }
    }
    g
}

/// 16-point deform: maps `src` through a bicubic Bézier surface whose 4×4 control points
/// are `grid` (row 0 = bottom). Flattened within `tol`. `None` for an empty `src`.
pub fn deform_grid(path: &BezPath, src: Rect, grid: [[Point; 4]; 4], tol: f64) -> Option<BezPath> {
    if !rect_ok(src) {
        return None;
    }
    Some(map_path(path, tol, |p| {
        let (u, v) = unit_coords(src, p);
        bicubic_eval(&grid, u, v)
    }))
}

/// Reflection across the infinite line through `a` and `b`. Identity when `a == b`.
pub fn mirror_across_line(a: Point, b: Point) -> Affine {
    let d = b - a;
    if d.hypot2() < 1e-24 {
        return Affine::IDENTITY;
    }
    let d = d.normalize();
    let (c2, s2) = (d.x * d.x - d.y * d.y, 2.0 * d.x * d.y);
    Affine::translate(a.to_vec2()) * Affine::new([c2, s2, s2, -c2, 0.0, 0.0]) * Affine::translate(-a.to_vec2())
}

/// Mirrors a path across the line through `a` and `b`.
pub fn mirror_path(path: &BezPath, a: Point, b: Point) -> BezPath {
    mirror_across_line(a, b) * path.clone()
}

/// Two-point rotate and scale: maps `from_a` onto `to_a` and `from_b` onto `to_b`.
/// With `uniform` the shape scales equally in both directions; otherwise it stretches
/// only along the `from_a`→`from_b` direction. `None` when `from_a == from_b`.
pub fn two_point_rotate_scale(from_a: Point, from_b: Point, to_a: Point, to_b: Point, uniform: bool) -> Option<Affine> {
    let v1 = from_b - from_a;
    let v2 = to_b - to_a;
    let l1 = v1.hypot();
    if l1 < 1e-12 {
        return None;
    }
    let s = v2.hypot() / l1;
    let angle = v2.atan2() - v1.atan2();
    let scale = if uniform {
        Affine::scale(s)
    } else {
        let u = v1 / l1;
        let k = s - 1.0;
        Affine::new([1.0 + k * u.x * u.x, k * u.x * u.y, k * u.x * u.y, 1.0 + k * u.y * u.y, 0.0, 0.0])
    };
    Some(Affine::translate(to_a.to_vec2()) * Affine::rotate(angle) * scale * Affine::translate(-from_a.to_vec2()))
}

/// Rotation about `pivot` that turns the direction to `from` into the direction to `to`.
pub fn two_point_rotate(pivot: Point, from: Point, to: Point) -> Affine {
    let a = (to - pivot).atan2() - (from - pivot).atan2();
    rotate_about(pivot, a)
}

/// Rotation by `angle` radians (counter-clockwise, Y up) about `center`.
pub fn rotate_about(center: Point, angle: f64) -> Affine {
    Affine::rotate_about(angle, center)
}

/// Scale by `sx`, `sy` about `center`.
pub fn scale_about(center: Point, sx: f64, sy: f64) -> Affine {
    Affine::translate(center.to_vec2()) * Affine::scale_non_uniform(sx, sy) * Affine::translate(-center.to_vec2())
}

/// Horizontal flip (mirror left-right) about the centre of `bbox`.
pub fn flip_h(bbox: Rect) -> Affine {
    scale_about(bbox.center(), -1.0, 1.0)
}

/// Vertical flip (mirror top-bottom) about the centre of `bbox`.
pub fn flip_v(bbox: Rect) -> Affine {
    scale_about(bbox.center(), 1.0, -1.0)
}

/// Scales about the centre of `bbox` so it gets the target width and/or height.
/// With `keep_aspect` and only one target given, the other side scales along.
/// Zero-size sides are left alone.
pub fn make_same_size(bbox: Rect, width: Option<f64>, height: Option<f64>, keep_aspect: bool) -> Affine {
    let fx = width.filter(|_| bbox.width() > 1e-12).map(|w| w / bbox.width());
    let fy = height.filter(|_| bbox.height() > 1e-12).map(|h| h / bbox.height());
    let (sx, sy) = match (fx, fy) {
        (Some(x), Some(y)) => (x, y),
        (Some(x), None) => (x, if keep_aspect { x } else { 1.0 }),
        (None, Some(y)) => (if keep_aspect { y } else { 1.0 }, y),
        (None, None) => (1.0, 1.0),
    };
    scale_about(bbox.center(), sx, sy)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path::{bounding_box, contours, signed_area};
    use kurbo::{Circle, Shape};

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> BezPath {
        Rect::new(x0, y0, x1, y1).to_path(0.1)
    }
    fn close(a: Point, b: Point, e: f64) -> bool {
        a.distance(b) < e
    }

    #[test]
    fn identity_warps_keep_rect() {
        let src = Rect::new(0.0, 0.0, 10.0, 5.0);
        let p = rect(0.0, 0.0, 10.0, 5.0);
        for w in [
            warp_bilinear(&p, src, rect_corners(src), 0.01).unwrap(),
            warp_perspective(&p, src, rect_corners(src), 0.01).unwrap(),
            deform_grid(&p, src, identity_grid(src), 0.01).unwrap(),
        ] {
            assert!((signed_area(&w) - 50.0).abs() < 1e-6);
            let bb = bounding_box(&w).unwrap();
            assert!((bb.width() - 10.0).abs() < 1e-9 && (bb.height() - 5.0).abs() < 1e-9);
            assert!(contours(&w)[0].closed);
        }
    }

    #[test]
    fn perspective_maps_corners_exactly_and_inverts() {
        let src = Rect::new(0.0, 0.0, 10.0, 10.0);
        let dst = [Point::new(0.0, 0.0), Point::new(20.0, 2.0), Point::new(15.0, 12.0), Point::new(3.0, 9.0)];
        let h = Homography::from_quads(rect_corners(src), dst).unwrap();
        for (s, d) in rect_corners(src).iter().zip(dst.iter()) {
            assert!(close(h.apply(*s), *d, 1e-9));
        }
        let inv = h.inverse().unwrap();
        for p in [Point::new(3.0, 4.0), Point::new(7.5, 1.25)] {
            assert!(close(inv.apply(h.apply(p)), p, 1e-9));
        }
        // Lines stay straight: a square's warped path has exactly 4 corners.
        let w = warp_perspective(&rect(0.0, 0.0, 10.0, 10.0), src, dst, 0.001).unwrap();
        assert_eq!(contours(&w)[0].segs.len(), 4);
    }

    #[test]
    fn degenerate_quads_rejected() {
        let src = Rect::new(0.0, 0.0, 10.0, 10.0);
        let collinear = [Point::new(0.0, 0.0), Point::new(5.0, 0.0), Point::new(10.0, 0.0), Point::new(0.0, 10.0)];
        let bowtie = [Point::new(0.0, 0.0), Point::new(10.0, 10.0), Point::new(10.0, 0.0), Point::new(0.0, 10.0)];
        let p = rect(0.0, 0.0, 10.0, 10.0);
        assert!(warp_perspective(&p, src, collinear, 0.01).is_none());
        assert!(warp_perspective(&p, src, bowtie, 0.01).is_none());
        assert!(warp_bilinear(&p, Rect::new(0.0, 0.0, 0.0, 10.0), rect_corners(src), 0.01).is_none());
        assert!(deform_grid(&p, Rect::new(1.0, 1.0, 1.0, 1.0), identity_grid(src), 0.01).is_none());
        assert!(warp_perspective(&BezPath::new(), src, rect_corners(src), 0.01).unwrap().is_empty());
    }

    #[test]
    fn bilinear_curves_lines_within_tolerance() {
        let src = Rect::new(0.0, 0.0, 10.0, 10.0);
        let dst = [Point::new(0.0, 0.0), Point::new(10.0, 0.0), Point::new(14.0, 10.0), Point::new(0.0, 6.0)];
        let line = {
            let mut b = BezPath::new();
            b.move_to((0.0, 5.0));
            b.line_to((10.0, 5.0));
            b
        };
        let w = warp_bilinear(&line, src, dst, 0.001).unwrap();
        let c = &contours(&w)[0];
        assert!(!c.closed);
        assert!(close(c.start().unwrap(), Point::new(0.0, 3.0), 1e-9));
        assert!(close(c.end().unwrap(), Point::new(12.0, 5.0), 1e-9));
        // Midpoint of the mapped line: u=0.5, v=0.5.
        let mid = Point::new(0.25 * (0.0 + 10.0 + 14.0 + 0.0), 0.25 * (0.0 + 0.0 + 10.0 + 6.0));
        assert!(crate::query::distance_to_path(&w, mid) < 0.002);
    }

    #[test]
    fn deform_identity_keeps_circle_and_moves_with_grid() {
        let src = Rect::new(-10.0, -10.0, 10.0, 10.0);
        let c = Circle::new((0.0, 0.0), 10.0).to_path(1e-6);
        let d = deform_grid(&c, src, identity_grid(src), 0.001).unwrap();
        for p in &crate::path::flatten_to_polylines(&d, 0.001)[0].points {
            assert!((p.to_vec2().hypot() - 10.0).abs() < 0.002);
        }
        let mut g = identity_grid(src);
        for row in g.iter_mut() {
            for p in row.iter_mut() {
                *p += Vec2::new(5.0, 0.0);
            }
        }
        let moved = deform_grid(&c, src, g, 0.001).unwrap();
        let bb = bounding_box(&moved).unwrap();
        assert!((bb.x0 + 5.0).abs() < 0.01 && (bb.x1 - 15.0).abs() < 0.01);
    }

    #[test]
    fn mirror_twice_is_identity_and_reflects() {
        let m = mirror_across_line(Point::new(1.0, 1.0), Point::new(4.0, 3.0));
        let mm = m * m;
        for c in (mm.as_coeffs().iter()).zip(Affine::IDENTITY.as_coeffs().iter()) {
            assert!((c.0 - c.1).abs() < 1e-12);
        }
        assert!(m.determinant() < 0.0);
        let d = mirror_across_line(Point::ZERO, Point::new(1.0, 1.0));
        assert!(close(d * Point::new(2.0, 0.0), Point::new(0.0, 2.0), 1e-12));
        assert_eq!(mirror_across_line(Point::ZERO, Point::ZERO), Affine::IDENTITY);
        let p = mirror_path(&rect(0.0, 0.0, 2.0, 1.0), Point::new(0.0, 0.0), Point::new(0.0, 1.0));
        assert!(close(bounding_box(&p).unwrap().origin(), Point::new(-2.0, 0.0), 1e-12));
        assert!(signed_area(&p) < 0.0);
    }

    #[test]
    fn two_point_maps_both_points() {
        let (a, b) = (Point::new(1.0, 1.0), Point::new(3.0, 1.0));
        let (a2, b2) = (Point::new(5.0, 5.0), Point::new(5.0, 9.0));
        for uniform in [true, false] {
            let t = two_point_rotate_scale(a, b, a2, b2, uniform).unwrap();
            assert!(close(t * a, a2, 1e-9));
            assert!(close(t * b, b2, 1e-9));
        }
        // Non-uniform keeps the perpendicular size.
        let t = two_point_rotate_scale(a, b, a, Point::new(5.0, 1.0), false).unwrap();
        assert!(close(t * Point::new(1.0, 2.0), Point::new(1.0, 2.0), 1e-9));
        let tu = two_point_rotate_scale(a, b, a, Point::new(5.0, 1.0), true).unwrap();
        assert!(close(tu * Point::new(1.0, 2.0), Point::new(1.0, 3.0), 1e-9));
        assert!(two_point_rotate_scale(a, a, a2, b2, true).is_none());
        let r = two_point_rotate(Point::ZERO, Point::new(1.0, 0.0), Point::new(0.0, 3.0));
        assert!(close(r * Point::new(2.0, 0.0), Point::new(0.0, 2.0), 1e-12));
    }

    #[test]
    fn affine_helpers() {
        let bb = Rect::new(0.0, 0.0, 4.0, 2.0);
        assert!(close(flip_h(bb) * Point::new(0.0, 0.0), Point::new(4.0, 0.0), 1e-12));
        assert!(close(flip_v(bb) * Point::new(0.0, 0.0), Point::new(0.0, 2.0), 1e-12));
        assert!(close(scale_about(bb.center(), 2.0, 3.0) * Point::new(2.0, 1.0), Point::new(2.0, 1.0), 1e-12));
        assert!(close(rotate_about(Point::new(1.0, 0.0), std::f64::consts::PI) * Point::ZERO, Point::new(2.0, 0.0), 1e-12));
        let s = make_same_size(bb, Some(8.0), None, true);
        let r = s.transform_rect_bbox(bb);
        assert!((r.width() - 8.0).abs() < 1e-12 && (r.height() - 4.0).abs() < 1e-12);
        assert!(close(r.center(), bb.center(), 1e-12));
        let r = make_same_size(bb, None, Some(1.0), false).transform_rect_bbox(bb);
        assert!((r.width() - 4.0).abs() < 1e-12 && (r.height() - 1.0).abs() < 1e-12);
        let flat = Rect::new(0.0, 0.0, 4.0, 0.0);
        assert_eq!(make_same_size(flat, None, Some(3.0), true), Affine::IDENTITY);
    }
}
