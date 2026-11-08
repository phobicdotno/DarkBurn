//! Planar homographies: normalised DLT (Hartley and Zisserman, public method) plus LM refinement.

use crate::linalg::{LmOptions, Matrix, levenberg_marquardt, null_vector};
use kurbo::{Affine, Point};
use serde::{Deserialize, Serialize};

/// A 3x3 projective transform, row-major, mapping `src → dst`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Homography {
    pub m: [f64; 9],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HomographyError {
    NotEnoughPoints,
    LengthMismatch,
    Degenerate,
}

impl std::fmt::Display for HomographyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotEnoughPoints => write!(f, "at least four point pairs are needed"),
            Self::LengthMismatch => write!(f, "source and destination lists differ in length"),
            Self::Degenerate => write!(f, "the points are degenerate (collinear or coincident)"),
        }
    }
}
impl std::error::Error for HomographyError {}

impl Homography {
    pub const IDENTITY: Homography = Homography { m: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0] };

    pub fn from_affine(a: Affine) -> Self {
        let c = a.as_coeffs();
        Self { m: [c[0], c[2], c[4], c[1], c[3], c[5], 0.0, 0.0, 1.0] }
    }

    /// Apply to a point. `None` when the point maps to infinity.
    pub fn apply(&self, p: Point) -> Option<Point> {
        let m = &self.m;
        let w = m[6] * p.x + m[7] * p.y + m[8];
        if w.abs() < 1e-12 {
            return None;
        }
        Some(Point::new((m[0] * p.x + m[1] * p.y + m[2]) / w, (m[3] * p.x + m[4] * p.y + m[5]) / w))
    }

    pub fn inverse(&self) -> Option<Homography> {
        let m = &self.m;
        let det = m[0] * (m[4] * m[8] - m[5] * m[7]) - m[1] * (m[3] * m[8] - m[5] * m[6]) + m[2] * (m[3] * m[7] - m[4] * m[6]);
        if det.abs() < 1e-300 {
            return None;
        }
        let i = 1.0 / det;
        let inv = [
            (m[4] * m[8] - m[5] * m[7]) * i,
            (m[2] * m[7] - m[1] * m[8]) * i,
            (m[1] * m[5] - m[2] * m[4]) * i,
            (m[5] * m[6] - m[3] * m[8]) * i,
            (m[0] * m[8] - m[2] * m[6]) * i,
            (m[2] * m[3] - m[0] * m[5]) * i,
            (m[3] * m[7] - m[4] * m[6]) * i,
            (m[1] * m[6] - m[0] * m[7]) * i,
            (m[0] * m[4] - m[1] * m[3]) * i,
        ];
        Some(Homography { m: inv }.normalised())
    }

    /// `self ∘ other` (apply `other` first).
    pub fn then_after(&self, other: &Homography) -> Homography {
        let (a, b) = (&self.m, &other.m);
        let mut o = [0.0; 9];
        for r in 0..3 {
            for c in 0..3 {
                o[r * 3 + c] = (0..3).map(|k| a[r * 3 + k] * b[k * 3 + c]).sum();
            }
        }
        Homography { m: o }.normalised()
    }

    fn normalised(mut self) -> Self {
        if self.m[8].abs() > 1e-12 {
            let s = 1.0 / self.m[8];
            for v in &mut self.m {
                *v *= s;
            }
        }
        self
    }

    /// Root mean square distance between `apply(src)` and `dst`.
    pub fn rms_error(&self, src: &[Point], dst: &[Point]) -> f64 {
        let mut s = 0.0;
        let mut n = 0.0;
        for (a, b) in src.iter().zip(dst) {
            if let Some(p) = self.apply(*a) {
                s += (p - *b).hypot2();
                n += 1.0;
            }
        }
        if n == 0.0 { f64::INFINITY } else { (s / n).sqrt() }
    }
}

fn normalisation(pts: &[Point]) -> ([f64; 9], Vec<Point>) {
    let n = pts.len() as f64;
    let cx = pts.iter().map(|p| p.x).sum::<f64>() / n;
    let cy = pts.iter().map(|p| p.y).sum::<f64>() / n;
    let md = pts.iter().map(|p| ((p.x - cx).powi(2) + (p.y - cy).powi(2)).sqrt()).sum::<f64>() / n;
    let s = if md > 1e-12 { std::f64::consts::SQRT_2 / md } else { 1.0 };
    let t = [s, 0.0, -s * cx, 0.0, s, -s * cy, 0.0, 0.0, 1.0];
    (t, pts.iter().map(|p| Point::new(s * (p.x - cx), s * (p.y - cy))).collect())
}

/// Normalised direct linear transform, no refinement.
pub fn fit_homography_dlt(src: &[Point], dst: &[Point]) -> Result<Homography, HomographyError> {
    if src.len() != dst.len() {
        return Err(HomographyError::LengthMismatch);
    }
    if src.len() < 4 {
        return Err(HomographyError::NotEnoughPoints);
    }
    let (ts, ns) = normalisation(src);
    let (td, nd) = normalisation(dst);
    let mut a = Matrix::zeros(2 * src.len(), 9);
    for (i, (s, d)) in ns.iter().zip(&nd).enumerate() {
        let r = 2 * i;
        let row1 = [-s.x, -s.y, -1.0, 0.0, 0.0, 0.0, d.x * s.x, d.x * s.y, d.x];
        let row2 = [0.0, 0.0, 0.0, -s.x, -s.y, -1.0, d.y * s.x, d.y * s.y, d.y];
        for c in 0..9 {
            a.set(r, c, row1[c]);
            a.set(r + 1, c, row2[c]);
        }
    }
    let h = null_vector(&a);
    let hn = Homography { m: [h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7], h[8]] };
    let td_inv = Homography { m: td }.inverse().ok_or(HomographyError::Degenerate)?;
    let out = td_inv.then_after(&hn).then_after(&Homography { m: ts });
    if out.m.iter().any(|v| !v.is_finite()) {
        return Err(HomographyError::Degenerate);
    }
    Ok(out)
}

/// DLT followed by Levenberg-Marquardt minimising the transfer error in the destination plane.
pub fn fit_homography(src: &[Point], dst: &[Point]) -> Result<Homography, HomographyError> {
    let h0 = fit_homography_dlt(src, dst)?;
    if h0.m[8].abs() < 1e-12 {
        return Ok(h0);
    }
    let init: Vec<f64> = h0.m[..8].to_vec();
    let res = levenberg_marquardt(
        &init,
        |p| {
            let h = Homography { m: [p[0], p[1], p[2], p[3], p[4], p[5], p[6], p[7], 1.0] };
            let mut r = Vec::with_capacity(src.len() * 2);
            for (s, d) in src.iter().zip(dst) {
                match h.apply(*s) {
                    Some(q) => {
                        r.push(q.x - d.x);
                        r.push(q.y - d.y);
                    }
                    None => {
                        r.push(1e9);
                        r.push(1e9);
                    }
                }
            }
            r
        },
        LmOptions { max_iterations: 50, tolerance: 1e-14 },
    );
    let p = res.params;
    let refined = Homography { m: [p[0], p[1], p[2], p[3], p[4], p[5], p[6], p[7], 1.0] };
    if refined.rms_error(src, dst) <= h0.rms_error(src, dst) { Ok(refined) } else { Ok(h0) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn truth() -> Homography {
        Homography { m: [1.2, 0.1, 30.0, -0.05, 0.9, 12.0, 0.0004, 0.0002, 1.0] }
    }

    fn grid() -> Vec<Point> {
        let mut v = Vec::new();
        for i in 0..4 {
            for j in 0..3 {
                v.push(Point::new(i as f64 * 100.0, j as f64 * 80.0));
            }
        }
        v
    }

    #[test]
    fn recovers_exact_homography() {
        let src = grid();
        let dst: Vec<Point> = src.iter().map(|p| truth().apply(*p).unwrap()).collect();
        let h = fit_homography(&src, &dst).unwrap();
        assert!(h.rms_error(&src, &dst) < 1e-8);
        let probe = Point::new(55.0, 33.0);
        let a = h.apply(probe).unwrap();
        let b = truth().apply(probe).unwrap();
        assert!((a - b).hypot() < 1e-6);
    }

    #[test]
    fn refinement_handles_noise() {
        let src = grid();
        let mut dst: Vec<Point> = src.iter().map(|p| truth().apply(*p).unwrap()).collect();
        for (i, d) in dst.iter_mut().enumerate() {
            d.x += ((i * 7 % 5) as f64 - 2.0) * 0.05;
            d.y += ((i * 3 % 5) as f64 - 2.0) * 0.05;
        }
        let h = fit_homography(&src, &dst).unwrap();
        assert!(h.rms_error(&src, &dst) < 0.15);
    }

    #[test]
    fn inverse_roundtrip() {
        let h = truth();
        let inv = h.inverse().unwrap();
        let p = Point::new(10.0, 20.0);
        let q = inv.apply(h.apply(p).unwrap()).unwrap();
        assert!((p - q).hypot() < 1e-9);
    }

    #[test]
    fn errors() {
        assert_eq!(fit_homography(&grid()[..3], &grid()[..3]).unwrap_err(), HomographyError::NotEnoughPoints);
        assert_eq!(fit_homography(&grid(), &grid()[..5]).unwrap_err(), HomographyError::LengthMismatch);
    }

    #[test]
    fn affine_conversion() {
        let a = Affine::translate((5.0, 6.0)) * Affine::scale(2.0);
        let h = Homography::from_affine(a);
        let p = Point::new(3.0, 4.0);
        assert!((h.apply(p).unwrap() - a * p).hypot() < 1e-12);
    }
}
