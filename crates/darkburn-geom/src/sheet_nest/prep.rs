//! Turns parts into flattened, simplified outlines for the true-shape nester.
//!
//! A part becomes closed rings (solid), usable holes and open lines. Contours that lie inside
//! a solid ring (engraving inside a cut outline, say) are dropped: the ring already covers them.
//! Parts with more than [`VERTEX_CAP`] points are simplified; the simplification error, plus
//! the flattening tolerance, is the part's slack, added to every distance it must keep, so the
//! real shapes keep the gap even though the nester works on simpler ones. Parts that cannot be
//! simplified enough fall back to their convex hull, and in the end to their bounding box.
//!
//! The work spent on one part is bounded by a [`Budget`]: when finding the holes would cost more,
//! the part simply has none (every closed contour is solid); when simplifying would, it falls
//! back to its hull or box. Both are safe, only less tight.

use super::space::{in_ring, seg_dist};
use crate::hull::convex_hull_points;
use crate::path::{flatten_to_polylines, polygon_signed_area};
use kurbo::{Affine, BezPath, Point, Rect, Vec2};
use std::ops::Range;

/// Most points a part keeps for the nester.
pub(super) const VERTEX_CAP: usize = 360;
/// Contours longer than this skip straight to the hull (simplifying them would be slow).
const HUGE: usize = 50_000;
/// Point-against-edge steps one part may spend on finding its holes, and again on simplifying.
pub(super) const PART_BUDGET: u64 = 60_000_000;

/// Work left for one step of preparing a part.
#[derive(Clone, Copy, Debug)]
pub(super) struct Budget(u64);

impl Budget {
    pub fn new(steps: u64) -> Self {
        Budget(steps)
    }

    /// Spends `n` steps; false (and nothing left) when there are not that many.
    fn take(&mut self, n: usize) -> bool {
        let n = n.max(1) as u64;
        if self.0 < n {
            self.0 = 0;
            return false;
        }
        self.0 -= n;
        true
    }
}

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
    /// Which input path it came from (cuts first, then marks).
    src: usize,
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

/// True when every point of `inner` is inside `ring` and no edges cross; `None` when the budget
/// runs out first.
fn piece_inside(inner: &Piece, ring: &Piece, b: &mut Budget) -> Option<bool> {
    if !rect_inside(&ring.bbox, &inner.bbox) {
        return Some(false);
    }
    for p in &inner.pts {
        if !b.take(ring.pts.len()) {
            return None;
        }
        if !in_ring(&ring.pts, *p) {
            return Some(false);
        }
    }
    crosses(inner, ring, b).map(|c| !c)
}

/// True when an edge of `a` touches an edge of the closed ring `b`; `None` when the budget runs
/// out first.
fn crosses(a: &Piece, b: &Piece, budget: &mut Budget) -> Option<bool> {
    for (p, q) in edges(&a.pts, a.closed) {
        if !budget.take(b.pts.len()) {
            return None;
        }
        let e = Rect::from_points(p, q);
        if edges(&b.pts, true).any(|(c, d)| Rect::from_points(c, d).overlaps(e) && seg_dist(p, q, c, d) <= 0.0) {
            return Some(true);
        }
    }
    Some(false)
}

/// True when piece `i` lies inside the closed piece `j`, which is bigger.
fn inside(pieces: &[Piece], i: usize, j: usize, b: &mut Budget) -> Option<bool> {
    if i == j || !pieces[j].closed || pieces[j].area <= pieces[i].area {
        return Some(false);
    }
    piece_inside(&pieces[i], &pieces[j], b)
}

/// True when the closed contour `i`, which nothing crosses, lies under a filled mark: inside an
/// odd number of the closed rings of one mark path (even-odd, so a fill with a window cut out
/// of it does not cover what is in the window).
fn under_mark(pieces: &[Piece], i: usize, b: &mut Budget) -> Option<bool> {
    let h = &pieces[i];
    let probe = h.pts[0];
    let mut counts: Vec<(usize, usize)> = Vec::new();
    for m in pieces.iter().filter(|m| !m.cut && m.closed && rect_inside(&m.bbox, &h.bbox)) {
        if !b.take(m.pts.len()) {
            return None;
        }
        if in_ring(&m.pts, probe) {
            match counts.iter_mut().find(|c| c.0 == m.src) {
                Some(c) => c.1 += 1,
                None => counts.push((m.src, 1)),
            }
        }
    }
    Some(counts.iter().any(|c| c.1 % 2 == 1))
}

/// Which pieces are usable holes and which lie inside a solid ring (and so add nothing);
/// `None` when the budget runs out.
fn analyse(pieces: &[Piece], allow_holes: bool, b: &mut Budget) -> Option<(Vec<bool>, Vec<bool>)> {
    let n = pieces.len();
    let mut hole = vec![false; n];
    if allow_holes {
        for i in 0..n {
            let p = &pieces[i];
            if !p.cut || !p.closed {
                continue;
            }
            let mut held = false;
            for j in 0..n {
                if inside(pieces, i, j, b)? {
                    held = true;
                    break;
                }
            }
            if !held || under_mark(pieces, i, b)? {
                continue;
            }
            // Usable only when nothing of the part reaches into it.
            let mut empty = true;
            for (k, q) in pieces.iter().enumerate() {
                if k == i || !q.bbox.overlaps(p.bbox) {
                    continue;
                }
                if !b.take(q.pts.len().saturating_mul(p.pts.len())) {
                    return None;
                }
                if q.pts.iter().any(|x| in_ring(&p.pts, *x)) || crosses(q, p, b)? {
                    empty = false;
                    break;
                }
            }
            hole[i] = empty;
        }
    }
    let mut covered = vec![false; n];
    for i in 0..n {
        if hole[i] {
            continue;
        }
        for j in (0..n).filter(|&j| !hole[j]) {
            if inside(pieces, i, j, b)? {
                covered[i] = true;
                break;
            }
        }
    }
    Some((hole, covered))
}

/// Douglas–Peucker on an open chain: keeps the ends and every point needed to stay within
/// `eps`. `None` when the budget runs out.
fn dp_open(pts: &[Point], eps: f64, b: &mut Budget) -> Option<Vec<Point>> {
    let n = pts.len();
    if n <= 2 {
        return Some(pts.to_vec());
    }
    let mut keep = vec![false; n];
    keep[0] = true;
    keep[n - 1] = true;
    let mut stack = vec![(0usize, n - 1)];
    while let Some((i, j)) = stack.pop() {
        if j <= i + 1 {
            continue;
        }
        if !b.take(j - i) {
            return None;
        }
        let (a, z) = (pts[i], pts[j]);
        let mut far = (0.0, i);
        for (k, p) in pts.iter().enumerate().take(j).skip(i + 1) {
            let d = seg_dist(*p, *p, a, z);
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
    Some(pts.iter().zip(keep).filter(|(_, k)| *k).map(|(p, _)| *p).collect())
}

/// Douglas–Peucker on a closed ring: split at the first point and the point farthest from it.
fn dp_ring(pts: &[Point], eps: f64, b: &mut Budget) -> Option<Vec<Point>> {
    let n = pts.len();
    if n <= 3 {
        return Some(pts.to_vec());
    }
    if !b.take(n) {
        return None;
    }
    let far = (1..n).max_by(|&a, &c| pts[a].distance(pts[0]).total_cmp(&pts[c].distance(pts[0]))).unwrap_or(n / 2);
    let mut first = dp_open(&pts[..=far], eps, b)?;
    let mut second: Vec<Point> = pts[far..].to_vec();
    second.push(pts[0]);
    let second = dp_open(&second, eps, b)?;
    first.pop();
    first.extend(&second[..second.len() - 1]);
    Some(first)
}

fn total(o: &Outline) -> usize {
    o.solids.iter().chain(&o.holes).chain(&o.lines).map(Vec::len).sum()
}

fn simplified(o: &Outline, eps: f64, b: &mut Budget) -> Option<Outline> {
    let mut out = Outline { slack: o.slack + eps, area: o.area, ..Default::default() };
    for r in &o.solids {
        let s = dp_ring(r, eps, b)?;
        if s.len() >= 3 { out.solids.push(s) } else { out.lines.push(s) }
    }
    for r in &o.holes {
        let s = dp_ring(r, eps, b)?;
        // A hole that collapses is simply not used.
        if s.len() >= 3 {
            out.holes.push(s);
        }
    }
    for l in &o.lines {
        out.lines.push(dp_open(l, eps, b)?);
    }
    Some(out)
}

/// The convex hull of everything, simplified until it fits the cap; the bounding box when even
/// that fails.
fn hull_fallback(o: &Outline, tol: f64, b: &mut Budget) -> Outline {
    let pts: Vec<Point> = o.solids.iter().chain(&o.lines).flatten().copied().collect();
    let hull = convex_hull_points(&pts);
    let base = Outline { slack: o.slack, area: o.area, ..Default::default() };
    let mut h = base.clone();
    if hull.len() >= 3 {
        h.solids.push(hull);
    } else {
        h.lines.push(hull);
    }
    let bx = bbox_of(&pts);
    let diag = (bx.width().hypot(bx.height())).max(tol);
    let mut eps = tol;
    while total(&h) > VERTEX_CAP && eps < diag * 0.05 {
        let Some(s) = simplified(&h, eps, b) else { break };
        if total(&s) <= VERTEX_CAP {
            return s;
        }
        eps *= 2.0;
    }
    if total(&h) <= VERTEX_CAP {
        return h;
    }
    let mut r = base;
    r.solids.push(vec![Point::new(bx.x0, bx.y0), Point::new(bx.x1, bx.y0), Point::new(bx.x1, bx.y1), Point::new(bx.x0, bx.y1)]);
    r
}

/// Builds a part's outline. `surround`: a closed cut contour whose box covers one of these
/// rectangles (within the tolerance) is a drawn material outline and counts by its line only;
/// marks (fills, images) never do, since they burn what they cover. `budget` bounds the work
/// (see the module notes). `None` when the part has no points at all.
pub(super) fn outline(cuts: &[BezPath], marks: &[BezPath], tol: f64, allow_holes: bool, surround: &[Rect], budget: u64) -> Option<Outline> {
    let mut pieces: Vec<Piece> = Vec::new();
    for (src, (path, cut)) in cuts.iter().map(|p| (p, true)).chain(marks.iter().map(|p| (p, false))).enumerate() {
        for pl in flatten_to_polylines(path, tol) {
            let mut pts = pl.points;
            let n = pts.len();
            let mut closed = pl.closed || (n >= 3 && pts[0].distance(pts[n - 1]) <= tol);
            if closed && n >= 2 && pts[0].distance(pts[pts.len() - 1]) <= 1e-12 {
                pts.pop();
            }
            let area = polygon_signed_area(&pts).abs();
            let bbox = bbox_of(&pts);
            let around = cut && surround.iter().any(|u| rect_inside(&bbox, &u.inflate(-tol, -tol)));
            if closed && (pts.len() < 3 || area <= tol * tol * 1e-3 || around) {
                // A flat or surrounding "closed" contour is just its line.
                if pts.len() >= 2 {
                    pts.push(pts[0]);
                }
                closed = false;
            }
            if !pts.is_empty() {
                pieces.push(Piece { pts, closed, cut, src, bbox, area: if closed { area } else { 0.0 } });
            }
        }
    }
    if pieces.is_empty() {
        return None;
    }
    let n = pieces.len();
    // Which closed pieces hold which: only for parts with a sane number of pieces, and only as
    // far as the budget goes. Otherwise there are no holes and every closed piece is solid.
    let mut b = Budget::new(budget);
    let found = if n <= 4000 { analyse(&pieces, allow_holes, &mut b) } else { None };
    let (hole, covered) = found.unwrap_or_else(|| (vec![false; n], vec![false; n]));
    let mut out = Outline { slack: tol, ..Default::default() };
    let mut covered_area = 0.0;
    for (i, p) in pieces.iter().enumerate() {
        if hole[i] {
            out.holes.push(p.pts.clone());
            continue;
        }
        if covered[i] {
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
    let mut b = Budget::new(budget);
    if out.solids.iter().chain(&out.holes).chain(&out.lines).any(|c| c.len() > HUGE) {
        return Some(hull_fallback(&out, tol, &mut b));
    }
    let bx = bbox_of(&out.solids.iter().chain(&out.lines).flatten().copied().collect::<Vec<_>>());
    let diag = bx.width().hypot(bx.height()).max(tol);
    let mut eps = tol * 0.5;
    while eps <= diag * 0.01 {
        let Some(s) = simplified(&out, eps, &mut b) else { break };
        if total(&s) <= VERTEX_CAP {
            return Some(s);
        }
        eps *= 2.0;
    }
    Some(hull_fallback(&out, tol, &mut b))
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
