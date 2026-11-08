//! Rubber-band outline: the convex hull of a set of paths.

use crate::path::flatten_to_polylines;
use kurbo::{BezPath, Point};

fn cross(o: Point, a: Point, b: Point) -> f64 {
    (a - o).cross(b - o)
}

/// Convex hull of `points` (Andrew's monotone chain), counter-clockwise with Y up,
/// collinear and duplicate points removed. Fewer than 3 distinct points are returned
/// as they are (deduplicated); all-collinear input gives its two extreme points.
pub fn convex_hull_points(points: &[Point]) -> Vec<Point> {
    let mut pts: Vec<Point> = points.iter().copied().filter(|p| p.x.is_finite() && p.y.is_finite()).collect();
    pts.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    pts.dedup();
    if pts.len() < 3 {
        return pts;
    }
    let mut lower: Vec<Point> = Vec::new();
    for &p in &pts {
        while lower.len() >= 2 && cross(lower[lower.len() - 2], lower[lower.len() - 1], p) <= 0.0 {
            lower.pop();
        }
        lower.push(p);
    }
    let mut upper: Vec<Point> = Vec::new();
    for &p in pts.iter().rev() {
        while upper.len() >= 2 && cross(upper[upper.len() - 2], upper[upper.len() - 1], p) <= 0.0 {
            upper.pop();
        }
        upper.push(p);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

/// Rubber-band outline: convex hull of every path flattened within `tol`, as one closed
/// counter-clockwise path. When all points are collinear the result is an open two-point
/// line between the extremes; a single point or empty input gives an empty path.
pub fn rubber_band_outline(paths: &[BezPath], tol: f64) -> BezPath {
    let pts: Vec<Point> = paths.iter().flat_map(|p| flatten_to_polylines(p, tol)).flat_map(|pl| pl.points.into_iter()).collect();
    let hull = convex_hull_points(&pts);
    let mut out = BezPath::new();
    if hull.len() < 2 {
        return out;
    }
    out.move_to(hull[0]);
    for p in &hull[1..] {
        out.line_to(*p);
    }
    if hull.len() >= 3 {
        out.close_path();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path::{is_closed, signed_area};
    use kurbo::{Circle, Rect, Shape};

    #[test]
    fn hull_of_square_with_inner_and_duplicate_points() {
        let pts = [
            Point::new(0.0, 0.0),
            Point::new(10.0, 0.0),
            Point::new(10.0, 10.0),
            Point::new(0.0, 10.0),
            Point::new(5.0, 5.0),
            Point::new(5.0, 0.0), // collinear on an edge
            Point::new(0.0, 0.0), // duplicate
        ];
        let h = convex_hull_points(&pts);
        assert_eq!(h.len(), 4);
        assert!(crate::path::polygon_signed_area(&h) > 0.0);
    }

    #[test]
    fn hull_degenerate() {
        assert!(convex_hull_points(&[]).is_empty());
        assert_eq!(convex_hull_points(&[Point::new(1.0, 1.0), Point::new(1.0, 1.0)]).len(), 1);
        let line = convex_hull_points(&[Point::new(0.0, 0.0), Point::new(1.0, 1.0), Point::new(2.0, 2.0)]);
        assert_eq!(line, vec![Point::new(0.0, 0.0), Point::new(2.0, 2.0)]);
        assert!(rubber_band_outline(&[], 0.1).is_empty());
        assert!(rubber_band_outline(&[BezPath::new()], 0.1).is_empty());
        let mut l = BezPath::new();
        l.move_to((0.0, 0.0));
        l.line_to((5.0, 0.0));
        let r = rubber_band_outline(&[l], 0.1);
        assert!(!is_closed(&r, 1e-9));
    }

    #[test]
    fn hull_of_circle_and_square() {
        let c = Circle::new((0.0, 0.0), 10.0).to_path(1e-6);
        let s = Rect::new(20.0, -5.0, 30.0, 5.0).to_path(0.1);
        let h = rubber_band_outline(&[c, s], 0.001);
        assert!(is_closed(&h, 1e-9));
        let a = signed_area(&h);
        // Half circle (157.08) + trapezoid-ish region to the square; bounded by the bbox.
        assert!(a > 0.0);
        let bb = crate::path::bounding_box(&h).unwrap();
        assert!((bb.x0 + 10.0).abs() < 0.01 && (bb.x1 - 30.0).abs() < 1e-9);
        assert!(a > std::f64::consts::PI * 100.0 * 0.5 + 300.0 && a < 40.0 * 20.0);
        // Every input point is inside or on the hull.
        assert!(crate::query::point_in_shape(&h, Point::new(25.0, 0.0), crate::FillRule::NonZero));
    }

    #[test]
    fn hull_of_open_self_intersecting_path() {
        let mut b = BezPath::new();
        b.move_to((0.0, 0.0));
        b.line_to((10.0, 10.0));
        b.line_to((10.0, 0.0));
        b.line_to((0.0, 10.0));
        let h = rubber_band_outline(&[b], 0.1);
        assert!((signed_area(&h) - 100.0).abs() < 1e-9);
    }
}
