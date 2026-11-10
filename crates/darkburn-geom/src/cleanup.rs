//! Path clean-up: closing paths, auto-join, deleting duplicates and overlapping lines,
//! break apart, optimize (fit lines and circular arcs) and flattening.

use crate::path::{Contour, append_contour, contours, flatten_contour, from_contours};
use crate::query::{circle_through, containment, distance_to_path};
use kurbo::{BezPath, CubicBez, Line, ParamCurve, ParamCurveNearest, PathSeg, Point, Rect};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Small segment helpers
// ---------------------------------------------------------------------------

/// Moves the start point of a segment, dragging the first handle along.
fn with_start(seg: PathSeg, p: Point) -> PathSeg {
    match crate::path::normalize_seg(seg) {
        PathSeg::Line(l) => PathSeg::Line(Line::new(p, l.p1)),
        PathSeg::Cubic(c) => {
            let d = p - c.p0;
            PathSeg::Cubic(CubicBez::new(p, c.p1 + d, c.p2, c.p3))
        }
        s => s,
    }
}

/// Moves the end point of a segment, dragging the last handle along.
fn with_end(seg: PathSeg, p: Point) -> PathSeg {
    match crate::path::normalize_seg(seg) {
        PathSeg::Line(l) => PathSeg::Line(Line::new(l.p0, p)),
        PathSeg::Cubic(c) => {
            let d = p - c.p3;
            PathSeg::Cubic(CubicBez::new(c.p0, c.p1, c.p2 + d, p))
        }
        s => s,
    }
}

/// A contour can be closed meaningfully when it has more than one segment or a curve
/// (a single straight line closed on itself has no area).
fn closable(c: &Contour) -> bool {
    c.segs.len() >= 2 || c.segs.iter().any(|s| !matches!(s, PathSeg::Line(_)))
}

/// Closes an open contour, snapping its end to its start when they are within `snap`
/// and adding a closing line otherwise.
fn close_contour(c: &mut Contour, snap: f64) {
    let (Some(s), Some(e)) = (c.start(), c.end()) else { return };
    if c.closed || !closable(c) {
        return;
    }
    if s.distance(e) <= snap {
        let n = c.segs.len();
        c.segs[n - 1] = with_end(c.segs[n - 1], s);
    } else {
        c.segs.push(PathSeg::Line(Line::new(e, s)));
    }
    c.closed = true;
}

// ---------------------------------------------------------------------------
// Close path
// ---------------------------------------------------------------------------

/// Closes every open subpath. When a subpath already ends on its start point it is just
/// marked closed (no zero-length segment); otherwise a straight closing line is added.
/// A subpath made of one straight line is left open (closing it would have no area).
pub fn close_path(path: &BezPath) -> BezPath {
    let mut cs = contours(path);
    for c in &mut cs {
        close_contour(c, 1e-9);
    }
    from_contours(cs.iter())
}

/// Closes only the open subpaths whose ends are within `tol` of each other, snapping the
/// end point onto the start point. Other subpaths are unchanged.
pub fn close_paths_with_tolerance(path: &BezPath, tol: f64) -> BezPath {
    let mut cs = contours(path);
    for c in &mut cs {
        if let (Some(s), Some(e)) = (c.start(), c.end())
            && !c.closed
            && s.distance(e) <= tol
        {
            close_contour(c, tol);
        }
    }
    from_contours(cs.iter())
}

// ---------------------------------------------------------------------------
// Auto-join
// ---------------------------------------------------------------------------

struct Grid {
    cell: f64,
    map: HashMap<(i64, i64), Vec<usize>>,
}

impl Grid {
    fn new(cell: f64) -> Self {
        Self { cell: cell.max(1e-9), map: HashMap::new() }
    }
    fn key(&self, p: Point) -> (i64, i64) {
        ((p.x / self.cell).floor() as i64, (p.y / self.cell).floor() as i64)
    }
    fn insert(&mut self, p: Point, id: usize) {
        let k = self.key(p);
        self.map.entry(k).or_default().push(id);
    }
    fn near(&self, p: Point) -> impl Iterator<Item = usize> + '_ {
        let (kx, ky) = self.key(p);
        (-1..=1).flat_map(move |dx| (-1..=1).map(move |dy| (kx + dx, ky + dy))).flat_map(|k| self.map.get(&k).into_iter().flatten().copied())
    }
}

/// Joins open subpaths whose end points lie within `tol` of each other into longer
/// chains, reversing pieces as needed. A chain whose two ends meet within `tol` is closed.
///
/// Safety at junctions: an end point is joined only when exactly one other end point
/// (of another piece) lies within `tol` *and* that end point in turn sees only this one.
/// Where three or more end points meet (a T or Y junction) none of them is joined, and an
/// end point is never joined to the interior of a segment.
///
/// Output: for each input path, its closed subpaths stay together as one path (in input
/// order); every joined or untouched open chain then follows as its own path.
///
/// `progress(done, total)` is called periodically; returning `false` cancels and the
/// function returns `None`.
pub fn auto_join(paths: &[BezPath], tol: f64, progress: &mut dyn FnMut(usize, usize) -> bool) -> Option<Vec<BezPath>> {
    let mut out = Vec::new();
    let mut pieces: Vec<Contour> = Vec::new();
    for p in paths {
        let mut kept = BezPath::new();
        for c in contours(p) {
            if c.closed {
                append_contour(&mut kept, &c);
            } else {
                pieces.push(c);
            }
        }
        if !kept.is_empty() {
            out.push(kept);
        }
    }
    let n = pieces.len();
    let total = 2 * n;
    let end_pos = |e: usize| -> Point {
        let c = &pieces[e / 2];
        if e.is_multiple_of(2) { c.start().unwrap_or_default() } else { c.end().unwrap_or_default() }
    };

    let mut grid = Grid::new(tol);
    for e in 0..2 * n {
        grid.insert(end_pos(e), e);
    }

    // Unique candidate for every end point (None when zero or ambiguous).
    let mut cand: Vec<Option<usize>> = vec![None; 2 * n];
    for (e, slot) in cand.iter_mut().enumerate() {
        if e.is_multiple_of(512) && !progress(e / 2, total) {
            return None;
        }
        let p = end_pos(e);
        let mut found: Option<usize> = None;
        let mut count = 0;
        for f in grid.near(p) {
            if f / 2 == e / 2 || end_pos(f).distance(p) > tol {
                continue;
            }
            count += 1;
            found = Some(f);
            if count > 1 {
                break;
            }
        }
        if count == 1 {
            *slot = found;
        }
    }
    let link: Vec<Option<usize>> = (0..2 * n).map(|e| cand[e].filter(|&f| cand[f] == Some(e))).collect();

    let mut visited = vec![false; n];
    let mut done = n;
    let walk = |first: usize, visited: &mut [bool]| -> (Contour, usize) {
        let mut segs: Vec<PathSeg> = Vec::new();
        let mut ent = first;
        let mut closed = false;
        let mut count = 0;
        loop {
            let p = ent / 2;
            visited[p] = true;
            count += 1;
            let piece = if ent.is_multiple_of(2) { pieces[p].clone() } else { pieces[p].reversed() };
            let mut psegs = piece.segs;
            if let Some(prev) = segs.last().map(|s| s.end()) {
                psegs[0] = with_start(psegs[0], prev);
            }
            segs.extend(psegs);
            let exit = ent ^ 1;
            match link[exit] {
                Some(f) if !visited[f / 2] => ent = f,
                Some(f) if f == first => {
                    closed = true;
                    break;
                }
                _ => break,
            }
        }
        let mut c = Contour::new(segs, false);
        if closed {
            if let Some(s) = c.start() {
                let k = c.segs.len() - 1;
                c.segs[k] = with_end(c.segs[k], s);
            }
            c.closed = true;
        } else if let (Some(s), Some(e)) = (c.start(), c.end())
            && s.distance(e) <= tol
        {
            close_contour(&mut c, tol);
        }
        (c, count)
    };

    let mut chains: Vec<Contour> = Vec::new();
    // Open chains: start from an end that has no link.
    for p in 0..n {
        if visited[p] {
            continue;
        }
        let start = if link[2 * p].is_none() {
            Some(2 * p)
        } else if link[2 * p + 1].is_none() {
            Some(2 * p + 1)
        } else {
            None
        };
        if let Some(s) = start {
            let (c, k) = walk(s, &mut visited);
            chains.push(c);
            if (done + k) / 256 != done / 256 && !progress(done + k, total) {
                return None;
            }
            done += k;
        }
    }
    // Remaining pieces form cycles.
    for p in 0..n {
        if !visited[p] {
            let (c, k) = walk(2 * p, &mut visited);
            chains.push(c);
            if (done + k) / 256 != done / 256 && !progress(done + k, total) {
                return None;
            }
            done += k;
        }
    }
    if !progress(total, total) {
        return None;
    }
    out.extend(chains.iter().map(Contour::to_bezpath));
    Some(out)
}

// ---------------------------------------------------------------------------
// Delete duplicates
// ---------------------------------------------------------------------------

struct DupInfo {
    bbox: Option<Rect>,
    length: f64,
    closed: Vec<bool>,
    samples: Vec<Point>,
}

fn dup_info(p: &BezPath, tol: f64) -> DupInfo {
    let cs = contours(p);
    let mut samples = Vec::new();
    for c in &cs {
        let pl = flatten_contour(c, (tol * 0.25).max(1e-6));
        samples.extend(pl.points.iter().copied());
        for w in pl.points.windows(2) {
            samples.push(w[0].midpoint(w[1]));
        }
    }
    let mut closed: Vec<bool> = cs.iter().map(|c| c.closed).collect();
    closed.sort();
    DupInfo { bbox: crate::path::bounding_box(p), length: crate::path::length(p), closed, samples }
}

fn same_shape(a: &BezPath, ia: &DupInfo, b: &BezPath, ib: &DupInfo, tol: f64) -> bool {
    let (Some(ba), Some(bb)) = (ia.bbox, ib.bbox) else { return false };
    if (ba.x0 - bb.x0).abs() > tol || (ba.y0 - bb.y0).abs() > tol || (ba.x1 - bb.x1).abs() > tol || (ba.y1 - bb.y1).abs() > tol {
        return false;
    }
    if ia.closed != ib.closed || (ia.length - ib.length).abs() > 8.0 * tol + 1e-6 * ia.length {
        return false;
    }
    ia.samples.iter().all(|&p| distance_to_path(b, p) <= tol) && ib.samples.iter().all(|&p| distance_to_path(a, p) <= tol)
}

/// Removes paths that trace the same geometry as an earlier path within `tol`, regardless
/// of start point, direction or how the outline is split into nodes. Returns the kept
/// paths (input order) and the number removed. Empty paths are kept as they are.
pub fn delete_duplicates(paths: &[BezPath], tol: f64) -> (Vec<BezPath>, usize) {
    let info: Vec<DupInfo> = paths.iter().map(|p| dup_info(p, tol)).collect();
    let mut order: Vec<usize> = (0..paths.len()).collect();
    order.sort_by(|a, b| {
        let ka = info[*a].bbox.map(|r| r.x0).unwrap_or(f64::INFINITY);
        let kb = info[*b].bbox.map(|r| r.x0).unwrap_or(f64::INFINITY);
        ka.total_cmp(&kb)
    });
    let mut removed = vec![false; paths.len()];
    for (oi, &i) in order.iter().enumerate() {
        let Some(bi) = info[i].bbox else { continue };
        for &j in &order[oi + 1..] {
            let Some(bj) = info[j].bbox else { break };
            if bj.x0 - bi.x0 > tol {
                break;
            }
            // Keep the earlier one in input order.
            let (keep, drop) = if i < j { (i, j) } else { (j, i) };
            if removed[drop] || removed[keep] {
                continue;
            }
            if same_shape(&paths[keep], &info[keep], &paths[drop], &info[drop], tol) {
                removed[drop] = true;
            }
        }
    }
    let kept: Vec<BezPath> = paths.iter().enumerate().filter(|(i, _)| !removed[*i]).map(|(_, p)| p.clone()).collect();
    let count = removed.iter().filter(|r| **r).count();
    (kept, count)
}

// ---------------------------------------------------------------------------
// Delete overlapping lines
// ---------------------------------------------------------------------------

enum Item {
    Seg(PathSeg),
    Gap,
}

fn perp_dist(l: &Line, p: Point) -> f64 {
    let v = l.p1 - l.p0;
    let len = v.hypot();
    if len < 1e-12 {
        return p.distance(l.p0);
    }
    ((p - l.p0).cross(v) / len).abs()
}

/// Removes straight line segments, or the parts of them, that lie on top of an earlier
/// straight segment (from any earlier path or earlier in the same path) within `dist`.
/// Remaining parts shorter than `dist` are dropped too. A closed path that loses a piece
/// becomes one or more open paths. Curves are kept as they are.
///
/// Typical use: two boxes sharing an edge, so the shared edge is cut only once.
/// Cost is quadratic in the number of line segments (bounding-box filtered).
pub fn delete_overlapping_lines(paths: &[BezPath], dist: f64) -> Vec<BezPath> {
    let mut accepted: Vec<(Line, Rect)> = Vec::new();
    let mut out = Vec::new();
    for p in paths {
        let mut result = BezPath::new();
        for c in contours(p) {
            let mut items: Vec<Item> = Vec::new();
            let mut any_gap = false;
            for seg in &c.segs {
                let PathSeg::Line(l) = *seg else {
                    items.push(Item::Seg(*seg));
                    continue;
                };
                let v = l.p1 - l.p0;
                let len2 = v.hypot2();
                if len2 < 1e-24 {
                    continue;
                }
                let lb = Rect::from_points(l.p0, l.p1).inflate(dist, dist);
                let mut covered: Vec<(f64, f64)> = Vec::new();
                for (a, ab) in &accepted {
                    if !ab.overlaps(lb) || perp_dist(a, l.p0) > dist || perp_dist(a, l.p1) > dist {
                        continue;
                    }
                    let t0 = (a.p0 - l.p0).dot(v) / len2;
                    let t1 = (a.p1 - l.p0).dot(v) / len2;
                    let (lo, hi) = (t0.min(t1).max(0.0), t0.max(t1).min(1.0));
                    if hi > lo {
                        covered.push((lo, hi));
                    }
                }
                accepted.push((l, Rect::from_points(l.p0, l.p1)));
                if covered.is_empty() {
                    items.push(Item::Seg(*seg));
                    continue;
                }
                covered.sort_by(|a, b| a.0.total_cmp(&b.0));
                let min_t = dist / len2.sqrt();
                let mut t = 0.0;
                let push_piece = |a: f64, b: f64, items: &mut Vec<Item>| {
                    if b - a > min_t.max(1e-12) {
                        items.push(Item::Seg(PathSeg::Line(Line::new(l.eval(a), l.eval(b)))));
                    }
                };
                for (lo, hi) in covered {
                    if lo > t {
                        push_piece(t, lo, &mut items);
                    }
                    if hi > t {
                        items.push(Item::Gap);
                        any_gap = true;
                        t = hi;
                    }
                }
                if t < 1.0 {
                    push_piece(t, 1.0, &mut items);
                }
            }
            let mut runs: Vec<Vec<PathSeg>> = vec![Vec::new()];
            for it in &items {
                match it {
                    Item::Seg(s) => runs.last_mut().expect("non-empty").push(*s),
                    Item::Gap => runs.push(Vec::new()),
                }
            }
            if c.closed && !any_gap {
                append_contour(&mut result, &Contour::new(runs.concat(), true));
                continue;
            }
            if c.closed && runs.len() > 1 {
                // The run before the first gap continues the run after the last gap.
                let first = runs.remove(0);
                runs.last_mut().expect("non-empty").extend(first);
            }
            for r in runs.into_iter().filter(|r| !r.is_empty()) {
                append_contour(&mut result, &Contour::new(r, false));
            }
        }
        out.push(result);
    }
    out
}

// ---------------------------------------------------------------------------
// Break apart
// ---------------------------------------------------------------------------

/// Splits a path into its subpaths. With `keep_holes_with_outer`, closed subpaths that sit
/// inside another closed subpath stay with their immediate outer contour as holes; islands
/// inside holes become shapes of their own. Open subpaths always become separate paths.
pub fn break_apart(path: &BezPath, keep_holes_with_outer: bool, tol: f64) -> Vec<BezPath> {
    let cs = contours(path);
    if !keep_holes_with_outer {
        return cs.iter().map(Contour::to_bezpath).collect();
    }
    let closed_idx: Vec<usize> = (0..cs.len()).filter(|i| cs[*i].is_closed_within(tol)).collect();
    let closed_paths: Vec<BezPath> = closed_idx.iter().map(|i| cs[*i].to_bezpath()).collect();
    let tree = containment(&closed_paths, tol);
    // (first original index, path) so the output keeps the input order.
    let mut groups: Vec<(usize, BezPath)> = Vec::new();
    for g in tree.fill_groups() {
        let mut p = BezPath::new();
        for &k in &g {
            append_contour(&mut p, &cs[closed_idx[k]]);
        }
        groups.push((closed_idx[g[0]], p));
    }
    for (i, c) in cs.iter().enumerate() {
        if !c.is_closed_within(tol) {
            groups.push((i, c.to_bezpath()));
        }
    }
    groups.sort_by_key(|g| g.0);
    groups.into_iter().map(|g| g.1).collect()
}

// ---------------------------------------------------------------------------
// Optimize
// ---------------------------------------------------------------------------

/// Node counts before and after [`optimize`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct OptimizeReport {
    pub nodes_before: usize,
    pub nodes_after: usize,
}

/// Counts nodes in a path (sum of [`Contour::node_count`]).
pub fn node_count(path: &BezPath) -> usize {
    contours(path).iter().map(Contour::node_count).sum()
}

fn dist_to_line_seg(a: Point, b: Point, p: Point) -> f64 {
    Line::new(a, b).nearest(p, 1e-12).distance_sq.sqrt()
}

fn line_fits(pts: &[Point], i: usize, j: usize, tol: f64) -> bool {
    (i + 1..j).all(|k| dist_to_line_seg(pts[i], pts[j], pts[k]) <= tol)
}

const MAX_ARC_SWEEP: f64 = 1.5 * std::f64::consts::PI;

/// Returns (centre, radius, sweep) when points i..=j lie on one circular arc within tol.
fn arc_fits(pts: &[Point], i: usize, j: usize, tol: f64) -> Option<(Point, f64, f64)> {
    if j < i + 2 {
        return None;
    }
    let (center, r) = circle_through(pts[i], pts[(i + j) / 2], pts[j])?;
    if !(r.is_finite()) || r > 1e6 {
        return None;
    }
    let mut sweep = 0.0;
    let mut sign = 0.0;
    let mut prev = (pts[i] - center).atan2();
    for (k, &q) in pts.iter().enumerate().take(j + 1).skip(i) {
        if ((q - center).hypot() - r).abs() > tol {
            return None;
        }
        if k == i {
            continue;
        }
        let a = (q - center).atan2();
        let mut d = a - prev;
        while d > std::f64::consts::PI {
            d -= std::f64::consts::TAU;
        }
        while d <= -std::f64::consts::PI {
            d += std::f64::consts::TAU;
        }
        prev = a;
        if d.abs() >= std::f64::consts::FRAC_PI_2 || d == 0.0 {
            return None;
        }
        if sign == 0.0 {
            sign = d.signum();
        } else if d.signum() != sign {
            return None;
        }
        // The original chord must stay within tolerance of the arc.
        if r * (1.0 - (d.abs() * 0.5).cos()) > tol {
            return None;
        }
        sweep += d;
    }
    if sweep.abs() > MAX_ARC_SWEEP {
        return None;
    }
    Some((center, r, sweep))
}

/// Furthest end index reachable from `i` with `fits`, using exponential then binary search.
fn reach(n: usize, i: usize, min_step: usize, fits: &dyn Fn(usize) -> bool) -> Option<usize> {
    if i + min_step >= n || !fits(i + min_step) {
        return None;
    }
    let mut good = i + min_step;
    let mut step = 1;
    let mut bad = None;
    while good + step < n {
        if fits(good + step) {
            good += step;
            step *= 2;
        } else {
            bad = Some(good + step);
            break;
        }
    }
    let mut hi = bad.unwrap_or(n);
    while hi > good + 1 {
        let mid = (good + hi) / 2;
        if fits(mid) {
            good = mid;
        } else {
            hi = mid;
        }
    }
    Some(good)
}

/// Cubic approximations of a circular arc, split so each piece deviates less than `tol`.
fn arc_to_cubics(center: Point, r: f64, a0: f64, sweep: f64, p_start: Point, p_end: Point, tol: f64) -> Vec<PathSeg> {
    let mut n = (sweep.abs() / std::f64::consts::FRAC_PI_2).ceil().max(1.0) as usize;
    loop {
        let th = sweep.abs() / n as f64;
        let s = (th / 4.0).sin();
        let err = r * 4.0 / 27.0 * s.powi(6) / (th / 4.0).cos().powi(2);
        if err <= tol || n > 64 {
            break;
        }
        n += 1;
    }
    let th = sweep / n as f64;
    let k = 4.0 / 3.0 * (th / 4.0).tan();
    let mut out = Vec::with_capacity(n);
    for m in 0..n {
        let s0 = a0 + th * m as f64;
        let s1 = s0 + th;
        let (c0, sn0) = (s0.cos(), s0.sin());
        let (c1, sn1) = (s1.cos(), s1.sin());
        let p0 = if m == 0 { p_start } else { center + kurbo::Vec2::new(c0, sn0) * r };
        let p3 = if m + 1 == n { p_end } else { center + kurbo::Vec2::new(c1, sn1) * r };
        let p1 = p0 + kurbo::Vec2::new(-sn0, c0) * (k * r);
        let p2 = p3 - kurbo::Vec2::new(-sn1, c1) * (k * r);
        out.push(PathSeg::Cubic(CubicBez::new(p0, p1, p2, p3)));
    }
    out
}

fn collinear_lines(a: &PathSeg, b: &PathSeg, tol: f64) -> bool {
    match (a, b) {
        (PathSeg::Line(x), PathSeg::Line(y)) => dist_to_line_seg(x.p0, y.p1, x.p1) <= tol && (x.p1 - x.p0).dot(y.p1 - y.p0) > 0.0,
        _ => false,
    }
}

fn optimize_contour(c: &Contour, tol: f64) -> Option<Contour> {
    let pl = flatten_contour(c, tol * 0.25);
    let mut pts = pl.points;
    if c.closed && pts.len() > 1 {
        pts.push(pts[0]);
    }
    if pts.len() < 2 {
        return None;
    }
    let ft = tol * 0.75;
    let n = pts.len();
    let mut segs: Vec<PathSeg> = Vec::new();
    let mut i = 0;
    while i + 1 < n {
        let line_end = reach(n, i, 1, &|j| line_fits(&pts, i, j, ft)).unwrap_or(i + 1);
        let arc_end = reach(n, i, 2, &|j| arc_fits(&pts, i, j, ft).is_some());
        match arc_end {
            Some(j) if j > line_end => {
                let (center, r, sweep) = arc_fits(&pts, i, j, ft).expect("arc fitted above");
                let a0 = (pts[i] - center).atan2();
                segs.extend(arc_to_cubics(center, r, a0, sweep, pts[i], pts[j], tol * 0.1));
                i = j;
            }
            _ => {
                segs.push(PathSeg::Line(Line::new(pts[i], pts[line_end])));
                i = line_end;
            }
        }
    }
    // A closed contour whose start sits in the middle of a straight run: merge across.
    if c.closed && segs.len() > 2 && collinear_lines(&segs[segs.len() - 1], &segs[0], ft) {
        let last = segs.pop().expect("len > 2");
        segs[0] = PathSeg::Line(Line::new(last.start(), segs[0].end()));
    }
    Some(Contour::new(segs, c.closed))
}

/// Optimizes a path: flattens curves, then refits each subpath with as few straight lines
/// and circular arcs (as cubic Béziers) as possible while staying within `tol` of the
/// original. Closed/open status is kept. A closed subpath may get a new start node when
/// its old start sat in the middle of a straight run. A subpath so small that it would shrink
/// to a point or to a line without area (a hole smaller than `tol`) is kept as it was: only
/// a subpath that had no length to begin with is dropped. Reports node counts before and after.
pub fn optimize(path: &BezPath, tol: f64) -> (BezPath, OptimizeReport) {
    let tol = tol.max(1e-6);
    let cs = contours(path);
    let before: usize = cs.iter().map(Contour::node_count).sum();
    let collapsed = |c: &Contour, o: &Contour| {
        let lines_only = o.segs.iter().all(|s| matches!(s, PathSeg::Line(_)));
        o.length() <= 1e-12 || (c.closed && lines_only && o.segs.len() < 3)
    };
    let out: Vec<Contour> =
        cs.iter().filter_map(|c| optimize_contour(c, tol).map(|o| if collapsed(c, &o) && c.length() > 1e-12 { c.clone() } else { o })).collect();
    let after: usize = out.iter().map(Contour::node_count).sum();
    (from_contours(out.iter()), OptimizeReport { nodes_before: before, nodes_after: after })
}

/// Flattens all curves to straight lines within `tol` (see [`crate::path::flatten`]).
pub fn flatten_curves(path: &BezPath, tol: f64) -> BezPath {
    crate::path::flatten(path, tol)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path::{bounding_box, is_closed, length, signed_area, subpaths};
    use kurbo::{Circle, Shape};

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> BezPath {
        Rect::new(x0, y0, x1, y1).to_path(0.1)
    }
    fn poly(pts: &[(f64, f64)], closed: bool) -> BezPath {
        let mut p = BezPath::new();
        p.move_to(pts[0]);
        for q in &pts[1..] {
            p.line_to(*q);
        }
        if closed {
            p.close_path();
        }
        p
    }
    fn never_cancel() -> impl FnMut(usize, usize) -> bool {
        |_, _| true
    }
    /// Max distance from samples of `a` to `b`, both ways.
    fn hausdorff(a: &BezPath, b: &BezPath) -> f64 {
        let s = |x: &BezPath, y: &BezPath| {
            crate::path::flatten_to_polylines(x, 1e-4).iter().flat_map(|pl| pl.points.clone()).map(|p| distance_to_path(y, p)).fold(0.0, f64::max)
        };
        s(a, b).max(s(b, a))
    }

    #[test]
    fn close_path_adds_line_or_marks_closed() {
        let open = poly(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)], false);
        let c = close_path(&open);
        assert!(is_closed(&c, 0.0));
        assert!((signed_area(&c) - 50.0).abs() < 1e-9);
        assert_eq!(contours(&c)[0].segs.len(), 3);
        // Ends already coincide: no zero-length segment.
        let ring = poly(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 0.0)], false);
        let c = close_path(&ring);
        assert_eq!(contours(&c)[0].segs.len(), 3);
        assert!(contours(&c)[0].closed);
        // Single line stays open; empty stays empty; closed stays unchanged.
        let line = poly(&[(0.0, 0.0), (1.0, 0.0)], false);
        assert!(!contours(&close_path(&line))[0].closed);
        assert!(close_path(&BezPath::new()).is_empty());
        let r = rect(0.0, 0.0, 1.0, 1.0);
        assert_eq!(contours(&close_path(&r)), contours(&r));
    }

    #[test]
    fn close_with_tolerance_snaps_only_near_ends() {
        let mut p = poly(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.05, 0.02)], false);
        p.extend(poly(&[(20.0, 0.0), (30.0, 0.0), (30.0, 10.0)], false));
        let c = close_paths_with_tolerance(&p, 0.1);
        let cs = contours(&c);
        assert!(cs[0].closed);
        assert_eq!(cs[0].segs.len(), 3);
        assert_eq!(cs[0].end(), cs[0].start());
        assert!(!cs[1].closed);
    }

    #[test]
    fn auto_join_chains_and_closes() {
        // A square drawn as four separate lines, some reversed, with small gaps.
        let parts = vec![
            poly(&[(0.0, 0.0), (10.0, 0.0)], false),
            poly(&[(10.0, 10.0), (10.0, 0.01)], false),
            poly(&[(10.0, 10.0), (0.0, 10.0)], false),
            poly(&[(0.0, 9.99), (0.0, 0.0)], false),
            rect(50.0, 50.0, 60.0, 60.0),
        ];
        let out = auto_join(&parts, 0.05, &mut never_cancel()).unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(contours(&out[0]), contours(&parts[4]));
        let cs = contours(&out[1]);
        assert_eq!(cs.len(), 1);
        assert!(cs[0].closed);
        assert_eq!(cs[0].segs.len(), 4);
        assert!((signed_area(&out[1]).abs() - 100.0).abs() < 0.5);
    }

    #[test]
    fn auto_join_open_chain_and_far_pieces() {
        let parts = vec![poly(&[(0.0, 0.0), (5.0, 0.0)], false), poly(&[(5.0, 0.0), (5.0, 5.0)], false), poly(&[(100.0, 0.0), (101.0, 0.0)], false)];
        let out = auto_join(&parts, 0.01, &mut never_cancel()).unwrap();
        assert_eq!(out.len(), 2);
        let c = &contours(&out[0])[0];
        assert!(!c.closed);
        assert_eq!(c.segs.len(), 2);
    }

    #[test]
    fn auto_join_t_junction_safe() {
        // Three lines meet at (0,0): ambiguous, none joined.
        // A fourth line ends on the middle of another line: never joined to the interior.
        let parts = vec![
            poly(&[(0.0, 0.0), (10.0, 0.0)], false),
            poly(&[(0.0, 0.0), (-10.0, 0.0)], false),
            poly(&[(0.0, 0.0), (0.0, 10.0)], false),
            poly(&[(20.0, -5.0), (20.0, 5.0)], false),
            poly(&[(20.0, 0.0), (30.0, 0.0)], false),
        ];
        let out = auto_join(&parts, 0.01, &mut never_cancel()).unwrap();
        assert_eq!(out.len(), 5);
        for p in &out {
            assert_eq!(contours(p)[0].segs.len(), 1);
        }
    }

    #[test]
    fn auto_join_cancel_and_empty() {
        let parts: Vec<BezPath> = (0..2000).map(|i| poly(&[(i as f64, 0.0), (i as f64 + 1.0, 0.0)], false)).collect();
        let mut calls = 0;
        let r = auto_join(&parts, 0.01, &mut |_, _| {
            calls += 1;
            calls < 2
        });
        assert!(r.is_none());
        let joined = auto_join(&parts, 0.01, &mut never_cancel()).unwrap();
        assert_eq!(joined.len(), 1);
        assert_eq!(contours(&joined[0])[0].segs.len(), 2000);
        assert!(auto_join(&[], 0.1, &mut never_cancel()).unwrap().is_empty());
        assert!(auto_join(&[BezPath::new()], 0.1, &mut never_cancel()).unwrap().is_empty());
    }

    #[test]
    fn auto_join_fast_on_10k_segments() {
        let parts: Vec<BezPath> = (0..10_000)
            .map(|i| {
                let a = i as f64 * std::f64::consts::TAU / 10_000.0;
                let b = (i + 1) as f64 * std::f64::consts::TAU / 10_000.0;
                poly(&[(100.0 * a.cos(), 100.0 * a.sin()), (100.0 * b.cos(), 100.0 * b.sin())], false)
            })
            .collect();
        let t = std::time::Instant::now();
        let out = auto_join(&parts, 0.001, &mut never_cancel()).unwrap();
        assert!(t.elapsed().as_secs_f64() < 5.0);
        assert_eq!(out.len(), 1);
        assert!(contours(&out[0])[0].closed);
    }

    #[test]
    fn delete_duplicates_any_start_and_direction() {
        let a = rect(0.0, 0.0, 10.0, 10.0);
        let b = crate::path::reverse(&crate::path::set_start_point(&a, Point::new(5.0, 0.0), true));
        let c = rect(0.0, 0.0, 10.0, 10.001);
        let d = rect(20.0, 0.0, 30.0, 10.0);
        let open = poly(&[(0.0, 0.0), (5.0, 5.0)], false);
        let open_rev = poly(&[(5.0, 5.0), (2.5, 2.5), (0.0, 0.0)], false);
        let (kept, n) = delete_duplicates(&[a.clone(), b, c, d.clone(), open.clone(), open_rev, BezPath::new()], 0.01);
        assert_eq!(n, 3);
        assert_eq!(kept.len(), 4);
        assert_eq!(kept[0], a);
        assert_eq!(kept[1], d);
        assert_eq!(kept[2], open);
        // Open vs closed of the same outline are not duplicates.
        let open_sq = poly(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0), (0.0, 0.0)], false);
        assert_eq!(delete_duplicates(&[rect(0.0, 0.0, 10.0, 10.0), open_sq], 0.01).1, 0);
        assert_eq!(delete_duplicates(&[], 0.01), (vec![], 0));
    }

    #[test]
    fn overlapping_shared_edge_cut_once() {
        let a = rect(0.0, 0.0, 10.0, 10.0);
        let b = rect(10.0, 0.0, 20.0, 10.0);
        let out = delete_overlapping_lines(&[a.clone(), b], 0.01);
        assert_eq!(out[0], a);
        let cs = contours(&out[1]);
        assert_eq!(cs.len(), 1);
        assert!(!cs[0].closed);
        assert_eq!(cs[0].segs.len(), 3);
        assert!((length(&out[1]) - 30.0).abs() < 1e-9);
    }

    #[test]
    fn overlapping_partial_and_within_distance() {
        let a = poly(&[(0.0, 0.0), (10.0, 0.0)], false);
        let b = poly(&[(5.0, 0.005), (15.0, 0.005)], false);
        let out = delete_overlapping_lines(&[a, b], 0.01);
        assert!((length(&out[1]) - 5.0).abs() < 1e-6);
        // Inside one: middle part removed, splitting into two runs.
        let long = poly(&[(0.0, 0.0), (30.0, 0.0)], false);
        let mid = poly(&[(10.0, 0.0), (20.0, 0.0)], false);
        let out = delete_overlapping_lines(&[mid, long], 0.01);
        assert_eq!(contours(&out[1]).len(), 2);
        assert!((length(&out[1]) - 20.0).abs() < 1e-9);
        // Parallel but too far: kept.
        let far = poly(&[(0.0, 1.0), (30.0, 1.0)], false);
        let out = delete_overlapping_lines(&[poly(&[(0.0, 0.0), (30.0, 0.0)], false), far.clone()], 0.01);
        assert_eq!(out[1], far);
        // Zero-length segments and empty paths are harmless.
        let z = poly(&[(0.0, 0.0), (0.0, 0.0), (1.0, 0.0)], false);
        assert_eq!(delete_overlapping_lines(&[z, BezPath::new()], 0.01).len(), 2);
    }

    #[test]
    fn overlapping_closed_ring_wrap_merge() {
        // Square whose start is mid bottom; a line covering the right side.
        let line = poly(&[(10.0, 0.0), (10.0, 10.0)], false);
        let sq = crate::path::set_start_point(&rect(0.0, 0.0, 10.0, 10.0), Point::new(5.0, 0.0), true);
        let out = delete_overlapping_lines(&[line, sq], 0.01);
        let cs = contours(&out[1]);
        assert_eq!(cs.len(), 1, "{cs:?}");
        assert!((cs[0].length() - 30.0).abs() < 1e-9);
        assert_eq!(cs[0].start(), Some(Point::new(10.0, 10.0)));
    }

    #[test]
    fn break_apart_plain_and_with_holes() {
        let mut p = rect(0.0, 0.0, 100.0, 100.0);
        p.extend(crate::path::reverse(&rect(10.0, 10.0, 50.0, 50.0)));
        p.extend(rect(20.0, 20.0, 30.0, 30.0)); // island in hole
        p.extend(rect(200.0, 0.0, 210.0, 10.0));
        p.extend(poly(&[(60.0, 60.0), (70.0, 70.0)], false)); // open, inside outer
        assert_eq!(break_apart(&p, false, 1e-6).len(), 5);
        let parts = break_apart(&p, true, 1e-6);
        assert_eq!(parts.len(), 4);
        assert_eq!(subpaths(&parts[0]).len(), 2);
        assert!((signed_area(&parts[0]) - (10000.0 - 1600.0)).abs() < 1e-6);
        assert_eq!(bounding_box(&parts[1]), Some(Rect::new(20.0, 20.0, 30.0, 30.0)));
        assert!(break_apart(&BezPath::new(), true, 1e-6).is_empty());
    }

    #[test]
    fn optimize_square_with_collinear_points() {
        let sq = poly(&[(5.0, 0.0), (10.0, 0.0), (10.0, 3.0), (10.0, 10.0), (4.0, 10.0), (0.0, 10.0), (0.0, 0.0), (2.0, 0.0)], true);
        let (o, rep) = optimize(&sq, 0.01);
        assert_eq!(rep.nodes_before, 8);
        assert_eq!(rep.nodes_after, 4);
        assert!(contours(&o)[0].closed);
        assert!((signed_area(&o) - 100.0).abs() < 1e-9);
        assert!(hausdorff(&o, &sq) < 0.01);
    }

    #[test]
    fn optimize_360_gon_to_arcs() {
        let pts: Vec<(f64, f64)> = (0..360)
            .map(|i| {
                let a = (i as f64).to_radians();
                (25.0 * a.cos(), 25.0 * a.sin())
            })
            .collect();
        let poly360 = poly(&pts, true);
        let tol = 0.01;
        let (o, rep) = optimize(&poly360, tol);
        assert_eq!(rep.nodes_before, 360);
        assert!(rep.nodes_after <= 8, "{rep:?}");
        assert!(contours(&o)[0].closed);
        assert!(hausdorff(&o, &poly360) <= tol, "{}", hausdorff(&o, &poly360));
        assert!(o.segments().any(|s| matches!(s, PathSeg::Cubic(_))));
    }

    #[test]
    fn optimize_keeps_curves_and_open_paths_within_tol() {
        let mut p = Circle::new((0.0, 0.0), 40.0).to_path(1e-6);
        let open = poly(&[(0.0, 0.0), (1.0, 0.0), (2.0, 0.0), (3.0, 1.0)], false);
        p.extend(open.clone());
        let (o, rep) = optimize(&p, 0.02);
        let cs = contours(&o);
        assert!(cs[0].closed && !cs[1].closed);
        assert_eq!(cs[1].node_count(), 3);
        assert!(rep.nodes_after <= rep.nodes_before + 6);
        assert!(hausdorff(&o, &p) <= 0.02);
        // Self-intersecting bow tie stays put.
        let bow = poly(&[(0.0, 0.0), (10.0, 10.0), (10.0, 0.0), (0.0, 10.0)], true);
        let (ob, r) = optimize(&bow, 0.01);
        assert_eq!(r.nodes_after, 4);
        assert!(hausdorff(&ob, &bow) < 1e-9);
    }

    #[test]
    fn optimize_degenerate() {
        let (o, r) = optimize(&BezPath::new(), 0.01);
        assert!(o.is_empty());
        assert_eq!(r, OptimizeReport::default());
        let mut dot = BezPath::new();
        dot.move_to((1.0, 1.0));
        dot.line_to((1.0, 1.0));
        let (o, _) = optimize(&dot, 0.01);
        assert!(o.is_empty());
        let zero_seg = poly(&[(0.0, 0.0), (0.0, 0.0), (5.0, 0.0), (5.0, 0.0), (10.0, 0.0)], false);
        let (o, r) = optimize(&zero_seg, 0.01);
        assert_eq!(r.nodes_after, 2);
        assert!((length(&o) - 10.0).abs() < 1e-9);
        assert_eq!(node_count(&zero_seg), 5);
    }

    #[test]
    fn flatten_curves_wrapper() {
        let c = Circle::new((0.0, 0.0), 10.0).to_path(1e-6);
        let f = flatten_curves(&c, 0.01);
        assert!(f.segments().all(|s| matches!(s, PathSeg::Line(_))));
        assert!(is_closed(&f, 0.0));
    }

    #[test]
    fn optimize_keeps_shapes_smaller_than_the_tolerance() {
        // A plate with a 0.3 mm hole, optimised with 0.5 mm: the hole is still cut.
        let mut plate = kurbo::Rect::new(0.0, 0.0, 50.0, 30.0).to_path(0.1);
        let hole = kurbo::Rect::new(10.0, 10.0, 10.3, 10.3).to_path(0.1);
        plate.extend(hole.clone());
        let (out, report) = optimize(&plate, 0.5);
        let cs = contours(&out);
        assert_eq!(cs.len(), 2, "the hole was dropped");
        assert!(cs[1].closed);
        assert!((cs[1].signed_area().abs() - 0.09).abs() < 1e-9, "{}", cs[1].signed_area());
        assert_eq!(report.nodes_after, 8);
        // A shape smaller than the tolerance on its own is not wiped out either.
        let (alone, _) = optimize(&hole, 100.0);
        assert_eq!(alone, hole);
        let dot = Circle::new((5.0, 5.0), 0.2).to_path(1e-3);
        let (kept, _) = optimize(&dot, 1.0);
        assert!((signed_area(&kept).abs() - std::f64::consts::PI * 0.04).abs() < 1e-3);
    }
}
