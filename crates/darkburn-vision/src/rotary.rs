//! Rotary attachment maths: chuck and roller types, design Y to rotation, mirror, test rotation,
//! and splitting a design into slices or shapes for galvo rotary marking. Pure functions over `kurbo` paths.

use kurbo::{Affine, BezPath, PathEl, Point, Shape};
use serde::{Deserialize, Serialize};
use std::f64::consts::PI;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RotaryKind {
    /// The object is gripped and turned directly: one rotation moves a surface length equal to the object circumference.
    #[default]
    Chuck,
    /// The object rests on driven rollers: one roller rotation moves a surface length equal to the roller circumference.
    Roller,
}

/// Which machine axis carries the rotary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum RotaryAxis {
    #[default]
    Y,
    A,
    B,
    C,
    U,
    V,
}

impl RotaryAxis {
    pub fn letter(&self) -> char {
        match self {
            Self::Y => 'Y',
            Self::A => 'A',
            Self::B => 'B',
            Self::C => 'C',
            Self::U => 'U',
            Self::V => 'V',
        }
    }

    pub fn all() -> [RotaryAxis; 6] {
        [Self::Y, Self::A, Self::B, Self::C, Self::U, Self::V]
    }
}

/// How the size of the object is entered.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum ObjectSize {
    Diameter(f64),
    Circumference(f64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RotaryError {
    Disabled,
    InvalidStepsPerRotation,
    InvalidObjectSize,
    InvalidRollerDiameter,
    InvalidSliceSize,
}

impl std::fmt::Display for RotaryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Disabled => "rotary is not enabled",
            Self::InvalidStepsPerRotation => "steps per rotation must be above zero",
            Self::InvalidObjectSize => "object diameter or circumference must be above zero",
            Self::InvalidRollerDiameter => "roller diameter must be above zero",
            Self::InvalidSliceSize => "slice size must be above zero",
        })
    }
}
impl std::error::Error for RotaryError {}

/// Rotary setup (ROT-01 to ROT-05).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct RotaryParams {
    pub enabled: bool,
    pub kind: RotaryKind,
    pub axis: RotaryAxis,
    /// Motor steps for one full turn of the chuck or roller drive.
    pub steps_per_rotation: f64,
    /// Steps per axis unit (mm) the controller uses for the chosen axis. 1.0 means axis units are steps.
    pub axis_steps_per_mm: f64,
    pub roller_diameter: f64,
    pub object: ObjectSize,
    /// Reverse the rotation direction.
    pub mirror: bool,
}

impl Default for RotaryParams {
    fn default() -> Self {
        Self {
            enabled: false,
            kind: RotaryKind::Chuck,
            axis: RotaryAxis::Y,
            steps_per_rotation: 1600.0,
            axis_steps_per_mm: 1.0,
            roller_diameter: 25.0,
            object: ObjectSize::Diameter(50.0),
            mirror: false,
        }
    }
}

impl RotaryParams {
    pub fn validate(&self) -> Result<(), RotaryError> {
        if !self.enabled {
            return Err(RotaryError::Disabled);
        }
        if !(self.steps_per_rotation > 0.0) || !(self.axis_steps_per_mm > 0.0) {
            return Err(RotaryError::InvalidStepsPerRotation);
        }
        if !(self.object_circumference() > 0.0) {
            return Err(RotaryError::InvalidObjectSize);
        }
        if self.kind == RotaryKind::Roller && !(self.roller_diameter > 0.0) {
            return Err(RotaryError::InvalidRollerDiameter);
        }
        Ok(())
    }

    pub fn object_circumference(&self) -> f64 {
        match self.object {
            ObjectSize::Diameter(d) => PI * d,
            ObjectSize::Circumference(c) => c,
        }
    }

    pub fn object_diameter(&self) -> f64 {
        self.object_circumference() / PI
    }

    /// Axis units (mm) for one full rotation of the driven part.
    pub fn axis_units_per_rotation(&self) -> f64 {
        self.steps_per_rotation / self.axis_steps_per_mm
    }

    /// Surface length (mm) that passes under the laser per rotation of the driven part.
    pub fn surface_per_rotation(&self) -> f64 {
        match self.kind {
            RotaryKind::Chuck => self.object_circumference(),
            RotaryKind::Roller => PI * self.roller_diameter,
        }
    }

    fn sign(&self) -> f64 {
        if self.mirror { -1.0 } else { 1.0 }
    }

    /// Turns of the driven part needed to move the design Y by `y` mm (signed, mirror applied).
    pub fn rotations(&self, y: f64) -> Result<f64, RotaryError> {
        self.validate()?;
        Ok(self.sign() * y / self.surface_per_rotation())
    }

    /// Rotation of the driven part, degrees.
    pub fn drive_angle_deg(&self, y: f64) -> Result<f64, RotaryError> {
        Ok(self.rotations(y)? * 360.0)
    }

    /// Rotation of the object itself, degrees. For a roller this is the roller angle times roller/object diameter.
    pub fn object_angle_deg(&self, y: f64) -> Result<f64, RotaryError> {
        self.validate()?;
        Ok(self.sign() * y / self.object_circumference() * 360.0)
    }

    /// Axis distance (axis units, mm) the controller must move for design Y of `y` mm.
    pub fn y_to_axis(&self, y: f64) -> Result<f64, RotaryError> {
        Ok(self.rotations(y)? * self.axis_units_per_rotation())
    }

    /// Inverse of [`y_to_axis`](Self::y_to_axis).
    pub fn axis_to_y(&self, axis: f64) -> Result<f64, RotaryError> {
        self.validate()?;
        Ok(self.sign() * axis / self.axis_units_per_rotation() * self.surface_per_rotation())
    }

    /// Axis units per design millimetre (roller compensation included: the object size drops out for rollers).
    pub fn axis_scale(&self) -> Result<f64, RotaryError> {
        self.y_to_axis(1.0)
    }

    /// Affine that rewrites design Y into axis units (Y scale, mirrored if requested). X is untouched.
    pub fn axis_affine(&self) -> Result<Affine, RotaryError> {
        Ok(Affine::scale_non_uniform(1.0, self.axis_scale()?))
    }

    /// Apply [`axis_affine`](Self::axis_affine) to a path.
    pub fn map_path(&self, path: &BezPath) -> Result<BezPath, RotaryError> {
        Ok(self.axis_affine()? * path.clone())
    }
}

/// One step of a test run.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct RotaryMove {
    pub axis: RotaryAxis,
    /// Relative move in axis units.
    pub distance: f64,
}

/// Rotate and return, so the user can check the object comes back to where it started (ROT-06).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TestRotationPlan {
    pub revolutions: f64,
    pub angle_deg: f64,
    pub steps: f64,
    pub moves: [RotaryMove; 2],
}

pub fn test_rotation_plan(params: &RotaryParams, revolutions: f64) -> Result<TestRotationPlan, RotaryError> {
    params.validate()?;
    let dist = revolutions * params.axis_units_per_rotation() * if params.mirror { -1.0 } else { 1.0 };
    Ok(TestRotationPlan {
        revolutions,
        angle_deg: revolutions * 360.0,
        steps: revolutions * params.steps_per_rotation,
        moves: [RotaryMove { axis: params.axis, distance: dist }, RotaryMove { axis: params.axis, distance: -dist }],
    })
}

/// A band of the design for rotary marking: draw it with the object turned to `rotation_deg`.
#[derive(Clone, Debug, PartialEq)]
pub struct RotarySlice {
    pub index: usize,
    pub y_min: f64,
    pub y_max: f64,
    pub centre_y: f64,
    /// Object rotation (degrees) to place the slice centre under the laser.
    pub rotation_deg: f64,
    /// Geometry shifted so the slice centre sits at Y = 0.
    pub paths: Vec<BezPath>,
    /// Which input shapes contributed (split by shape).
    pub shape_indices: Vec<usize>,
    /// True when a shape was taller than the limit and had to be cut across.
    pub cut_across: bool,
}

#[derive(Clone, Debug, PartialEq)]
struct Poly {
    pts: Vec<Point>,
    closed: bool,
}

fn polylines(path: &BezPath) -> Vec<Poly> {
    let mut out: Vec<Poly> = Vec::new();
    kurbo::flatten(path.elements().iter().copied(), 0.01, |el| match el {
        PathEl::MoveTo(p) => out.push(Poly { pts: vec![p], closed: false }),
        PathEl::LineTo(p) => {
            if let Some(l) = out.last_mut() {
                l.pts.push(p);
            }
        }
        PathEl::ClosePath => {
            if let Some(l) = out.last_mut() {
                l.closed = true;
            }
        }
        _ => {}
    });
    out.retain(|p| p.pts.len() > 1);
    out
}

/// Clip a segment to `y0 <= y <= y1`; returns the parameter interval.
fn clip_segment(a: Point, b: Point, y0: f64, y1: f64, inclusive_end: bool) -> Option<(f64, f64)> {
    let dy = b.y - a.y;
    if dy.abs() < 1e-12 {
        let inside = a.y >= y0 && (a.y < y1 || (inclusive_end && a.y <= y1));
        return inside.then_some((0.0, 1.0));
    }
    let (ta, tb) = ((y0 - a.y) / dy, (y1 - a.y) / dy);
    let (lo, hi) = if ta < tb { (ta, tb) } else { (tb, ta) };
    let (t0, t1) = (lo.max(0.0), hi.min(1.0));
    (t1 - t0 > 1e-12).then_some((t0, t1))
}

/// The part of `path` inside the horizontal band `y0..y1`. Closed shapes that lie wholly inside stay closed;
/// shapes cut by the band edge become open lines (fills are made from scan lines, so they stay correct).
pub fn clip_to_band(path: &BezPath, y0: f64, y1: f64, inclusive_end: bool) -> BezPath {
    let mut out = BezPath::new();
    for poly in polylines(path) {
        let inside = |p: &Point| p.y >= y0 && (p.y < y1 || (inclusive_end && p.y <= y1));
        if poly.closed && poly.pts.iter().all(inside) {
            out.move_to(poly.pts[0]);
            for p in &poly.pts[1..] {
                out.line_to(*p);
            }
            out.close_path();
            continue;
        }
        let mut pts = poly.pts.clone();
        if poly.closed && pts.first() != pts.last() {
            pts.push(pts[0]);
        }
        let mut last: Option<Point> = None;
        for w in pts.windows(2) {
            let (a, b) = (w[0], w[1]);
            if let Some((t0, t1)) = clip_segment(a, b, y0, y1, inclusive_end) {
                let p0 = a.lerp(b, t0);
                let p1 = a.lerp(b, t1);
                if last.is_none_or(|l| (l - p0).hypot() > 1e-9) {
                    out.move_to(p0);
                }
                out.line_to(p1);
                last = Some(p1);
            } else {
                last = None;
            }
        }
    }
    out
}

fn y_extent(paths: &[BezPath]) -> Option<(f64, f64)> {
    let mut e: Option<(f64, f64)> = None;
    for p in paths {
        if p.elements().is_empty() {
            continue;
        }
        let b = p.bounding_box();
        if !b.y0.is_finite() {
            continue;
        }
        e = Some(match e {
            None => (b.y0, b.y1),
            Some((a, c)) => (a.min(b.y0), c.max(b.y1)),
        });
    }
    e
}

/// Cut the design into equal bands no taller than `max_size` mm (galvo field limit), one rotation per band.
pub fn split_by_slice(paths: &[BezPath], max_size: f64, params: &RotaryParams) -> Result<Vec<RotarySlice>, RotaryError> {
    if !(max_size > 0.0) {
        return Err(RotaryError::InvalidSliceSize);
    }
    params.validate()?;
    let Some((ymin, ymax)) = y_extent(paths) else { return Ok(Vec::new()) };
    let n = (((ymax - ymin) / max_size) - 1e-9).ceil().max(1.0) as usize;
    let size = (ymax - ymin) / n as f64;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let (y0, y1) = (ymin + i as f64 * size, ymin + (i + 1) as f64 * size);
        let cy = (y0 + y1) / 2.0;
        let last = i + 1 == n;
        let clipped: Vec<BezPath> = paths
            .iter()
            .map(|p| clip_to_band(p, y0, y1, last))
            .filter(|p| !p.elements().is_empty())
            .map(|p| Affine::translate((0.0, -cy)) * p)
            .collect();
        let shape_indices = paths.iter().enumerate().filter(|(_, p)| !clip_to_band(p, y0, y1, last).elements().is_empty()).map(|(k, _)| k).collect();
        out.push(RotarySlice {
            index: i,
            y_min: y0,
            y_max: y1,
            centre_y: cy,
            rotation_deg: params.object_angle_deg(cy)?,
            paths: clipped,
            shape_indices,
            cut_across: n > 1,
        });
    }
    Ok(out)
}

/// Group whole shapes into slices no taller than `max_size`; a shape taller than the limit is cut across instead.
pub fn split_by_shape(shapes: &[BezPath], max_size: f64, params: &RotaryParams) -> Result<Vec<RotarySlice>, RotaryError> {
    if !(max_size > 0.0) {
        return Err(RotaryError::InvalidSliceSize);
    }
    params.validate()?;
    let mut order: Vec<(usize, f64, f64)> = shapes
        .iter()
        .enumerate()
        .filter(|(_, s)| !s.elements().is_empty())
        .filter_map(|(i, s)| y_extent(std::slice::from_ref(s)).map(|(a, b)| (i, a, b)))
        .collect();
    order.sort_by(|a, b| a.1.total_cmp(&b.1));
    let mut out: Vec<RotarySlice> = Vec::new();
    let mut group: Vec<(usize, f64, f64)> = Vec::new();
    let flush = |group: &mut Vec<(usize, f64, f64)>, out: &mut Vec<RotarySlice>| -> Result<(), RotaryError> {
        if group.is_empty() {
            return Ok(());
        }
        let y0 = group.iter().map(|g| g.1).fold(f64::INFINITY, f64::min);
        let y1 = group.iter().map(|g| g.2).fold(f64::NEG_INFINITY, f64::max);
        let cy = (y0 + y1) / 2.0;
        let paths = group.iter().map(|g| Affine::translate((0.0, -cy)) * shapes[g.0].clone()).collect();
        out.push(RotarySlice {
            index: out.len(),
            y_min: y0,
            y_max: y1,
            centre_y: cy,
            rotation_deg: params.object_angle_deg(cy)?,
            paths,
            shape_indices: group.iter().map(|g| g.0).collect(),
            cut_across: false,
        });
        group.clear();
        Ok(())
    };
    for item in order {
        if item.2 - item.1 > max_size {
            flush(&mut group, &mut out)?;
            for mut s in split_by_slice(std::slice::from_ref(&shapes[item.0]), max_size, params)? {
                s.index = out.len();
                s.shape_indices = vec![item.0];
                out.push(s);
            }
            continue;
        }
        let g0 = group.iter().map(|g| g.1).fold(item.1, f64::min);
        let g1 = group.iter().map(|g| g.2).fold(item.2, f64::max);
        if !group.is_empty() && g1 - g0 > max_size {
            flush(&mut group, &mut out)?;
        }
        group.push(item);
    }
    flush(&mut group, &mut out)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::Rect;

    fn chuck() -> RotaryParams {
        RotaryParams {
            enabled: true,
            kind: RotaryKind::Chuck,
            steps_per_rotation: 3200.0,
            axis_steps_per_mm: 80.0,
            object: ObjectSize::Diameter(50.0),
            ..Default::default()
        }
    }

    fn roller() -> RotaryParams {
        RotaryParams { kind: RotaryKind::Roller, roller_diameter: 20.0, object: ObjectSize::Diameter(60.0), ..chuck() }
    }

    #[test]
    fn chuck_maps_circumference_to_one_rotation() {
        let p = chuck();
        let circ = PI * 50.0;
        assert!((p.rotations(circ).unwrap() - 1.0).abs() < 1e-12);
        assert!((p.drive_angle_deg(circ / 4.0).unwrap() - 90.0).abs() < 1e-9);
        assert!((p.y_to_axis(circ).unwrap() - 40.0).abs() < 1e-9); // 3200 / 80
        assert!((p.axis_to_y(p.y_to_axis(33.3).unwrap()).unwrap() - 33.3).abs() < 1e-9);
    }

    #[test]
    fn roller_ignores_object_size_for_travel() {
        let r = roller();
        let c = chuck();
        let scale_roller = r.axis_scale().unwrap();
        // one roller rotation moves PI*20 of surface
        assert!((r.rotations(PI * 20.0).unwrap() - 1.0).abs() < 1e-12);
        let bigger = RotaryParams { object: ObjectSize::Diameter(200.0), ..r };
        assert!((bigger.axis_scale().unwrap() - scale_roller).abs() < 1e-12);
        assert!((c.axis_scale().unwrap() - scale_roller).abs() > 1e-6);
        // object turns slower than the roller by roller/object diameter
        let obj = r.object_angle_deg(PI * 20.0).unwrap();
        assert!((obj - 360.0 * 20.0 / 60.0).abs() < 1e-9);
    }

    #[test]
    fn circumference_input_and_mirror() {
        let a = RotaryParams { object: ObjectSize::Circumference(100.0), mirror: true, ..chuck() };
        assert!((a.object_diameter() - 100.0 / PI).abs() < 1e-12);
        assert!((a.rotations(25.0).unwrap() + 0.25).abs() < 1e-12);
        assert!(a.axis_scale().unwrap() < 0.0);
        let path = BezPath::from_vec(vec![PathEl::MoveTo(Point::new(0.0, 0.0)), PathEl::LineTo(Point::new(10.0, 100.0))]);
        let m = a.map_path(&path).unwrap();
        let end = match m.elements()[1] {
            PathEl::LineTo(p) => p,
            _ => unreachable!(),
        };
        assert!((end.x - 10.0).abs() < 1e-12 && end.y < 0.0);
    }

    #[test]
    fn validation() {
        assert_eq!(RotaryParams::default().validate().unwrap_err(), RotaryError::Disabled);
        assert_eq!(RotaryParams { steps_per_rotation: 0.0, ..chuck() }.validate().unwrap_err(), RotaryError::InvalidStepsPerRotation);
        assert_eq!(RotaryParams { object: ObjectSize::Diameter(0.0), ..chuck() }.validate().unwrap_err(), RotaryError::InvalidObjectSize);
        assert_eq!(RotaryParams { roller_diameter: 0.0, ..roller() }.validate().unwrap_err(), RotaryError::InvalidRollerDiameter);
        assert!(chuck().y_to_axis(1.0).is_ok());
        assert_eq!(RotaryAxis::A.letter(), 'A');
    }

    #[test]
    fn test_rotation_returns_to_start() {
        let plan = test_rotation_plan(&chuck(), 1.0).unwrap();
        assert!((plan.moves[0].distance - 40.0).abs() < 1e-9);
        assert!((plan.moves[0].distance + plan.moves[1].distance).abs() < 1e-12);
        assert_eq!(plan.angle_deg, 360.0);
        assert_eq!(plan.steps, 3200.0);
        let m = test_rotation_plan(&RotaryParams { mirror: true, ..chuck() }, 0.5).unwrap();
        assert!(m.moves[0].distance < 0.0);
    }

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> BezPath {
        Rect::new(x0, y0, x1, y1).to_path(0.1)
    }

    #[test]
    fn slices_cover_everything_without_overlap() {
        let shapes = vec![rect(0.0, 0.0, 10.0, 100.0)];
        let slices = split_by_slice(&shapes, 30.0, &chuck()).unwrap();
        assert_eq!(slices.len(), 4);
        for s in &slices {
            assert!(s.y_max - s.y_min <= 30.0 + 1e-9);
            assert!(!s.paths.is_empty());
        }
        // total length of the long vertical edges across slices equals the original
        let len = |p: &BezPath| -> f64 {
            let mut l = 0.0;
            for poly in polylines(p) {
                for w in poly.pts.windows(2) {
                    l += (w[1] - w[0]).hypot();
                }
            }
            l
        };
        let total: f64 = slices.iter().flat_map(|s| s.paths.iter()).map(len).sum();
        // two vertical edges of 100 + horizontal edges at both ends of the rectangle (10 each) + 0 elsewhere
        assert!((total - (200.0 + 20.0)).abs() < 1e-6, "total {total}");
        // slice centres are re-centred at 0
        for s in &slices {
            let b = s.paths[0].bounding_box();
            assert!(b.y0 >= -s.paths_half() - 1e-9 && b.y1 <= s.paths_half() + 1e-9);
        }
        // rotation increases by slice height / circumference
        let step = slices[1].rotation_deg - slices[0].rotation_deg;
        assert!((step - 25.0 / (PI * 50.0) * 360.0).abs() < 1e-9);
    }

    impl RotarySlice {
        fn paths_half(&self) -> f64 {
            (self.y_max - self.y_min) / 2.0
        }
    }

    #[test]
    fn small_design_is_one_slice() {
        let slices = split_by_slice(&[rect(0.0, 5.0, 10.0, 15.0)], 30.0, &chuck()).unwrap();
        assert_eq!(slices.len(), 1);
        assert!(!slices[0].cut_across);
        assert!((slices[0].centre_y - 10.0).abs() < 1e-12);
        assert!(split_by_slice(&[], 30.0, &chuck()).unwrap().is_empty());
        assert_eq!(split_by_slice(&[], 0.0, &chuck()).unwrap_err(), RotaryError::InvalidSliceSize);
    }

    #[test]
    fn shapes_stay_whole_when_they_fit() {
        let shapes = vec![rect(0.0, 0.0, 10.0, 10.0), rect(0.0, 12.0, 10.0, 22.0), rect(0.0, 60.0, 10.0, 70.0), rect(0.0, 100.0, 10.0, 250.0)];
        let slices = split_by_shape(&shapes, 30.0, &chuck()).unwrap();
        // shapes 0+1 together, 2 alone, shape 3 is cut across (150 tall)
        assert_eq!(slices[0].shape_indices, vec![0, 1]);
        assert_eq!(slices[1].shape_indices, vec![2]);
        assert!(!slices[0].cut_across && !slices[1].cut_across);
        let cut: Vec<&RotarySlice> = slices.iter().filter(|s| s.shape_indices == vec![3]).collect();
        assert_eq!(cut.len(), 5);
        assert!(cut.iter().all(|s| s.cut_across));
        for (i, s) in slices.iter().enumerate() {
            assert_eq!(s.index, i);
        }
    }

    #[test]
    fn clip_keeps_inside_shapes_closed() {
        let p = rect(0.0, 2.0, 5.0, 4.0);
        let c = clip_to_band(&p, 0.0, 10.0, false);
        assert!(c.elements().iter().any(|e| matches!(e, PathEl::ClosePath)));
        let cut = clip_to_band(&p, 0.0, 3.0, false);
        assert!(!cut.elements().iter().any(|e| matches!(e, PathEl::ClosePath)));
        assert!(clip_to_band(&p, 10.0, 20.0, false).elements().is_empty());
    }
}
