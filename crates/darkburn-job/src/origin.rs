//! Job origin and coordinates: maps design coordinates to what the machine expects.

use kurbo::{Affine, Point, Rect, Vec2};
use serde::{Deserialize, Serialize};

use crate::ir::{CoordSpace, Op, Toolpath};

/// Corner of the bed where the machine's 0,0 is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Corner {
    #[default]
    BottomLeft,
    BottomRight,
    TopLeft,
    TopRight,
}

/// The machine's work area and axis layout.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct MachineGeometry {
    pub width: f64,
    pub height: f64,
    pub origin: Corner,
    /// CNC style: the work area is in negative coordinates from the home corner.
    pub negative_coords: bool,
    pub swap_xy: bool,
}

impl Default for MachineGeometry {
    fn default() -> Self {
        Self { width: 400.0, height: 400.0, origin: Corner::BottomLeft, negative_coords: false, swap_xy: false }
    }
}

impl MachineGeometry {
    /// Design (bed, Y up, bottom-left origin) to machine coordinates.
    pub fn affine(&self) -> Affine {
        let (mut sx, mut tx) = match self.origin {
            Corner::BottomLeft | Corner::TopLeft => (1.0, 0.0),
            Corner::BottomRight | Corner::TopRight => (-1.0, self.width),
        };
        let (mut sy, mut ty) = match self.origin {
            Corner::BottomLeft | Corner::BottomRight => (1.0, 0.0),
            Corner::TopLeft | Corner::TopRight => (-1.0, self.height),
        };
        if self.negative_coords {
            sx = -sx;
            tx = -tx;
            sy = -sy;
            ty = -ty;
        }
        if self.swap_xy { Affine::new([0.0, sx, sy, 0.0, ty, tx]) } else { Affine::new([sx, 0.0, 0.0, sy, tx, ty]) }
    }

    /// Machine work area in machine coordinates.
    pub fn work_area(&self) -> Rect {
        let m = self.affine();
        Rect::from_points(m * Point::ZERO, m * Point::new(self.width, self.height))
    }
}

/// Where the job is placed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum StartFrom {
    /// Design coordinates are bed coordinates.
    #[default]
    Absolute,
    /// The job anchor is wherever the head is when the job starts (relative moves).
    CurrentPosition,
    /// The job anchor is at this user origin (machine coordinates).
    UserOrigin(Point),
}

/// The 9-point job origin.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobAnchor {
    TopLeft,
    Top,
    TopRight,
    Left,
    Center,
    Right,
    #[default]
    BottomLeft,
    Bottom,
    BottomRight,
}

impl JobAnchor {
    /// The anchor point of a rectangle (Y up).
    pub fn point(self, r: Rect) -> Point {
        let x = match self {
            JobAnchor::TopLeft | JobAnchor::Left | JobAnchor::BottomLeft => r.x0,
            JobAnchor::Top | JobAnchor::Center | JobAnchor::Bottom => (r.x0 + r.x1) / 2.0,
            _ => r.x1,
        };
        let y = match self {
            JobAnchor::TopLeft | JobAnchor::Top | JobAnchor::TopRight => r.y1,
            JobAnchor::Left | JobAnchor::Center | JobAnchor::Right => (r.y0 + r.y1) / 2.0,
            _ => r.y0,
        };
        Point::new(x, y)
    }
}

/// Where the head goes after the job.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum FinishPosition {
    /// Stay where the job ended.
    #[default]
    Stay,
    /// Machine 0,0 (absolute modes only).
    MachineOrigin,
    /// Back to where the job started (start position, user origin or machine origin).
    StartPosition,
    /// A point in machine coordinates (absolute modes only).
    Point(Point),
}

/// Job origin settings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct OriginSettings {
    pub machine: MachineGeometry,
    pub start_from: StartFrom,
    pub anchor: JobAnchor,
    /// "Use selection origin": anchor on these bounds instead of the job bounds.
    pub selection_bounds: Option<Rect>,
    pub finish: FinishPosition,
}

impl OriginSettings {
    /// Anchor point in design coordinates.
    pub fn anchor_point(&self, job_bounds: Rect) -> Point {
        self.anchor.point(self.selection_bounds.unwrap_or(job_bounds))
    }

    /// Design to output coordinates for the given job bounds.
    pub fn transform(&self, job_bounds: Rect) -> Affine {
        let m = self.machine.affine();
        let a = m * self.anchor_point(job_bounds);
        match self.start_from {
            StartFrom::Absolute => m,
            StartFrom::CurrentPosition => Affine::translate(-a.to_vec2()) * m,
            StartFrom::UserOrigin(u) => Affine::translate(u - a) * m,
        }
    }

    /// Where the head is when the job starts, in design coordinates (used by the planner).
    pub fn design_start(&self, job_bounds: Rect) -> Point {
        match self.start_from {
            StartFrom::Absolute => self.machine.affine().inverse() * Point::ZERO,
            _ => self.anchor_point(job_bounds),
        }
    }

    /// Map a design-space toolpath to output coordinates and add the finish move.
    pub fn apply(&self, tp: &Toolpath, job_bounds: Rect) -> Toolpath {
        let t = self.transform(job_bounds);
        let flip = t.determinant() < 0.0;
        let mut ops: Vec<Op> = tp
            .ops
            .iter()
            .map(|op| match op.clone() {
                Op::Rapid { to } => Op::Rapid { to: t * to },
                Op::Cut { to, speed, power } => Op::Cut { to: t * to, speed, power },
                Op::Overscan { to, speed } => Op::Overscan { to: t * to, speed },
                Op::Arc { to, center, clockwise, speed, power } => {
                    Op::Arc { to: t * to, center: t * center, clockwise: clockwise ^ flip, speed, power }
                }
                other => other,
            })
            .collect();
        let (space, start) = match self.start_from {
            StartFrom::Absolute => (CoordSpace::MachineAbsolute, t * tp.start),
            StartFrom::CurrentPosition => (CoordSpace::RelativeToStart, Point::ZERO),
            StartFrom::UserOrigin(u) => (CoordSpace::MachineAbsolute, u),
        };
        let relative = space == CoordSpace::RelativeToStart;
        match self.finish {
            FinishPosition::Stay => {}
            FinishPosition::StartPosition => ops.push(Op::Rapid { to: start }),
            FinishPosition::MachineOrigin if !relative => ops.push(Op::Rapid { to: Point::ZERO }),
            FinishPosition::Point(p) if !relative => ops.push(Op::Rapid { to: p }),
            _ => ops.push(Op::Comment { text: "finish position needs absolute coordinates".into() }),
        }
        Toolpath { space, layers: tp.layers.clone(), ops, start }
    }
}

/// Offset applied by a pointer (red dot) offset, for jobs framed with the pointer.
pub fn pointer_offset(tp: &Toolpath, offset: Vec2) -> Toolpath {
    let t = Affine::translate(offset);
    let ops = tp
        .ops
        .iter()
        .map(|op| match op.clone() {
            Op::Rapid { to } => Op::Rapid { to: t * to },
            Op::Cut { to, speed, power } => Op::Cut { to: t * to, speed, power },
            Op::Overscan { to, speed } => Op::Overscan { to: t * to, speed },
            Op::Arc { to, center, clockwise, speed, power } => Op::Arc { to: t * to, center: t * center, clockwise, speed, power },
            other => other,
        })
        .collect();
    Toolpath { ops, start: tp.start, space: tp.space, layers: tp.layers.clone() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tp() -> Toolpath {
        Toolpath {
            space: CoordSpace::Design,
            layers: Vec::new(),
            ops: vec![
                Op::Rapid { to: Point::new(10.0, 10.0) },
                Op::Cut { to: Point::new(20.0, 10.0), speed: 10.0, power: 50.0 },
                Op::Arc { to: Point::new(20.0, 30.0), center: Point::new(20.0, 20.0), clockwise: false, speed: 10.0, power: 50.0 },
            ],
            start: Point::ZERO,
        }
    }

    const B: Rect = Rect::new(10.0, 10.0, 30.0, 30.0);

    #[test]
    fn corner_flips() {
        let mut s = OriginSettings { machine: MachineGeometry { width: 100.0, height: 50.0, ..Default::default() }, ..Default::default() };
        let out = s.apply(&tp(), B);
        assert_eq!(out.ops[1].target(), Some(Point::new(20.0, 10.0)));
        s.machine.origin = Corner::TopLeft;
        let out = s.apply(&tp(), B);
        assert_eq!(out.ops[1].target(), Some(Point::new(20.0, 40.0)));
        let Op::Arc { clockwise, .. } = out.ops[2] else { panic!() };
        assert!(clockwise, "mirrored arc changes direction");
        s.machine.origin = Corner::TopRight;
        s.machine.negative_coords = true;
        let out = s.apply(&tp(), B);
        assert_eq!(out.ops[1].target(), Some(Point::new(-80.0, -40.0)));
        let Op::Arc { clockwise, .. } = out.ops[2] else { panic!() };
        assert!(!clockwise, "two mirrors keep the direction");
        s.machine = MachineGeometry { width: 100.0, height: 50.0, swap_xy: true, ..Default::default() };
        let out = s.apply(&tp(), B);
        assert_eq!(out.ops[1].target(), Some(Point::new(10.0, 20.0)));
    }

    #[test]
    fn current_position_and_user_origin() {
        let mut s = OriginSettings { start_from: StartFrom::CurrentPosition, anchor: JobAnchor::Center, ..Default::default() };
        let out = s.apply(&tp(), B);
        assert_eq!(out.space, CoordSpace::RelativeToStart);
        assert_eq!(out.ops[0].target(), Some(Point::new(-10.0, -10.0)));
        s.start_from = StartFrom::UserOrigin(Point::new(100.0, 100.0));
        s.anchor = JobAnchor::TopRight;
        s.finish = FinishPosition::StartPosition;
        let out = s.apply(&tp(), B);
        assert_eq!(out.ops[0].target(), Some(Point::new(80.0, 80.0)));
        assert_eq!(out.ops.last().unwrap().target(), Some(Point::new(100.0, 100.0)));
        // Selection origin overrides job bounds.
        s.selection_bounds = Some(Rect::new(0.0, 0.0, 10.0, 10.0));
        let out = s.apply(&tp(), B);
        assert_eq!(out.ops[0].target(), Some(Point::new(100.0, 100.0)));
    }

    #[test]
    fn nine_anchors() {
        let r = Rect::new(0.0, 0.0, 10.0, 20.0);
        assert_eq!(JobAnchor::TopLeft.point(r), Point::new(0.0, 20.0));
        assert_eq!(JobAnchor::Center.point(r), Point::new(5.0, 10.0));
        assert_eq!(JobAnchor::BottomRight.point(r), Point::new(10.0, 0.0));
        assert_eq!(JobAnchor::Right.point(r), Point::new(10.0, 10.0));
    }

    #[test]
    fn finish_positions() {
        let mut s = OriginSettings { finish: FinishPosition::MachineOrigin, ..Default::default() };
        let out = s.apply(&tp(), B);
        assert_eq!(out.ops.last().unwrap(), &Op::Rapid { to: Point::ZERO });
        s.finish = FinishPosition::Point(Point::new(5.0, 6.0));
        let out = s.apply(&tp(), B);
        assert_eq!(out.ops.last().unwrap(), &Op::Rapid { to: Point::new(5.0, 6.0) });
        s.start_from = StartFrom::CurrentPosition;
        let out = s.apply(&tp(), B);
        assert!(matches!(out.ops.last().unwrap(), Op::Comment { .. }));
    }
}
