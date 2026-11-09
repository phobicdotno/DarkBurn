//! Clip a stroke (open or closed outline) to a filled region.
//!
//! Unlike the boolean operations this keeps the path as it is: curves stay curves, and only the
//! pieces inside the region are returned, cut exactly where the outline crosses the region's
//! edge. The region is given as the closed paths a boolean operation returns (outer contours and
//! holes, filled by the non-zero rule).

use crate::path::{Contour, contours, flatten_contour};
use crate::query::{seg_intersections, winding_number};
use kurbo::{BezPath, ParamCurve, ParamCurveExtrema, PathSeg, Point};

const JOIN: f64 = 1e-6;

/// The straight edges of the flattened region.
fn region_edges(region: &[BezPath], tol: f64) -> Vec<PathSeg> {
    let mut edges = Vec::new();
    for r in region {
        for c in contours(r) {
            let pl = flatten_contour(&c, tol);
            let pts = &pl.points;
            for w in pts.windows(2) {
                edges.push(PathSeg::Line(kurbo::Line::new(w[0], w[1])));
            }
            if let (Some(f), Some(l)) = (pts.first(), pts.last())
                && f.distance(*l) > 1e-12
            {
                edges.push(PathSeg::Line(kurbo::Line::new(*l, *f)));
            }
        }
    }
    edges
}

fn all_region(region: &[BezPath]) -> BezPath {
    let mut all = BezPath::new();
    for r in region {
        all.extend(r.iter());
    }
    all
}

fn inside(all: &BezPath, p: Point) -> bool {
    winding_number(all, p) != 0
}

/// The pieces of `path` inside `region`, as open subpaths (a closed outline that is wholly inside
/// stays closed). `tol` is the flattening tolerance of the region's curves, in millimetres.
pub fn clip_stroke(path: &BezPath, region: &[BezPath], tol: f64) -> BezPath {
    let edges = region_edges(region, tol);
    let edge_boxes: Vec<kurbo::Rect> = edges.iter().map(ParamCurveExtrema::bounding_box).collect();
    let all = all_region(region);
    let mut out = BezPath::new();
    for c in contours(path) {
        let pieces = clip_contour(&c, &edges, &edge_boxes, &all);
        match pieces {
            Pieces::All => out.extend(c.to_bezpath().iter()),
            Pieces::Some(list) => {
                for piece in list {
                    out.extend(piece.iter());
                }
            }
        }
    }
    out
}

enum Pieces {
    All,
    Some(Vec<BezPath>),
}

fn clip_contour(c: &Contour, edges: &[PathSeg], boxes: &[kurbo::Rect], all: &BezPath) -> Pieces {
    // Kept sub-segments in order, with whether each continues the one before it.
    let mut kept: Vec<PathSeg> = Vec::new();
    let mut dropped_any = false;
    for seg in &c.segs {
        let sb = ParamCurveExtrema::bounding_box(seg).inflate(1e-9, 1e-9);
        let mut ts = vec![0.0, 1.0];
        for (e, eb) in edges.iter().zip(boxes) {
            if !sb.overlaps(*eb) {
                continue;
            }
            for (t, _) in seg_intersections(seg, e) {
                if t > 1e-9 && t < 1.0 - 1e-9 {
                    ts.push(t);
                }
            }
        }
        ts.sort_by(f64::total_cmp);
        ts.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
        for w in ts.windows(2) {
            let sub = seg.subsegment(w[0]..w[1]);
            if inside(all, sub.eval(0.5)) {
                kept.push(sub);
            } else {
                dropped_any = true;
                kept.push(PathSeg::Line(kurbo::Line::new(Point::new(f64::NAN, f64::NAN), Point::new(f64::NAN, f64::NAN))));
            }
        }
    }
    if !dropped_any {
        return Pieces::All;
    }
    // Group runs of kept segments (a NaN line marks a dropped piece).
    let is_gap = |s: &PathSeg| matches!(s, PathSeg::Line(l) if l.p0.x.is_nan());
    let mut runs: Vec<Vec<PathSeg>> = Vec::new();
    let mut cur: Vec<PathSeg> = Vec::new();
    for s in kept {
        if is_gap(&s) {
            if !cur.is_empty() {
                runs.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(s);
        }
    }
    if !cur.is_empty() {
        runs.push(cur);
    }
    // A closed outline whose last and first runs meet is one run.
    if c.closed
        && runs.len() > 1
        && let (Some(first), Some(last)) = (runs.first(), runs.last())
        && let (Some(f), Some(l)) = (first.first(), last.last())
        && f.start().distance(l.end()) < JOIN
    {
        let mut head = runs.pop().unwrap_or_default();
        head.extend(runs.remove(0));
        runs.insert(0, head);
    }
    let paths = runs
        .into_iter()
        .map(|run| {
            let mut p = BezPath::new();
            p.move_to(run[0].start());
            for s in &run {
                match s {
                    PathSeg::Line(l) => p.line_to(l.p1),
                    PathSeg::Quad(q) => p.quad_to(q.p1, q.p2),
                    PathSeg::Cubic(cu) => p.curve_to(cu.p1, cu.p2, cu.p3),
                }
            }
            p
        })
        .collect();
    Pieces::Some(paths)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::{Circle, Rect, Shape};

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<BezPath> {
        vec![Rect::new(x0, y0, x1, y1).to_path(0.1)]
    }

    fn length(p: &BezPath) -> f64 {
        kurbo::segments(p.iter()).map(|s| kurbo::ParamCurveArclen::arclen(&s, 1e-6)).sum()
    }

    #[test]
    fn an_open_line_is_cut_at_the_edge() {
        let mut line = BezPath::new();
        line.move_to((-10.0, 5.0));
        line.line_to((30.0, 5.0));
        let out = clip_stroke(&line, &rect(0.0, 0.0, 10.0, 10.0), 0.01);
        assert!((length(&out) - 10.0).abs() < 1e-6, "{}", length(&out));
        let b = out.bounding_box();
        assert!((b.x0 - 0.0).abs() < 1e-9 && (b.x1 - 10.0).abs() < 1e-9);
    }

    #[test]
    fn a_line_wholly_outside_vanishes_and_one_inside_is_kept() {
        let mut outside = BezPath::new();
        outside.move_to((20.0, 20.0));
        outside.line_to((30.0, 20.0));
        assert!(clip_stroke(&outside, &rect(0.0, 0.0, 10.0, 10.0), 0.01).elements().is_empty());
        let mut inside = BezPath::new();
        inside.move_to((2.0, 2.0));
        inside.line_to((8.0, 2.0));
        assert_eq!(clip_stroke(&inside, &rect(0.0, 0.0, 10.0, 10.0), 0.01), inside);
    }

    #[test]
    fn a_curve_keeps_its_curve_inside_the_clip() {
        let mut arc = BezPath::new();
        arc.move_to((-5.0, 0.0));
        arc.curve_to((-5.0, 10.0), (15.0, 10.0), (15.0, 0.0));
        let out = clip_stroke(&arc, &rect(0.0, -1.0, 10.0, 20.0), 0.01);
        assert!(out.elements().iter().any(|e| matches!(e, kurbo::PathEl::CurveTo(..))), "still a curve: {:?}", out.elements());
        let b = out.bounding_box();
        assert!(b.x0 >= -1e-9 && b.x1 <= 10.0 + 1e-9, "{b:?}");
    }

    #[test]
    fn a_circle_outline_cut_by_a_box_is_one_piece_with_the_seam_joined() {
        let circle = Circle::new((0.0, 0.0), 10.0).to_path(0.01);
        // Keep the right half: the outline starts at (10, 0), in the middle of the kept side.
        let out = clip_stroke(&circle, &rect(0.0, -20.0, 20.0, 20.0), 0.01);
        let pieces = contours(&out).len();
        assert_eq!(pieces, 1, "one arc, not two touching at the start of the outline");
        assert!((length(&out) - std::f64::consts::PI * 10.0).abs() < 0.01, "{}", length(&out));
    }

    #[test]
    fn a_hole_in_the_region_removes_the_stroke_inside_it() {
        let mut region = Rect::new(0.0, 0.0, 30.0, 30.0).to_path(0.1);
        let mut hole = BezPath::new();
        hole.move_to((10.0, 10.0));
        hole.line_to((10.0, 20.0));
        hole.line_to((20.0, 20.0));
        hole.line_to((20.0, 10.0));
        hole.close_path();
        region.extend(hole.iter());
        let mut line = BezPath::new();
        line.move_to((0.0, 15.0));
        line.line_to((30.0, 15.0));
        let out = clip_stroke(&line, &[region], 0.01);
        assert!((length(&out) - 20.0).abs() < 1e-6, "{}", length(&out));
        assert_eq!(contours(&out).len(), 2);
    }
}
