//! Turns parts into flattened, simplified outlines for the true-shape nester.
//!
//! A part becomes closed rings (solid), usable holes and open lines. Contours that lie inside
//! a solid ring (engraving inside a cut outline, say) are dropped: the ring already covers them.
//! Parts with more than [`VERTEX_CAP`] points are simplified; the simplification error, plus
//! the flattening tolerance, is the part's slack, added to every distance it must keep, so the
//! real shapes keep the gap even though the nester works on simpler ones. Parts that cannot be
//! simplified enough fall back to their convex hull, and in the end to their bounding box.

use super::space::{in_ring, seg_dist};
use crate::hull::convex_hull_points;
use crate::path::{flatten_to_polylines, polygon_signed_area};
use kurbo::{Affine, BezPath, Point, Rect, Vec2};
use std::ops::Range;

/// Most points a part keeps for the nester.
pub(super) const VERTEX_CAP: usize = 360;
/// Contours longer than this skip straight to the hull (simplifying them would be slow).
const HUGE: usize = 50_000;

/// A part's shape for the nester, in the part's own coordinates.
#[derive(Clone, Debug, Default)]
pub(super) struct Outline {
    pub solids: Vec<Vec<Point>>,
    pub holes: Vec<Vec<Point>>,
    pub lines: Vec<Vec<Point>>,
    /// Largest distance between this outline and the real shape, mm.
    pub slack: f64,
    /// Filled area, for the utilization figure.
    pub area: f64,
}

struct Piece {
    pts: Vec<Point>,
    closed: bool,
    cut: bool,
    bbox: Rect,
    area: f64,
}

fn bbox_of(pts: &[Point]) -> Rect {
    let mut r = Rect::new(f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for p in pts {
        r.x0 = r.x0.min(p.x);
        r.y0 = r.y0.min(p.y);
        r.x1 = r.x1.max(p.x);
        r.y1 = r.y1.max(p.y);
    }
    r
}

fn rect_inside(outer: &Rect, inner: &Rect) -> bool {
    outer.x0 <= inner.x0 && outer.y0 <= inner.y0 && outer.x1 >= inner.x1 && outer.y1 >= inner.y1
}

fn edges(pts: &[Point], closed: bool) -> impl Iterator<Item = (Point, Point)> + '_ {
    let n = pts.len();
    let m = if closed && n > 2 { n } else { n.saturating_sub(1) };
    (0..m).map(move |i| (pts[i], pts[(i + 1) % n]))
}

/// True when every point of `inner` is inside `ring` and no edges cross. Gives up (false)
/// when the test would be too slow.
fn piece_inside(inner: &Piece, ring: &Piece) -> bool {
    if !rect_inside(&ring.bbox, &inner.bbox) || inner.pts.len().saturating_mul(ring.pts.len()) > 4_000_000 {
        return false;
    }
    if !inner.pts.iter().all(|p| in_ring(&ring.pts, *p)) {
        return false;
    }
    for (a, b) in edges(&inner.pts, inner.closed) {
        let eb = Rect::from_points(a, b);
        for (c, d) in edges(&ring.pts, true) {
            if Rect::from_points(c, d).overlaps(eb) && seg_dist(a, b, c, d) <= 0.0 {
                return false;
            }
        }
    }
    true
}

/// Douglas–Peucker on an open chain: keeps the ends and every point needed to stay within
/// `eps`.
fn dp_open(pts: &[Point], eps: f64) -> Vec<Point> {
    let n = pts.len();
    if n <= 2 {
        return pts.to_vec();
    }
    let mut keep = vec![false; n];
    keep[0] = true;
    keep[n - 1] = true;
    let mut stack = vec![(0usize, n - 1)];
    while let Some((i, j)) = stack.pop() {
        if j <= i + 1 {
            continue;
        }
        let (a, b) = (pts[i], pts[j]);
        let mut far = (0.0, i);
        for (k, p) in pts.iter().enumerate().take(j).skip(i + 1) {
            let d = seg_dist(*p, *p, a, b);
            if d > far.0 {
                far = (d, k);
            }
        }
        if far.0 > eps {
            keep[far.1] = true;
            stack.push((i, far.1));
            stack.push((far.1, j));
        }
    }
    pts.iter().zip(keep).filter(|(_, k)| *k).map(|(p, _)| *p).collect()
}

/// Douglas–Peucker on a closed ring: split at the first point and the point farthest from it.
fn dp_ring(pts: &[Point], eps: f64) -> Vec<Point> {
    let n = pts.len();
    if n <= 3 {
        return pts.to_vec();
    }
    let far = (1..n).max_by(|&a, &b| pts[a].distance(pts[0]).total_cmp(&pts[b].distance(pts[0]))).unwrap_or(n / 2);
    let mut first = dp_open(&pts[..=far], eps);
    let mut second: Vec<Point> = pts[far..].to_vec();
    second.push(pts[0]);
    let second = dp_open(&second, eps);
    first.pop();
    first.extend(&second[..second.len() - 1]);
    first
}

fn total(o: &Outline) -> usize {
    o.solids.iter().chain(&o.holes).chain(&o.lines).map(Vec::len).sum()
}

fn simplified(o: &Outline, eps: f64) -> Outline {
    let mut out = Outline { slack: o.slack + eps, area: o.area, ..Default::default() };
    for r in &o.solids {
        let s = dp_ring(r, eps);
        if s.len() >= 3 { out.solids.push(s) } else { out.lines.push(s) }
    }
    for r in &o.holes {
        let s = dp_ring(r, eps);
        // A hole that collapses is simply not used.
        if s.len() >= 3 {
            out.holes.push(s);
        }
    }
    for l in &o.lines {
        out.lines.push(dp_open(l, eps));
    }
    out
}

/// The convex hull of everything, simplified until it fits the cap; the bounding box when even
/// that fails.
fn hull_fallback(o: &Outline, tol: f64) -> Outline {
    let pts: Vec<Point> = o.solids.iter().chain(&o.lines).flatten().copied().collect();
    let hull = convex_hull_points(&pts);
    let base = Outline { slack: o.slack, area: o.area, ..Default::default() };
    let mut h = base.clone();
    if hull.len() >= 3 {
        h.solids.push(hull);
    } else {
        h.lines.push(hull);
    }
    let b = bbox_of(&pts);
    let diag = (b.width().hypot(b.height())).max(tol);
    let mut eps = tol;
    while total(&h) > VERTEX_CAP && eps < diag * 0.05 {
        let s = simplified(&h, eps);
        if total(&s) <= VERTEX_CAP {
            return s;
        }
        eps *= 2.0;
    }
    if total(&h) <= VERTEX_CAP {
        return h;
    }
    let mut r = base;
    r.solids.push(vec![Point::new(b.x0, b.y0), Point::new(b.x1, b.y0), Point::new(b.x1, b.y1), Point::new(b.x0, b.y1)]);
    r
}

/// Builds a part's outline. `surround`: an obstacle's closed contour whose box covers this
/// rectangle (within the tolerance) counts by its line only. `None` when the part has no points at all.
pub(super) fn outline(cuts: &[BezPath], marks: &[BezPath], tol: f64, allow_holes: bool, surround: Option<Rect>) -> Option<Outline> {
    let mut pieces: Vec<Piece> = Vec::new();
    for (paths, cut) in [(cuts, true), (marks, false)] {
        for path in paths {
            for pl in flatten_to_polylines(path, tol) {
                let mut pts = pl.points;
                let n = pts.len();
                let mut closed = pl.closed || (n >= 3 && pts[0].distance(pts[n - 1]) <= tol);
                if closed && n >= 2 && pts[0].distance(pts[pts.len() - 1]) <= 1e-12 {
                    pts.pop();
                }
                let area = polygon_signed_area(&pts).abs();
                let bbox = bbox_of(&pts);
                let around = surround.is_some_and(|u| rect_inside(&bbox, &u.inflate(-tol, -tol)));
                if closed && (pts.len() < 3 || area <= tol * tol * 1e-3 || around) {
                    // A flat or surrounding "closed" contour is just its line.
                    if pts.len() >= 2 {
                        pts.push(pts[0]);
                    }
                    closed = false;
                }
                if !pts.is_empty() {
                    pieces.push(Piece { pts, closed, cut, bbox, area: if closed { area } else { 0.0 } });
                }
            }
        }
    }
    if pieces.is_empty() {
        return None;
    }
    let n = pieces.len();
    // Which closed pieces hold which: only for parts with a sane number of pieces.
    let nested = n <= 4000;
    let inside = |i: usize, j: usize| i != j && pieces[j].closed && pieces[j].area > pieces[i].area && piece_inside(&pieces[i], &pieces[j]);
    let mut hole = vec![false; n];
    if allow_holes && nested {
        for i in 0..n {
            let p = &pieces[i];
            if !p.cut || !p.closed || !(0..n).any(|j| inside(i, j)) {
                continue;
            }
            // Usable only when nothing of the part reaches into it.
            let empty = (0..n)
                .all(|k| k == i || !pieces[k].bbox.overlaps(p.bbox) || !pieces[k].pts.iter().any(|q| in_ring(&p.pts, *q)) && !crosses(&pieces[k], p));
            hole[i] = empty;
        }
    }
    let mut out = Outline { slack: tol, ..Default::default() };
    let mut covered_area = 0.0;
    for i in 0..n {
        let p = &pieces[i];
        if hole[i] {
            out.holes.push(p.pts.clone());
            continue;
        }
        let covered = nested && (0..n).any(|j| !hole[j] && inside(i, j));
        if covered {
            continue;
        }
        if p.closed {
            covered_area += p.area;
            out.solids.push(p.pts.clone());
        } else {
            out.lines.push(p.pts.clone());
        }
    }
    let hole_area: f64 = out.holes.iter().map(|h| polygon_signed_area(h).abs()).sum();
    out.area = (covered_area - hole_area).max(0.0);
    if total(&out) <= VERTEX_CAP {
        return Some(out);
    }
    if out.solids.iter().chain(&out.holes).chain(&out.lines).any(|c| c.len() > HUGE) {
        return Some(hull_fallback(&out, tol));
    }
    let b = bbox_of(&out.solids.iter().chain(&out.lines).flatten().copied().collect::<Vec<_>>());
    let diag = b.width().hypot(b.height()).max(tol);
    let mut eps = tol * 0.5;
    while eps <= diag * 0.01 {
        let s = simplified(&out, eps);
        if total(&s) <= VERTEX_CAP {
            return Some(s);
        }
        eps *= 2.0;
    }
    Some(hull_fallback(&out, tol))
}

fn crosses(a: &Piece, b: &Piece) -> bool {
    if a.pts.len().saturating_mul(b.pts.len()) > 4_000_000 {
        return true;
    }
    edges(&a.pts, a.closed).any(|(p, q)| {
        let e = Rect::from_points(p, q);
        edges(&b.pts, true).any(|(c, d)| Rect::from_points(c, d).overlaps(e) && seg_dist(p, q, c, d) <= 0.0)
    })
}

/// One rotation of an outline, moved so its bounding box starts at the origin.
#[derive(Clone, Debug)]
pub(super) struct Oriented {
    pub pts: Vec<Point>,
    /// Edges as point index pairs (a lone point is an edge from itself to itself).
    pub segs: Vec<(u32, u32)>,
    pub rings: Vec<Range<usize>>,
    pub holes: Vec<Range<usize>>,
    /// First point of every contour.
    pub probes: Vec<usize>,
    pub size: Vec2,
    /// From the part's coordinates to these.
    pub xf: Affine,
    pub slack: f64,
}

impl Oriented {
    /// True when local point `p` is inside the solid part (in a ring, not in a hole).
    pub fn contains(&self, p: Point) -> bool {
        if p.x < 0.0 || p.y < 0.0 || p.x > self.size.x || p.y > self.size.y {
            return false;
        }
        self.rings.iter().any(|r| in_ring(&self.pts[r.clone()], p)) && !self.holes.iter().any(|r| in_ring(&self.pts[r.clone()], p))
    }
}

pub(super) fn orient(o: &Outline, deg: f64) -> Oriented {
    let rot = Affine::rotate(deg.to_radians());
    let mut pts: Vec<Point> = Vec::with_capacity(total(o));
    let mut segs = Vec::new();
    let mut rings = Vec::new();
    let mut holes = Vec::new();
    let mut probes = Vec::new();
    let mut add = |c: &[Point], closed: bool, pts: &mut Vec<Point>| {
        let start = pts.len();
        pts.extend(c.iter().map(|p| rot * *p));
        let n = c.len();
        probes.push(start);
        if n == 1 {
            segs.push((start as u32, start as u32));
        }
        for i in 0..n.saturating_sub(1) {
            segs.push(((start + i) as u32, (start + i + 1) as u32));
        }
        if closed && n > 2 {
            segs.push(((start + n - 1) as u32, start as u32));
        }
        start..start + n
    };
    for r in &o.solids {
        rings.push(add(r, true, &mut pts));
    }
    for r in &o.holes {
        holes.push(add(r, true, &mut pts));
    }
    for l in &o.lines {
        add(l, false, &mut pts);
    }
    let b = bbox_of(&pts);
    let shift = Vec2::new(-b.x0, -b.y0);
    for p in &mut pts {
        *p += shift;
    }
    Oriented { pts, segs, rings, holes, probes, size: Vec2::new(b.width(), b.height()), xf: Affine::translate(shift) * rot, slack: o.slack }
}
