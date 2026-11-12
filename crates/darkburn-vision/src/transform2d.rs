//! Least-squares 2D transform fits: similarity (Umeyama, 1991), full affine.

use kurbo::{Affine, Point};

/// Rotation (radians, counter-clockwise), uniform scale and translation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Similarity {
    pub scale: f64,
    pub angle: f64,
    pub translation: (f64, f64),
}

impl Similarity {
    pub fn to_affine(&self) -> Affine {
        let (s, c) = self.angle.sin_cos();
        Affine::new([self.scale * c, self.scale * s, -self.scale * s, self.scale * c, self.translation.0, self.translation.1])
    }
}

fn centroid(p: &[Point]) -> Point {
    let n = p.len() as f64;
    Point::new(p.iter().map(|q| q.x).sum::<f64>() / n, p.iter().map(|q| q.y).sum::<f64>() / n)
}

/// Best rotation + translation (+ uniform scale when `allow_scale`) taking `src` onto `dst` in the least-squares sense.
/// Needs at least two distinct source points, or one point for a pure translation.
pub fn fit_similarity(src: &[Point], dst: &[Point], allow_scale: bool) -> Option<Similarity> {
    if src.len() != dst.len() || src.is_empty() {
        return None;
    }
    let (ma, mb) = (centroid(src), centroid(dst));
    if src.len() == 1 {
        return Some(Similarity { scale: 1.0, angle: 0.0, translation: (mb.x - ma.x, mb.y - ma.y) });
    }
    let (mut sxx, mut sxy, mut denom) = (0.0, 0.0, 0.0);
    for (a, b) in src.iter().zip(dst) {
        let (ax, ay) = (a.x - ma.x, a.y - ma.y);
        let (bx, by) = (b.x - mb.x, b.y - mb.y);
        sxx += ax * bx + ay * by;
        sxy += ax * by - ay * bx;
        denom += ax * ax + ay * ay;
    }
    if denom < 1e-18 {
        return None;
    }
    let angle = sxy.atan2(sxx);
    let scale = if allow_scale { sxx.hypot(sxy) / denom } else { 1.0 };
    if !(scale.is_finite() && scale > 0.0) {
        return None;
    }
    let (s, c) = angle.sin_cos();
    let tx = mb.x - scale * (c * ma.x - s * ma.y);
    let ty = mb.y - scale * (s * ma.x + c * ma.y);
    if !(angle.is_finite() && tx.is_finite() && ty.is_finite()) {
        return None;
    }
    Some(Similarity { scale, angle, translation: (tx, ty) })
}

/// Full 6-parameter affine fit; needs at least three non-collinear points.
pub fn fit_affine(src: &[Point], dst: &[Point]) -> Option<Affine> {
    if src.len() != dst.len() || src.len() < 3 {
        return None;
    }
    // Least squares about the centroids: the linear part from the spread of the points, then the shift.
    let (ma, mb) = (centroid(src), centroid(dst));
    let (mut sxx, mut sxy, mut syy) = (0.0, 0.0, 0.0);
    let (mut bxx, mut bxy, mut byx, mut byy) = (0.0, 0.0, 0.0, 0.0);
    for (p, q) in src.iter().zip(dst) {
        let (x, y) = (p.x - ma.x, p.y - ma.y);
        let (u, v) = (q.x - mb.x, q.y - mb.y);
        sxx += x * x;
        sxy += x * y;
        syy += y * y;
        bxx += u * x;
        bxy += u * y;
        byx += v * x;
        byy += v * y;
    }
    // Points on one line (or all the same) have no area: the spread matrix cannot be inverted.
    let det = sxx * syy - sxy * sxy;
    let spread = sxx + syy;
    if !(det > 1e-12 * spread * spread) {
        return None;
    }
    let (a, c) = ((bxx * syy - bxy * sxy) / det, (bxy * sxx - bxx * sxy) / det);
    let (b, d) = ((byx * syy - byy * sxy) / det, (byy * sxx - byx * sxy) / det);
    let (tx, ty) = (mb.x - a * ma.x - c * ma.y, mb.y - b * ma.x - d * ma.y);
    let out = [a, b, c, d, tx, ty];
    out.iter().all(|v| v.is_finite()).then(|| Affine::new(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pts() -> Vec<Point> {
        vec![Point::new(0.0, 0.0), Point::new(100.0, 0.0), Point::new(100.0, 60.0), Point::new(0.0, 60.0)]
    }

    #[test]
    fn similarity_recovers_exact() {
        let truth = Similarity { scale: 1.02, angle: 0.1, translation: (5.0, -3.0) }.to_affine();
        let src = pts();
        let dst: Vec<Point> = src.iter().map(|p| truth * *p).collect();
        let f = fit_similarity(&src, &dst, true).unwrap();
        assert!((f.scale - 1.02).abs() < 1e-9 && (f.angle - 0.1).abs() < 1e-9);
        assert!((f.translation.0 - 5.0).abs() < 1e-9 && (f.translation.1 + 3.0).abs() < 1e-9);
        // without scale the scale stays 1
        assert_eq!(fit_similarity(&src, &dst, false).unwrap().scale, 1.0);
    }

    #[test]
    fn two_points_are_enough() {
        let src = [Point::new(0.0, 0.0), Point::new(10.0, 0.0)];
        let dst = [Point::new(1.0, 1.0), Point::new(1.0, 21.0)];
        let f = fit_similarity(&src, &dst, true).unwrap();
        assert!((f.scale - 2.0).abs() < 1e-12);
        assert!((f.angle - std::f64::consts::FRAC_PI_2).abs() < 1e-12);
        assert!((f.to_affine() * src[1] - dst[1]).hypot() < 1e-9);
    }

    #[test]
    fn affine_recovers_shear() {
        let truth = Affine::new([1.1, 0.05, 0.2, 0.9, 7.0, 8.0]);
        let src = pts();
        let dst: Vec<Point> = src.iter().map(|p| truth * *p).collect();
        let f = fit_affine(&src, &dst).unwrap();
        for (a, b) in f.as_coeffs().iter().zip(truth.as_coeffs()) {
            assert!((a - b).abs() < 1e-9);
        }
        assert!(fit_affine(&src[..2], &dst[..2]).is_none());
        let line = [Point::new(0.0, 0.0), Point::new(1.0, 1.0), Point::new(2.0, 2.0)];
        assert!(fit_affine(&line, &line).is_none());
    }

    #[test]
    fn points_on_a_line_give_no_affine_whatever_their_values() {
        // not whole numbers, so the sums are not exactly singular
        let line = [Point::new(0.1, 0.3), Point::new(0.2, 0.6), Point::new(0.7, 2.1), Point::new(1.3, 3.9)];
        let dst = [Point::new(0.0, 0.0), Point::new(10.0, 0.0), Point::new(10.0, 10.0), Point::new(0.0, 10.0)];
        assert!(fit_affine(&line, &dst).is_none());
        let far = [Point::new(1000.1, 2000.3), Point::new(1100.2, 2100.4), Point::new(1250.35, 2250.55)];
        assert!(fit_affine(&far, &dst[..3]).is_none());
        // repeated points: only two different ones
        let two = [Point::new(3.3, 1.1), Point::new(3.3, 1.1), Point::new(7.7, 4.9)];
        assert!(fit_affine(&two, &dst[..3]).is_none());
    }

    #[test]
    fn affine_far_from_the_origin_is_still_exact() {
        let truth = Affine::new([0.25, 0.01, -0.02, -0.25, 12.0, 300.0]);
        let src = [Point::new(5000.0, 4000.0), Point::new(5100.0, 4003.0), Point::new(5098.0, 4060.0)];
        let dst: Vec<Point> = src.iter().map(|p| truth * *p).collect();
        let f = fit_affine(&src, &dst).unwrap();
        for (p, d) in src.iter().zip(&dst) {
            assert!((f * *p - *d).hypot() < 1e-6, "{:?}", f * *p - *d);
        }
    }

    #[test]
    fn values_that_are_not_numbers_give_no_fit() {
        let src = [Point::new(0.0, 0.0), Point::new(f64::NAN, 0.0), Point::new(5.0, 5.0)];
        let dst = [Point::new(0.0, 0.0), Point::new(10.0, 0.0), Point::new(5.0, 5.0)];
        for scale in [true, false] {
            assert!(fit_similarity(&src, &dst, scale).is_none());
            assert!(fit_similarity(&dst, &src, scale).is_none());
        }
        assert!(fit_affine(&src, &dst).is_none());
        assert!(fit_affine(&dst, &src).is_none());
    }

    #[test]
    fn degenerate_inputs() {
        let p = [Point::new(1.0, 1.0), Point::new(1.0, 1.0)];
        assert!(fit_similarity(&p, &p, true).is_none());
        assert!(fit_similarity(&[], &[], true).is_none());
    }
}
