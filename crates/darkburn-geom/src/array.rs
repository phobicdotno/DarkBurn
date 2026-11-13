//! Arrays: grid, circular and copy-along-path. Every function returns one
//! [`kurbo::Affine`] per copy (apply it to the original item); no geometry is copied.

use crate::path::{DEFAULT_ACCURACY, contours, nearest_on_contour, seg_tangent};
use kurbo::{Affine, BezPath, ParamCurve, ParamCurveArclen, Point, Rect, Vec2};

/// The most copies an array is meant to have. Every array function stops one copy past this,
/// so a caller can tell that the limit was passed while a mistyped count or spacing cannot
/// fill the memory.
pub const MAX_COPIES: usize = 100_000;

/// Distance between neighbouring grid items.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Spacing {
    /// Empty space between the bounding boxes of neighbours.
    Gap(Vec2),
    /// Distance between the centres of neighbours.
    CenterToCenter(Vec2),
}

impl Spacing {
    /// Centre-to-centre step for an item of the given bounding box.
    pub fn step(&self, item: Rect) -> Vec2 {
        match *self {
            Spacing::Gap(g) => Vec2::new(item.width() + g.x, item.height() + g.y),
            Spacing::CenterToCenter(s) => s,
        }
    }
}

/// Offsetting of alternate rows or columns.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shift {
    None,
    /// Odd rows move right by this many mm.
    AltRows(f64),
    /// Odd columns move up by this many mm.
    AltCols(f64),
    /// Hex layout: odd rows move by half a step and rows sit `step.y * √3/2` apart.
    HexRows,
}

/// Options for [`grid_array`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridOptions {
    pub rows: usize,
    pub cols: usize,
    pub spacing: Spacing,
    pub shift: Shift,
    /// Flip odd rows vertically (about each item's centre).
    pub mirror_alt_rows: bool,
    /// Flip odd columns horizontally (about each item's centre).
    pub mirror_alt_cols: bool,
    /// Grow toward -X instead of +X.
    pub reverse_x: bool,
    /// Grow toward -Y instead of +Y.
    pub reverse_y: bool,
}

impl Default for GridOptions {
    fn default() -> Self {
        Self {
            rows: 1,
            cols: 1,
            spacing: Spacing::Gap(Vec2::ZERO),
            shift: Shift::None,
            mirror_alt_rows: false,
            mirror_alt_cols: false,
            reverse_x: false,
            reverse_y: false,
        }
    }
}

/// Grid array transforms in row-major order (row 0 first). Item 0 is the identity. At most
/// [`MAX_COPIES`]` + 1` transforms are returned.
pub fn grid_array(item_bbox: Rect, o: &GridOptions) -> Vec<Affine> {
    let step = o.spacing.step(item_bbox);
    let row_step = if o.shift == Shift::HexRows { step.y * 3f64.sqrt() / 2.0 } else { step.y };
    let sx = if o.reverse_x { -1.0 } else { 1.0 };
    let sy = if o.reverse_y { -1.0 } else { 1.0 };
    let c = item_bbox.center().to_vec2();
    let mut out = Vec::with_capacity(o.rows.saturating_mul(o.cols).min(MAX_COPIES + 1));
    'rows: for r in 0..o.rows {
        for col in 0..o.cols {
            if out.len() > MAX_COPIES {
                break 'rows;
            }
            let mut d = Vec2::new(sx * step.x * col as f64, sy * row_step * r as f64);
            match o.shift {
                Shift::AltRows(dx) if r % 2 == 1 => d.x += dx,
                Shift::AltCols(dy) if col % 2 == 1 => d.y += dy,
                Shift::HexRows if r % 2 == 1 => d.x += sx * step.x * 0.5,
                _ => {}
            }
            let fx = if o.mirror_alt_cols && col % 2 == 1 { -1.0 } else { 1.0 };
            let fy = if o.mirror_alt_rows && r % 2 == 1 { -1.0 } else { 1.0 };
            let flip = Affine::translate(c) * Affine::scale_non_uniform(fx, fy) * Affine::translate(-c);
            out.push(Affine::translate(d) * flip);
        }
    }
    out
}

/// How many rows and columns fit inside `target` (the first item at its bottom-left).
pub fn grid_fit(item_bbox: Rect, target: Rect, spacing: Spacing) -> (usize, usize) {
    let step = spacing.step(item_bbox);
    let fit = |avail: f64, size: f64, step: f64| -> usize {
        if avail + 1e-9 < size || size < 0.0 {
            0
        } else if step <= 1e-12 {
            1
        } else {
            // An area far larger than any count: as many as there are numbers for, not one past it.
            (((avail - size + 1e-9) / step).floor() as usize).saturating_add(1)
        }
    };
    (fit(target.height(), item_bbox.height(), step.y), fit(target.width(), item_bbox.width(), step.x))
}

/// Options for [`circular_array`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CircularOptions {
    pub center: Point,
    pub count: usize,
    /// Angle of the first copy relative to the original item, degrees.
    pub start_angle_deg: f64,
    /// Total sweep, degrees. A full circle (|sweep| ≥ 360) spaces copies evenly with no
    /// copy on top of the first; otherwise the copies span both ends of the sweep.
    pub sweep_deg: f64,
    /// Turn each copy with its angle (otherwise copies only move).
    pub rotate_items: bool,
    /// Go clockwise instead of counter-clockwise.
    pub clockwise: bool,
}

/// Circular (radial) array of an item whose centre is `item_center`. Copy `i` sits at
/// `start + i * step` around `center`; with a start angle of 0 copy 0 is the identity. At most
/// [`MAX_COPIES`]` + 1` transforms are returned.
pub fn circular_array(item_center: Point, o: &CircularOptions) -> Vec<Affine> {
    if o.count == 0 {
        return Vec::new();
    }
    let full = o.sweep_deg.abs() >= 360.0 - 1e-9;
    let step = if full {
        o.sweep_deg / o.count as f64
    } else if o.count > 1 {
        o.sweep_deg / (o.count - 1) as f64
    } else {
        0.0
    };
    let sign = if o.clockwise { -1.0 } else { 1.0 };
    (0..o.count.min(MAX_COPIES + 1))
        .map(|i| {
            let a = sign * (o.start_angle_deg + step * i as f64).to_radians();
            let rot = Affine::rotate_about(a, o.center);
            if o.rotate_items { rot } else { Affine::translate(rot * item_center - item_center) }
        })
        .collect()
}

/// How copies are spread along the path.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AlongMode {
    /// This many copies spread over the whole length.
    Count(usize),
    /// A copy every this many mm.
    Spacing(f64),
}

/// Options for [`copy_along_path`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AlongOptions {
    pub mode: AlongMode,
    /// Turn copies to follow the path direction (relative to the direction at the first copy).
    pub rotate_items: bool,
    /// Start at the point of the path nearest the item anchor instead of the path start.
    pub start_at_nearest: bool,
}

struct Walker {
    segs: Vec<kurbo::PathSeg>,
    cum: Vec<f64>,
    total: f64,
    closed: bool,
}

impl Walker {
    fn at(&self, s: f64) -> (Point, f64) {
        let s = s.clamp(0.0, self.total);
        let i = match self.cum.binary_search_by(|c| c.total_cmp(&s)) {
            Ok(i) => i.min(self.segs.len() - 1),
            Err(i) => i.saturating_sub(1).min(self.segs.len() - 1),
        };
        let seg = &self.segs[i];
        let len = self.cum[i + 1] - self.cum[i];
        let t = if len > 0.0 { seg.inv_arclen((s - self.cum[i]).min(len), DEFAULT_ACCURACY) } else { 0.0 };
        (seg.eval(t), seg_tangent(seg, t).atan2())
    }
}

/// Copies an item along the first subpath of `path`. `anchor` is the item point placed on
/// the path (usually its bounding-box centre). On closed paths there is no copy on top of
/// the first one at the end. At most [`MAX_COPIES`]` + 1` transforms are returned.
pub fn copy_along_path(anchor: Point, path: &BezPath, o: &AlongOptions) -> Vec<Affine> {
    let Some(c) = contours(path).into_iter().next() else { return Vec::new() };
    let mut cum = vec![0.0];
    for s in &c.segs {
        let l = cum[cum.len() - 1] + s.arclen(DEFAULT_ACCURACY);
        cum.push(l);
    }
    let total = cum[cum.len() - 1];
    // A path so large that its length is not a number has no places along it.
    if !total.is_finite() {
        return Vec::new();
    }
    let closed = c.is_closed_within(1e-9);
    let mut s0 = 0.0;
    if o.start_at_nearest
        && let Some((sp, _)) = nearest_on_contour(&c, anchor)
    {
        let seg = &c.segs[sp.seg];
        s0 = cum[sp.seg] + seg.subsegment(0.0..sp.t).arclen(DEFAULT_ACCURACY);
    }
    let w = Walker { segs: c.segs, cum, total, closed };
    let avail = if w.closed { w.total } else { w.total - s0 };
    let eps = 1e-9;
    let offsets: Vec<f64> = match o.mode {
        AlongMode::Count(0) => Vec::new(),
        AlongMode::Count(n) => {
            let step = if w.closed {
                avail / n as f64
            } else if n > 1 {
                avail / (n - 1) as f64
            } else {
                0.0
            };
            (0..n.min(MAX_COPIES + 1)).map(|i| step * i as f64).collect()
        }
        AlongMode::Spacing(d) if d > eps => {
            let mut v = Vec::new();
            let mut k = 0.0;
            while k <= avail + eps && !(w.closed && k > avail - eps && !v.is_empty()) && v.len() <= MAX_COPIES {
                v.push(k);
                k += d;
            }
            v
        }
        AlongMode::Spacing(_) => vec![0.0],
    };
    let mut first_angle = None;
    offsets
        .iter()
        .map(|off| {
            let s = if w.closed && w.total > 0.0 { (s0 + off).rem_euclid(w.total) } else { s0 + off };
            let (p, ang) = w.at(s);
            let a0 = *first_angle.get_or_insert(ang);
            let mv = Affine::translate(p - anchor);
            if o.rotate_items { Affine::rotate_about(ang - a0, p) * mv } else { mv }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::{Circle, Shape};

    fn close(a: Point, b: Point) -> bool {
        a.distance(b) < 1e-6
    }

    #[test]
    fn a_mistyped_count_or_spacing_cannot_fill_the_memory() {
        let item = Rect::new(0.0, 0.0, 10.0, 5.0);
        let huge = GridOptions { rows: usize::MAX, cols: usize::MAX, ..Default::default() };
        assert_eq!(grid_array(item, &huge).len(), MAX_COPIES + 1);
        let wide = GridOptions { rows: 2, cols: 100_000_000_000, ..Default::default() };
        assert_eq!(grid_array(item, &wide).len(), MAX_COPIES + 1);
        let c =
            CircularOptions { center: Point::ZERO, count: usize::MAX, start_angle_deg: 0.0, sweep_deg: 360.0, rotate_items: true, clockwise: false };
        assert_eq!(circular_array(Point::new(10.0, 0.0), &c).len(), MAX_COPIES + 1);
        let mut line = BezPath::new();
        line.move_to((0.0, 0.0));
        line.line_to((1000.0, 0.0));
        let close = AlongOptions { mode: AlongMode::Spacing(1e-6), rotate_items: false, start_at_nearest: false };
        assert_eq!(copy_along_path(Point::ZERO, &line, &close).len(), MAX_COPIES + 1);
        let many = AlongOptions { mode: AlongMode::Count(usize::MAX), rotate_items: false, start_at_nearest: false };
        assert_eq!(copy_along_path(Point::ZERO, &line, &many).len(), MAX_COPIES + 1);
        // Arrays within the limit are whole.
        let ok = GridOptions { rows: 100, cols: 1000, ..Default::default() };
        assert_eq!(grid_array(item, &ok).len(), MAX_COPIES);
    }

    #[test]
    fn grid_gap_vs_center() {
        let item = Rect::new(0.0, 0.0, 10.0, 5.0);
        let o = GridOptions { rows: 2, cols: 3, spacing: Spacing::Gap(Vec2::new(2.0, 1.0)), ..Default::default() };
        let t = grid_array(item, &o);
        assert_eq!(t.len(), 6);
        assert_eq!(t[0], Affine::IDENTITY);
        assert!(close(t[2] * Point::ZERO, Point::new(24.0, 0.0)));
        assert!(close(t[3] * Point::ZERO, Point::new(0.0, 6.0)));
        let o2 = GridOptions { spacing: Spacing::CenterToCenter(Vec2::new(20.0, 30.0)), ..o };
        let t2 = grid_array(item, &o2);
        assert!(close(t2[5] * Point::ZERO, Point::new(40.0, 30.0)));
        assert!(grid_array(item, &GridOptions { rows: 0, ..o }).is_empty());
    }

    #[test]
    fn grid_shifts_and_hex() {
        let item = Rect::new(0.0, 0.0, 10.0, 10.0);
        let base = GridOptions { rows: 2, cols: 2, ..Default::default() };
        let t = grid_array(item, &GridOptions { shift: Shift::AltRows(3.0), ..base });
        assert!(close(t[2] * Point::ZERO, Point::new(3.0, 10.0)));
        let t = grid_array(item, &GridOptions { shift: Shift::AltCols(4.0), ..base });
        assert!(close(t[1] * Point::ZERO, Point::new(10.0, 4.0)));
        let t = grid_array(item, &GridOptions { shift: Shift::HexRows, ..base });
        assert!(close(t[2] * Point::ZERO, Point::new(5.0, 10.0 * 3f64.sqrt() / 2.0)));
        // Hex neighbours are equally far apart.
        let d01 = (t[1] * Point::ZERO).distance(t[0] * Point::ZERO);
        let d02 = (t[2] * Point::ZERO).distance(t[0] * Point::ZERO);
        assert!((d01 - d02).abs() < 1e-9);
    }

    #[test]
    fn grid_mirror_and_reverse() {
        let item = Rect::new(0.0, 0.0, 10.0, 4.0);
        let o = GridOptions { rows: 2, cols: 2, mirror_alt_rows: true, mirror_alt_cols: true, ..Default::default() };
        let t = grid_array(item, &o);
        assert!(t[0].determinant() > 0.0);
        assert!(t[1].determinant() < 0.0);
        assert!(t[2].determinant() < 0.0);
        assert!(t[3].determinant() > 0.0);
        // Mirrored copies keep their slot.
        let bb = t[1].transform_rect_bbox(item);
        assert!(close(bb.origin(), Point::new(10.0, 0.0)));
        let r = grid_array(item, &GridOptions { reverse_x: true, reverse_y: true, ..o });
        assert!(r[3].transform_rect_bbox(item).x0 < 0.0 && r[3].transform_rect_bbox(item).y0 < 0.0);
    }

    #[test]
    fn grid_fit_counts() {
        let item = Rect::new(0.0, 0.0, 10.0, 10.0);
        assert_eq!(grid_fit(item, Rect::new(0.0, 0.0, 100.0, 35.0), Spacing::Gap(Vec2::new(0.0, 2.0))), (3, 10));
        assert_eq!(grid_fit(item, Rect::new(0.0, 0.0, 5.0, 50.0), Spacing::Gap(Vec2::ZERO)), (5, 0));
        assert_eq!(grid_fit(item, Rect::new(0.0, 0.0, 30.0, 30.0), Spacing::CenterToCenter(Vec2::ZERO)), (1, 1));
    }

    #[test]
    fn circular_full_and_partial() {
        let item = Point::new(10.0, 0.0);
        let o = CircularOptions { center: Point::ZERO, count: 4, start_angle_deg: 0.0, sweep_deg: 360.0, rotate_items: false, clockwise: false };
        let t = circular_array(item, &o);
        assert_eq!(t.len(), 4);
        assert!(close(t[0] * item, item));
        assert!(close(t[1] * item, Point::new(0.0, 10.0)));
        assert!(close(t[3] * item, Point::new(0.0, -10.0)));
        // Without rotation a copy only translates.
        assert!((t[1].as_coeffs()[0] - 1.0).abs() < 1e-12);
        let half = circular_array(item, &CircularOptions { count: 3, sweep_deg: 180.0, ..o });
        assert!(close(half[2] * item, Point::new(-10.0, 0.0)));
        let cw = circular_array(item, &CircularOptions { clockwise: true, ..o });
        assert!(close(cw[1] * item, Point::new(0.0, -10.0)));
        let rot = circular_array(item, &CircularOptions { rotate_items: true, start_angle_deg: 90.0, ..o });
        assert!(close(rot[0] * item, Point::new(0.0, 10.0)));
        assert!(close(rot[0] * Point::new(11.0, 0.0), Point::new(0.0, 11.0)));
        assert!(circular_array(item, &CircularOptions { count: 0, ..o }).is_empty());
        assert_eq!(circular_array(item, &CircularOptions { count: 1, sweep_deg: 90.0, ..o }).len(), 1);
    }

    fn line(a: (f64, f64), b: (f64, f64)) -> BezPath {
        let mut p = BezPath::new();
        p.move_to(a);
        p.line_to(b);
        p
    }

    #[test]
    fn along_open_line_count_and_spacing() {
        let l = line((0.0, 0.0), (100.0, 0.0));
        let o = AlongOptions { mode: AlongMode::Count(5), rotate_items: false, start_at_nearest: false };
        let t = copy_along_path(Point::new(0.0, 0.0), &l, &o);
        assert_eq!(t.len(), 5);
        assert!(close(t[4] * Point::ZERO, Point::new(100.0, 0.0)));
        assert!(close(t[1] * Point::ZERO, Point::new(25.0, 0.0)));
        let s = copy_along_path(Point::ZERO, &l, &AlongOptions { mode: AlongMode::Spacing(30.0), ..o });
        assert_eq!(s.len(), 4);
        assert!(close(s[3] * Point::ZERO, Point::new(90.0, 0.0)));
        let n = copy_along_path(Point::new(40.0, 5.0), &l, &AlongOptions { start_at_nearest: true, ..o });
        assert!(close(n[0] * Point::new(40.0, 5.0), Point::new(40.0, 0.0)));
        assert!(close(n[4] * Point::new(40.0, 5.0), Point::new(100.0, 0.0)));
        assert!(copy_along_path(Point::ZERO, &BezPath::new(), &o).is_empty());
    }

    #[test]
    fn along_closed_circle_rotates() {
        let c = Circle::new((0.0, 0.0), 10.0).to_path(1e-6);
        let start = crate::path::start_point(&c).unwrap();
        let o = AlongOptions { mode: AlongMode::Count(4), rotate_items: true, start_at_nearest: false };
        let t = copy_along_path(start, &c, &o);
        assert_eq!(t.len(), 4);
        let pts: Vec<Point> = t.iter().map(|a| *a * start).collect();
        for (i, p) in pts.iter().enumerate() {
            assert!((p.to_vec2().hypot() - 10.0).abs() < 1e-3);
            for q in &pts[i + 1..] {
                assert!(p.distance(*q) > 10.0);
            }
        }
        // Rotation follows the tangent: a point just outside the circle stays outside by the same amount.
        let out = start + (start.to_vec2().normalize() * 2.0);
        for a in &t {
            assert!(((*a * out).to_vec2().hypot() - 12.0).abs() < 1e-3);
        }
        let sp = copy_along_path(start, &c, &AlongOptions { mode: AlongMode::Spacing(crate::path::length(&c) / 8.0), ..o });
        assert_eq!(sp.len(), 8);
    }
}
