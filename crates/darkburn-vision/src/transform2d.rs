//! Least-squares 2D transform fits: similarity (Umeyama, 1991), full affine.

use crate::linalg::{Matrix, solve};
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
    Some(Similarity { scale, angle, translation: (tx, ty) })
}

/// Full 6-parameter affine fit; needs at least three non-collinear points.
pub fn fit_affine(src: &[Point], dst: &[Point]) -> Option<Affine> {
    if src.len() != dst.len() || src.len() < 3 {
        return None;
    }
    let mut a = Matrix::zeros(src.len(), 3);
    for (i, p) in src.iter().enumerate() {
        a.set(i, 0, p.x);
        a.set(i, 1, p.y);
        a.set(i, 2, 1.0);
    }
    let g = a.gram();
    let bx: Vec<f64> = dst.iter().map(|p| p.x).collect();
    let by: Vec<f64> = dst.iter().map(|p| p.y).collect();
    let row_x = solve(&g, &a.t_mul_vec(&bx))?;
    let row_y = solve(&g, &a.t_mul_vec(&by))?;
    Some(Affine::new([row_x[0], row_y[0], row_x[1], row_y[1], row_x[2], row_y[2]]))
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
    fn degenerate_inputs() {
        let p = [Point::new(1.0, 1.0), Point::new(1.0, 1.0)];
        assert!(fit_similarity(&p, &p, true).is_none());
        assert!(fit_similarity(&[], &[], true).is_none());
    }
}
