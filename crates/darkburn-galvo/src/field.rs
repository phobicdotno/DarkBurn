//! The galvo field: millimetres <-> galvo units (0..=65535 on each axis), field size and offset,
//! galvo 1/2 calibration (scale, skew, bulge, trapezoid, negate, is-X), orientation and the
//! lens-correction test pattern.
//!
//! The calibration maths is DarkBurn's own. The equations are simple low-order terms in the
//! normalised field position `(u, v)` (each -1..1 across the field) and are documented on
//! [`GalvoCal`]; how closely they match other software's parameters is listed in
//! `docs/hardware-checks-galvo.md`.

use kurbo::{BezPath, Point, Rect};

use crate::cor::CorTable;

/// Largest galvo coordinate.
pub const GALVO_MAX: f64 = 65535.0;
const HALF: f64 = GALVO_MAX / 2.0;

/// Calibration of one galvo mirror.
///
/// With `(a, b)` = the position along this galvo's own axis and the other axis, normalised to
/// -1..1, the corrected output on this galvo's axis is
/// `scale * (a + skew * b + bulge * a * b^2 + trapezoid * a * b)`, negated when `negate` is set.
/// `is_x` says whether the galvo drives the X axis (otherwise Y).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GalvoCal {
    /// Multiplier, 1.0 = no change.
    pub scale: f64,
    /// Cross-axis shear.
    pub skew: f64,
    /// Barrel / pincushion term.
    pub bulge: f64,
    /// Keystone term.
    pub trapezoid: f64,
    pub negate: bool,
    pub is_x: bool,
}

impl GalvoCal {
    pub const fn identity(is_x: bool) -> Self {
        Self { scale: 1.0, skew: 0.0, bulge: 0.0, trapezoid: 0.0, negate: false, is_x }
    }

    fn apply(&self, a: f64, b: f64) -> f64 {
        let v = self.scale * (a + self.skew * b + self.bulge * a * b * b + self.trapezoid * a * b);
        if self.negate { -v } else { v }
    }

    /// New scale after measuring a marked square: `nominal_mm` was asked for, `measured_mm`
    /// came out. (GALVO-17 scale calibration.)
    pub fn scaled_for_measurement(mut self, nominal_mm: f64, measured_mm: f64) -> Self {
        if nominal_mm > 0.0 && measured_mm > 0.0 {
            self.scale *= nominal_mm / measured_mm;
        }
        self
    }
}

/// Where job coordinate (0, 0) sits in the field.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FieldOrigin {
    /// The middle of the field (galvo centre 0x8000).
    #[default]
    Center,
    /// The lower-left corner (Y up).
    LowerLeft,
}

/// Field configuration.
#[derive(Clone, Debug, PartialEq)]
pub struct FieldConfig {
    /// Side of the square field in mm (lens size, e.g. 110).
    pub size_mm: f64,
    /// Field offset in mm: job (0, 0) lands here, relative to the origin.
    pub offset_mm: (f64, f64),
    pub origin: FieldOrigin,
    pub galvo1: GalvoCal,
    pub galvo2: GalvoCal,
    pub flip_x: bool,
    pub flip_y: bool,
    pub swap_xy: bool,
    /// Clockwise quarter turns (0..=3).
    pub rotate_quarters: u8,
}

impl Default for FieldConfig {
    fn default() -> Self {
        Self {
            size_mm: 110.0,
            offset_mm: (0.0, 0.0),
            origin: FieldOrigin::Center,
            galvo1: GalvoCal::identity(true),
            galvo2: GalvoCal::identity(false),
            flip_x: false,
            flip_y: false,
            swap_xy: false,
            rotate_quarters: 0,
        }
    }
}

/// Error for a position outside the field.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OutOfField {
    pub point: Point,
}

impl std::fmt::Display for OutOfField {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "The point ({:.2}, {:.2}) mm is outside the marking field.", self.point.x, self.point.y)
    }
}
impl std::error::Error for OutOfField {}

/// A configured field. Converts between job millimetres and galvo units.
#[derive(Clone, Debug)]
pub struct Field {
    cfg: FieldConfig,
    /// 2x2 orientation matrix applied to normalised (u, v).
    m: [[f64; 2]; 2],
    /// Host side lens correction (normally `None`; the board applies uploaded tables itself).
    host_cor: Option<CorTable>,
}

impl Field {
    pub fn new(cfg: FieldConfig) -> Self {
        let mut m = [[1.0, 0.0], [0.0, 1.0]];
        if cfg.swap_xy {
            m = [[0.0, 1.0], [1.0, 0.0]];
        }
        let mut flip = [1.0, 1.0];
        if cfg.flip_x {
            flip[0] = -1.0;
        }
        if cfg.flip_y {
            flip[1] = -1.0;
        }
        m = [[m[0][0] * flip[0], m[0][1] * flip[0]], [m[1][0] * flip[1], m[1][1] * flip[1]]];
        for _ in 0..(cfg.rotate_quarters % 4) {
            // clockwise quarter turn with Y up: (u, v) -> (v, -u)
            m = [[m[1][0], m[1][1]], [-m[0][0], -m[0][1]]];
        }
        Self { cfg, m, host_cor: None }
    }

    pub fn with_host_correction(mut self, cor: Option<CorTable>) -> Self {
        self.host_cor = cor;
        self
    }

    pub fn config(&self) -> &FieldConfig {
        &self.cfg
    }

    pub fn units_per_mm(&self) -> f64 {
        GALVO_MAX / self.cfg.size_mm
    }

    /// Job rectangle (mm) that maps inside the field, before calibration.
    pub fn bounds_mm(&self) -> Rect {
        let h = self.cfg.size_mm / 2.0;
        let (cx, cy) = self.center_mm();
        Rect::new(cx - h, cy - h, cx + h, cy + h)
    }

    fn center_mm(&self) -> (f64, f64) {
        let c = match self.cfg.origin {
            FieldOrigin::Center => 0.0,
            FieldOrigin::LowerLeft => self.cfg.size_mm / 2.0,
        };
        (c + self.cfg.offset_mm.0, c + self.cfg.offset_mm.1)
    }

    fn cal(&self, u: f64, v: f64) -> (f64, f64) {
        let g = |c: &GalvoCal| if c.is_x { c.apply(u, v) } else { c.apply(v, u) };
        let (g1, g2) = (&self.cfg.galvo1, &self.cfg.galvo2);
        let x = if g1.is_x { g(g1) } else { g(g2) };
        let y = if g1.is_x { g(g2) } else { g(g1) };
        (x, y)
    }

    /// mm to floating galvo units (not rounded, not range checked).
    pub fn mm_to_units_f(&self, p: Point) -> (f64, f64) {
        let (cx, cy) = self.center_mm();
        let h = self.cfg.size_mm / 2.0;
        let u = (p.x - cx) / h;
        let v = (p.y - cy) / h;
        let (u, v) = (self.m[0][0] * u + self.m[0][1] * v, self.m[1][0] * u + self.m[1][1] * v);
        let (u, v) = self.cal(u, v);
        let (mut x, mut y) = (HALF + u * HALF, HALF + v * HALF);
        if let Some(cor) = &self.host_cor {
            (x, y) = cor.correct(x, y);
        }
        (x, y)
    }

    /// mm to galvo units, rounded; errors when the result leaves 0..=65535.
    pub fn mm_to_galvo(&self, p: Point) -> Result<(u16, u16), OutOfField> {
        let (x, y) = self.mm_to_units_f(p);
        if !(-0.5..=GALVO_MAX + 0.5).contains(&x) || !(-0.5..=GALVO_MAX + 0.5).contains(&y) {
            return Err(OutOfField { point: p });
        }
        Ok((x.round().clamp(0.0, GALVO_MAX) as u16, y.round().clamp(0.0, GALVO_MAX) as u16))
    }

    /// Galvo units back to mm (inverts calibration iteratively; host correction is not undone).
    pub fn galvo_to_mm(&self, x: f64, y: f64) -> Point {
        let (tu, tv) = ((x - HALF) / HALF, (y - HALF) / HALF);
        // Newton iteration with a numeric Jacobian (the calibration terms are smooth).
        let (mut u, mut v) = (tu, tv);
        for _ in 0..30 {
            let (cu, cv) = self.cal(u, v);
            let (ru, rv) = (tu - cu, tv - cv);
            if ru.abs() < 1e-12 && rv.abs() < 1e-12 {
                break;
            }
            let h = 1e-6;
            let (au, av) = self.cal(u + h, v);
            let (bu, bv) = self.cal(u, v + h);
            let (j00, j10, j01, j11) = ((au - cu) / h, (av - cv) / h, (bu - cu) / h, (bv - cv) / h);
            let det = j00 * j11 - j01 * j10;
            if det.abs() < 1e-12 {
                break;
            }
            u += (ru * j11 - rv * j01) / det;
            v += (rv * j00 - ru * j10) / det;
        }
        // orientation matrix is orthogonal: inverse = transpose
        let (a, b) = (self.m[0][0] * u + self.m[1][0] * v, self.m[0][1] * u + self.m[1][1] * v);
        let (cx, cy) = self.center_mm();
        let h = self.cfg.size_mm / 2.0;
        Point::new(cx + a * h, cy + b * h)
    }

    /// Distance in galvo units between two mm points (used for jump distance words).
    pub fn galvo_distance(&self, a: (u16, u16), b: (u16, u16)) -> u16 {
        let dx = a.0 as f64 - b.0 as f64;
        let dy = a.1 as f64 - b.1 as f64;
        dx.hypot(dy).round().min(GALVO_MAX) as u16
    }
}

/// Lens-correction test pattern (GALVO-17): a border, grid lines and a small cross at each grid
/// node, to be marked on card and measured against the nominal size.
#[derive(Clone, Debug, PartialEq)]
pub struct LensTestPattern {
    /// Side of the tested square in mm (centred on the job origin).
    pub size_mm: f64,
    /// Cells per side.
    pub divisions: u32,
    /// Half-length of the crosses in mm (0 for none).
    pub cross_mm: f64,
    /// Draw the grid lines (otherwise border and crosses only).
    pub grid_lines: bool,
}

impl Default for LensTestPattern {
    fn default() -> Self {
        Self { size_mm: 100.0, divisions: 10, cross_mm: 1.5, grid_lines: true }
    }
}

impl LensTestPattern {
    /// Paths in mm, centred on (0, 0).
    pub fn paths(&self) -> Vec<BezPath> {
        let n = self.divisions.max(1);
        let h = self.size_mm / 2.0;
        let step = self.size_mm / n as f64;
        let mut out = Vec::new();
        let mut border = BezPath::new();
        border.move_to((-h, -h));
        border.line_to((h, -h));
        border.line_to((h, h));
        border.line_to((-h, h));
        border.close_path();
        out.push(border);
        if self.grid_lines {
            for i in 1..n {
                let p = -h + step * i as f64;
                let mut a = BezPath::new();
                a.move_to((p, -h));
                a.line_to((p, h));
                out.push(a);
                let mut b = BezPath::new();
                b.move_to((-h, p));
                b.line_to((h, p));
                out.push(b);
            }
        }
        if self.cross_mm > 0.0 {
            for i in 0..=n {
                for j in 0..=n {
                    let (x, y) = (-h + step * i as f64, -h + step * j as f64);
                    let c = self.cross_mm;
                    let mut a = BezPath::new();
                    a.move_to((x - c, y));
                    a.line_to((x + c, y));
                    out.push(a);
                    let mut b = BezPath::new();
                    b.move_to((x, y - c));
                    b.line_to((x, y + c));
                    out.push(b);
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Point, b: Point, tol: f64) -> bool {
        (a.x - b.x).abs() < tol && (a.y - b.y).abs() < tol
    }

    #[test]
    fn centre_and_edges() {
        let f = Field::new(FieldConfig { size_mm: 100.0, ..Default::default() });
        assert_eq!(f.mm_to_galvo(Point::new(0.0, 0.0)).unwrap(), (32768, 32768));
        assert_eq!(f.mm_to_galvo(Point::new(-50.0, -50.0)).unwrap(), (0, 0));
        assert_eq!(f.mm_to_galvo(Point::new(50.0, 50.0)).unwrap(), (65535, 65535));
        assert!(f.mm_to_galvo(Point::new(50.1, 0.0)).is_err());
        assert!((f.units_per_mm() - 655.35).abs() < 1e-9);
    }

    #[test]
    fn lower_left_origin_and_offset() {
        let f = Field::new(FieldConfig { size_mm: 100.0, origin: FieldOrigin::LowerLeft, offset_mm: (5.0, -5.0), ..Default::default() });
        // job (55, 45) is the field centre
        assert_eq!(f.mm_to_galvo(Point::new(55.0, 45.0)).unwrap(), (32768, 32768));
        let r = f.bounds_mm();
        assert_eq!((r.x0, r.y0, r.x1, r.y1), (5.0, -5.0, 105.0, 95.0));
    }

    #[test]
    fn orientation_round_trips() {
        for (fx, fy, sw, rot) in
            [(false, false, false, 0), (true, false, false, 0), (false, true, true, 1), (true, true, true, 3), (false, false, false, 2)]
        {
            let f = Field::new(FieldConfig { size_mm: 80.0, flip_x: fx, flip_y: fy, swap_xy: sw, rotate_quarters: rot, ..Default::default() });
            let p = Point::new(12.5, -30.0);
            let (x, y) = f.mm_to_units_f(p);
            assert!(close(f.galvo_to_mm(x, y), p, 1e-6), "{fx} {fy} {sw} {rot}");
        }
    }

    #[test]
    fn rotation_moves_points_clockwise() {
        let f = Field::new(FieldConfig { size_mm: 100.0, rotate_quarters: 1, ..Default::default() });
        // +X in the job goes to -Y on the galvo (clockwise, Y up)
        let (x, y) = f.mm_to_units_f(Point::new(25.0, 0.0));
        assert!((x - HALF).abs() < 1e-6 && y < HALF);
    }

    #[test]
    fn calibration_round_trips() {
        let cal1 = GalvoCal { scale: 1.02, skew: 0.01, bulge: 0.02, trapezoid: -0.015, negate: false, is_x: true };
        let cal2 = GalvoCal { scale: 0.99, skew: -0.005, bulge: 0.01, trapezoid: 0.01, negate: true, is_x: false };
        let f = Field::new(FieldConfig { size_mm: 110.0, galvo1: cal1, galvo2: cal2, ..Default::default() });
        for p in [Point::new(0.0, 0.0), Point::new(40.0, -30.0), Point::new(-50.0, 50.0)] {
            let (x, y) = f.mm_to_units_f(p);
            assert!(close(f.galvo_to_mm(x, y), p, 1e-6));
        }
    }

    #[test]
    fn scale_is_applied_and_negate_flips() {
        let mut cfg = FieldConfig { size_mm: 100.0, ..Default::default() };
        cfg.galvo1.scale = 1.1;
        let f = Field::new(cfg.clone());
        let (x, _) = f.mm_to_units_f(Point::new(25.0, 0.0));
        assert!((x - (HALF + 0.5 * 1.1 * HALF)).abs() < 1e-6);
        cfg.galvo1.negate = true;
        let (x, _) = Field::new(cfg).mm_to_units_f(Point::new(25.0, 0.0));
        assert!(x < HALF);
    }

    #[test]
    fn galvo_roles_can_be_swapped() {
        let mut cfg = FieldConfig { size_mm: 100.0, ..Default::default() };
        cfg.galvo1 = GalvoCal { scale: 1.2, ..GalvoCal::identity(false) };
        cfg.galvo2 = GalvoCal::identity(true);
        let f = Field::new(cfg);
        let (x, y) = f.mm_to_units_f(Point::new(25.0, 25.0));
        assert!((x - (HALF + 0.5 * HALF)).abs() < 1e-6);
        assert!((y - (HALF + 0.5 * 1.2 * HALF)).abs() < 1e-6);
    }

    #[test]
    fn scale_measurement_fixes_size() {
        let cal = GalvoCal::identity(true).scaled_for_measurement(100.0, 102.0);
        assert!((cal.scale - 100.0 / 102.0).abs() < 1e-12);
        let same = GalvoCal::identity(true).scaled_for_measurement(0.0, 5.0);
        assert_eq!(same.scale, 1.0);
    }

    #[test]
    fn host_correction_moves_points() {
        let cor = CorTable::from_fn(|_, _| (100, -50));
        let f = Field::new(FieldConfig { size_mm: 100.0, ..Default::default() }).with_host_correction(Some(cor));
        let (x, y) = f.mm_to_units_f(Point::new(0.0, 0.0));
        assert!((x - (HALF + 100.0)).abs() < 1e-9 && (y - (HALF - 50.0)).abs() < 1e-9);
    }

    #[test]
    fn test_pattern_counts() {
        let p = LensTestPattern { size_mm: 100.0, divisions: 4, cross_mm: 1.0, grid_lines: true };
        // border + 3 vertical + 3 horizontal + 25 nodes * 2 strokes
        assert_eq!(p.paths().len(), 1 + 6 + 50);
        let q = LensTestPattern { grid_lines: false, cross_mm: 0.0, ..p };
        assert_eq!(q.paths().len(), 1);
        for path in LensTestPattern::default().paths() {
            let bb = kurbo::Shape::bounding_box(&path);
            assert!(bb.x0 >= -51.6 && bb.x1 <= 51.6);
        }
    }
}
