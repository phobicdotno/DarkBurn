//! The drawing and selection tools: which one is active, and the pure geometry behind them
//! (the pen of the line tool, shapes from a drag, the selection handles and how dragging one
//! transforms the selection, nudge steps). The canvas (`canvas_input.rs`) feeds these from the
//! pointer and keys; everything here is in document millimetres, Y up, and testable without a
//! window.
//!
//! Modifier keys, as the canvas uses them:
//! - Selecting: Shift+click adds or removes a shape, Ctrl+Shift+click removes it. A box dragged
//!   left to right selects what it encloses, right to left what it crosses.
//! - Moving: Shift keeps the move horizontal or vertical. Ctrl turns snapping off for a moment,
//!   Alt widens the snap distance.
//! - Corner handles keep the aspect ratio; Shift frees it. Ctrl scales about the centre.
//! - Rotating snaps to 15° steps with Ctrl and 5° steps with Shift.
//! - Rectangle and ellipse: Ctrl draws a square or circle, Shift draws from the centre.
//! - Polygon: drawn from its centre; Shift keeps a corner on top.
//! - Line: Shift snaps the segment to 45° steps.
//! - Arrow keys nudge 1 mm, Shift 10 mm, Ctrl 0.1 mm, Ctrl+Shift 0.01 mm.

use darkburn_core::Shape;
use darkburn_core::kurbo::{Affine, BezPath, Point, Rect, Vec2};

/// The active canvas tool.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tool {
    #[default]
    Select,
    Line,
    Rect,
    Ellipse,
    Polygon,
    /// Edit nodes (node_tool.rs).
    Nodes,
    /// Place tabs by hand.
    Tabs,
    Measure,
    Warp,
    Deform,
    StartPoints,
    RoundCorners,
    /// Pick the two points of a mirror line.
    Mirror,
    /// Pick the two point pairs of a two-point rotate and scale.
    TwoPoint,
    /// Place and type text (text_ui.rs).
    Text,
    /// Click to move the laser head there (laser_position.rs).
    PositionLaser,
}

impl Tool {
    /// The command that picks this tool.
    pub fn command(self) -> &'static str {
        match self {
            Tool::Select => "tools.select",
            Tool::Line => "tools.line",
            Tool::Rect => "tools.rect",
            Tool::Ellipse => "tools.ellipse",
            Tool::Polygon => "tools.polygon",
            Tool::Nodes => "tools.nodes",
            Tool::Tabs => "tools.tabs",
            Tool::Measure => "tools.measure",
            Tool::Warp => "tools.warp",
            Tool::Deform => "tools.deform",
            Tool::StartPoints => "tools.start_points",
            Tool::RoundCorners => "tools.round_corners",
            Tool::Mirror => "arrange.mirror",
            Tool::TwoPoint => "arrange.two_point",
            Tool::Text => "tools.text",
            Tool::PositionLaser => "tools.position_laser",
        }
    }

    /// The status bar hint while the pointer is over the canvas.
    pub fn hint(self) -> &'static str {
        match self {
            Tool::Select => {
                "Click to select (Shift adds, Ctrl+Shift removes), drag a box: left to right encloses, right to left crosses. Click the selection again for rotate handles."
            }
            Tool::Line => "Click for lines, drag for curves, Shift for 45°. Double-click, Enter or Esc finishes; click the start point to close.",
            Tool::Rect => "Drag a rectangle. Ctrl: square, Shift: from the centre.",
            Tool::Ellipse => "Drag an ellipse. Ctrl: circle, Shift: from the centre.",
            Tool::Polygon => "Drag a polygon from its centre. Shift keeps a corner on top.",
            Tool::Text => crate::text_ui::HINT,
            Tool::PositionLaser => "Click to move the laser head to that point. Needs a connected laser.",
            other => crate::node_tool::hint(other),
        }
    }
}

/// Default number of sides for new polygons.
pub const POLYGON_SIDES: u32 = 6;

/// The arrow-key nudge distance in mm for the held modifiers.
pub fn nudge_step(shift: bool, ctrl: bool) -> f64 {
    crate::options::get().nudge_step(shift, ctrl)
}

/// The rectangle a drag from `a` to `b` draws: a square when `square`, centred on `a` when
/// `centred`.
pub fn rect_from_drag(a: Point, b: Point, square: bool, centred: bool) -> Rect {
    let mut d = b - a;
    if square {
        let s = d.x.abs().max(d.y.abs());
        d = Vec2::new(s.copysign(d.x), s.copysign(d.y));
    }
    if centred { Rect::from_points(a - d, a + d) } else { Rect::from_points(a, a + d) }
}

/// A polygon dragged from `centre` to `to`: a corner at `to` (or on top when `upright`).
pub fn polygon_from_drag(centre: Point, to: Point, sides: u32, upright: bool, layer: usize) -> Shape {
    let d = to - centre;
    let mut s = Shape::polygon(centre, sides, d.hypot(), layer);
    if !upright && d.hypot() > 1e-12 {
        let a = d.y.atan2(d.x) - std::f64::consts::FRAC_PI_2;
        s.transform *= Affine::rotate(a);
    }
    s
}

/// One node of the line tool's path. `handle` is the outgoing tangent from a click-drag; the
/// incoming one mirrors it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PenNode {
    pub at: Point,
    pub handle: Option<Vec2>,
}

/// The line tool's path being drawn.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pen {
    pub nodes: Vec<PenNode>,
}

impl Pen {
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn last(&self) -> Option<Point> {
        self.nodes.last().map(|n| n.at)
    }

    pub fn start(&self) -> Option<Point> {
        self.nodes.first().map(|n| n.at)
    }

    /// Add a corner node.
    pub fn add(&mut self, p: Point) {
        self.nodes.push(PenNode { at: p, handle: None });
    }

    /// Set the tangent handle of the last node (a click-drag), towards `to`.
    pub fn drag_handle(&mut self, to: Point) {
        if let Some(n) = self.nodes.last_mut() {
            let h = to - n.at;
            n.handle = (h.hypot() > 1e-9).then_some(h);
        }
    }

    /// Whether `p` is within `tol` of the first node (clicking it closes the path).
    pub fn is_on_start(&self, p: Point, tol: f64) -> bool {
        self.nodes.len() >= 3 && self.start().is_some_and(|s| s.distance(p) <= tol)
    }

    fn segment(path: &mut BezPath, a: &PenNode, b: &PenNode) {
        if a.handle.is_none() && b.handle.is_none() {
            path.line_to(b.at);
        } else {
            let c1 = a.at + a.handle.unwrap_or_default();
            let c2 = b.at - b.handle.unwrap_or_default();
            path.curve_to(c1, c2, b.at);
        }
    }

    /// The path so far, closed back to the start when `closed`.
    pub fn path(&self, closed: bool) -> BezPath {
        let mut p = BezPath::new();
        let Some(first) = self.nodes.first() else { return p };
        p.move_to(first.at);
        for w in self.nodes.windows(2) {
            Self::segment(&mut p, &w[0], &w[1]);
        }
        if closed && self.nodes.len() >= 3 {
            Self::segment(&mut p, self.nodes.last().unwrap_or(first), first);
            p.close_path();
        }
        p
    }

    /// The path with a rubber-band segment to the pointer at `to`.
    pub fn preview(&self, to: Point) -> BezPath {
        let mut p = self.path(false);
        if let Some(last) = self.nodes.last() {
            Self::segment(&mut p, last, &PenNode { at: to, handle: None });
        }
        p
    }

    /// Finish: the path to add (`None` when there is not a segment yet), and the pen empties.
    pub fn finish(&mut self, closed: bool) -> Option<BezPath> {
        // A double-click lands a second node on the last one: drop zero-length ends.
        while self.nodes.len() >= 2 && self.nodes[self.nodes.len() - 1].at.distance(self.nodes[self.nodes.len() - 2].at) < 1e-9 {
            let last = self.nodes.pop();
            if let (Some(l), Some(n)) = (last, self.nodes.last_mut())
                && n.handle.is_none()
            {
                n.handle = l.handle;
            }
        }
        let out = (self.nodes.len() >= 2).then(|| self.path(closed));
        self.nodes.clear();
        out
    }
}

/// The length (mm) and angle (degrees, counter-clockwise from +X) from `a` to `b`, for the
/// readout beside the pointer.
pub fn length_angle(a: Point, b: Point) -> (f64, f64) {
    let d = b - a;
    (d.hypot(), d.y.atan2(d.x).to_degrees())
}

/// What the selection handles do: scale (the first click) or rotate and shear (click the
/// selection again).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HandleMode {
    #[default]
    Scale,
    Rotate,
}

/// A handle around the selection box: -1, 0 or 1 along each axis (x left to right, y bottom
/// to top). (0, 0) is not a handle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Handle {
    pub x: i8,
    pub y: i8,
}

impl Handle {
    pub const ALL: [Handle; 8] = [
        Handle { x: -1, y: -1 },
        Handle { x: 0, y: -1 },
        Handle { x: 1, y: -1 },
        Handle { x: 1, y: 0 },
        Handle { x: 1, y: 1 },
        Handle { x: 0, y: 1 },
        Handle { x: -1, y: 1 },
        Handle { x: -1, y: 0 },
    ];

    pub fn is_corner(self) -> bool {
        self.x != 0 && self.y != 0
    }

    /// Where the handle sits on box `r`.
    pub fn on(self, r: Rect) -> Point {
        let f = |k: i8, lo: f64, hi: f64| match k {
            -1 => lo,
            1 => hi,
            _ => (lo + hi) / 2.0,
        };
        Point::new(f(self.x, r.x0, r.x1), f(self.y, r.y0, r.y1))
    }

    /// The handle across the box.
    pub fn opposite(self) -> Handle {
        Handle { x: -self.x, y: -self.y }
    }
}

/// What a selection drag does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DragKind {
    Move,
    Scale(Handle),
    Rotate,
    Shear(Handle),
}

impl DragKind {
    /// The lock (move, size, rotate, shear) that forbids this drag.
    pub fn lock(self) -> usize {
        match self {
            DragKind::Move => 0,
            DragKind::Scale(_) => 1,
            DragKind::Rotate => 2,
            DragKind::Shear(_) => 3,
        }
    }

    /// The handle kind for `h` in `mode`.
    pub fn for_handle(h: Handle, mode: HandleMode) -> DragKind {
        match (mode, h.is_corner()) {
            (HandleMode::Scale, _) => DragKind::Scale(h),
            (HandleMode::Rotate, true) => DragKind::Rotate,
            (HandleMode::Rotate, false) => DragKind::Shear(h),
        }
    }
}

/// Modifier keys during a drag.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

/// The transform (document coordinates) a selection drag from `start` to `now` applies to
/// shapes whose bounding box was `bbox` when the drag began.
pub fn drag_transform(kind: DragKind, bbox: Rect, start: Point, now: Point, mods: Mods) -> Affine {
    let about = |c: Point, a: Affine| Affine::translate(c.to_vec2()) * a * Affine::translate(-c.to_vec2());
    match kind {
        DragKind::Move => {
            let mut d = now - start;
            if mods.shift {
                if d.x.abs() >= d.y.abs() {
                    d.y = 0.0;
                } else {
                    d.x = 0.0;
                }
            }
            Affine::translate(d)
        }
        DragKind::Scale(h) => {
            let anchor = if mods.ctrl { bbox.center() } else { h.opposite().on(bbox) };
            let grip = h.on(bbox);
            let ratio = |now: f64, grip: f64, anchor: f64| {
                let span = grip - anchor;
                if span.abs() < 1e-9 { 1.0 } else { nonzero((now - anchor) / span) }
            };
            let mut sx = if h.x != 0 { ratio(now.x, grip.x, anchor.x) } else { 1.0 };
            let mut sy = if h.y != 0 { ratio(now.y, grip.y, anchor.y) } else { 1.0 };
            if h.is_corner() && !mods.shift {
                // Keep the aspect: follow whichever axis moved further, keeping each one's flip.
                let s = sx.abs().max(sy.abs());
                sx = s.copysign(sx);
                sy = s.copysign(sy);
            }
            about(anchor, Affine::scale_non_uniform(sx, sy))
        }
        DragKind::Rotate => {
            let c = bbox.center();
            let (a0, a1) = ((start - c).atan2(), (now - c).atan2());
            let mut a = (a1 - a0).to_degrees();
            let step = if mods.ctrl {
                15.0
            } else if mods.shift {
                5.0
            } else {
                0.0
            };
            if step > 0.0 {
                a = (a / step).round() * step;
            }
            about(c, Affine::rotate(a.to_radians()))
        }
        DragKind::Shear(h) => {
            let anchor = h.opposite().on(bbox);
            let grip = h.on(bbox);
            if h.y != 0 {
                // Top or bottom edge: slide sideways.
                let span = grip.y - anchor.y;
                let k = if span.abs() < 1e-9 { 0.0 } else { (now.x - start.x) / span };
                about(anchor, Affine::new([1.0, 0.0, k, 1.0, 0.0, 0.0]))
            } else {
                let span = grip.x - anchor.x;
                let k = if span.abs() < 1e-9 { 0.0 } else { (now.y - start.y) / span };
                about(anchor, Affine::new([1.0, k, 0.0, 1.0, 0.0, 0.0]))
            }
        }
    }
}

fn nonzero(s: f64) -> f64 {
    if s.abs() < 1e-4 { 1e-4f64.copysign(if s == 0.0 { 1.0 } else { s }) } else { s }
}

/// The rotation angle (degrees) of a rotate drag, for the readout.
pub fn rotation_degrees(a: Affine) -> f64 {
    let c = a.as_coeffs();
    c[1].atan2(c[0]).to_degrees()
}

/// Whether `a` changes nothing (within a micrometre over a 1 m shape).
pub fn is_identity(a: Affine) -> bool {
    a.as_coeffs().iter().zip(Affine::IDENTITY.as_coeffs()).all(|(x, y)| (x - y).abs() < 1e-9)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: Point, b: Point) -> bool {
        a.distance(b) < 1e-6
    }

    fn bbox() -> Rect {
        Rect::new(0.0, 0.0, 20.0, 10.0)
    }

    #[test]
    fn nudge_steps() {
        assert_eq!(nudge_step(false, false), 1.0);
        assert_eq!(nudge_step(true, false), 10.0);
        assert_eq!(nudge_step(false, true), 0.1);
        assert_eq!(nudge_step(true, true), 0.01);
    }

    #[test]
    fn rectangles_from_drags() {
        let r = rect_from_drag(Point::new(10.0, 10.0), Point::new(4.0, 30.0), false, false);
        assert_eq!(r, Rect::new(4.0, 10.0, 10.0, 30.0));
        let sq = rect_from_drag(Point::new(0.0, 0.0), Point::new(5.0, -12.0), true, false);
        assert_eq!(sq, Rect::new(0.0, -12.0, 12.0, 0.0));
        let c = rect_from_drag(Point::new(10.0, 10.0), Point::new(13.0, 12.0), false, true);
        assert_eq!(c, Rect::new(7.0, 8.0, 13.0, 12.0));
    }

    #[test]
    fn polygon_corner_follows_the_pointer() {
        let s = polygon_from_drag(Point::new(10.0, 10.0), Point::new(20.0, 10.0), 6, false, 2);
        assert_eq!(s.layer, 2);
        assert!(s.nodes().iter().any(|n| near(*n, Point::new(20.0, 10.0))), "{:?}", s.nodes());
        let up = polygon_from_drag(Point::new(10.0, 10.0), Point::new(20.0, 10.0), 5, true, 0);
        assert!(up.nodes().iter().any(|n| near(*n, Point::new(10.0, 20.0))));
        assert_eq!(up.nodes().len(), 5);
    }

    #[test]
    fn pen_lines_curves_and_closing() {
        let mut pen = Pen::default();
        pen.add(Point::new(0.0, 0.0));
        pen.add(Point::new(10.0, 0.0));
        pen.add(Point::new(10.0, 10.0));
        pen.drag_handle(Point::new(15.0, 15.0));
        assert!(pen.is_on_start(Point::new(0.3, 0.2), 0.5));
        assert!(!pen.is_on_start(Point::new(3.0, 0.0), 0.5));
        let preview = pen.preview(Point::new(5.0, 5.0));
        assert_eq!(preview.elements().len(), 4);
        let path = pen.finish(true).unwrap();
        assert!(pen.is_empty());
        let els = path.elements();
        assert!(matches!(els[1], darkburn_core::kurbo::PathEl::LineTo(_)));
        assert!(matches!(els[2], darkburn_core::kurbo::PathEl::CurveTo(..)), "the dragged node curves");
        assert!(matches!(els.last(), Some(darkburn_core::kurbo::PathEl::ClosePath)));
        assert!(darkburn_core::shape::path_is_closed(&path));
    }

    #[test]
    fn pen_double_click_and_too_short() {
        let mut pen = Pen::default();
        pen.add(Point::new(0.0, 0.0));
        assert!(pen.clone().finish(false).is_none(), "one node is not a line");
        pen.add(Point::new(5.0, 0.0));
        pen.add(Point::new(5.0, 0.0));
        let path = pen.finish(false).unwrap();
        assert_eq!(path.elements().len(), 2, "the double-click's repeat node is dropped");
        let (len, ang) = length_angle(Point::ZERO, Point::new(0.0, 3.0));
        assert!((len - 3.0).abs() < 1e-9 && (ang - 90.0).abs() < 1e-9);
    }

    #[test]
    fn move_with_axis_lock() {
        let a = drag_transform(DragKind::Move, bbox(), Point::new(1.0, 1.0), Point::new(6.0, 3.0), Mods::default());
        assert!(near(a * Point::ZERO, Point::new(5.0, 2.0)));
        let a = drag_transform(DragKind::Move, bbox(), Point::new(1.0, 1.0), Point::new(6.0, 3.0), Mods { shift: true, ..Mods::default() });
        assert!(near(a * Point::ZERO, Point::new(5.0, 0.0)));
    }

    #[test]
    fn corner_scaling_keeps_aspect_unless_shift() {
        let h = Handle { x: 1, y: 1 };
        let a = drag_transform(DragKind::Scale(h), bbox(), Point::new(20.0, 10.0), Point::new(40.0, 12.0), Mods::default());
        assert!(near(a * Point::new(20.0, 10.0), Point::new(40.0, 20.0)), "twice as big");
        assert!(near(a * Point::ZERO, Point::ZERO), "the opposite corner stays");
        let free =
            drag_transform(DragKind::Scale(h), bbox(), Point::new(20.0, 10.0), Point::new(40.0, 12.0), Mods { shift: true, ..Mods::default() });
        assert!(near(free * Point::new(20.0, 10.0), Point::new(40.0, 12.0)));
        let centred =
            drag_transform(DragKind::Scale(h), bbox(), Point::new(20.0, 10.0), Point::new(30.0, 15.0), Mods { ctrl: true, ..Mods::default() });
        assert!(near(centred * Point::new(10.0, 5.0), Point::new(10.0, 5.0)), "Ctrl scales about the centre");
    }

    #[test]
    fn edge_scaling_moves_one_axis() {
        let a = drag_transform(DragKind::Scale(Handle { x: -1, y: 0 }), bbox(), Point::new(0.0, 5.0), Point::new(-10.0, 9.0), Mods::default());
        assert!(near(a * Point::new(0.0, 0.0), Point::new(-10.0, 0.0)));
        assert!(near(a * Point::new(20.0, 10.0), Point::new(20.0, 10.0)));
    }

    #[test]
    fn rotation_snaps_with_modifiers() {
        let c = bbox().center();
        let start = c + Vec2::new(10.0, 0.0);
        let now = c + Vec2::from_angle(37f64.to_radians()) * 10.0;
        let free = drag_transform(DragKind::Rotate, bbox(), start, now, Mods::default());
        assert!((rotation_degrees(free) - 37.0).abs() < 1e-6);
        let ctrl = drag_transform(DragKind::Rotate, bbox(), start, now, Mods { ctrl: true, ..Mods::default() });
        assert!((rotation_degrees(ctrl) - 30.0).abs() < 1e-6);
        let shift = drag_transform(DragKind::Rotate, bbox(), start, now, Mods { shift: true, ..Mods::default() });
        assert!((rotation_degrees(shift) - 35.0).abs() < 1e-6);
        assert!(near(free * c, c), "rotates about the centre");
    }

    #[test]
    fn shearing_slides_an_edge() {
        let top = Handle { x: 0, y: 1 };
        let a = drag_transform(DragKind::Shear(top), bbox(), Point::new(10.0, 10.0), Point::new(15.0, 10.0), Mods::default());
        assert!(near(a * Point::new(0.0, 10.0), Point::new(5.0, 10.0)));
        assert!(near(a * Point::new(0.0, 0.0), Point::new(0.0, 0.0)), "the bottom edge stays");
        let right = Handle { x: 1, y: 0 };
        let a = drag_transform(DragKind::Shear(right), bbox(), Point::new(20.0, 5.0), Point::new(20.0, 9.0), Mods::default());
        assert!(near(a * Point::new(20.0, 0.0), Point::new(20.0, 4.0)));
        assert!(near(a * Point::new(0.0, 0.0), Point::new(0.0, 0.0)));
    }

    #[test]
    fn handles_and_locks() {
        let r = bbox();
        assert!(near(Handle { x: 1, y: -1 }.on(r), Point::new(20.0, 0.0)));
        assert!(near(Handle { x: 0, y: 1 }.on(r), Point::new(10.0, 10.0)));
        assert_eq!(DragKind::for_handle(Handle { x: 1, y: 1 }, HandleMode::Rotate), DragKind::Rotate);
        assert_eq!(DragKind::for_handle(Handle { x: 1, y: 0 }, HandleMode::Rotate).lock(), 3);
        assert_eq!(DragKind::for_handle(Handle { x: 1, y: 0 }, HandleMode::Scale).lock(), 1);
        assert_eq!(DragKind::Move.lock(), 0);
        assert!(is_identity(drag_transform(DragKind::Move, r, Point::ZERO, Point::ZERO, Mods::default())));
    }
}
