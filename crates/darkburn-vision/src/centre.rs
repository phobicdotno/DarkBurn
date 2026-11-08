//! Centre finder (OTHER-03, LT-13): centre and diameter of round stock from edge points measured on the laser.

use crate::linalg::{LmOptions, Matrix, levenberg_marquardt, solve};
use kurbo::Point;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CircleFit {
    pub centre: Point,
    pub radius: f64,
    pub diameter: f64,
    /// Root mean square distance of the points from the circle, mm.
    pub rms_mm: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CircleError {
    NotEnoughPoints,
    /// The points lie on a line.
    Collinear,
}

impl std::fmt::Display for CircleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NotEnoughPoints => "at least three edge points are needed",
            Self::Collinear => "the edge points lie on a line",
        })
    }
}
impl std::error::Error for CircleError {}

/// Least-squares circle through three or more edge points (algebraic fit, then geometric refinement).
pub fn fit_circle(points: &[Point]) -> Result<CircleFit, CircleError> {
    if points.len() < 3 {
        return Err(CircleError::NotEnoughPoints);
    }
    // Centre the data for conditioning.
    let n = points.len() as f64;
    let (mx, my) = (points.iter().map(|p| p.x).sum::<f64>() / n, points.iter().map(|p| p.y).sum::<f64>() / n);
    // Kåsa fit: x² + y² = 2 a x + 2 b y + c
    let mut a = Matrix::zeros(points.len(), 3);
    let mut rhs = Vec::with_capacity(points.len());
    for (i, p) in points.iter().enumerate() {
        let (x, y) = (p.x - mx, p.y - my);
        a.set(i, 0, 2.0 * x);
        a.set(i, 1, 2.0 * y);
        a.set(i, 2, 1.0);
        rhs.push(x * x + y * y);
    }
    let g = a.gram();
    let sol = solve(&g, &a.t_mul_vec(&rhs)).ok_or(CircleError::Collinear)?;
    let (cx, cy) = (sol[0], sol[1]);
    let r2 = sol[2] + cx * cx + cy * cy;
    if !(r2.is_finite() && r2 > 0.0) {
        return Err(CircleError::Collinear);
    }
    let init = [cx, cy, r2.sqrt()];
    // Geometric refinement.
    let res = levenberg_marquardt(
        &init,
        |p| points.iter().map(|q| ((q.x - mx - p[0]).powi(2) + (q.y - my - p[1]).powi(2)).sqrt() - p[2]).collect(),
        LmOptions { max_iterations: 50, tolerance: 1e-14 },
    );
    let p = res.params;
    let rms = (res.cost / n).sqrt();
    if !(p[2].is_finite() && p[2] > 0.0) {
        return Err(CircleError::Collinear);
    }
    Ok(CircleFit { centre: Point::new(p[0] + mx, p[1] + my), radius: p[2], diameter: 2.0 * p[2], rms_mm: rms })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on_circle(c: Point, r: f64, angles_deg: &[f64]) -> Vec<Point> {
        angles_deg.iter().map(|a| Point::new(c.x + r * a.to_radians().cos(), c.y + r * a.to_radians().sin())).collect()
    }

    #[test]
    fn exact_three_points() {
        let pts = on_circle(Point::new(120.0, 80.0), 25.0, &[10.0, 130.0, 250.0]);
        let f = fit_circle(&pts).unwrap();
        assert!((f.centre - Point::new(120.0, 80.0)).hypot() < 1e-7);
        assert!((f.diameter - 50.0).abs() < 1e-7 && f.rms_mm < 1e-7);
    }

    #[test]
    fn noisy_many_points_and_short_arc() {
        let mut pts = on_circle(Point::new(-30.0, 15.0), 40.0, &[0.0, 40.0, 80.0, 120.0, 160.0, 200.0, 240.0, 280.0, 320.0]);
        for (i, p) in pts.iter_mut().enumerate() {
            p.x += ((i * 5 % 7) as f64 - 3.0) * 0.02;
            p.y += ((i * 3 % 5) as f64 - 2.0) * 0.02;
        }
        let f = fit_circle(&pts).unwrap();
        assert!((f.centre - Point::new(-30.0, 15.0)).hypot() < 0.1);
        assert!((f.radius - 40.0).abs() < 0.1 && f.rms_mm < 0.1);
        // only a quarter of the edge reachable
        let arc = on_circle(Point::new(0.0, 0.0), 30.0, &[0.0, 20.0, 45.0, 70.0, 90.0]);
        let f = fit_circle(&arc).unwrap();
        assert!((f.radius - 30.0).abs() < 1e-5 && f.centre.to_vec2().hypot() < 1e-5);
    }

    #[test]
    fn errors() {
        assert_eq!(fit_circle(&[Point::ZERO, Point::new(1.0, 1.0)]).unwrap_err(), CircleError::NotEnoughPoints);
        let line = [Point::new(0.0, 0.0), Point::new(1.0, 1.0), Point::new(2.0, 2.0), Point::new(3.0, 3.0)];
        assert_eq!(fit_circle(&line).unwrap_err(), CircleError::Collinear);
    }
}
