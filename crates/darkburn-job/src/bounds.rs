//! Work area check: reports moves that leave the machine's work area.

use kurbo::{Point, Rect};
use serde::{Deserialize, Serialize};

use crate::geom;
use crate::ir::{Op, Toolpath};

/// One move that leaves the work area.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct OutOfBounds {
    pub op_index: usize,
    /// A point of the move outside the area.
    pub point: Point,
    /// True if the laser fires on this move.
    pub burning: bool,
}

/// Result of [`check_bounds`].
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BoundsReport {
    /// Bounds of every move, including travel and overscan.
    pub motion_bounds: Option<Rect>,
    pub violations: Vec<OutOfBounds>,
}

impl BoundsReport {
    pub fn is_ok(&self) -> bool {
        self.violations.is_empty()
    }
}

/// Check every move of `tp` against `area`, given in the toolpath's own coordinates (for a
/// relative toolpath, shift the area by the start position first).
pub fn check_bounds(tp: &Toolpath, area: Rect) -> BoundsReport {
    let tol = 1e-6;
    let inside = |p: Point| p.x >= area.x0 - tol && p.x <= area.x1 + tol && p.y >= area.y0 - tol && p.y <= area.y1 + tol;
    let mut report = BoundsReport::default();
    let mut pos = tp.start;
    for (i, op) in tp.ops.iter().enumerate() {
        let pts: Vec<Point> = match op {
            Op::Arc { to, center, clockwise, .. } => {
                // Sample the arc.
                let r = pos.distance(*center);
                let a0 = (pos - *center).atan2();
                let mut sweep = (*to - *center).atan2() - a0;
                if *clockwise && sweep >= 0.0 {
                    sweep -= std::f64::consts::TAU;
                } else if !*clockwise && sweep <= 0.0 {
                    sweep += std::f64::consts::TAU;
                }
                let n = ((r * sweep.abs()) / 0.5).ceil().clamp(4.0, 256.0) as usize;
                (0..=n).map(|k| *center + geom::rotate(pos - *center, sweep * k as f64 / n as f64)).collect()
            }
            op => match op.target() {
                Some(t) => vec![t],
                None => Vec::new(),
            },
        };
        let burning = matches!(op, Op::Cut { power, .. } | Op::Arc { power, .. } | Op::Dwell { power, .. } if *power > 0.0);
        if matches!(op, Op::Dwell { .. }) && !inside(pos) {
            report.violations.push(OutOfBounds { op_index: i, point: pos, burning });
        }
        for p in &pts {
            report.motion_bounds = Some(match report.motion_bounds {
                Some(r) => r.union_pt(*p),
                None => Rect::from_points(pos, *p),
            });
        }
        if let Some(p) = pts.iter().find(|p| !inside(**p)) {
            report.violations.push(OutOfBounds { op_index: i, point: *p, burning });
        }
        if let Some(t) = op.target() {
            pos = t;
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::CoordSpace;

    #[test]
    fn reports_moves_outside() {
        let tp = Toolpath {
            space: CoordSpace::MachineAbsolute,
            layers: Vec::new(),
            ops: vec![
                Op::Rapid { to: Point::new(10.0, 10.0) },
                Op::Cut { to: Point::new(110.0, 10.0), speed: 10.0, power: 50.0 },
                Op::Overscan { to: Point::new(10.0, -1.0), speed: 10.0 },
                Op::Arc { to: Point::new(10.0, 9.0), center: Point::new(10.0, 4.0), clockwise: true, speed: 1.0, power: 1.0 },
            ],
            start: Point::ZERO,
        };
        let r = check_bounds(&tp, Rect::new(0.0, 0.0, 100.0, 100.0));
        assert_eq!(r.violations.iter().map(|v| v.op_index).collect::<Vec<_>>(), vec![1, 2, 3]);
        assert!(r.violations[0].burning);
        assert!(!r.violations[1].burning);
        let ok = check_bounds(&tp, Rect::new(-10.0, -10.0, 200.0, 100.0));
        assert!(ok.is_ok());
        assert_eq!(ok.motion_bounds.unwrap().x1, 110.0);
    }
}
