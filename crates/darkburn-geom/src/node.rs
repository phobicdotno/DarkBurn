//! Node-level edits: insert and delete nodes, break segments, line/curve conversion,
//! corner/smooth nodes, trim and extend at intersections, align and move nodes.
//!
//! Nodes are addressed with [`NodeRef`]: the subpath index in [`contours`] order and the
//! node index within it (node `i` is the start of segment `i`; see [`Contour`]).

use crate::path::{Contour, contours, from_contours, nearest_on_contour, seg_tangent, split_seg};
use crate::query::seg_intersections;
use kurbo::{BezPath, CubicBez, Line, ParamCurve, ParamCurveArclen, PathSeg, Point, Vec2};

/// Address of one node.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NodeRef {
    /// Subpath index in [`contours`] order.
    pub subpath: usize,
    /// Node index within the subpath.
    pub node: usize,
}

impl NodeRef {
    /// Shorthand constructor.
    pub fn new(subpath: usize, node: usize) -> Self {
        Self { subpath, node }
    }
}

/// Why a node edit could not be done.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeError {
    /// The subpath index does not exist.
    NoSubpath,
    /// The node index does not exist in that subpath.
    NoNode,
    /// The segment index does not exist in that subpath.
    NoSegment,
    /// The operation needs an end node of an open subpath.
    NotOpenEnd,
    /// Nothing to intersect with in range.
    NoIntersection,
}

impl std::fmt::Display for NodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            NodeError::NoSubpath => "no such subpath",
            NodeError::NoNode => "no such node",
            NodeError::NoSegment => "no such segment",
            NodeError::NotOpenEnd => "node is not the end of an open subpath",
            NodeError::NoIntersection => "no intersection found",
        };
        f.write_str(s)
    }
}

impl std::error::Error for NodeError {}

/// Replaces contour `sub` with the contours returned by `f` (zero, one or more).
fn edit_contour(path: &BezPath, sub: usize, f: impl FnOnce(&Contour) -> Result<Vec<Contour>, NodeError>) -> Result<BezPath, NodeError> {
    let mut cs = contours(path);
    let c = cs.get(sub).ok_or(NodeError::NoSubpath)?;
    let repl = f(c)?;
    cs.splice(sub..sub + 1, repl.into_iter().filter(|c| !c.segs.is_empty()));
    Ok(from_contours(cs.iter()))
}

fn as_cubic(s: &PathSeg) -> CubicBez {
    match s {
        PathSeg::Line(l) => CubicBez::new(l.p0, l.p0.lerp(l.p1, 1.0 / 3.0), l.p0.lerp(l.p1, 2.0 / 3.0), l.p1),
        other => other.to_cubic(),
    }
}

/// Index of the segment ending at node `i`, if any.
fn incoming(c: &Contour, i: usize) -> Option<usize> {
    let n = c.segs.len();
    if c.closed {
        Some((i + n - 1) % n)
    } else if i > 0 && i <= n {
        Some(i - 1)
    } else {
        None
    }
}

/// Index of the segment starting at node `i`, if any.
fn outgoing(c: &Contour, i: usize) -> Option<usize> {
    if i < c.segs.len() { Some(i) } else { None }
}

fn check_node(c: &Contour, i: usize) -> Result<(), NodeError> {
    if i < c.node_count() { Ok(()) } else { Err(NodeError::NoNode) }
}

/// Inserts a node on segment `seg` of subpath `subpath` at parameter `t` (clamped to
/// `0..=1`). Curves are split exactly, so the shape does not change.
pub fn insert_node(path: &BezPath, subpath: usize, seg: usize, t: f64) -> Result<BezPath, NodeError> {
    edit_contour(path, subpath, |c| {
        let s = c.segs.get(seg).ok_or(NodeError::NoSegment)?;
        let t = t.clamp(0.0, 1.0);
        let mut c = c.clone();
        if t > 1e-12 && t < 1.0 - 1e-12 {
            let (a, b) = split_seg(s, t);
            c.segs[seg] = a;
            c.segs.insert(seg + 1, b);
        }
        Ok(vec![c])
    })
}

/// Merges two consecutive segments into one. Two lines give a line; otherwise one cubic
/// whose outer handles keep their directions and are scaled by how much longer the merged
/// segment is than each original part (an approximation of both).
fn merge_segs(a: &PathSeg, b: &PathSeg) -> PathSeg {
    if let (PathSeg::Line(la), PathSeg::Line(lb)) = (a, b) {
        return PathSeg::Line(Line::new(la.p0, lb.p1));
    }
    let ca = as_cubic(a);
    let cb = as_cubic(b);
    let la = a.arclen(1e-6).max(1e-12);
    let lb = b.arclen(1e-6).max(1e-12);
    let total = la + lb;
    let h1 = (ca.p1 - ca.p0) * (total / la);
    let h2 = (cb.p2 - cb.p3) * (total / lb);
    PathSeg::Cubic(CubicBez::new(ca.p0, ca.p0 + h1, cb.p3 + h2, cb.p3))
}

/// Deletes a node, joining its neighbours (see [`merge_segs`] behaviour: two lines become
/// one line, curves become one approximating cubic). End nodes of open subpaths drop
/// their segment. A subpath left with fewer than two nodes (open) or fewer than two
/// segments (closed) is removed.
pub fn delete_node(path: &BezPath, node: NodeRef) -> Result<BezPath, NodeError> {
    edit_contour(path, node.subpath, |c| {
        check_node(c, node.node)?;
        let n = c.segs.len();
        let i = node.node;
        let mut c = c.clone();
        if c.closed {
            if n <= 2 {
                return Ok(vec![]);
            }
            let p = (i + n - 1) % n;
            let merged = merge_segs(&c.segs[p], &c.segs[i]);
            c.segs[p] = merged;
            c.segs.remove(i);
        } else {
            if n <= 1 {
                return Ok(vec![]);
            }
            if i == 0 {
                c.segs.remove(0);
            } else if i == n {
                c.segs.pop();
            } else {
                let merged = merge_segs(&c.segs[i - 1], &c.segs[i]);
                c.segs[i - 1] = merged;
                c.segs.remove(i);
            }
        }
        Ok(vec![c])
    })
}

/// Deletes one segment (break). A closed subpath becomes open, starting right after the
/// removed segment; an open subpath splits in two (empty pieces vanish).
pub fn delete_segment(path: &BezPath, subpath: usize, seg: usize) -> Result<BezPath, NodeError> {
    edit_contour(path, subpath, |c| {
        if seg >= c.segs.len() {
            return Err(NodeError::NoSegment);
        }
        if c.closed {
            let mut segs: Vec<PathSeg> = c.segs[seg + 1..].to_vec();
            segs.extend_from_slice(&c.segs[..seg]);
            Ok(vec![Contour::new(segs, false)])
        } else {
            Ok(vec![Contour::new(c.segs[..seg].to_vec(), false), Contour::new(c.segs[seg + 1..].to_vec(), false)])
        }
    })
}

/// Breaks a subpath at a node: a closed subpath opens there (it starts and ends at the
/// node); an open subpath splits into two at the node. Breaking an open end is a no-op.
pub fn break_at_node(path: &BezPath, node: NodeRef) -> Result<BezPath, NodeError> {
    edit_contour(path, node.subpath, |c| {
        check_node(c, node.node)?;
        let i = node.node;
        if c.closed {
            let mut segs = c.segs.clone();
            segs.rotate_left(i);
            Ok(vec![Contour::new(segs, false)])
        } else if i == 0 || i == c.segs.len() {
            Ok(vec![c.clone()])
        } else {
            Ok(vec![Contour::new(c.segs[..i].to_vec(), false), Contour::new(c.segs[i..].to_vec(), false)])
        }
    })
}

/// Turns a segment into a straight line between its end points.
pub fn segment_to_line(path: &BezPath, subpath: usize, seg: usize) -> Result<BezPath, NodeError> {
    edit_contour(path, subpath, |c| {
        let s = c.segs.get(seg).ok_or(NodeError::NoSegment)?;
        let mut c = c.clone();
        c.segs[seg] = PathSeg::Line(Line::new(s.start(), s.end()));
        Ok(vec![c])
    })
}

/// Turns a line into a cubic curve with handles at 1/3 and 2/3 (same shape, now
/// editable as a curve). Curves are left as they are.
pub fn segment_to_curve(path: &BezPath, subpath: usize, seg: usize) -> Result<BezPath, NodeError> {
    edit_contour(path, subpath, |c| {
        let s = c.segs.get(seg).ok_or(NodeError::NoSegment)?;
        let mut c = c.clone();
        c.segs[seg] = PathSeg::Cubic(as_cubic(s));
        Ok(vec![c])
    })
}

/// Marks a node as a corner. Paths carry no node-type flag, so handles are already
/// independent; this only validates the reference and returns the path unchanged.
pub fn set_node_corner(path: &BezPath, node: NodeRef) -> Result<BezPath, NodeError> {
    edit_contour(path, node.subpath, |c| {
        check_node(c, node.node)?;
        Ok(vec![c.clone()])
    })
}

/// Makes a node smooth: the incoming and outgoing handles become collinear along the
/// average of their directions, keeping their lengths. Line neighbours are turned into
/// curves first. End nodes of open subpaths are left unchanged.
pub fn set_node_smooth(path: &BezPath, node: NodeRef) -> Result<BezPath, NodeError> {
    edit_contour(path, node.subpath, |c| {
        check_node(c, node.node)?;
        let (Some(si), Some(so)) = (incoming(c, node.node), outgoing(c, node.node)) else {
            return Ok(vec![c.clone()]);
        };
        let mut c = c.clone();
        let mut ci = as_cubic(&c.segs[si]);
        let mut co = as_cubic(&c.segs[so]);
        let v = co.p0;
        let mut h_in = ci.p2 - v;
        let mut h_out = co.p1 - v;
        if h_in.hypot() < 1e-12 {
            h_in = (ci.p0 - v) / 3.0;
        }
        if h_out.hypot() < 1e-12 {
            h_out = (co.p3 - v) / 3.0;
        }
        let (li, lo) = (h_in.hypot(), h_out.hypot());
        if li < 1e-12 || lo < 1e-12 {
            return Ok(vec![c]);
        }
        let mut d = h_out / lo - h_in / li;
        if d.hypot() < 1e-9 {
            // Handles point the same way (a cusp): use the perpendicular.
            d = Vec2::new(-h_out.y, h_out.x);
        }
        let d = d.normalize();
        ci.p2 = v - d * li;
        co.p1 = v + d * lo;
        c.segs[si] = PathSeg::Cubic(ci);
        c.segs[so] = PathSeg::Cubic(co);
        Ok(vec![c])
    })
}

fn move_node_in(c: &mut Contour, i: usize, delta: Vec2) {
    if let Some(si) = incoming(c, i) {
        c.segs[si] = match c.segs[si] {
            PathSeg::Line(l) => PathSeg::Line(Line::new(l.p0, l.p1 + delta)),
            PathSeg::Quad(q) => PathSeg::Quad(kurbo::QuadBez::new(q.p0, q.p1, q.p2 + delta)),
            PathSeg::Cubic(cb) => PathSeg::Cubic(CubicBez::new(cb.p0, cb.p1, cb.p2 + delta, cb.p3 + delta)),
        };
    }
    if let Some(so) = outgoing(c, i) {
        c.segs[so] = match c.segs[so] {
            PathSeg::Line(l) => PathSeg::Line(Line::new(l.p0 + delta, l.p1)),
            PathSeg::Quad(q) => PathSeg::Quad(kurbo::QuadBez::new(q.p0 + delta, q.p1, q.p2)),
            PathSeg::Cubic(cb) => PathSeg::Cubic(CubicBez::new(cb.p0 + delta, cb.p1 + delta, cb.p2, cb.p3)),
        };
    }
}

/// Moves a node by `delta`, carrying its adjacent handles along.
pub fn move_node(path: &BezPath, node: NodeRef, delta: Vec2) -> Result<BezPath, NodeError> {
    edit_contour(path, node.subpath, |c| {
        check_node(c, node.node)?;
        let mut c = c.clone();
        move_node_in(&mut c, node.node, delta);
        Ok(vec![c])
    })
}

/// How [`align_nodes`] lines nodes up (Y up, so `Top` is the largest y).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    /// Common x = smallest x.
    Left,
    /// Common x = largest x.
    Right,
    /// Common y = largest y.
    Top,
    /// Common y = smallest y.
    Bottom,
    /// Common x = middle of the nodes' x range (a vertical line).
    CenterH,
    /// Common y = middle of the nodes' y range (a horizontal line).
    CenterV,
}

/// Moves the selected nodes (with their handles) onto a common x or y.
pub fn align_nodes(path: &BezPath, nodes: &[NodeRef], align: Align) -> Result<BezPath, NodeError> {
    let mut cs = contours(path);
    let mut sel: Vec<NodeRef> = nodes.to_vec();
    sel.sort_by_key(|n| (n.subpath, n.node));
    sel.dedup();
    let mut pts = Vec::with_capacity(sel.len());
    for n in &sel {
        let c = cs.get(n.subpath).ok_or(NodeError::NoSubpath)?;
        pts.push(c.node(n.node).ok_or(NodeError::NoNode)?);
    }
    if pts.is_empty() {
        return Ok(path.clone());
    }
    let minx = pts.iter().map(|p| p.x).fold(f64::INFINITY, f64::min);
    let maxx = pts.iter().map(|p| p.x).fold(f64::NEG_INFINITY, f64::max);
    let miny = pts.iter().map(|p| p.y).fold(f64::INFINITY, f64::min);
    let maxy = pts.iter().map(|p| p.y).fold(f64::NEG_INFINITY, f64::max);
    for (n, p) in sel.iter().zip(pts) {
        let delta = match align {
            Align::Left => Vec2::new(minx - p.x, 0.0),
            Align::Right => Vec2::new(maxx - p.x, 0.0),
            Align::CenterH => Vec2::new((minx + maxx) * 0.5 - p.x, 0.0),
            Align::Top => Vec2::new(0.0, maxy - p.y),
            Align::Bottom => Vec2::new(0.0, miny - p.y),
            Align::CenterV => Vec2::new(0.0, (miny + maxy) * 0.5 - p.y),
        };
        move_node_in(&mut cs[n.subpath], n.node, delta);
    }
    Ok(from_contours(cs.iter()))
}

/// Segments of `c` between global parameters `u0 < u1` (`u = segment index + t`).
fn extract(c: &Contour, u0: f64, u1: f64) -> Vec<PathSeg> {
    let mut out = Vec::new();
    if u1 - u0 <= 1e-9 {
        return out;
    }
    let k0 = u0.floor().max(0.0) as usize;
    let k1 = (u1.ceil() as usize).min(c.segs.len());
    for (k, seg) in c.segs.iter().enumerate().take(k1).skip(k0) {
        let t0 = (u0 - k as f64).max(0.0);
        let t1 = (u1 - k as f64).min(1.0);
        if t1 - t0 > 1e-9 {
            out.push(if t0 <= 0.0 && t1 >= 1.0 { *seg } else { seg.subsegment(t0..t1) });
        }
    }
    out
}

/// Parameters (`seg + t`) where contour `c` crosses `others` or itself.
fn crossing_params(c: &Contour, others: &[Contour]) -> Vec<f64> {
    let n = c.segs.len();
    let mut us = Vec::new();
    for (i, s) in c.segs.iter().enumerate() {
        for o in others {
            for os in &o.segs {
                for (t, _) in seg_intersections(s, os) {
                    us.push(i as f64 + t);
                }
            }
        }
        for (j, s2) in c.segs.iter().enumerate() {
            if i == j {
                continue;
            }
            let adjacent_next = j == i + 1 || (c.closed && j == (i + 1) % n);
            let adjacent_prev = j + 1 == i || (c.closed && i == (j + 1) % n);
            for (t, t2) in seg_intersections(s, s2) {
                // Skip the shared node between neighbours.
                if (adjacent_next && t > 1.0 - 1e-7 && t2 < 1e-7) || (adjacent_prev && t < 1e-7 && t2 > 1.0 - 1e-7) {
                    continue;
                }
                us.push(i as f64 + t);
            }
        }
    }
    us.sort_by(f64::total_cmp);
    us.dedup_by(|a, b| (*a - *b).abs() < 1e-7);
    us
}

/// Node-mode trim: on the subpath nearest `at`, removes the piece around `at` that lies
/// between the nearest crossings (with the path itself and `others`) on either side.
/// On an open subpath, a side without a crossing is trimmed to the end. A closed subpath
/// with fewer than two crossings is removed entirely.
pub fn trim_at_intersection(path: &BezPath, others: &[BezPath], at: Point) -> Result<BezPath, NodeError> {
    let cs = contours(path);
    let (ci, sp) = cs
        .iter()
        .enumerate()
        .filter_map(|(i, c)| nearest_on_contour(c, at).map(|(sp, d)| (i, sp, d)))
        .min_by(|a, b| a.2.total_cmp(&b.2))
        .map(|(i, sp, _)| (i, sp))
        .ok_or(NodeError::NoSubpath)?;
    let c = &cs[ci];
    let mut cutters: Vec<Contour> = cs.iter().enumerate().filter(|(i, _)| *i != ci).map(|(_, c)| c.clone()).collect();
    for o in others {
        cutters.extend(contours(o));
    }
    let n = c.segs.len() as f64;
    let mut us = crossing_params(c, &cutters);
    if c.closed {
        // u = 0 and u = n are the same point on a closed contour.
        if us.len() > 1 && us[0] < 1e-7 && us[us.len() - 1] > n - 1e-7 {
            us.pop();
        }
    }
    let u = sp.seg as f64 + sp.t;
    let lower = us.iter().rev().copied().find(|x| *x < u);
    let upper = us.iter().copied().find(|x| *x > u);
    let kept: Vec<Contour> = if c.closed {
        if us.len() < 2 {
            vec![]
        } else {
            let lo = lower.unwrap_or(us[us.len() - 1]);
            let hi = upper.unwrap_or(us[0]);
            // Keep from `hi` forward (wrapping) to `lo`.
            let segs = if hi < lo {
                extract(c, hi, lo)
            } else {
                let mut s = extract(c, hi, n);
                s.extend(extract(c, 0.0, lo));
                s
            };
            vec![Contour::new(segs, false)]
        }
    } else {
        let lo = lower.unwrap_or(0.0);
        let hi = upper.unwrap_or(n);
        vec![Contour::new(extract(c, 0.0, lo), false), Contour::new(extract(c, hi, n), false)]
    };
    let mut out = cs.clone();
    out.splice(ci..ci + 1, kept.into_iter().filter(|c| !c.segs.is_empty()));
    Ok(from_contours(out.iter()))
}

/// Extends an end node of an open subpath straight along its end tangent until it meets
/// `others` (or the path itself) within `max_dist`. A line end is lengthened; a curve end
/// gets a new line segment. Errors when the node is not an open end or nothing is hit.
pub fn extend_to_intersection(path: &BezPath, others: &[BezPath], node: NodeRef, max_dist: f64) -> Result<BezPath, NodeError> {
    let cs = contours(path);
    let c = cs.get(node.subpath).ok_or(NodeError::NoSubpath)?;
    check_node(c, node.node)?;
    let n = c.segs.len();
    if c.closed || (node.node != 0 && node.node != n) {
        return Err(NodeError::NotOpenEnd);
    }
    let at_end = node.node == n;
    let (p, dir, end_seg) =
        if at_end { (c.segs[n - 1].end(), seg_tangent(&c.segs[n - 1], 1.0), n - 1) } else { (c.segs[0].start(), -seg_tangent(&c.segs[0], 0.0), 0) };
    if dir.hypot2() == 0.0 {
        return Err(NodeError::NoIntersection);
    }
    let ray = Line::new(p, p + dir * max_dist);
    let mut best: Option<f64> = None;
    let mut consider = |s: &PathSeg| {
        for i in s.intersect_line(ray) {
            if i.line_t * max_dist > 1e-7 && best.is_none_or(|b| i.line_t < b) {
                best = Some(i.line_t);
            }
        }
    };
    for (ci, oc) in cs.iter().enumerate() {
        for (si, s) in oc.segs.iter().enumerate() {
            if ci == node.subpath && si == end_seg {
                continue;
            }
            consider(s);
        }
    }
    for o in others {
        for oc in contours(o) {
            for s in &oc.segs {
                consider(s);
            }
        }
    }
    let t = best.ok_or(NodeError::NoIntersection)?;
    let q = ray.eval(t);
    edit_contour(path, node.subpath, |c| {
        let mut c = c.clone();
        if at_end {
            match c.segs[n - 1] {
                PathSeg::Line(l) => c.segs[n - 1] = PathSeg::Line(Line::new(l.p0, q)),
                _ => c.segs.push(PathSeg::Line(Line::new(p, q))),
            }
        } else {
            match c.segs[0] {
                PathSeg::Line(l) => c.segs[0] = PathSeg::Line(Line::new(q, l.p1)),
                _ => c.segs.insert(0, PathSeg::Line(Line::new(q, p))),
            }
        }
        Ok(vec![c])
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path::{length, signed_area};
    use kurbo::{Circle, Rect, Shape};

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> BezPath {
        Rect::new(x0, y0, x1, y1).to_path(0.1)
    }
    fn line(a: (f64, f64), b: (f64, f64)) -> BezPath {
        let mut p = BezPath::new();
        p.move_to(a);
        p.line_to(b);
        p
    }
    fn polyline(pts: &[(f64, f64)]) -> BezPath {
        let mut p = BezPath::new();
        p.move_to(pts[0]);
        for q in &pts[1..] {
            p.line_to(*q);
        }
        p
    }

    #[test]
    fn insert_keeps_geometry() {
        let c = Circle::new((0.0, 0.0), 10.0).to_path(1e-6);
        let before = contours(&c)[0].clone();
        let p = insert_node(&c, 0, 0, 0.3).unwrap();
        let after = contours(&p)[0].clone();
        assert_eq!(after.segs.len(), before.segs.len() + 1);
        for k in 0..=10 {
            let t = k as f64 / 10.0;
            let q = before.segs[0].eval(t);
            let d = crate::query::distance_to_path(&p, q);
            assert!(d < 1e-6, "{d}");
        }
        assert!((signed_area(&p) - signed_area(&c)).abs() < 1e-9);
        assert_eq!(insert_node(&c, 0, 99, 0.5), Err(NodeError::NoSegment));
        assert_eq!(insert_node(&c, 3, 0, 0.5), Err(NodeError::NoSubpath));
        // t at an end changes nothing.
        assert_eq!(contours(&insert_node(&c, 0, 0, 0.0).unwrap())[0].segs.len(), before.segs.len());
    }

    #[test]
    fn delete_node_closed_and_open() {
        let r = rect(0.0, 0.0, 10.0, 10.0);
        let p = delete_node(&r, NodeRef::new(0, 1)).unwrap();
        let c = &contours(&p)[0];
        assert_eq!(c.segs.len(), 3);
        assert!((signed_area(&p).abs() - 50.0).abs() < 1e-9);
        // Deleting node 0 of a closed contour.
        let p0 = delete_node(&r, NodeRef::new(0, 0)).unwrap();
        assert!((signed_area(&p0).abs() - 50.0).abs() < 1e-9);
        let pl = polyline(&[(0.0, 0.0), (5.0, 5.0), (10.0, 0.0)]);
        let q = delete_node(&pl, NodeRef::new(0, 1)).unwrap();
        assert_eq!(contours(&q)[0].segs, vec![PathSeg::Line(Line::new((0.0, 0.0), (10.0, 0.0)))]);
        let q = delete_node(&pl, NodeRef::new(0, 2)).unwrap();
        assert_eq!(contours(&q)[0].node_count(), 2);
        // Down to nothing.
        let l = line((0.0, 0.0), (1.0, 0.0));
        assert!(delete_node(&l, NodeRef::new(0, 0)).unwrap().is_empty());
        assert_eq!(delete_node(&l, NodeRef::new(0, 5)), Err(NodeError::NoNode));
    }

    #[test]
    fn delete_node_between_curves_makes_one_cubic() {
        let c = Circle::new((0.0, 0.0), 10.0).to_path(1e-6);
        let n0 = contours(&c)[0].segs.len();
        let p = delete_node(&c, NodeRef::new(0, 1)).unwrap();
        let cs = contours(&p);
        assert_eq!(cs[0].segs.len(), n0 - 1);
        assert!(matches!(cs[0].segs[0], PathSeg::Cubic(_)));
    }

    #[test]
    fn delete_segment_closed_and_open() {
        let r = rect(0.0, 0.0, 10.0, 10.0);
        let p = delete_segment(&r, 0, 1).unwrap();
        let cs = contours(&p);
        assert_eq!(cs.len(), 1);
        assert!(!cs[0].closed);
        assert_eq!(cs[0].segs.len(), 3);
        assert_eq!(cs[0].start(), Some(contours(&r)[0].segs[2].start()));
        let pl = polyline(&[(0.0, 0.0), (1.0, 0.0), (2.0, 0.0), (3.0, 0.0)]);
        let q = delete_segment(&pl, 0, 1).unwrap();
        assert_eq!(contours(&q).len(), 2);
        let q = delete_segment(&pl, 0, 0).unwrap();
        assert_eq!(contours(&q).len(), 1);
        assert_eq!(delete_segment(&pl, 0, 3), Err(NodeError::NoSegment));
    }

    #[test]
    fn break_at_node_variants() {
        let r = rect(0.0, 0.0, 10.0, 10.0);
        let p = break_at_node(&r, NodeRef::new(0, 2)).unwrap();
        let c = &contours(&p)[0];
        assert!(!c.closed);
        assert_eq!(c.segs.len(), 4);
        assert_eq!(c.start(), c.end());
        assert_eq!(c.start(), contours(&r)[0].node(2));
        let pl = polyline(&[(0.0, 0.0), (1.0, 0.0), (2.0, 0.0)]);
        assert_eq!(contours(&break_at_node(&pl, NodeRef::new(0, 1)).unwrap()).len(), 2);
        assert_eq!(contours(&break_at_node(&pl, NodeRef::new(0, 0)).unwrap()).len(), 1);
    }

    #[test]
    fn line_curve_conversion() {
        let r = rect(0.0, 0.0, 10.0, 10.0);
        let p = segment_to_curve(&r, 0, 0).unwrap();
        assert!(matches!(contours(&p)[0].segs[0], PathSeg::Cubic(_)));
        assert!((signed_area(&p) - signed_area(&r)).abs() < 1e-9);
        let back = segment_to_line(&p, 0, 0).unwrap();
        assert!(matches!(contours(&back)[0].segs[0], PathSeg::Line(_)));
        assert_eq!(set_node_corner(&r, NodeRef::new(0, 0)).unwrap(), r);
        assert_eq!(set_node_corner(&r, NodeRef::new(0, 4)), Err(NodeError::NoNode));
    }

    #[test]
    fn smooth_node_makes_handles_collinear() {
        let pl = polyline(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)]);
        let p = set_node_smooth(&pl, NodeRef::new(0, 1)).unwrap();
        let c = &contours(&p)[0];
        let (PathSeg::Cubic(a), PathSeg::Cubic(b)) = (c.segs[0], c.segs[1]) else { panic!("expected cubics") };
        let v = a.p3;
        assert_eq!(v, Point::new(10.0, 0.0));
        let h_in = a.p2 - v;
        let h_out = b.p1 - v;
        assert!(h_in.cross(h_out).abs() < 1e-9);
        assert!(h_in.dot(h_out) < 0.0);
        // End nodes stay as they are.
        assert_eq!(set_node_smooth(&pl, NodeRef::new(0, 0)).unwrap(), pl);
    }

    #[test]
    fn move_and_align_nodes() {
        let r = rect(0.0, 0.0, 10.0, 10.0);
        let p = move_node(&r, NodeRef::new(0, 0), Vec2::new(-1.0, -1.0)).unwrap();
        assert_eq!(contours(&p)[0].node(0), Some(Point::new(-1.0, -1.0)));
        assert!(crate::path::is_closed(&p, 0.0));
        let zig = polyline(&[(0.0, 0.0), (5.0, 3.0), (10.0, 1.0)]);
        let a = align_nodes(&zig, &[NodeRef::new(0, 0), NodeRef::new(0, 1), NodeRef::new(0, 2)], Align::Top).unwrap();
        for n in contours(&a)[0].nodes() {
            assert_eq!(n.y, 3.0);
        }
        let b = align_nodes(&zig, &[NodeRef::new(0, 0), NodeRef::new(0, 2)], Align::CenterH).unwrap();
        let ns = contours(&b)[0].nodes();
        assert_eq!(ns[0].x, 5.0);
        assert_eq!(ns[2].x, 5.0);
        assert_eq!(ns[1], Point::new(5.0, 3.0));
        let l = align_nodes(&zig, &[NodeRef::new(0, 1), NodeRef::new(0, 2)], Align::Left).unwrap();
        assert_eq!(contours(&l)[0].nodes()[2].x, 5.0);
        assert_eq!(align_nodes(&zig, &[NodeRef::new(1, 0)], Align::Left), Err(NodeError::NoSubpath));
        assert_eq!(align_nodes(&zig, &[], Align::Left).unwrap(), zig);
    }

    #[test]
    fn trim_between_crossing_lines() {
        // A horizontal line crossed by two verticals: trimming the middle leaves two stubs.
        let h = line((0.0, 0.0), (10.0, 0.0));
        let v1 = line((3.0, -5.0), (3.0, 5.0));
        let v2 = line((7.0, -5.0), (7.0, 5.0));
        let t = trim_at_intersection(&h, &[v1.clone(), v2.clone()], Point::new(5.0, 0.1)).unwrap();
        let cs = contours(&t);
        assert_eq!(cs.len(), 2);
        assert!((length(&t) - 6.0).abs() < 1e-9);
        // Trimming the end piece leaves the rest.
        let t2 = trim_at_intersection(&h, &[v1.clone(), v2], Point::new(9.0, 0.0)).unwrap();
        assert!((length(&t2) - 7.0).abs() < 1e-9);
        // Nothing crossing: the whole subpath goes.
        let t3 = trim_at_intersection(&h, &[], Point::new(5.0, 0.0)).unwrap();
        assert!(t3.is_empty());
        assert_eq!(trim_at_intersection(&BezPath::new(), &[v1], Point::ZERO), Err(NodeError::NoSubpath));
    }

    #[test]
    fn trim_closed_shape() {
        let r = rect(0.0, 0.0, 10.0, 10.0);
        let v = line((5.0, -5.0), (5.0, 15.0));
        // Remove the right half of the square outline.
        let t = trim_at_intersection(&r, &[v], Point::new(10.0, 5.0)).unwrap();
        let cs = contours(&t);
        assert_eq!(cs.len(), 1);
        assert!(!cs[0].closed);
        assert!((length(&t) - 20.0).abs() < 1e-9);
        for n in cs[0].nodes() {
            assert!(n.x <= 5.0 + 1e-9);
        }
    }

    #[test]
    fn trim_self_intersection() {
        // An open "alpha" loop crossing itself once.
        let p = polyline(&[(0.0, 0.0), (10.0, 10.0), (10.0, 0.0), (0.0, 10.0)]);
        let t = trim_at_intersection(&p, &[], Point::new(10.0, 5.0)).unwrap();
        // The loop between the crossing at (5,5) on both passes is removed.
        assert_eq!(contours(&t).len(), 2);
        assert!((length(&t) - 2.0 * 50f64.sqrt()).abs() < 1e-6);
    }

    #[test]
    fn extend_line_to_rectangle() {
        let l = line((0.0, 5.0), (2.0, 5.0));
        let r = rect(10.0, 0.0, 20.0, 10.0);
        let p = extend_to_intersection(&l, std::slice::from_ref(&r), NodeRef::new(0, 1), 100.0).unwrap();
        assert_eq!(contours(&p)[0].end(), Some(Point::new(10.0, 5.0)));
        assert_eq!(contours(&p)[0].segs.len(), 1);
        // Extending the start goes the other way and hits nothing.
        assert_eq!(extend_to_intersection(&l, std::slice::from_ref(&r), NodeRef::new(0, 0), 100.0), Err(NodeError::NoIntersection));
        // Too short a reach.
        assert_eq!(extend_to_intersection(&l, std::slice::from_ref(&r), NodeRef::new(0, 1), 5.0), Err(NodeError::NoIntersection));
        // Not an end node / closed path.
        assert_eq!(extend_to_intersection(&r, &[], NodeRef::new(0, 0), 5.0), Err(NodeError::NotOpenEnd));
        // Start extension to a shape on the left.
        let left = rect(-20.0, 0.0, -10.0, 10.0);
        let q = extend_to_intersection(&l, &[left], NodeRef::new(0, 0), 100.0).unwrap();
        assert_eq!(contours(&q)[0].start(), Some(Point::new(-10.0, 5.0)));
    }

    #[test]
    fn extend_curve_end_adds_line() {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.curve_to((1.0, 0.0), (2.0, 0.0), (3.0, 0.0));
        let wall = line((8.0, -5.0), (8.0, 5.0));
        let e = extend_to_intersection(&p, &[wall], NodeRef::new(0, 1), 50.0).unwrap();
        let c = &contours(&e)[0];
        assert_eq!(c.segs.len(), 2);
        assert!(c.end().unwrap().distance(Point::new(8.0, 0.0)) < 1e-9);
    }
}
