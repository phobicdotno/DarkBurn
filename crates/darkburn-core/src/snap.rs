//! Snapping for pointer positions on the canvas: to the grid, and to shapes' nodes, centres,
//! segment midpoints, crossings and the nearest point along their outlines.
//!
//! Everything works in document millimetres. The canvas turns its snap distance (screen
//! points) into millimetres before asking.

use darkburn_geom::query;
use kurbo::{ParamCurve, Point, Shape as _, Vec2};

use crate::document::Document;
use crate::shape::ShapeId;

/// What a snapped point sits on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapKind {
    /// An end point of a segment.
    Node,
    /// The centre of a shape's bounding box.
    Centre,
    /// Halfway along a segment.
    Midpoint,
    /// Where two outlines cross.
    Intersection,
    /// The nearest point along a line or curve.
    OnPath,
    /// A grid crossing.
    Grid,
}

impl SnapKind {
    pub fn label(self) -> &'static str {
        match self {
            SnapKind::Node => "Node",
            SnapKind::Centre => "Centre",
            SnapKind::Midpoint => "Midpoint",
            SnapKind::Intersection => "Intersection",
            SnapKind::OnPath => "On path",
            SnapKind::Grid => "Grid",
        }
    }
}

/// A snapped point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Snap {
    pub point: Point,
    pub kind: SnapKind,
}

/// How to snap.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SnapOptions {
    /// Grid spacing in mm, or `None` for no grid snap.
    pub grid: Option<f64>,
    /// Snap to shapes.
    pub objects: bool,
    /// How far a point may jump to a shape feature, in mm.
    pub radius: f64,
}

/// Most shapes looked at for crossings (pairs grow with the square of this).
const MAX_CROSSING_SHAPES: usize = 24;

/// Snap `p`: to the nearest shape feature within `opts.radius` (nodes, centres, midpoints and
/// crossings first, then the nearest point along an outline), else to the grid. Shapes in
/// `exclude` (the ones being dragged) and shapes that are not selectable are ignored.
pub fn snap(doc: &Document, p: Point, opts: &SnapOptions, exclude: &[ShapeId]) -> Option<Snap> {
    if opts.objects && opts.radius > 0.0 {
        let near = nearby(doc, p, opts.radius, exclude);
        let mut best: Option<(f64, Snap)> = None;
        let mut offer = |q: Point, kind: SnapKind| {
            let d = q.distance(p);
            if d <= opts.radius && best.is_none_or(|(bd, _)| d < bd - 1e-9) {
                best = Some((d, Snap { point: q, kind }));
            }
        };
        // Crossings first: a crossing on a midpoint or node wins the tie.
        for (i, a) in near.iter().enumerate().take(MAX_CROSSING_SHAPES) {
            for b in near.iter().skip(i + 1).take(MAX_CROSSING_SHAPES) {
                for q in query::path_intersections(a, b) {
                    offer(q, SnapKind::Intersection);
                }
            }
        }
        for outline in &near {
            for seg in outline.segments() {
                offer(seg.start(), SnapKind::Node);
                offer(seg.end(), SnapKind::Node);
                offer(seg.eval(0.5), SnapKind::Midpoint);
            }
            if let Some(c) = (outline.elements().len() > 1).then(|| outline.bounding_box().center()) {
                offer(c, SnapKind::Centre);
            }
        }
        if let Some((_, s)) = best {
            return Some(s);
        }
        let along = near
            .iter()
            .filter_map(|o| query::nearest_point(o, p))
            .filter(|h| h.distance <= opts.radius)
            .min_by(|a, b| a.distance.total_cmp(&b.distance));
        if let Some(h) = along {
            return Some(Snap { point: h.point, kind: SnapKind::OnPath });
        }
    }
    opts.grid.filter(|g| *g > 0.0).map(|g| Snap { point: snap_to_grid(p, g), kind: SnapKind::Grid })
}

/// Outlines (document coordinates) of the selectable shapes, group members one by one, whose
/// bounds come within `radius` of `p`.
fn nearby(doc: &Document, p: Point, radius: f64, exclude: &[ShapeId]) -> Vec<kurbo::BezPath> {
    let mut out = Vec::new();
    for s in doc.iter().filter(|s| doc.is_selectable(s) && !exclude.contains(&s.id)) {
        if !s.bbox().inflate(radius, radius).contains(p) {
            continue;
        }
        for leaf in s.leaves() {
            let o = leaf.outline();
            if !o.elements().is_empty() && o.bounding_box().inflate(radius, radius).contains(p) {
                out.push(o);
            }
        }
    }
    out
}

/// The nearest grid crossing to `p` for a grid `step` mm apart.
pub fn snap_to_grid(p: Point, step: f64) -> Point {
    if !(step > 0.0) {
        return p;
    }
    Point::new((p.x / step).round() * step, (p.y / step).round() * step)
}

/// `p` moved onto the horizontal or vertical line through `from`, whichever is nearer.
pub fn axis_lock(from: Point, p: Point) -> Point {
    let d = p - from;
    if d.x.abs() >= d.y.abs() { Point::new(p.x, from.y) } else { Point::new(from.x, p.y) }
}

/// `p` moved onto the nearest ray from `from` at a multiple of `step_deg` degrees, keeping
/// its distance along that ray.
pub fn snap_angle(from: Point, p: Point, step_deg: f64) -> Point {
    let d = p - from;
    let len = d.hypot();
    if len < 1e-12 || !(step_deg > 0.0) {
        return p;
    }
    let step = step_deg.to_radians();
    let a = (d.y.atan2(d.x) / step).round() * step;
    let dir = Vec2::from_angle(a);
    from + dir * d.dot(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shape::Shape;
    use kurbo::{BezPath, Rect};

    fn opts(radius: f64) -> SnapOptions {
        SnapOptions { grid: Some(10.0), objects: true, radius }
    }

    fn near(a: Point, b: Point) -> bool {
        a.distance(b) < 1e-6
    }

    #[test]
    fn grid_snap_rounds_to_the_nearest_crossing() {
        assert!(near(snap_to_grid(Point::new(12.0, -7.0), 5.0), Point::new(10.0, -5.0)));
        assert!(near(snap_to_grid(Point::new(1.3, 1.0), 0.0), Point::new(1.3, 1.0)));
        let d = Document::new();
        let s = snap(&d, Point::new(14.0, 26.0), &opts(1.0), &[]).unwrap();
        assert_eq!(s.kind, SnapKind::Grid);
        assert!(near(s.point, Point::new(10.0, 30.0)));
        let off = SnapOptions { grid: None, objects: true, radius: 1.0 };
        assert!(snap(&d, Point::new(14.0, 26.0), &off, &[]).is_none());
    }

    #[test]
    fn nodes_centres_and_midpoints() {
        let mut d = Document::new();
        let id = d.add(Shape::rect(Rect::new(0.0, 0.0, 20.0, 10.0), 0.0, 0));
        let s = snap(&d, Point::new(19.6, 10.3), &opts(1.0), &[]).unwrap();
        assert_eq!(s.kind, SnapKind::Node);
        assert!(near(s.point, Point::new(20.0, 10.0)));
        let s = snap(&d, Point::new(10.4, 0.3), &opts(1.0), &[]).unwrap();
        assert_eq!(s.kind, SnapKind::Midpoint);
        assert!(near(s.point, Point::new(10.0, 0.0)));
        let s = snap(&d, Point::new(10.2, 5.3), &opts(1.0), &[]).unwrap();
        assert_eq!(s.kind, SnapKind::Centre);
        assert!(near(s.point, Point::new(10.0, 5.0)));
        let s = snap(&d, Point::new(4.0, 9.6), &opts(1.0), &[]).unwrap();
        assert_eq!(s.kind, SnapKind::OnPath);
        assert!(near(s.point, Point::new(4.0, 10.0)));
        let s = snap(&d, Point::new(19.6, 10.3), &opts(1.0), &[id]).unwrap();
        assert_eq!(s.kind, SnapKind::Grid, "the shape being dragged is not a target");
    }

    #[test]
    fn crossings_of_two_shapes() {
        let mut d = Document::new();
        let mut a = BezPath::new();
        a.move_to((0.0, 0.0));
        a.line_to((10.0, 10.0));
        let mut b = BezPath::new();
        b.move_to((0.0, 10.0));
        b.line_to((10.0, 0.0));
        d.add(Shape::path(a, 0));
        d.add(Shape::path(b, 0));
        let s = snap(&d, Point::new(5.3, 4.8), &opts(1.0), &[]).unwrap();
        assert_eq!(s.kind, SnapKind::Intersection);
        assert!(near(s.point, Point::new(5.0, 5.0)));
    }

    #[test]
    fn hidden_shapes_are_not_snapped_to() {
        let mut d = Document::new();
        d.add(Shape::rect(Rect::new(0.0, 0.0, 20.0, 10.0), 0.0, 3));
        d.layer_mut(3).show = false;
        assert_eq!(snap(&d, Point::new(19.6, 10.3), &opts(1.0), &[]).unwrap().kind, SnapKind::Grid);
    }

    #[test]
    fn axis_lock_and_angle_snap() {
        let o = Point::new(1.0, 1.0);
        assert!(near(axis_lock(o, Point::new(9.0, 3.0)), Point::new(9.0, 1.0)));
        assert!(near(axis_lock(o, Point::new(2.0, -8.0)), Point::new(1.0, -8.0)));
        let p = snap_angle(Point::ZERO, Point::new(10.0, 9.0), 45.0);
        assert!((p.x - p.y).abs() < 1e-9 && p.x > 9.0);
        let p = snap_angle(Point::ZERO, Point::new(10.0, 1.0), 45.0);
        assert!(near(p, Point::new(10.0, 0.0)));
        assert!(near(snap_angle(o, o, 45.0), o));
    }
}
