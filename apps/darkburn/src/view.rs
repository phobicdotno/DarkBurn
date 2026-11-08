//! The canvas view: where the work area sits on screen and how far it is zoomed.
//!
//! The page is measured in millimetres from its top-left corner, `u` to the right and `v` down,
//! like the screen. What the user reads (rulers, the status bar) are machine coordinates, which
//! start at the device's origin corner; [`Origin`] turns one into the other.

#[cfg(test)]
use egui::vec2;
use egui::{Pos2, Rect, Vec2, pos2};
use serde::{Deserialize, Serialize};

/// The smallest and largest zoom, in screen points per millimetre.
pub const MIN_ZOOM: f32 = 0.02;
pub const MAX_ZOOM: f32 = 400.0;
/// One wheel notch or zoom key multiplies the zoom by this.
pub const ZOOM_STEP: f32 = 1.25;
/// Free space around the page when it is zoomed to fit, as a share of the view.
pub const PAGE_MARGIN: f32 = 0.06;

/// The corner of the work area where the machine's 0,0 is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Origin {
    #[default]
    FrontLeft,
    FrontRight,
    RearLeft,
    RearRight,
}

impl Origin {
    pub const ALL: [Origin; 4] = [Origin::FrontLeft, Origin::FrontRight, Origin::RearLeft, Origin::RearRight];

    pub fn label(self) -> &'static str {
        match self {
            Origin::FrontLeft => "Front left",
            Origin::FrontRight => "Front right",
            Origin::RearLeft => "Rear left",
            Origin::RearRight => "Rear right",
        }
    }

    /// X grows to the left when the origin is on the right.
    pub fn flips_x(self) -> bool {
        matches!(self, Origin::FrontRight | Origin::RearRight)
    }

    /// Y grows towards the back (up on screen) when the origin is at the front.
    pub fn flips_y(self) -> bool {
        matches!(self, Origin::FrontLeft | Origin::FrontRight)
    }

    /// Machine coordinates of page point `p` on a page `page` big.
    pub fn to_machine(self, p: Pos2, page: Vec2) -> Pos2 {
        pos2(if self.flips_x() { page.x - p.x } else { p.x }, if self.flips_y() { page.y - p.y } else { p.y })
    }

    #[cfg(test)]
    /// The page point of machine coordinates `m` (the same mapping: it is its own inverse).
    pub fn to_page(self, m: Pos2, page: Vec2) -> Pos2 {
        self.to_machine(m, page)
    }
}

/// Zoom and pan of the canvas.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct View {
    /// Screen points per millimetre.
    pub zoom: f32,
    /// Where the page's top-left corner is, relative to the canvas rect's top-left corner.
    pub offset: Vec2,
    /// The view has been placed at least once (else the next frame zooms to the page).
    pub placed: bool,
}

impl Default for View {
    fn default() -> Self {
        View { zoom: 1.0, offset: Vec2::ZERO, placed: false }
    }
}

impl View {
    /// Screen position of page point `p` (mm) in the canvas `rect`.
    pub fn to_screen(self, rect: Rect, p: Pos2) -> Pos2 {
        rect.min + self.offset + p.to_vec2() * self.zoom
    }

    /// Page point (mm) under screen position `s` in the canvas `rect`.
    pub fn to_page(self, rect: Rect, s: Pos2) -> Pos2 {
        ((s - rect.min - self.offset) / self.zoom).to_pos2()
    }

    /// Screen rectangle of the page `page` (mm).
    pub fn page_rect(self, rect: Rect, page: Vec2) -> Rect {
        Rect::from_min_max(self.to_screen(rect, Pos2::ZERO), self.to_screen(rect, page.to_pos2()))
    }

    /// Zoom by `factor` keeping the page point under screen position `at` where it is.
    pub fn zoom_about(&mut self, rect: Rect, at: Pos2, factor: f32) {
        if !factor.is_finite() || factor <= 0.0 {
            return;
        }
        let fixed = self.to_page(rect, at);
        self.zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        self.offset = at - rect.min - fixed.to_vec2() * self.zoom;
    }

    /// Move the view by `delta` screen points.
    pub fn pan(&mut self, delta: Vec2) {
        if delta.is_finite() {
            self.offset += delta;
        }
    }

    /// Fit the whole page in the canvas `rect`, centred, with a small margin.
    pub fn zoom_to_page(&mut self, rect: Rect, page: Vec2) {
        if !(rect.width() > 0.0 && rect.height() > 0.0 && page.x > 0.0 && page.y > 0.0) {
            return;
        }
        let room = rect.size() * (1.0 - 2.0 * PAGE_MARGIN);
        self.zoom = (room.x / page.x).min(room.y / page.y).clamp(MIN_ZOOM, MAX_ZOOM);
        self.offset = (rect.size() - page * self.zoom) / 2.0;
        self.placed = true;
    }

    /// Fit the page rectangle `r` (mm) in the canvas `rect`, centred, with a margin.
    pub fn zoom_to_rect(&mut self, rect: Rect, r: Rect) {
        let size = r.size().max(Vec2::splat(1.0));
        let room = rect.size() * (1.0 - 4.0 * PAGE_MARGIN);
        self.zoom = (room.x / size.x).min(room.y / size.y).clamp(MIN_ZOOM, MAX_ZOOM);
        self.offset = rect.size() / 2.0 - r.center().to_vec2() * self.zoom;
        self.placed = true;
    }

    /// The zoom as a percentage, where 100 % shows a millimetre at its real size on a 96 DPI screen.
    pub fn percent(&self) -> f32 {
        self.zoom / POINTS_PER_MM * 100.0
    }
}

/// Screen points in a millimetre at 100 % (96 points per inch).
pub const POINTS_PER_MM: f32 = 96.0 / 25.4;

/// The wheel zoom factor for a scroll of `delta_y` points (positive = away from the user).
/// Each 50 points (one notch on most mice) is one [`ZOOM_STEP`]; `invert` swaps the direction.
pub fn wheel_factor(delta_y: f32, invert: bool) -> f32 {
    let notches = (delta_y / 50.0).clamp(-10.0, 10.0);
    ZOOM_STEP.powf(if invert { -notches } else { notches })
}

/// A round grid spacing in millimetres (1, 2 or 5 times a power of ten) whose lines are at
/// least `min_px` screen points apart at `zoom`.
pub fn grid_step(zoom: f32, min_px: f32) -> f32 {
    let want = (min_px / zoom.max(f32::EPSILON)).max(1e-4);
    let mut decade = 10f32.powf(want.log10().floor());
    loop {
        for m in [1.0, 2.0, 5.0] {
            let step = m * decade;
            if step >= want * 0.999 {
                return step;
            }
        }
        decade *= 10.0;
    }
}

/// The major and minor grid spacing at `zoom`: major lines at least 60 points apart, and the
/// finer grid (a tenth, or a fifth for 2-steps) once it is at least 8 points apart.
pub fn grid_steps(zoom: f32) -> (f32, Option<f32>) {
    let major = grid_step(zoom, 60.0);
    let leading = major / 10f32.powf(major.log10().floor());
    let minor = major / if (leading - 2.0).abs() < 0.01 { 4.0 } else { 5.0 };
    let minor = if minor * zoom >= 8.0 { Some(minor) } else { None };
    (major, minor)
}

/// The multiples of `step` from `from` to `to` (in that order, inclusive).
pub fn ticks(from: f32, to: f32, step: f32) -> impl Iterator<Item = f32> {
    let (lo, hi) = (from.min(to), from.max(to));
    let first = (lo / step).ceil() as i64;
    let last = (hi / step).floor() as i64;
    let count = (last - first + 1).clamp(0, 10_000);
    (0..count).map(move |i| (first + i) as f32 * step)
}

/// A ruler or status label for `mm`, with as many decimals as the step needs.
pub fn format_mm(mm: f32, step: f32) -> String {
    let decimals = if step >= 1.0 { 0 } else { (-step.log10().floor()) as usize };
    let v = if mm.abs() < step * 1e-3 { 0.0 } else { mm };
    format!("{v:.decimals$}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canvas() -> Rect {
        Rect::from_min_size(pos2(100.0, 50.0), vec2(800.0, 600.0))
    }

    #[test]
    fn screen_and_page_round_trip() {
        let view = View { zoom: 2.5, offset: vec2(30.0, -12.0), placed: true };
        for p in [pos2(0.0, 0.0), pos2(400.0, 400.0), pos2(-17.25, 123.5)] {
            let back = view.to_page(canvas(), view.to_screen(canvas(), p));
            assert!((back - p).length() < 1e-3, "{p:?} came back as {back:?}");
        }
    }

    #[test]
    fn zoom_keeps_the_point_under_the_cursor() {
        let mut view = View { zoom: 1.0, offset: vec2(40.0, 40.0), placed: true };
        let at = pos2(437.0, 288.0);
        let before = view.to_page(canvas(), at);
        view.zoom_about(canvas(), at, ZOOM_STEP);
        assert!((view.zoom - ZOOM_STEP).abs() < 1e-6);
        assert!((view.to_page(canvas(), at) - before).length() < 1e-3);
        view.zoom_about(canvas(), at, 1.0 / ZOOM_STEP);
        assert!((view.zoom - 1.0).abs() < 1e-6);
        assert!((view.to_page(canvas(), at) - before).length() < 1e-3);
    }

    #[test]
    fn zoom_stays_within_its_limits() {
        let mut view = View::default();
        view.zoom_about(canvas(), canvas().center(), 1e9);
        assert_eq!(view.zoom, MAX_ZOOM);
        view.zoom_about(canvas(), canvas().center(), 1e-9);
        assert_eq!(view.zoom, MIN_ZOOM);
        view.zoom_about(canvas(), canvas().center(), f32::NAN);
        assert_eq!(view.zoom, MIN_ZOOM);
    }

    #[test]
    fn zoom_to_page_fits_and_centres_the_page() {
        let mut view = View::default();
        let page = vec2(400.0, 400.0);
        view.zoom_to_page(canvas(), page);
        let shown = view.page_rect(canvas(), page);
        assert!(canvas().contains_rect(shown));
        assert!((shown.center() - canvas().center()).length() < 1e-3);
        // The limiting side (height 600) keeps the margin on both ends.
        assert!((shown.height() - 600.0 * (1.0 - 2.0 * PAGE_MARGIN)).abs() < 1e-2);
        assert!(view.placed);
    }

    #[test]
    fn panning_moves_the_page_with_the_pointer() {
        let mut view = View { zoom: 3.0, offset: Vec2::ZERO, placed: true };
        let p = pos2(10.0, 10.0);
        let before = view.to_screen(canvas(), p);
        view.pan(vec2(25.0, -40.0));
        assert_eq!(view.to_screen(canvas(), p), before + vec2(25.0, -40.0));
    }

    #[test]
    fn wheel_direction_can_be_inverted() {
        assert!(wheel_factor(50.0, false) > 1.0);
        assert!(wheel_factor(50.0, true) < 1.0);
        assert!((wheel_factor(50.0, false) * wheel_factor(50.0, true) - 1.0).abs() < 1e-6);
        assert_eq!(wheel_factor(0.0, false), 1.0);
    }

    #[test]
    fn origin_corner_flips_the_axes() {
        let page = vec2(400.0, 300.0);
        let top_left = pos2(0.0, 0.0);
        assert_eq!(Origin::FrontLeft.to_machine(top_left, page), pos2(0.0, 300.0));
        assert_eq!(Origin::RearLeft.to_machine(top_left, page), pos2(0.0, 0.0));
        assert_eq!(Origin::FrontRight.to_machine(top_left, page), pos2(400.0, 300.0));
        assert_eq!(Origin::RearRight.to_machine(top_left, page), pos2(400.0, 0.0));
        for o in Origin::ALL {
            let p = pos2(123.0, 45.0);
            assert_eq!(o.to_page(o.to_machine(p, page), page), p);
        }
    }

    #[test]
    fn grid_gets_finer_when_zoomed_in() {
        let (far, far_minor) = grid_steps(0.5);
        let (near, near_minor) = grid_steps(20.0);
        assert!(near < far);
        assert!(far_minor.is_none() || far_minor.unwrap() * 0.5 >= 8.0);
        assert!(near_minor.is_some(), "a close view shows the fine grid");
        for zoom in [0.03, 0.5, 1.0, 3.7, 20.0, 300.0] {
            let (major, minor) = grid_steps(zoom);
            assert!(major * zoom >= 60.0 * 0.999, "major lines at least 60 points apart at {zoom}");
            assert!(major * zoom < 60.0 * 2.6, "and not much more at {zoom}");
            if let Some(minor) = minor {
                assert!(minor < major && minor * zoom >= 8.0);
            }
        }
    }

    #[test]
    fn ticks_cover_the_range() {
        let t: Vec<f32> = ticks(-12.0, 31.0, 10.0).collect();
        assert_eq!(t, vec![-10.0, 0.0, 10.0, 20.0, 30.0]);
        assert_eq!(ticks(5.0, 1.0, 2.0).count(), 2);
        assert_eq!(format_mm(12.0, 10.0), "12");
        assert_eq!(format_mm(0.5, 0.5), "0.5");
        assert_eq!(format_mm(-0.0001, 1.0), "0");
    }
}
