//! True nest: greedy placement of real outlines.
//!
//! For each part (in a pass's order) and each rotation, start positions are tried: dropped in
//! from the highest free spot at a spread of x positions, next to the bodies already placed,
//! and in the middle of usable holes. From each start that fits, the part slides down, then
//! left, then down again, until it settles. The settled spot with the lowest top edge (then
//! the leftmost) wins. Every spot is checked with the full fit test before it is taken.

use super::prep::{Oriented, Outline, orient, outline};
use super::space::{Dir, Space};
use super::{NestPart, NestProgress, Placed, Run, SheetNestResult, Ticker, finish, orders};
use kurbo::{Affine, Vec2};

/// Bodies (most recent first) used for start positions next to them.
const NEAR_BODIES: usize = 48;
/// Most start positions dropped in from the top.
const TOP_STARTS: usize = 32;
/// Most start heights tried at each of those x positions, highest first.
const TOP_LEVELS: usize = 12;
/// Most holes tried as start positions.
const HOLE_STARTS: usize = 64;

/// How a pass went: placements and (unplaced, sheets, top of last sheet, right of last sheet).
type Key = (usize, usize, f64, f64);

struct Nester<'a> {
    run: &'a Run,
    parts: Vec<Option<Outline>>,
    obstacles: Vec<(Oriented, Vec2)>,
}

fn thin(mut xs: Vec<f64>, max: usize) -> Vec<f64> {
    xs.sort_by(f64::total_cmp);
    xs.dedup_by(|a, b| (*a - *b).abs() < 0.01);
    if xs.len() > max {
        let n = xs.len();
        xs = (0..max).map(|i| xs[i * (n - 1) / (max - 1)]).collect();
    }
    xs
}

/// Start positions in groups: from each group the first one that fits is used.
fn candidates(space: &Space, o: &Oriented, gap: f64) -> Vec<Vec<Vec2>> {
    let u = space.usable;
    let s = o.slack;
    let (w, h) = (o.size.x, o.size.y);
    let (x0, x1, y0, y1) = (u.x0 + s, u.x1 - s - w, u.y0 + s, u.y1 - s - h);
    if x1 < x0 - 1e-9 || y1 < y0 - 1e-9 {
        return Vec::new();
    }
    let (x1, y1) = (x1.max(x0), y1.max(y0));
    let at = |x: f64, y: f64| Vec2::new(x.clamp(x0, x1), y.clamp(y0, y1));
    let mut xs: Vec<f64> = (0..=8).map(|i| x0 + (x1 - x0) * i as f64 / 8.0).collect();
    let mut ys: Vec<f64> = (0..=8).map(|i| y0 + (y1 - y0) * i as f64 / 8.0).collect();
    let near: Vec<_> = space.bodies.iter().rev().take(NEAR_BODIES).collect();
    for b in &near {
        let c = gap + s + b.slack + 1e-5;
        xs.push((b.bbox.x1 + c).clamp(x0, x1));
        xs.push((b.bbox.x0 - c - w).clamp(x0, x1));
        ys.push((b.bbox.y1 + c).clamp(y0, y1));
        ys.push((b.bbox.y0 - c - h).clamp(y0, y1));
    }
    let mut ys = thin(ys, TOP_LEVELS);
    ys.reverse();
    // Dropped in from the top: the highest start that fits at each x.
    let mut out: Vec<Vec<Vec2>> = thin(xs, TOP_STARTS).into_iter().map(|x| ys.iter().map(|&y| Vec2::new(x, y)).collect()).collect();
    for b in &near {
        let c = gap + s + b.slack + 1e-5;
        let r = b.bbox;
        out.extend([at(r.x1 + c, r.y0), at(r.x0, r.y1 + c), at(r.x1 + c, r.y1 - h), at(r.x1 - w, r.y1 + c)].map(|p| vec![p]));
    }
    let holes = space.bodies.iter().flat_map(|b| (0..b.holes.len()).map(move |i| (b, i))).take(HOLE_STARTS);
    for (b, i) in holes {
        let (hb, area) = b.hole(i);
        let c = gap + s + b.slack + 1e-5;
        if hb.width() < w + 2.0 * c || hb.height() < h + 2.0 * c || area < w.min(h) * w.min(h) * 0.5 {
            continue;
        }
        let mid = hb.center();
        out.push(vec![at(mid.x - w / 2.0, mid.y - h / 2.0)]);
        out.push(vec![at(hb.x0 + c, hb.y0 + c)]);
    }
    out
}

/// Slides down and left until the part rests.
fn settle(space: &mut Space, o: &Oriented, start: Vec2) -> Vec2 {
    let mut pos = start;
    for _ in 0..24 {
        let dy = space.slide(o, pos, Dir::Down);
        pos.y -= dy;
        let dx = space.slide(o, pos, Dir::Left);
        pos.x -= dx;
        if dy + dx < 1e-4 {
            break;
        }
    }
    pos
}

fn better(a: (f64, f64), b: (f64, f64)) -> bool {
    a.0 < b.0 - 1e-7 || ((a.0 - b.0).abs() <= 1e-7 && a.1 < b.1 - 1e-7)
}

impl Nester<'_> {
    fn space(&self, k: usize) -> Space {
        let mut s = Space::new(self.run.usable(k), self.run.gap);
        if k == 0 {
            for (o, off) in &self.obstacles {
                s.add(o, *off);
            }
        }
        s
    }

    /// The best settled spot on one sheet: (orientation, offset).
    fn best_spot(&self, space: &mut Space, orients: &[Oriented], tick: &mut Ticker) -> Option<(usize, Vec2)> {
        let mut best: Option<((f64, f64), usize, Vec2)> = None;
        for (oi, o) in orients.iter().enumerate() {
            if oi > 0 && tick.stop() {
                break;
            }
            for group in candidates(space, o, self.run.gap) {
                let Some(start) = group.into_iter().find(|p| space.fits(o, *p)) else { continue };
                let pos = settle(space, o, start);
                if !space.fits(o, pos) {
                    continue;
                }
                let score = (pos.y + o.size.y, pos.x);
                if best.as_ref().is_none_or(|b| better(score, b.0)) {
                    best = Some((score, oi, pos));
                }
            }
        }
        best.map(|b| (b.1, b.2))
    }

    fn place(&self, spaces: &mut Vec<Space>, orients: &[Oriented], tick: &mut Ticker) -> Option<(usize, usize, Vec2)> {
        let mut k = 0;
        loop {
            let fresh = k == spaces.len();
            if fresh {
                if k >= self.run.max_sheets {
                    return None;
                }
                spaces.push(self.space(k));
            }
            if let Some((oi, pos)) = self.best_spot(&mut spaces[k], orients, tick) {
                spaces[k].add(&orients[oi], pos);
                return Some((k, oi, pos));
            }
            // A part that does not fit an empty sheet never fits a new one either.
            if fresh && k > 0 {
                spaces.pop();
                return None;
            }
            if tick.stopped {
                return None;
            }
            k += 1;
        }
    }

    fn pass(&self, order: &[usize], tick: &mut Ticker) -> (Vec<Placed>, Key) {
        let mut spaces: Vec<Space> = Vec::new();
        let mut placed: Vec<Placed> = vec![None; self.parts.len()];
        let mut ends: Vec<(usize, f64, f64)> = Vec::new();
        for &i in order {
            if let Some(out) = &self.parts[i] {
                let orients: Vec<Oriented> = self.run.rotations.iter().map(|&a| orient(out, a)).collect();
                if let Some((k, oi, pos)) = self.place(&mut spaces, &orients, tick) {
                    let o = &orients[oi];
                    placed[i] = Some((Affine::translate(pos) * o.xf, k));
                    ends.push((k, pos.y + o.size.y, pos.x + o.size.x));
                }
            }
            if tick.stopped || tick.step() {
                break;
            }
        }
        let unplaced = placed.iter().filter(|p| p.is_none()).count();
        let sheets = ends.iter().map(|e| e.0 + 1).max().unwrap_or(0);
        let last = ends.iter().filter(|e| e.0 + 1 == sheets);
        let (top, right) = last.fold((0.0f64, 0.0f64), |a, e| (a.0.max(e.1), a.1.max(e.2)));
        (placed, (unplaced, sheets, top, right))
    }
}

fn key_better(a: &Key, b: &Key) -> bool {
    (a.0, a.1) < (b.0, b.1) || ((a.0, a.1) == (b.0, b.1) && better((a.2, a.3), (b.2, b.3)))
}

pub(super) fn nest(input: &[NestPart], obstacles: &[NestPart], run: &Run, report: &mut dyn FnMut(NestProgress) -> bool) -> SheetNestResult {
    let parts: Vec<Option<Outline>> = input.iter().map(|p| outline(&p.cuts, &p.marks, run.tol, run.allow_holes, None)).collect();
    let obstacles: Vec<(Oriented, Vec2)> = obstacles
        .iter()
        .filter_map(|p| outline(&p.cuts, &p.marks, run.tol, run.allow_holes, Some(run.usable)))
        .map(|o| {
            let or = orient(&o, 0.0);
            let off = -or.xf.translation();
            (or, off)
        })
        .collect();
    let nester = Nester { run, parts, obstacles };
    let sizes: Vec<(f64, f64, f64)> = nester
        .parts
        .iter()
        .map(|p| match p {
            Some(o) => {
                let or = orient(o, 0.0);
                (o.area.max(or.size.x * or.size.y * 1e-3), or.size.x, or.size.y)
            }
            None => (0.0, 0.0, 0.0),
        })
        .collect();
    let orders = orders(input.len(), |i| sizes[i]);
    let mut tick = Ticker { done: 0, total: input.len() * orders.len(), deadline: run.deadline, report, stopped: false };
    let mut best: Option<(Vec<Placed>, Key)> = None;
    for order in &orders {
        if tick.stop() {
            break;
        }
        let (placed, key) = nester.pass(order, &mut tick);
        if best.as_ref().is_none_or(|b| key_better(&key, &b.1)) {
            best = Some((placed, key));
        }
        if tick.stopped {
            break;
        }
    }
    let placed = best.map(|b| b.0).unwrap_or_else(|| vec![None; input.len()]);
    let areas: Vec<f64> = nester.parts.iter().map(|p| p.as_ref().map_or(0.0, |o| o.area)).collect();
    finish(placed, &areas, run.usable, tick.stopped)
}
