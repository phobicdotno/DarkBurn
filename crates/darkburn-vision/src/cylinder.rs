//! Cylinder correction (OTHER-05): engrave a flat design onto a cylinder lying under the laser without a rotary,
//! convex (outside of a tube) or concave (inside of a bowl or tube). Pure functions over `kurbo` paths.

use kurbo::{BezPath, PathEl, Point};
use serde::{Deserialize, Serialize};
use std::f64::consts::FRAC_PI_2;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CylinderKind {
    #[default]
    Convex,
    Concave,
}

/// Which bed axis the cylinder's own axis runs along. The other bed axis wraps around the surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CylinderAxis {
    /// Axis along X, the surface wraps in Y.
    #[default]
    AlongX,
    /// Axis along Y, the surface wraps in X.
    AlongY,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CylinderError {
    InvalidRadius,
    /// The design reaches further round the surface than a top-down beam can see (±90 degrees).
    OutOfRange {
        distance: f64,
        limit: f64,
    },
}

impl std::fmt::Display for CylinderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRadius => write!(f, "radius must be above zero"),
            Self::OutOfRange { distance, limit } => write!(f, "design reaches {distance:.2} mm round the surface, the limit is {limit:.2} mm"),
        }
    }
}
impl std::error::Error for CylinderError {}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CylinderCorrection {
    pub radius: f64,
    pub kind: CylinderKind,
    pub axis: CylinderAxis,
    /// Bed coordinate (mm) of the line where the surface is nearest the beam (the top of the cylinder, or the bottom of a concave one).
    pub centre: f64,
}

impl CylinderCorrection {
    /// Furthest arc length from the centre line that can be reached from above.
    pub fn limit(&self) -> f64 {
        FRAC_PI_2 * self.radius
    }

    /// Bed offset from the centre line for an arc length `s` along the surface.
    pub fn arc_to_offset(&self, s: f64) -> f64 {
        self.radius * (s / self.radius).sin()
    }

    /// Arc length for a bed offset (inverse of [`arc_to_offset`](Self::arc_to_offset)).
    pub fn offset_to_arc(&self, o: f64) -> f64 {
        self.radius * (o / self.radius).clamp(-1.0, 1.0).asin()
    }

    /// Height change of the surface relative to the centre line, mm. Convex surfaces fall away, concave ones rise.
    pub fn z_offset(&self, s: f64) -> f64 {
        let d = self.radius * (1.0 - (s / self.radius).cos());
        match self.kind {
            CylinderKind::Convex => -d,
            CylinderKind::Concave => d,
        }
    }

    /// Local compression of the design along the wrapped direction at arc length `s` (1 at the centre line, towards 0 at the edge).
    /// Dividing speed or multiplying line density by this keeps the engraving even.
    pub fn scale_at(&self, s: f64) -> f64 {
        (s / self.radius).cos()
    }

    fn check(&self) -> Result<(), CylinderError> {
        if self.radius > 0.0 && self.radius.is_finite() && self.centre.is_finite() { Ok(()) } else { Err(CylinderError::InvalidRadius) }
    }

    /// Map a flat design point to the bed position that lands there on the surface.
    pub fn map_point(&self, p: Point) -> Result<Point, CylinderError> {
        self.check()?;
        let wrapped = match self.axis {
            CylinderAxis::AlongX => p.y,
            CylinderAxis::AlongY => p.x,
        };
        let s = wrapped - self.centre;
        if s.abs() > self.limit() + 1e-9 {
            return Err(CylinderError::OutOfRange { distance: s.abs(), limit: self.limit() });
        }
        let off = self.centre + self.arc_to_offset(s);
        Ok(match self.axis {
            CylinderAxis::AlongX => Point::new(p.x, off),
            CylinderAxis::AlongY => Point::new(off, p.y),
        })
    }

    /// Map a whole path. Curves are flattened and long segments are split so the bend is followed.
    pub fn map_path(&self, path: &BezPath, max_segment_mm: f64) -> Result<BezPath, CylinderError> {
        self.check()?;
        let step = max_segment_mm.max(0.01);
        let mut out = BezPath::new();
        let mut cur = Point::ZERO;
        let mut start = Point::ZERO;
        let mut err: Option<CylinderError> = None;
        kurbo::flatten(path.elements().iter().copied(), 0.01, |el| {
            let line_to = |from: Point, to: Point, out: &mut BezPath, err: &mut Option<CylinderError>| {
                let n = ((to - from).hypot() / step).ceil().max(1.0) as usize;
                for i in 1..=n {
                    match self.map_point(from.lerp(to, i as f64 / n as f64)) {
                        Ok(m) => out.line_to(m),
                        Err(e) => {
                            err.get_or_insert(e);
                        }
                    }
                }
            };
            match el {
                PathEl::MoveTo(p) => {
                    match self.map_point(p) {
                        Ok(m) => out.move_to(m),
                        Err(e) => {
                            err.get_or_insert(e);
                        }
                    }
                    cur = p;
                    start = p;
                }
                PathEl::LineTo(p) => {
                    line_to(cur, p, &mut out, &mut err);
                    cur = p;
                }
                PathEl::ClosePath => {
                    if (cur - start).hypot() > 1e-12 {
                        line_to(cur, start, &mut out, &mut err);
                    }
                    out.close_path();
                    cur = start;
                }
                _ => {}
            }
        });
        match err {
            Some(e) => Err(e),
            None => Ok(out),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::{Rect, Shape};

    fn cyl(kind: CylinderKind) -> CylinderCorrection {
        CylinderCorrection { radius: 30.0, kind, axis: CylinderAxis::AlongX, centre: 100.0 }
    }

    #[test]
    fn centre_line_is_unchanged_and_edges_compress() {
        let c = cyl(CylinderKind::Convex);
        assert_eq!(c.map_point(Point::new(5.0, 100.0)).unwrap(), Point::new(5.0, 100.0));
        let p = c.map_point(Point::new(5.0, 100.0 + 20.0)).unwrap();
        assert!(p.y < 120.0 && (p.y - (100.0 + 30.0 * (20.0f64 / 30.0).sin())).abs() < 1e-12);
        assert_eq!(p.x, 5.0);
        // arc length ≈ design distance: round-trip through offset_to_arc
        assert!((c.offset_to_arc(p.y - 100.0) - 20.0).abs() < 1e-9);
    }

    #[test]
    fn quarter_turn_limit() {
        let c = cyl(CylinderKind::Convex);
        let edge = c.map_point(Point::new(0.0, 100.0 + c.limit())).unwrap();
        assert!((edge.y - 130.0).abs() < 1e-9);
        let e = c.map_point(Point::new(0.0, 100.0 + c.limit() + 1.0)).unwrap_err();
        assert!(matches!(e, CylinderError::OutOfRange { .. }));
        assert_eq!(CylinderCorrection { radius: 0.0, ..c }.map_point(Point::ZERO).unwrap_err(), CylinderError::InvalidRadius);
    }

    #[test]
    fn concave_and_convex_differ_in_height_only() {
        let (a, b) = (cyl(CylinderKind::Convex), cyl(CylinderKind::Concave));
        let p = Point::new(0.0, 112.0);
        assert_eq!(a.map_point(p).unwrap(), b.map_point(p).unwrap());
        assert!(a.z_offset(12.0) < 0.0 && b.z_offset(12.0) > 0.0);
        assert!((a.z_offset(12.0) + b.z_offset(12.0)).abs() < 1e-12);
        assert!((a.scale_at(0.0) - 1.0).abs() < 1e-12 && a.scale_at(20.0) < 1.0);
    }

    #[test]
    fn axis_along_y_wraps_x() {
        let c = CylinderCorrection { axis: CylinderAxis::AlongY, ..cyl(CylinderKind::Convex) };
        let p = c.map_point(Point::new(115.0, 7.0)).unwrap();
        assert_eq!(p.y, 7.0);
        assert!((p.x - (100.0 + 30.0 * 0.5f64.sin())).abs() < 1e-12);
    }

    #[test]
    fn path_is_subdivided_and_bent() {
        let c = cyl(CylinderKind::Convex);
        let line = BezPath::from_vec(vec![PathEl::MoveTo(Point::new(0.0, 100.0)), PathEl::LineTo(Point::new(0.0, 120.0))]);
        let m = c.map_path(&line, 1.0).unwrap();
        assert!(m.elements().len() >= 20);
        let end = match m.elements().last().unwrap() {
            PathEl::LineTo(p) => *p,
            _ => unreachable!(),
        };
        assert!((end.y - (100.0 + 30.0 * (20.0f64 / 30.0).sin())).abs() < 1e-9);
        // a rectangle straddling the centre stays closed and gets shorter than designed
        let r = Rect::new(0.0, 90.0, 10.0, 110.0).to_path(0.1);
        let mr = c.map_path(&r, 1.0).unwrap();
        assert!(mr.elements().iter().any(|e| matches!(e, PathEl::ClosePath)));
        assert!(mr.bounding_box().height() < 20.0);
        let far = Rect::new(0.0, 90.0, 10.0, 200.0).to_path(0.1);
        assert!(c.map_path(&far, 1.0).is_err());
    }
}
