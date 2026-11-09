//! Outline-accurate docking: how far a body can slide along an axis before its outline touches
//! another body's outline. Bounding boxes only decide which bodies could be in the way; the
//! distance itself comes from the flattened outlines, so a shape slides past shapes whose boxes
//! overlap but whose outlines do not.

use darkburn_core::kurbo::{BezPath, Point, Rect, Vec2};
use darkburn_geom::path::flatten_to_polylines;

/// Flattening tolerance for docking, millimetres.
const FLATTEN: f64 = 0.05;
const EPS: f64 = 1e-6;

/// A body that takes part in docking: its bounding box and its flattened outline rings.
#[derive(Clone, Debug)]
pub struct Body {
    pub bbox: Rect,
    pub rings: Vec<Vec<Point>>,
}

impl Body {
    pub fn from_path(path: &BezPath, bbox: Rect) -> Body {
        let rings = flatten_to_polylines(path, FLATTEN)
            .into_iter()
            .map(|p| {
                let mut pts = p.points;
                if p.closed
                    && let Some(f) = pts.first().copied()
                    && pts.last() != Some(&f)
                {
                    pts.push(f);
                }
                pts
            })
            .filter(|r| r.len() >= 2)
            .collect();
        Body { bbox, rings }
    }

    pub fn translated(&self, v: Vec2) -> Body {
        Body { bbox: self.bbox + v, rings: self.rings.iter().map(|r| r.iter().map(|p| *p + v).collect()).collect() }
    }
}

/// Distance along `dir` from point `p` to the crossing of segment `a`-`b`.
fn ray_hit(p: Point, dir: Vec2, a: Point, b: Point) -> Option<f64> {
    let e = b - a;
    let denom = dir.cross(e);
    if denom.abs() < 1e-12 {
        return None; // parallel: its end points are covered by the other body's vertices
    }
    let ap = a - p;
    let t = ap.cross(e) / denom;
    let s = ap.cross(dir) / denom;
    (t >= -EPS && (-EPS..=1.0 + EPS).contains(&s)).then_some(t.max(0.0))
}

fn first_hit(from: &[Vec<Point>], dir: Vec2, onto: &[Vec<Point>]) -> Option<f64> {
    let mut best: Option<f64> = None;
    for ring in from {
        for p in ring {
            for other in onto {
                for w in other.windows(2) {
                    if let Some(t) = ray_hit(*p, dir, w[0], w[1]) {
                        best = Some(best.map_or(t, |b| b.min(t)));
                    }
                }
            }
        }
    }
    best
}

/// Distance `m` can slide along `dir` (a unit axis vector) before touching `o`, if `o` is in the
/// way. `None` when the outlines never meet on that path.
pub fn gap(dir: Vec2, m: &Body, o: &Body) -> Option<f64> {
    let horizontal = dir.x != 0.0;
    let overlap = if horizontal {
        m.bbox.y0 < o.bbox.y1 - EPS && o.bbox.y0 < m.bbox.y1 - EPS
    } else {
        m.bbox.x0 < o.bbox.x1 - EPS && o.bbox.x0 < m.bbox.x1 - EPS
    };
    if !overlap {
        return None;
    }
    // Is `o` ahead of `m`?
    let ahead = if dir.x > 0.0 {
        o.bbox.x1 > m.bbox.x0 + EPS && o.bbox.x0 >= m.bbox.x1 - EPS
    } else if dir.x < 0.0 {
        o.bbox.x0 < m.bbox.x1 - EPS && o.bbox.x1 <= m.bbox.x0 + EPS
    } else if dir.y > 0.0 {
        o.bbox.y1 > m.bbox.y0 + EPS && o.bbox.y0 >= m.bbox.y1 - EPS
    } else {
        o.bbox.y0 < m.bbox.y1 - EPS && o.bbox.y1 <= m.bbox.y0 + EPS
    };
    if !ahead {
        return None;
    }
    let forward = first_hit(&m.rings, dir, &o.rings);
    let backward = first_hit(&o.rings, -dir, &m.rings);
    match (forward, backward) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use darkburn_core::kurbo::Shape;

    fn body(path: BezPath) -> Body {
        let bb = path.bounding_box();
        Body::from_path(&path, bb)
    }
    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Body {
        body(Rect::new(x0, y0, x1, y1).to_path(0.1))
    }

    #[test]
    fn rectangles_stop_at_the_gap() {
        let g = gap(Vec2::new(1.0, 0.0), &rect(0.0, 0.0, 10.0, 10.0), &rect(25.0, 2.0, 30.0, 8.0)).unwrap();
        assert!((g - 15.0).abs() < 1e-6, "{g}");
        assert!(gap(Vec2::new(1.0, 0.0), &rect(0.0, 0.0, 10.0, 10.0), &rect(25.0, 12.0, 30.0, 18.0)).is_none());
        assert!(gap(Vec2::new(-1.0, 0.0), &rect(0.0, 0.0, 10.0, 10.0), &rect(25.0, 2.0, 30.0, 8.0)).is_none());
    }

    #[test]
    fn a_circle_stops_at_a_diamonds_point_not_its_box() {
        let diamond = body({
            let mut p = BezPath::new();
            p.move_to((20.0, 0.0));
            p.line_to((25.0, 5.0));
            p.line_to((20.0, 10.0));
            p.line_to((15.0, 5.0));
            p.close_path();
            p
        });
        // On the diamond's centre line the circle meets the left corner at x = 15.
        let c = body(darkburn_core::kurbo::Circle::new((5.0, 5.0), 3.0).to_path(0.01));
        let g = gap(Vec2::new(1.0, 0.0), &c, &diamond).unwrap();
        assert!((g - (15.0 - 8.0)).abs() < 0.1, "stops at the corner, 7 mm: {g}");
        // Near the top the diamond edge is far to the right: the box (gap 7) would stop it too early.
        let high = body(darkburn_core::kurbo::Circle::new((5.0, 9.0), 1.0).to_path(0.01));
        let g = gap(Vec2::new(1.0, 0.0), &high, &diamond).unwrap();
        assert!(g > 8.5, "outline gap {g} is larger than the box gap 9");
    }

    #[test]
    fn translated_keeps_rings_and_box_together() {
        let b = rect(0.0, 0.0, 4.0, 4.0).translated(Vec2::new(10.0, 1.0));
        assert_eq!(b.bbox, Rect::new(10.0, 1.0, 14.0, 5.0));
        assert!(b.rings[0].iter().all(|p| p.x >= 10.0 - 1e-9));
    }
}
