//! Taper warp: reshapes flat artwork so it comes out right when it is wrapped round a tapered
//! cup or cone turned in a rotary.
//!
//! Two modes:
//! - [`TaperMode::Simple`] squeezes the artwork row by row towards the narrow end, about its
//!   vertical centre line. Straight rows stay straight.
//! - [`TaperMode::Cone`] lays the artwork out on the unrolled cone: vertical lines lean towards
//!   the apex and horizontal lines become arcs round it.
//!
//! The wide end keeps the artwork's width. All sizes are in mm, Y up.

use crate::transform::map_path;
use kurbo::{BezPath, Point, Rect, Vec2};
use std::f64::consts::{PI, TAU};

/// How the artwork is reshaped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaperMode {
    /// Squeeze each row towards the narrow end about the centre line.
    Simple,
    /// Unroll the cone: rows become arcs, columns lean towards the apex.
    Cone,
}

/// The size of the reshaped artwork: the length of its top and bottom edge (arc lengths in cone
/// mode) and its height (measured along the side of the object in cone mode).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Taper {
    pub top: f64,
    pub bottom: f64,
    pub height: f64,
}

/// Why a taper warp cannot be made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaperError {
    /// A size is zero, negative or not a number.
    BadSize,
    /// The artwork has no width or no height.
    EmptyArtwork,
    /// The artwork does not fit on the object at the chosen place.
    OffObject,
    /// The artwork would go more than once round the object (or, in cone mode with plain
    /// widths, round the unrolled cone).
    WrapsAround,
}

impl std::fmt::Display for TaperError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            TaperError::BadSize => "sizes must be greater than zero",
            TaperError::EmptyArtwork => "the artwork needs both width and height",
            TaperError::OffObject => "the artwork does not fit on the object at that place",
            TaperError::WrapsAround => "the artwork would go more than once round the cone",
        })
    }
}

impl std::error::Error for TaperError {}

fn positive(v: f64) -> bool {
    v.is_finite() && v > 0.0
}

impl Taper {
    /// The taper for artwork `art` placed on an object with the given top and bottom diameter
    /// and height (straight up). `from_top` is the distance along the side from the object's
    /// top edge to the artwork's top edge; `None` centres the artwork on the side.
    ///
    /// The artwork's height is taken as a distance along the side. Its wide end keeps the
    /// artwork's width; the other end shrinks in step with the diameter there. Artwork wider
    /// than the object's circumference at its wide end would overlap itself and is refused.
    pub fn from_object(art: Rect, top_d: f64, bottom_d: f64, height: f64, from_top: Option<f64>) -> Result<Taper, TaperError> {
        if !(positive(top_d) && positive(bottom_d) && positive(height)) {
            return Err(TaperError::BadSize);
        }
        let (w, h) = (art.width(), art.height());
        if !(positive(w) && positive(h)) || w < 1e-9 || h < 1e-9 {
            return Err(TaperError::EmptyArtwork);
        }
        let side = height.hypot((bottom_d - top_d) / 2.0);
        let start = match from_top {
            Some(s) if !s.is_finite() || s < 0.0 => return Err(TaperError::OffObject),
            Some(s) => s,
            None => (side - h) / 2.0,
        };
        if start < -1e-9 || start + h > side + 1e-6 {
            return Err(TaperError::OffObject);
        }
        let start = start.max(0.0);
        let end = (start + h).min(side);
        let d = |s: f64| top_d + (bottom_d - top_d) * s / side;
        let (dt, db) = (d(start), d(end));
        let wide = dt.max(db);
        // The wide edge keeps the artwork's width and must fit once round the object there;
        // the narrow edge shrinks in step with its diameter, so it fits as well.
        if w > PI * wide * (1.0 + 1e-12) {
            return Err(TaperError::WrapsAround);
        }
        Ok(Taper { top: w * dt / wide, bottom: w * db / wide, height: h })
    }

    fn check(&self) -> Result<(), TaperError> {
        if positive(self.top) && positive(self.bottom) && positive(self.height) { Ok(()) } else { Err(TaperError::BadSize) }
    }
}

#[derive(Clone, Copy, Debug)]
enum Kind {
    Simple,
    Cone {
        apex: Point,
        /// +1 when the apex is below the artwork (top is the wide end), -1 when above.
        up: f64,
        rho_bottom: f64,
        /// Radians per mm of source width.
        per_mm: f64,
    },
}

/// A ready-made taper warp from a source box to its reshaped form.
#[derive(Clone, Copy, Debug)]
pub struct TaperWarp {
    src: Rect,
    taper: Taper,
    kind: Kind,
}

impl TaperWarp {
    /// Builds the warp that reshapes artwork in the box `src`. In cone mode equal top and
    /// bottom sizes fall back to the simple mode (a cylinder unrolls to a rectangle).
    pub fn new(src: Rect, taper: Taper, mode: TaperMode) -> Result<TaperWarp, TaperError> {
        taper.check()?;
        let (w, h) = (src.width(), src.height());
        if !(w.is_finite() && h.is_finite()) || w < 1e-9 || h < 1e-9 {
            return Err(TaperError::EmptyArtwork);
        }
        let wide = taper.top.max(taper.bottom);
        let narrow = taper.top.min(taper.bottom);
        let kind = if mode == TaperMode::Simple || wide - narrow <= 1e-9 * wide {
            Kind::Simple
        } else {
            let rho_n = taper.height * narrow / (wide - narrow);
            let rho_w = rho_n + taper.height;
            if wide / rho_w > TAU + 1e-9 {
                return Err(TaperError::WrapsAround);
            }
            let up = if taper.top >= taper.bottom { 1.0 } else { -1.0 };
            let rho_bottom = if up > 0.0 { rho_n } else { rho_w };
            let c = src.center();
            let apex = Point::new(c.x, c.y - up * (rho_n + rho_w) / 2.0);
            Kind::Cone { apex, up, rho_bottom, per_mm: (wide / w) / rho_w }
        };
        Ok(TaperWarp { src, taper, kind })
    }

    /// The size the artwork is given.
    pub fn taper(&self) -> Taper {
        self.taper
    }

    /// True when the warp leaves every point where it is.
    pub fn is_identity(&self) -> bool {
        let w = self.src.width();
        let h = self.src.height();
        let close = |a: f64, b: f64| (a - b).abs() <= 1e-9 * b.abs().max(1.0);
        close(self.taper.top, w) && close(self.taper.bottom, w) && close(self.taper.height, h)
    }

    /// In cone mode: the apex, and the distance from it to the artwork's bottom and top edge.
    pub fn cone(&self) -> Option<(Point, f64, f64)> {
        match self.kind {
            Kind::Simple => None,
            Kind::Cone { apex, up, rho_bottom, .. } => Some((apex, rho_bottom, rho_bottom + up * self.taper.height)),
        }
    }

    /// Where the warp sends the source point `p`.
    pub fn apply(&self, p: Point) -> Point {
        let s = self.src;
        let c = s.center();
        let v = (p.y - s.y0) / s.height();
        match self.kind {
            Kind::Simple => {
                let width = self.taper.bottom + (self.taper.top - self.taper.bottom) * v;
                Point::new(c.x + (p.x - c.x) * width / s.width(), c.y + (p.y - c.y) * self.taper.height / s.height())
            }
            Kind::Cone { apex, up, rho_bottom, per_mm } => {
                let rho = rho_bottom + up * v * self.taper.height;
                let a = (p.x - c.x) * per_mm;
                apex + Vec2::new(a.sin(), up * a.cos()) * rho
            }
        }
    }

    /// The source point the warp sends to `q`, if there is one.
    pub fn invert(&self, q: Point) -> Option<Point> {
        let s = self.src;
        let c = s.center();
        match self.kind {
            Kind::Simple => {
                let y = c.y + (q.y - c.y) * s.height() / self.taper.height;
                let v = (y - s.y0) / s.height();
                let width = self.taper.bottom + (self.taper.top - self.taper.bottom) * v;
                (width > 1e-12).then(|| Point::new(c.x + (q.x - c.x) * s.width() / width, y))
            }
            Kind::Cone { apex, up, rho_bottom, per_mm } => {
                let d = q - apex;
                let rho = d.hypot();
                if rho < 1e-12 {
                    return None;
                }
                let a = d.x.atan2(up * d.y);
                let v = (rho - rho_bottom) * up / self.taper.height;
                Some(Point::new(c.x + a / per_mm, s.y0 + v * s.height()))
            }
        }
    }

    /// The most the warp stretches any short piece of line (at least 1).
    fn stretch(&self) -> f64 {
        let (w, h) = (self.src.width(), self.src.height());
        let t = self.taper;
        let wide = t.top.max(t.bottom);
        (1.0f64).max(t.height / h + wide / w + (t.top - t.bottom).abs() / h)
    }

    /// Warps a whole path. Curves are followed within `tol` (mm) in the result; lines that bend
    /// are split as finely as needed.
    pub fn warp_path(&self, path: &BezPath, tol: f64) -> BezPath {
        if self.is_identity() {
            return path.clone();
        }
        map_path(path, tol / self.stretch(), |p| self.apply(p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::nearest_point;
    use kurbo::{Circle, Line, Shape};

    fn near(a: Point, b: Point, eps: f64) -> bool {
        (a - b).hypot() <= eps
    }

    fn vertices(p: &BezPath) -> Vec<Point> {
        p.elements().iter().filter_map(|e| e.end_point()).collect()
    }

    #[test]
    fn simple_mode_turns_a_square_into_the_expected_trapezium() {
        let src = Rect::new(0.0, 0.0, 10.0, 10.0);
        let w = TaperWarp::new(src, Taper { top: 10.0, bottom: 6.0, height: 10.0 }, TaperMode::Simple).unwrap();
        assert!(near(w.apply(Point::new(0.0, 0.0)), Point::new(2.0, 0.0), 1e-12));
        assert!(near(w.apply(Point::new(10.0, 0.0)), Point::new(8.0, 0.0), 1e-12));
        assert!(near(w.apply(Point::new(10.0, 10.0)), Point::new(10.0, 10.0), 1e-12));
        assert!(near(w.apply(Point::new(0.0, 10.0)), Point::new(0.0, 10.0), 1e-12));
        // The centre line stays put and rows stay straight.
        assert!(near(w.apply(Point::new(5.0, 3.0)), Point::new(5.0, 3.0), 1e-12));
        assert!((w.apply(Point::new(1.0, 5.0)).y - 5.0).abs() < 1e-12);
        let out = w.warp_path(&src.to_path(0.01), 0.01);
        let pts = vertices(&out);
        for c in [Point::new(2.0, 0.0), Point::new(8.0, 0.0), Point::new(10.0, 10.0), Point::new(0.0, 10.0)] {
            assert!(pts.iter().any(|p| near(*p, c, 1e-9)), "corner {c:?} missing from {pts:?}");
        }
        // A trapezium's sides are straight, so no extra points are needed.
        assert!(pts.len() <= 5, "{pts:?}");
    }

    #[test]
    fn simple_mode_changes_height_about_the_centre() {
        let src = Rect::new(0.0, 0.0, 20.0, 10.0);
        let w = TaperWarp::new(src, Taper { top: 12.0, bottom: 20.0, height: 8.0 }, TaperMode::Simple).unwrap();
        assert!(near(w.apply(Point::new(0.0, 10.0)), Point::new(4.0, 9.0), 1e-12));
        assert!(near(w.apply(Point::new(0.0, 0.0)), Point::new(0.0, 1.0), 1e-12));
    }

    #[test]
    fn cone_mode_bends_a_horizontal_line_into_an_arc_of_the_expected_radius() {
        let src = Rect::new(0.0, 0.0, 20.0, 10.0);
        let w = TaperWarp::new(src, Taper { top: 20.0, bottom: 10.0, height: 10.0 }, TaperMode::Cone).unwrap();
        let (apex, rho_bottom, rho_top) = w.cone().unwrap();
        // Narrow edge 10 and wide edge 20, 10 apart: the cone's apex is 10 below the narrow edge.
        assert!((rho_bottom - 10.0).abs() < 1e-12 && (rho_top - 20.0).abs() < 1e-12);
        assert!(near(apex, Point::new(10.0, -10.0), 1e-12));
        let tol = 0.01;
        let top = w.warp_path(&Line::new((0.0, 10.0), (20.0, 10.0)).to_path(tol), tol);
        let pts = vertices(&top);
        assert!(pts.len() > 10, "the arc is split into many pieces");
        for p in &pts {
            assert!(((*p - apex).hypot() - 20.0).abs() < 1e-9, "{p:?} is not on the radius 20 arc");
        }
        // Every piece stays within the tolerance of the arc.
        for pair in pts.windows(2) {
            let m = pair[0].midpoint(pair[1]);
            assert!(20.0 - (m - apex).hypot() <= tol + 1e-9);
        }
        // The arc keeps the wide edge's length, the narrow edge shrinks to its own length.
        let len: f64 = pts.windows(2).map(|p| (p[1] - p[0]).hypot()).sum();
        assert!((len - 20.0).abs() < 0.05, "top arc length {len}");
        let bottom = vertices(&w.warp_path(&Line::new((0.0, 0.0), (20.0, 0.0)).to_path(tol), tol));
        let len: f64 = bottom.windows(2).map(|p| (p[1] - p[0]).hypot()).sum();
        assert!((len - 10.0).abs() < 0.05, "bottom arc length {len}");
        assert!(bottom.iter().all(|p| ((*p - apex).hypot() - 10.0).abs() < 1e-9));
        // The middle of the top edge stays where it was.
        assert!(near(w.apply(Point::new(10.0, 10.0)), Point::new(10.0, 10.0), 1e-12));
    }

    #[test]
    fn cone_mode_leans_vertical_lines_towards_the_apex() {
        let src = Rect::new(0.0, 0.0, 20.0, 10.0);
        let w = TaperWarp::new(src, Taper { top: 20.0, bottom: 10.0, height: 10.0 }, TaperMode::Cone).unwrap();
        let (apex, _, _) = w.cone().unwrap();
        for x in [0.0, 4.0, 20.0] {
            let a = w.apply(Point::new(x, 0.0)) - apex;
            let b = w.apply(Point::new(x, 10.0)) - apex;
            assert!(a.cross(b).abs() < 1e-9, "column {x} does not point at the apex");
            assert!(((b.hypot() - a.hypot()) - 10.0).abs() < 1e-9);
        }
    }

    #[test]
    fn cone_mode_with_the_wide_end_at_the_bottom_puts_the_apex_above() {
        let src = Rect::new(0.0, 0.0, 20.0, 10.0);
        let w = TaperWarp::new(src, Taper { top: 10.0, bottom: 20.0, height: 10.0 }, TaperMode::Cone).unwrap();
        let (apex, rho_bottom, rho_top) = w.cone().unwrap();
        assert!(near(apex, Point::new(10.0, 20.0), 1e-12));
        assert!((rho_bottom - 20.0).abs() < 1e-12 && (rho_top - 10.0).abs() < 1e-12);
        assert!(near(w.apply(Point::new(10.0, 0.0)), Point::new(10.0, 0.0), 1e-12));
        assert!(w.apply(Point::new(0.0, 0.0)).y > 0.0, "the bottom corners curl up towards the apex");
    }

    #[test]
    fn points_round_trip_in_both_modes() {
        let src = Rect::new(-3.0, 2.0, 37.0, 27.0);
        for (mode, t) in [
            (TaperMode::Simple, Taper { top: 40.0, bottom: 29.0, height: 25.0 }),
            (TaperMode::Simple, Taper { top: 21.0, bottom: 40.0, height: 31.0 }),
            (TaperMode::Cone, Taper { top: 40.0, bottom: 29.0, height: 25.0 }),
            (TaperMode::Cone, Taper { top: 21.0, bottom: 40.0, height: 31.0 }),
        ] {
            let w = TaperWarp::new(src, t, mode).unwrap();
            for i in 0..=10 {
                for j in 0..=10 {
                    let p = Point::new(src.x0 + src.width() * i as f64 / 10.0, src.y0 + src.height() * j as f64 / 10.0);
                    let back = w.invert(w.apply(p)).unwrap();
                    assert!(near(back, p, 1e-9), "{mode:?}: {p:?} came back as {back:?}");
                }
            }
        }
    }

    #[test]
    fn curves_stay_within_the_tolerance() {
        let circle = Circle::new((30.0, 20.0), 15.0).to_path(1e-4);
        let src = circle.bounding_box();
        for mode in [TaperMode::Simple, TaperMode::Cone] {
            let w = TaperWarp::new(src, Taper { top: 30.0, bottom: 18.0, height: 30.0 }, mode).unwrap();
            let tol = 0.01;
            let out = w.warp_path(&circle, tol);
            // Points on the true circle land on the warped outline...
            for k in 0..360 {
                let a = (k as f64).to_radians();
                let q = w.apply(Point::new(30.0 + 15.0 * a.cos(), 20.0 + 15.0 * a.sin()));
                let d = nearest_point(&out, q).unwrap().distance;
                assert!(d <= tol, "{mode:?}: {d} off at {k} degrees");
            }
            // ...and every point of the warped outline comes from the circle.
            for p in vertices(&out) {
                let back = w.invert(p).unwrap();
                assert!(((back - Point::new(30.0, 20.0)).hypot() - 15.0).abs() <= tol, "{mode:?}: {back:?}");
            }
        }
    }

    #[test]
    fn equal_diameters_change_nothing() {
        let art = Rect::new(5.0, 5.0, 45.0, 25.0);
        let t = Taper::from_object(art, 80.0, 80.0, 100.0, None).unwrap();
        assert_eq!(t, Taper { top: 40.0, bottom: 40.0, height: 20.0 });
        for mode in [TaperMode::Simple, TaperMode::Cone] {
            let w = TaperWarp::new(art, t, mode).unwrap();
            assert!(w.is_identity());
            let p = art.to_path(0.01);
            assert_eq!(w.warp_path(&p, 0.01), p);
            assert!(near(w.apply(Point::new(7.0, 9.0)), Point::new(7.0, 9.0), 1e-12));
        }
    }

    #[test]
    fn diameters_give_widths_in_step_with_the_place_on_the_object() {
        // A cup 100 tall, 80 across the top and 60 across the bottom: side length 100.5.
        let art = Rect::new(0.0, 0.0, 40.0, 20.0);
        let side = 100.0f64.hypot(10.0);
        let t = Taper::from_object(art, 80.0, 60.0, 100.0, Some(0.0)).unwrap();
        let d_low = 80.0 - 20.0 * 20.0 / side;
        assert!((t.top - 40.0).abs() < 1e-12);
        assert!((t.bottom - 40.0 * d_low / 80.0).abs() < 1e-12);
        assert_eq!(t.height, 20.0);
        // Centred, the artwork sits halfway down the side.
        let t = Taper::from_object(art, 80.0, 60.0, 100.0, None).unwrap();
        let s0 = (side - 20.0) / 2.0;
        let d = |s: f64| 80.0 - 20.0 * s / side;
        assert!((t.bottom / t.top - d(s0 + 20.0) / d(s0)).abs() < 1e-12);
        // Upside down: the bottom is wide and keeps the width.
        let t = Taper::from_object(art, 60.0, 80.0, 100.0, None).unwrap();
        assert!((t.bottom - 40.0).abs() < 1e-12 && t.top < 40.0);
    }

    #[test]
    fn diameters_fit_the_cone_exactly() {
        // On a real cone the unrolled rows are arcs round its apex, so the warp's radii must
        // match the distance from the apex along the side.
        let art = Rect::new(0.0, 0.0, 30.0, 20.0);
        let t = Taper::from_object(art, 90.0, 50.0, 80.0, Some(10.0)).unwrap();
        let w = TaperWarp::new(art, t, TaperMode::Cone).unwrap();
        let (_, rho_bottom, rho_top) = w.cone().unwrap();
        let side = 80.0f64.hypot(20.0);
        // Distance from the apex along the side to a place with diameter d.
        let from_apex = |d: f64| d / 2.0 * side / 20.0;
        let d = |s: f64| 90.0 - 40.0 * s / side;
        assert!((rho_top - from_apex(d(10.0))).abs() < 1e-9);
        assert!((rho_bottom - from_apex(d(30.0))).abs() < 1e-9);
    }

    #[test]
    fn zero_or_negative_sizes_are_refused() {
        let art = Rect::new(0.0, 0.0, 10.0, 10.0);
        for (t, b, h) in [(0.0, 50.0, 50.0), (50.0, -1.0, 50.0), (50.0, 40.0, 0.0), (f64::NAN, 40.0, 50.0)] {
            assert_eq!(Taper::from_object(art, t, b, h, None), Err(TaperError::BadSize));
            assert_eq!(TaperWarp::new(art, Taper { top: t, bottom: b, height: h }, TaperMode::Simple).err(), Some(TaperError::BadSize));
        }
        let flat = Rect::new(0.0, 0.0, 10.0, 0.0);
        assert_eq!(Taper::from_object(flat, 50.0, 40.0, 50.0, None), Err(TaperError::EmptyArtwork));
        assert_eq!(TaperWarp::new(flat, Taper { top: 5.0, bottom: 4.0, height: 1.0 }, TaperMode::Cone).err(), Some(TaperError::EmptyArtwork));
    }

    #[test]
    fn artwork_that_does_not_fit_on_the_object_is_refused() {
        let art = Rect::new(0.0, 0.0, 10.0, 30.0);
        assert_eq!(Taper::from_object(art, 50.0, 40.0, 20.0, None), Err(TaperError::OffObject));
        assert_eq!(Taper::from_object(art, 50.0, 40.0, 50.0, Some(25.0)), Err(TaperError::OffObject));
        assert_eq!(Taper::from_object(art, 50.0, 40.0, 50.0, Some(-1.0)), Err(TaperError::OffObject));
        assert!(Taper::from_object(art, 50.0, 40.0, 50.0, Some(20.0)).is_ok());
    }

    #[test]
    fn artwork_wider_than_the_object_is_round_is_refused() {
        // A cup 80 across the top, 60 across the bottom and 100 tall: 251.3 mm round the top.
        // The unrolled sector of such a cup is far narrower than a full turn, so the sector
        // check alone would let artwork ten times round the cup through.
        let round = PI * 80.0;
        let wide = |w: f64| Rect::new(0.0, 0.0, w, 20.0);
        for w in [2500.0, 300.0, round + 0.01] {
            assert_eq!(Taper::from_object(wide(w), 80.0, 60.0, 100.0, Some(0.0)), Err(TaperError::WrapsAround), "{w} mm wide");
        }
        let t = Taper::from_object(wide(round - 0.01), 80.0, 60.0, 100.0, Some(0.0)).unwrap();
        assert!(TaperWarp::new(wide(round - 0.01), t, TaperMode::Cone).is_ok());
        // Lower down the cup is narrower, so less fits round it.
        let side = 100.0f64.hypot(10.0);
        let low = 80.0 - 20.0 * 60.0 / side;
        assert_eq!(Taper::from_object(wide(PI * low + 0.5), 80.0, 60.0, 100.0, Some(60.0)), Err(TaperError::WrapsAround));
        assert!(Taper::from_object(wide(PI * low - 0.5), 80.0, 60.0, 100.0, Some(60.0)).is_ok());
        // Upside down the wide end is at the bottom, and it is the one that counts.
        assert!(Taper::from_object(wide(PI * 80.0 - 0.01), 60.0, 80.0, 100.0, Some(side - 20.0)).is_ok());
        assert_eq!(Taper::from_object(wide(PI * 80.0 + 0.01), 60.0, 80.0, 100.0, Some(side - 20.0)), Err(TaperError::WrapsAround));
        // A cylinder too: the simple mode cannot wrap artwork more than once round either.
        assert_eq!(Taper::from_object(wide(PI * 50.0 + 0.01), 50.0, 50.0, 100.0, None), Err(TaperError::WrapsAround));
    }

    #[test]
    fn artwork_going_more_than_once_round_the_cone_is_refused() {
        let src = Rect::new(0.0, 0.0, 100.0, 1.0);
        let t = Taper { top: 100.0, bottom: 10.0, height: 1.0 };
        assert_eq!(TaperWarp::new(src, t, TaperMode::Cone).err(), Some(TaperError::WrapsAround));
        // The simple mode has no such limit.
        assert!(TaperWarp::new(src, t, TaperMode::Simple).is_ok());
    }
}
