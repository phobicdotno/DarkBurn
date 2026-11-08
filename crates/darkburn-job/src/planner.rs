//! Cut planner: decides the order, start point and direction of every cut unit.
//!
//! The planner works on [`PlanItem`]s: closed polygons (start point can rotate, direction can
//! flip), open polylines (can be reversed) and blocks (a fill scan with a fixed entry and exit
//! that may be reversible). Items are sorted by the [`OrderKey`]s, grouped, kept in inner-before-
//! outer order, then ordered greedily by nearest neighbour and improved with 2-opt inside a time
//! budget.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use kurbo::{Point, Rect, Vec2};
use serde::{Deserialize, Serialize};

use crate::geom::{self, EPS, Poly};
use crate::input::ForcedDirection;
use crate::progress::{Cancelled, Progress};

/// Sort keys for "order by".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderKey {
    /// Finish one layer before the next (layers in priority order).
    Layer,
    /// Keep shapes of one group together.
    Group,
    /// Manual cut order priority of shapes (lower first).
    Priority,
}

/// "Cut in direction order".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScanDirection {
    BottomToTop,
    TopToBottom,
    LeftToRight,
    RightToLeft,
}

/// Optimisation settings (saved in the project by the app).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlannerSettings {
    /// Optimise the cut path. Off = drawing order, drawn start points and directions.
    pub optimize: bool,
    pub order_by: Vec<OrderKey>,
    pub inner_first: bool,
    pub direction_order: Option<ScanDirection>,
    pub reduce_travel: bool,
    /// Approach every start point from the same side so backlash does not show.
    pub hide_backlash: bool,
    /// Length of the backlash approach move, mm.
    pub backlash_approach: f64,
    pub reduce_direction_changes: bool,
    pub best_start: bool,
    pub prefer_corners: bool,
    pub best_direction: bool,
    pub remove_overlaps: bool,
    /// Distance under which line segments count as overlapping, mm.
    pub overlap_distance: f64,
    /// Time budget for 2-opt improvement, ms.
    pub time_budget_ms: u64,
}

impl Default for PlannerSettings {
    fn default() -> Self {
        Self {
            optimize: true,
            order_by: vec![OrderKey::Layer, OrderKey::Group],
            inner_first: true,
            direction_order: None,
            reduce_travel: true,
            hide_backlash: false,
            backlash_approach: 0.5,
            reduce_direction_changes: false,
            best_start: true,
            prefer_corners: true,
            best_direction: true,
            remove_overlaps: false,
            overlap_distance: 0.05,
            time_budget_ms: 500,
        }
    }
}

/// Geometry of a plannable item.
#[derive(Clone, Debug, PartialEq)]
pub enum ItemGeom {
    Closed(Vec<Point>),
    Open(Vec<Point>),
    Block { entry: Point, exit: Point, reversible: bool, bounds: Rect },
}

/// One unit for the planner.
#[derive(Clone, Debug, PartialEq)]
pub struct PlanItem {
    pub geom: ItemGeom,
    pub layer_rank: usize,
    pub group: Option<u32>,
    pub priority: i32,
    /// Forced start vertex (closed items).
    pub forced_start: Option<usize>,
    pub forced_direction: Option<ForcedDirection>,
}

impl PlanItem {
    pub fn new(geom: ItemGeom) -> Self {
        Self { geom, layer_rank: 0, group: None, priority: 0, forced_start: None, forced_direction: None }
    }

    fn bounds(&self) -> Rect {
        match &self.geom {
            ItemGeom::Closed(p) | ItemGeom::Open(p) => geom::bbox(p),
            ItemGeom::Block { bounds, .. } => *bounds,
        }
    }
}

/// The planner's decision for one item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Planned {
    pub item: usize,
    /// Start vertex for closed items.
    pub start: usize,
    /// Traverse backwards (closed: from `start` going to lower indices).
    pub reversed: bool,
}

impl Planned {
    pub fn entry(&self, items: &[PlanItem]) -> Point {
        match &items[self.item].geom {
            ItemGeom::Closed(p) => p[self.start],
            ItemGeom::Open(p) => {
                if self.reversed {
                    *p.last().unwrap()
                } else {
                    p[0]
                }
            }
            ItemGeom::Block { entry, exit, .. } => {
                if self.reversed {
                    *exit
                } else {
                    *entry
                }
            }
        }
    }

    pub fn exit(&self, items: &[PlanItem]) -> Point {
        match &items[self.item].geom {
            ItemGeom::Closed(p) => p[self.start],
            ItemGeom::Open(p) => {
                if self.reversed {
                    p[0]
                } else {
                    *p.last().unwrap()
                }
            }
            ItemGeom::Block { entry, exit, .. } => {
                if self.reversed {
                    *entry
                } else {
                    *exit
                }
            }
        }
    }

    /// Ordered vertices for a closed or open item (closed: first vertex repeated at the end).
    pub fn ordered_points(&self, items: &[PlanItem]) -> Vec<Point> {
        match &items[self.item].geom {
            ItemGeom::Closed(p) => {
                let n = p.len();
                let mut out: Vec<Point> =
                    (0..=n).map(|k| if self.reversed { p[(self.start + n * 2 - k) % n] } else { p[(self.start + k) % n] }).collect();
                out.dedup_by(|a, b| a.distance(*b) < EPS);
                out
            }
            ItemGeom::Open(p) => {
                if self.reversed {
                    p.iter().rev().copied().collect()
                } else {
                    p.clone()
                }
            }
            ItemGeom::Block { .. } => Vec::new(),
        }
    }
}

/// Direction a closed polygon must be traversed in for a forced direction, if any.
fn forced_reversal(item: &PlanItem) -> Option<bool> {
    let fd = item.forced_direction?;
    match (&item.geom, fd) {
        (_, ForcedDirection::AsDrawn) => Some(false),
        (_, ForcedDirection::Reversed) => Some(true),
        (ItemGeom::Closed(p), ForcedDirection::Clockwise) => Some(geom::signed_area(p) > 0.0),
        (ItemGeom::Closed(p), ForcedDirection::CounterClockwise) => Some(geom::signed_area(p) < 0.0),
        _ => Some(false),
    }
}

/// Plan the cut order.
pub fn plan(items: &[PlanItem], s: &PlannerSettings, start: Point, progress: &mut Progress) -> Result<Vec<Planned>, Cancelled> {
    if !s.optimize {
        return Ok((0..items.len())
            .map(|i| Planned { item: i, start: items[i].forced_start.unwrap_or(0), reversed: forced_reversal(&items[i]).unwrap_or(false) })
            .collect());
    }
    // Sort into buckets by the sort keys.
    let key = |i: usize| -> Vec<i64> {
        s.order_by
            .iter()
            .filter_map(|k| match k {
                OrderKey::Layer => Some(items[i].layer_rank as i64),
                OrderKey::Priority => Some(items[i].priority as i64),
                OrderKey::Group => None,
            })
            .collect()
    };
    let mut idx: Vec<usize> = (0..items.len()).collect();
    idx.sort_by_key(|&i| key(i));
    let mut buckets: Vec<Vec<usize>> = Vec::new();
    for i in idx {
        match buckets.last_mut() {
            Some(b) if key(b[0]) == key(i) => b.push(i),
            _ => buckets.push(vec![i]),
        }
    }
    let deadline = Instant::now() + Duration::from_millis(s.time_budget_ms);
    let mut out = Vec::with_capacity(items.len());
    let mut pos = start;
    let mut heading = Vec2::ZERO;
    let total = items.len().max(1) as f32;
    for b in &buckets {
        let planned = order_bucket(items, b, s, pos, heading, deadline, progress, out.len() as f32 / total, total)?;
        if let Some(last) = planned.last() {
            pos = last.exit(items);
            heading = exit_heading(items, last);
        }
        out.extend(planned);
    }
    progress.report(1.0)?;
    Ok(out)
}

/// Heading of the last cut move of a planned item.
fn exit_heading(items: &[PlanItem], p: &Planned) -> Vec2 {
    match &items[p.item].geom {
        ItemGeom::Block { entry, exit, .. } => {
            let (a, b) = if p.reversed { (*exit, *entry) } else { (*entry, *exit) };
            geom::unit(b - a)
        }
        _ => {
            let pts = p.ordered_points(items);
            let n = pts.len();
            if n >= 2 { geom::unit(pts[n - 1] - pts[n - 2]) } else { Vec2::ZERO }
        }
    }
}

/// Heading of the first cut move.
fn entry_heading(items: &[PlanItem], p: &Planned) -> Vec2 {
    match &items[p.item].geom {
        ItemGeom::Block { entry, exit, .. } => {
            let (a, b) = if p.reversed { (*exit, *entry) } else { (*entry, *exit) };
            geom::unit(b - a)
        }
        _ => {
            let pts = p.ordered_points(items);
            if pts.len() >= 2 { geom::unit(pts[1] - pts[0]) } else { Vec2::ZERO }
        }
    }
}

/// Best start/direction for an item from `pos` and its cost.
fn best_choice(items: &[PlanItem], i: usize, s: &PlannerSettings, pos: Point, heading: Vec2) -> (f64, Planned) {
    let item = &items[i];
    let forced_rev = forced_reversal(item);
    let mut options: Vec<Planned> = Vec::new();
    match &item.geom {
        ItemGeom::Closed(pts) => {
            let start = if let Some(fs) = item.forced_start {
                fs.min(pts.len() - 1)
            } else if s.best_start {
                choose_start(pts, pos, s.prefer_corners)
            } else {
                0
            };
            match forced_rev {
                Some(r) => options.push(Planned { item: i, start, reversed: r }),
                None if s.best_direction => {
                    options.push(Planned { item: i, start, reversed: false });
                    options.push(Planned { item: i, start, reversed: true });
                }
                None => options.push(Planned { item: i, start, reversed: false }),
            }
        }
        ItemGeom::Open(_) => match forced_rev {
            Some(r) => options.push(Planned { item: i, start: 0, reversed: r }),
            None => {
                options.push(Planned { item: i, start: 0, reversed: false });
                if s.best_direction {
                    options.push(Planned { item: i, start: 0, reversed: true });
                }
            }
        },
        ItemGeom::Block { reversible, .. } => {
            options.push(Planned { item: i, start: 0, reversed: false });
            if *reversible && s.best_direction {
                options.push(Planned { item: i, start: 0, reversed: true });
            }
        }
    }
    let mut best = (f64::INFINITY, options[0]);
    for o in options {
        let entry = o.entry(items);
        let travel = entry - pos;
        let mut cost = travel.hypot();
        let first = entry_heading(items, &o);
        // Smooth entry: prefer a first cut move that continues the travel direction.
        let tdir = if travel.hypot() > EPS { geom::unit(travel) } else { heading };
        cost += 1e-3 * (1.0 - tdir.dot(first));
        if s.reduce_direction_changes && heading != Vec2::ZERO && travel.hypot() > EPS {
            cost += 2.0 * (1.0 - heading.dot(geom::unit(travel))) / 2.0;
        }
        if cost < best.0 {
            best = (cost, o);
        }
    }
    best
}

/// Choose the start vertex of a closed polygon nearest to `pos`, preferring corners.
pub fn choose_start(pts: &[Point], pos: Point, prefer_corners: bool) -> usize {
    let d: Vec<f64> = pts.iter().map(|p| p.distance(pos)).collect();
    let (imin, dmin) = d.iter().copied().enumerate().fold((0, f64::INFINITY), |a, (i, v)| if v < a.1 { (i, v) } else { a });
    if !prefer_corners {
        return imin;
    }
    let slack = (dmin * 0.1).max(0.5);
    // A real corner, not a facet of a flattened curve.
    let corner = std::f64::consts::FRAC_PI_3;
    let mut best: Option<(usize, f64)> = None;
    for (i, &di) in d.iter().enumerate() {
        if di <= dmin + slack && geom::corner_angle(pts, i) >= corner && best.is_none_or(|b| di < b.1) {
            best = Some((i, di));
        }
    }
    best.map(|b| b.0).unwrap_or(imin)
}

/// Pairs `(inner, outer)`: `inner` lies inside closed item `outer`.
fn containment(items: &[PlanItem], idx: &[usize]) -> HashSet<(usize, usize)> {
    let mut set = HashSet::new();
    let info: Vec<(usize, Rect, f64)> = idx
        .iter()
        .map(|&i| {
            let area = match &items[i].geom {
                ItemGeom::Closed(p) => geom::signed_area(p).abs(),
                _ => 0.0,
            };
            (i, items[i].bounds(), area)
        })
        .collect();
    for &(o, ob, oa) in &info {
        let ItemGeom::Closed(opts) = &items[o].geom else { continue };
        for &(n, nb, na) in &info {
            if n == o || na >= oa || !geom::rect_inside(nb, ob) {
                continue;
            }
            let samples: Vec<Point> = match &items[n].geom {
                ItemGeom::Closed(p) | ItemGeom::Open(p) => vec![p[0], p[p.len() / 2]],
                ItemGeom::Block { entry, exit, .. } => vec![entry.midpoint(*exit)],
            };
            if samples.iter().all(|p| geom::point_in_polygon(*p, opts)) {
                set.insert((n, o));
            }
        }
    }
    set
}

fn direction_key(r: Rect, d: ScanDirection) -> f64 {
    match d {
        ScanDirection::BottomToTop => r.y0,
        ScanDirection::TopToBottom => -r.y1,
        ScanDirection::LeftToRight => r.x0,
        ScanDirection::RightToLeft => -r.x1,
    }
}

#[allow(clippy::too_many_arguments)]
fn order_bucket(
    items: &[PlanItem],
    idx: &[usize],
    s: &PlannerSettings,
    start: Point,
    start_heading: Vec2,
    deadline: Instant,
    progress: &mut Progress,
    done_before: f32,
    total: f32,
) -> Result<Vec<Planned>, Cancelled> {
    let pairs = if s.inner_first { containment(items, idx) } else { HashSet::new() };
    let local = |i: usize| idx.iter().position(|&x| x == i).unwrap();
    let n = idx.len();
    let mut pending = vec![0usize; n];
    let mut outers: Vec<Vec<usize>> = vec![Vec::new(); n];
    for &(inner, outer) in &pairs {
        pending[local(outer)] += 1;
        outers[local(inner)].push(local(outer));
    }
    let grouped = s.order_by.contains(&OrderKey::Group);
    let cluster = |li: usize| -> i64 {
        match items[idx[li]].group {
            Some(g) if grouped => g as i64,
            _ => -(li as i64) - 1,
        }
    };
    let bounds: Vec<Rect> = idx.iter().map(|&i| items[i].bounds()).collect();
    let mut done = vec![false; n];
    let mut seq: Vec<Planned> = Vec::with_capacity(n);
    let mut pos = start;
    let mut heading = start_heading;
    let mut cur_cluster: Option<i64> = None;
    for step in 0..n {
        if step % 64 == 0 {
            progress.report((done_before + step as f32 * 0.5) / total)?;
        }
        let ready: Vec<usize> = (0..n).filter(|&k| !done[k] && pending[k] == 0).collect();
        // Fall back to any remaining item if precedence is cyclic (should not happen).
        let ready = if ready.is_empty() { (0..n).filter(|&k| !done[k]).collect() } else { ready };
        let in_cluster: Vec<usize> = match cur_cluster {
            Some(c) => ready.iter().copied().filter(|&k| cluster(k) == c).collect(),
            None => Vec::new(),
        };
        let cands = if in_cluster.is_empty() { ready } else { in_cluster };
        let pick = if let Some(dir) = s.direction_order {
            let k = *cands
                .iter()
                .min_by(|&&a, &&b| {
                    let ka = direction_key(bounds[a], dir);
                    let kb = direction_key(bounds[b], dir);
                    ka.partial_cmp(&kb).unwrap().then(a.cmp(&b))
                })
                .unwrap();
            best_choice(items, idx[k], s, pos, heading).1
        } else {
            let mut best: Option<(f64, Planned)> = None;
            for &k in &cands {
                // Cheap lower bound: distance to the bounding box.
                let r = bounds[k];
                let dx = (r.x0 - pos.x).max(pos.x - r.x1).max(0.0);
                let dy = (r.y0 - pos.y).max(pos.y - r.y1).max(0.0);
                if best.is_some_and(|b| dx.hypot(dy) > b.0 + 1e-3) {
                    continue;
                }
                let c = best_choice(items, idx[k], s, pos, heading);
                if best.is_none_or(|b| c.0 < b.0) {
                    best = Some(c);
                }
            }
            best.unwrap().1
        };
        let k = local(pick.item);
        done[k] = true;
        for &o in &outers[k] {
            pending[o] = pending[o].saturating_sub(1);
        }
        cur_cluster = Some(cluster(k));
        pos = pick.exit(items);
        heading = exit_heading(items, &pick);
        seq.push(pick);
    }
    if s.reduce_travel && s.direction_order.is_none() && seq.len() > 2 {
        // 2-opt inside each contiguous cluster run.
        let mut ranges = Vec::new();
        let mut a = 0;
        for i in 1..=seq.len() {
            if i == seq.len() || (grouped && cluster(local(seq[i].item)) != cluster(local(seq[a].item))) {
                ranges.push((a, i));
                a = i;
            }
        }
        if !grouped {
            ranges = vec![(0, seq.len())];
        }
        for (a, b) in ranges {
            two_opt(items, &mut seq, a, b, start, s, &pairs, deadline, progress)?;
        }
        // Re-pick start points of closed shapes for their new predecessors.
        let mut pos = start;
        let mut heading = start_heading;
        for p in seq.iter_mut() {
            if matches!(items[p.item].geom, ItemGeom::Closed(_)) && items[p.item].forced_start.is_none() {
                *p = best_choice(items, p.item, s, pos, heading).1;
            }
            pos = p.exit(items);
            heading = exit_heading(items, p);
        }
    }
    Ok(seq)
}

fn flippable(items: &[PlanItem], p: &Planned, s: &PlannerSettings) -> bool {
    match &items[p.item].geom {
        ItemGeom::Closed(_) => true,
        ItemGeom::Open(_) => s.best_direction && items[p.item].forced_direction.is_none(),
        ItemGeom::Block { reversible, .. } => *reversible && s.best_direction,
    }
}

#[allow(clippy::too_many_arguments)]
fn two_opt(
    items: &[PlanItem],
    seq: &mut [Planned],
    a: usize,
    b: usize,
    start: Point,
    s: &PlannerSettings,
    pairs: &HashSet<(usize, usize)>,
    deadline: Instant,
    progress: &mut Progress,
) -> Result<(), Cancelled> {
    if b - a < 2 {
        return Ok(());
    }
    let mut improved = true;
    while improved {
        improved = false;
        for i in a..b {
            if Instant::now() > deadline {
                return Ok(());
            }
            progress.check()?;
            let prev = if i == 0 { start } else { seq[i - 1].exit(items) };
            let mut all_flip = true;
            for j in i..b {
                if !flippable(items, &seq[j], s) {
                    all_flip = false;
                }
                if !all_flip {
                    break;
                }
                if j == i {
                    continue;
                }
                let next = if j + 1 < seq.len() { Some(seq[j + 1].entry(items)) } else { None };
                let old = prev.distance(seq[i].entry(items)) + next.map_or(0.0, |n| seq[j].exit(items).distance(n));
                let new = prev.distance(seq[j].exit(items)) + next.map_or(0.0, |n| seq[i].entry(items).distance(n));
                if new + 1e-9 < old && precedence_ok(seq, i, j, pairs) {
                    seq[i..=j].reverse();
                    for p in seq[i..=j].iter_mut() {
                        if !matches!(items[p.item].geom, ItemGeom::Closed(_)) {
                            p.reversed = !p.reversed;
                        }
                    }
                    improved = true;
                    break;
                }
            }
        }
    }
    Ok(())
}

/// Reversing `seq[i..=j]` keeps every inner item before its outer item.
fn precedence_ok(seq: &[Planned], i: usize, j: usize, pairs: &HashSet<(usize, usize)>) -> bool {
    if pairs.is_empty() {
        return true;
    }
    for x in i..=j {
        for y in x + 1..=j {
            if pairs.contains(&(seq[x].item, seq[y].item)) {
                return false;
            }
        }
    }
    true
}

/// Remove line segments that overlap segments already kept (within `tol`), splitting paths
/// where segments are removed. `tag` follows each piece back to its source.
pub fn remove_overlaps<T: Copy>(polys: Vec<(Poly, T)>, tol: f64) -> Vec<(Poly, T)> {
    let mut kept: Vec<(Point, Point)> = Vec::new();
    let mut out = Vec::new();
    for (poly, tag) in polys {
        let n = poly.pts.len();
        let segs: Vec<(Point, Point)> = if poly.closed {
            (0..n).map(|i| (poly.pts[i], poly.pts[(i + 1) % n])).collect()
        } else {
            poly.pts.windows(2).map(|w| (w[0], w[1])).collect()
        };
        let mut keep = Vec::with_capacity(segs.len());
        for &(a, b) in &segs {
            let covered = kept.iter().any(|&(c, d)| {
                geom::dist_point_segment(a, c, d) <= tol
                    && geom::dist_point_segment(b, c, d) <= tol
                    && geom::dist_point_segment(a.midpoint(b), c, d) <= tol
            });
            keep.push(!covered);
        }
        for (k, s) in keep.iter().zip(&segs) {
            if *k {
                kept.push(*s);
            }
        }
        if keep.iter().all(|k| *k) {
            out.push((poly, tag));
            continue;
        }
        // Rotate closed paths so a removed segment comes first, then collect kept runs.
        let m = segs.len();
        let rot = if poly.closed { keep.iter().position(|k| !k).unwrap_or(0) } else { 0 };
        let mut run: Vec<Point> = Vec::new();
        for t in 0..m {
            let i = (rot + t) % m;
            if keep[i] {
                if run.is_empty() {
                    run.push(segs[i].0);
                }
                run.push(segs[i].1);
            } else if run.len() >= 2 {
                out.push((Poly { pts: std::mem::take(&mut run), closed: false }, tag));
            } else {
                run.clear();
            }
        }
        if run.len() >= 2 {
            out.push((Poly { pts: run, closed: false }, tag));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(x: f64, y: f64, s: f64) -> Vec<Point> {
        vec![Point::new(x, y), Point::new(x + s, y), Point::new(x + s, y + s), Point::new(x, y + s)]
    }

    #[test]
    fn inner_shapes_first() {
        let items = vec![PlanItem::new(ItemGeom::Closed(square(0.0, 0.0, 100.0))), PlanItem::new(ItemGeom::Closed(square(40.0, 40.0, 20.0)))];
        let p = plan(&items, &PlannerSettings::default(), Point::ZERO, &mut Progress::none()).unwrap();
        assert_eq!(p.iter().map(|p| p.item).collect::<Vec<_>>(), vec![1, 0]);
        let s = PlannerSettings { inner_first: false, ..Default::default() };
        let p = plan(&items, &s, Point::ZERO, &mut Progress::none()).unwrap();
        assert_eq!(p[0].item, 0);
    }

    #[test]
    fn nearest_neighbour_and_reversal() {
        let items = vec![
            PlanItem::new(ItemGeom::Open(vec![Point::new(100.0, 0.0), Point::new(110.0, 0.0)])),
            PlanItem::new(ItemGeom::Open(vec![Point::new(20.0, 0.0), Point::new(10.0, 0.0)])),
            PlanItem::new(ItemGeom::Open(vec![Point::new(50.0, 0.0), Point::new(60.0, 0.0)])),
        ];
        let p = plan(&items, &PlannerSettings::default(), Point::ZERO, &mut Progress::none()).unwrap();
        assert_eq!(p.iter().map(|p| p.item).collect::<Vec<_>>(), vec![1, 2, 0]);
        assert!(p[0].reversed);
        assert!(!p[1].reversed);
    }

    #[test]
    fn two_opt_beats_greedy() {
        // Greedy from origin zig-zags; total travel after planning must not exceed greedy.
        let pts = [(0.0, 1.0), (5.0, 0.0), (5.0, 9.0), (0.0, 10.0), (10.0, 10.0), (10.0, 1.0), (3.0, 3.0), (8.0, 4.0)];
        let items: Vec<PlanItem> = pts.iter().map(|&(x, y)| PlanItem::new(ItemGeom::Open(vec![Point::new(x, y), Point::new(x + 0.1, y)]))).collect();
        let travel = |p: &[Planned]| {
            let mut pos = Point::ZERO;
            let mut t = 0.0;
            for q in p {
                t += pos.distance(q.entry(&items));
                pos = q.exit(&items);
            }
            t
        };
        let greedy = plan(&items, &PlannerSettings { reduce_travel: false, ..Default::default() }, Point::ZERO, &mut Progress::none()).unwrap();
        let opt = plan(&items, &PlannerSettings::default(), Point::ZERO, &mut Progress::none()).unwrap();
        assert!(travel(&opt) <= travel(&greedy) + 1e-9);
        assert_eq!(opt.len(), items.len());
    }

    #[test]
    fn prefers_corner_start() {
        // A rounded-ish polygon with one sharp corner near the start position.
        let pts = vec![Point::new(0.0, 0.0), Point::new(10.0, 0.0), Point::new(10.0, 10.0), Point::new(0.0, 10.0)];
        let mut more = pts.clone();
        more.insert(1, Point::new(0.3, 0.0)); // nearly straight vertex closest to (0.3, -1)
        let s = choose_start(&more, Point::new(0.3, -1.0), true);
        assert_eq!(more[s], Point::new(0.0, 0.0));
        let s = choose_start(&more, Point::new(0.3, -1.0), false);
        assert_eq!(more[s], Point::new(0.3, 0.0));
    }

    #[test]
    fn manual_priority_and_forced_start() {
        let mut a = PlanItem::new(ItemGeom::Closed(square(0.0, 0.0, 10.0)));
        a.priority = 2;
        let mut b = PlanItem::new(ItemGeom::Closed(square(50.0, 0.0, 10.0)));
        b.priority = 1;
        b.forced_start = Some(2);
        b.forced_direction = Some(ForcedDirection::Clockwise);
        let s = PlannerSettings { order_by: vec![OrderKey::Priority], ..Default::default() };
        let p = plan(&[a, b], &s, Point::ZERO, &mut Progress::none()).unwrap();
        assert_eq!(p[0].item, 1);
        assert_eq!(p[0].start, 2);
        assert!(p[0].reversed, "square is CCW, clockwise needs reversal");
    }

    #[test]
    fn groups_stay_together() {
        let mk = |x: f64, g: u32| {
            let mut it = PlanItem::new(ItemGeom::Open(vec![Point::new(x, 0.0), Point::new(x + 1.0, 0.0)]));
            it.group = Some(g);
            it
        };
        // Group 1 at 0 and 30, group 2 at 10 and 20: grouping forces 0,30 then 20,10.
        let items = vec![mk(0.0, 1), mk(10.0, 2), mk(20.0, 2), mk(30.0, 1)];
        let p = plan(&items, &PlannerSettings::default(), Point::ZERO, &mut Progress::none()).unwrap();
        let order: Vec<usize> = p.iter().map(|p| p.item).collect();
        let pos3 = order.iter().position(|&i| i == 3).unwrap();
        let pos0 = order.iter().position(|&i| i == 0).unwrap();
        assert_eq!(pos3.abs_diff(pos0), 1, "{order:?}");
    }

    #[test]
    fn direction_order_bottom_to_top() {
        let items: Vec<PlanItem> = [50.0, 10.0, 30.0].iter().map(|&y| PlanItem::new(ItemGeom::Closed(square(0.0, y, 5.0)))).collect();
        let s = PlannerSettings { direction_order: Some(ScanDirection::BottomToTop), ..Default::default() };
        let p = plan(&items, &s, Point::new(0.0, 100.0), &mut Progress::none()).unwrap();
        assert_eq!(p.iter().map(|p| p.item).collect::<Vec<_>>(), vec![1, 2, 0]);
    }

    #[test]
    fn optimize_off_keeps_order() {
        let items: Vec<PlanItem> = [50.0, 10.0, 30.0].iter().map(|&y| PlanItem::new(ItemGeom::Closed(square(0.0, y, 5.0)))).collect();
        let s = PlannerSettings { optimize: false, ..Default::default() };
        let p = plan(&items, &s, Point::ZERO, &mut Progress::none()).unwrap();
        assert_eq!(p.iter().map(|p| (p.item, p.start, p.reversed)).collect::<Vec<_>>(), vec![(0, 0, false), (1, 0, false), (2, 0, false)]);
    }

    #[test]
    fn cancel_and_progress() {
        use std::sync::atomic::AtomicBool;
        let items: Vec<PlanItem> = (0..10).map(|i| PlanItem::new(ItemGeom::Closed(square(i as f64 * 10.0, 0.0, 5.0)))).collect();
        let flag = AtomicBool::new(true);
        let r = plan(&items, &PlannerSettings::default(), Point::ZERO, &mut Progress::new(Some(&flag), None));
        assert_eq!(r, Err(Cancelled));
        let mut last = 0.0f32;
        let mut cb = |f: f32| last = f;
        let r = plan(&items, &PlannerSettings::default(), Point::ZERO, &mut Progress::new(None, Some(&mut cb)));
        assert!(r.is_ok());
        assert_eq!(last, 1.0);
    }

    #[test]
    fn overlapping_lines_removed() {
        // Two squares sharing the edge x = 10.
        let a = Poly { pts: square(0.0, 0.0, 10.0), closed: true };
        let b = Poly { pts: square(10.0, 0.0, 10.0), closed: true };
        let out = remove_overlaps(vec![(a, 0), (b, 1)], 0.01);
        assert_eq!(out.len(), 2);
        assert!(out[0].0.closed);
        assert!(!out[1].0.closed);
        let len: f64 = geom::polyline_length(&out[1].0.pts);
        assert!((len - 30.0).abs() < 1e-9, "{len}");
    }
}
