//! The free space of one sheet for the true-shape nester: placed outlines in a grid of cells,
//! with a fit test and exact sliding.
//!
//! Two outlines are clear of each other when every edge of one stays at least the clearance
//! from every edge of the other and neither has a point inside the other's solid area. Sliding
//! moves an outline in a straight line until a point of one outline would come closer than the
//! clearance to an edge of the other (a ray against a capsule), which is exact for straight
//! edges.

use super::prep::Oriented;
use kurbo::{Point, Rect, Vec2};
use std::ops::Range;

/// Contacts are found this much inside the clearance, so outlines resting exactly at the
/// clearance can slide along each other.
const SHRINK: f64 = 2e-6;
/// Slides stop this far before the contact.
const BACK: f64 = 1e-6;
/// The fit test accepts distances this much under the clearance.
const FIT_EPS: f64 = 1e-5;
/// Most grid cells along one side.
const MAX_CELLS: usize = 64;

/// Crossing-number test: true when `p` is inside the closed ring `pts`.
pub(super) fn in_ring(pts: &[Point], p: Point) -> bool {
    let n = pts.len();
    if n < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (pts[i], pts[j]);
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// True when `p` is in `r`, edges included.
fn in_box(r: &Rect, p: Point) -> bool {
    p.x >= r.x0 && p.x <= r.x1 && p.y >= r.y0 && p.y <= r.y1
}

fn point_seg(p: Point, a: Point, b: Point) -> f64 {
    let ab = b - a;
    let l2 = ab.hypot2();
    let t = if l2 <= 0.0 { 0.0 } else { ((p - a).dot(ab) / l2).clamp(0.0, 1.0) };
    p.distance(a + ab * t)
}

fn orient2(a: Point, b: Point, c: Point) -> f64 {
    (b - a).cross(c - a)
}

/// Distance between segments `ab` and `cd` (0 when they touch or cross).
pub(super) fn seg_dist(a: Point, b: Point, c: Point, d: Point) -> f64 {
    let (d1, d2) = (orient2(a, b, c), orient2(a, b, d));
    let (d3, d4) = (orient2(c, d, a), orient2(c, d, b));
    if ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0)) && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0)) {
        return 0.0;
    }
    point_seg(a, c, d).min(point_seg(b, c, d)).min(point_seg(c, a, b)).min(point_seg(d, a, b))
}

/// First time `t ≥ 0` at which `p + t·d` (`d` a unit vector) comes within `r` of point `c`.
/// Starting inside counts only when moving closer.
fn hit_disc(p: Point, d: Vec2, c: Point, r: f64) -> Option<f64> {
    let m = p - c;
    let b = m.dot(d);
    let cc = m.hypot2() - r * r;
    if cc <= 0.0 {
        return (b < 0.0).then_some(0.0);
    }
    if b >= 0.0 {
        return None;
    }
    let disc = b * b - cc;
    (disc >= 0.0).then(|| (-b - disc.sqrt()).max(0.0))
}

/// First time `t ≥ 0` at which `p + t·d` comes within `r` of segment `ab`.
pub(super) fn hit_capsule(p: Point, d: Vec2, a: Point, b: Point, r: f64) -> Option<f64> {
    let mut best = hit_disc(p, d, a, r);
    let ab = b - a;
    let len = ab.hypot();
    if len > 1e-12 {
        best = min_opt(best, hit_disc(p, d, b, r));
        let u = ab / len;
        let nrm = Vec2::new(-u.y, u.x);
        let m = p - a;
        let (pu, pw) = (m.dot(u), m.dot(nrm));
        let (du, dw) = (d.dot(u), d.dot(nrm));
        let slab = |p0: f64, dv: f64, lo: f64, hi: f64| -> Option<(f64, f64)> {
            if dv.abs() < 1e-15 {
                (p0 >= lo && p0 <= hi).then_some((f64::NEG_INFINITY, f64::INFINITY))
            } else {
                let (t0, t1) = ((lo - p0) / dv, (hi - p0) / dv);
                Some((t0.min(t1), t0.max(t1)))
            }
        };
        if let (Some(w), Some(along)) = (slab(pw, dw, -r, r), slab(pu, du, 0.0, len)) {
            let (t0, t1) = (w.0.max(along.0), w.1.min(along.1));
            if t0 <= t1 && t1 > 0.0 {
                if t0 > 0.0 {
                    best = min_opt(best, Some(t0));
                } else if pw * dw < 0.0 {
                    // Already inside the band and moving towards the edge.
                    best = Some(0.0);
                }
            }
        }
    }
    best
}

fn min_opt(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.min(y)),
        (x, None) => x,
        (None, y) => y,
    }
}

/// A placed outline (or obstacle) in sheet coordinates.
#[derive(Clone, Debug)]
pub(super) struct Body {
    pub bbox: Rect,
    pts: Vec<Point>,
    rings: Vec<Range<usize>>,
    pub holes: Vec<Range<usize>>,
    probes: Vec<Point>,
    pub slack: f64,
}

impl Body {
    fn contains(&self, p: Point) -> bool {
        in_box(&self.bbox, p)
            && self.rings.iter().any(|r| in_ring(&self.pts[r.clone()], p))
            && !self.holes.iter().any(|r| in_ring(&self.pts[r.clone()], p))
    }

    /// Bounding box and area of hole `i`.
    pub fn hole(&self, i: usize) -> (Rect, f64) {
        let pts = &self.pts[self.holes[i].clone()];
        let mut r = Rect::from_points(pts[0], pts[0]);
        for p in pts {
            r = r.union_pt(*p);
        }
        (r, crate::path::polygon_signed_area(pts).abs())
    }
}

#[derive(Clone, Copy, Debug)]
struct Seg {
    a: Point,
    b: Point,
    slack: f64,
}

/// One sheet's placed outlines.
pub(super) struct Space {
    pub usable: Rect,
    gap: f64,
    max_slack: f64,
    pub bodies: Vec<Body>,
    segs: Vec<Seg>,
    verts: Vec<(Point, f64)>,
    origin: Point,
    cw: f64,
    ch: f64,
    nx: usize,
    ny: usize,
    seg_cells: Vec<Vec<u32>>,
    vert_cells: Vec<Vec<u32>>,
    seg_mark: Vec<u32>,
    vert_mark: Vec<u32>,
    mark: u32,
}

/// Directions outlines slide in.
#[derive(Clone, Copy, Debug)]
pub(super) enum Dir {
    Down,
    Left,
}

impl Space {
    pub fn new(usable: Rect, gap: f64) -> Self {
        let side = usable.width().max(usable.height()) / 48.0;
        let nx = ((usable.width() / side).ceil() as usize).clamp(1, MAX_CELLS);
        let ny = ((usable.height() / side).ceil() as usize).clamp(1, MAX_CELLS);
        Self {
            usable,
            gap,
            max_slack: 0.0,
            bodies: Vec::new(),
            segs: Vec::new(),
            verts: Vec::new(),
            origin: usable.origin(),
            cw: usable.width() / nx as f64,
            ch: usable.height() / ny as f64,
            nx,
            ny,
            seg_cells: vec![Vec::new(); nx * ny],
            vert_cells: vec![Vec::new(); nx * ny],
            seg_mark: Vec::new(),
            vert_mark: Vec::new(),
            mark: 0,
        }
    }

    /// Cell index ranges covering `r` (clamped to the grid; anything outside the sheet lands in
    /// the edge cells).
    fn cells(&self, r: Rect) -> (Range<usize>, Range<usize>) {
        let ix = |x: f64| (((x - self.origin.x) / self.cw).floor().max(0.0) as usize).min(self.nx - 1);
        let iy = |y: f64| (((y - self.origin.y) / self.ch).floor().max(0.0) as usize).min(self.ny - 1);
        (ix(r.x0)..ix(r.x1) + 1, iy(r.y0)..iy(r.y1) + 1)
    }

    fn next_mark(&mut self) -> u32 {
        self.mark = self.mark.wrapping_add(1);
        if self.mark == 0 {
            self.seg_mark.iter_mut().for_each(|m| *m = 0);
            self.vert_mark.iter_mut().for_each(|m| *m = 0);
            self.mark = 1;
        }
        self.mark
    }

    /// Adds outline `o` at offset `off` as a fixed body.
    pub fn add(&mut self, o: &Oriented, off: Vec2) {
        let pts: Vec<Point> = o.pts.iter().map(|p| *p + off).collect();
        self.max_slack = self.max_slack.max(o.slack);
        for &(i, j) in &o.segs {
            let s = Seg { a: pts[i as usize], b: pts[j as usize], slack: o.slack };
            let id = self.segs.len() as u32;
            let (xs, ys) = self.cells(Rect::from_points(s.a, s.b));
            for y in ys {
                for x in xs.clone() {
                    self.seg_cells[y * self.nx + x].push(id);
                }
            }
            self.segs.push(s);
            self.seg_mark.push(0);
        }
        for p in &pts {
            let id = self.verts.len() as u32;
            let (xs, ys) = self.cells(Rect::from_points(*p, *p));
            self.vert_cells[ys.start * self.nx + xs.start].push(id);
            self.verts.push((*p, o.slack));
            self.vert_mark.push(0);
        }
        let probes = o.probes.iter().map(|&i| pts[i]).collect();
        let bbox = Rect::from_origin_size(off.to_point(), o.size.to_size());
        self.bodies.push(Body { bbox, pts, rings: o.rings.clone(), holes: o.holes.clone(), probes, slack: o.slack });
    }

    /// True when `o` at `off` lies inside the sheet and clear of every body.
    pub fn fits(&mut self, o: &Oriented, off: Vec2) -> bool {
        let s = o.slack;
        let u = self.usable;
        let e = 1e-9;
        if off.x < u.x0 + s - e || off.y < u.y0 + s - e || off.x + o.size.x > u.x1 - s + e || off.y + o.size.y > u.y1 - s + e {
            return false;
        }
        let rmax = self.gap + s + self.max_slack;
        for &(i, j) in &o.segs {
            let (a, b) = (o.pts[i as usize] + off, o.pts[j as usize] + off);
            let r = Rect::from_points(a, b).inflate(rmax, rmax);
            let mark = self.next_mark();
            let (xs, ys) = self.cells(r);
            for y in ys {
                for x in xs.clone() {
                    for &id in &self.seg_cells[y * self.nx + x] {
                        let id = id as usize;
                        if self.seg_mark[id] == mark {
                            continue;
                        }
                        self.seg_mark[id] = mark;
                        let sg = self.segs[id];
                        let need = self.gap + s + sg.slack - FIT_EPS;
                        if !Rect::from_points(sg.a, sg.b).inflate(need, need).overlaps(Rect::from_points(a, b)) {
                            continue;
                        }
                        if seg_dist(a, b, sg.a, sg.b) < need {
                            return false;
                        }
                    }
                }
            }
        }
        let abox = Rect::from_origin_size(off.to_point(), o.size.to_size());
        for body in &self.bodies {
            if !body.bbox.overlaps(abox) {
                continue;
            }
            if o.probes.iter().any(|&i| body.contains(o.pts[i] + off)) {
                return false;
            }
            if body.probes.iter().any(|q| in_box(&abox, *q) && o.contains(*q - off)) {
                return false;
            }
        }
        true
    }

    /// How far `o` at `off` can move in `dir` before it would come too close to a body or the
    /// sheet edge. Assumes `o` fits where it is.
    pub fn slide(&mut self, o: &Oriented, off: Vec2, dir: Dir) -> f64 {
        let s = o.slack;
        let u = self.usable;
        let (d, mut best) = match dir {
            Dir::Down => (Vec2::new(0.0, -1.0), off.y - (u.y0 + s)),
            Dir::Left => (Vec2::new(-1.0, 0.0), off.x - (u.x0 + s)),
        };
        if best <= 0.0 {
            return 0.0;
        }
        let rmax = self.gap + s + self.max_slack;
        // Points of `o` against edges of the bodies.
        for p in &o.pts {
            let p = *p + off;
            let mark = self.next_mark();
            let (xs, ys) = self.cells(Rect::from_points(p, p + d * best).inflate(rmax, rmax));
            for y in ys {
                for x in xs.clone() {
                    for &id in &self.seg_cells[y * self.nx + x] {
                        let id = id as usize;
                        if self.seg_mark[id] == mark {
                            continue;
                        }
                        self.seg_mark[id] = mark;
                        let sg = self.segs[id];
                        let r = self.gap + s + sg.slack - SHRINK;
                        if !Rect::from_points(sg.a, sg.b).inflate(r, r).overlaps(Rect::from_points(p, p + d * best)) {
                            continue;
                        }
                        if let Some(t) = hit_capsule(p, d, sg.a, sg.b, r)
                            && t < best
                        {
                            best = t;
                            if best <= 0.0 {
                                return 0.0;
                            }
                        }
                    }
                }
            }
        }
        // Points of the bodies against edges of `o` (they move the other way, relatively).
        for &(i, j) in &o.segs {
            let (a, b) = (o.pts[i as usize] + off, o.pts[j as usize] + off);
            let mark = self.next_mark();
            let sweep = Rect::from_points(a, b).union(Rect::from_points(a + d * best, b + d * best));
            let (xs, ys) = self.cells(sweep.inflate(rmax, rmax));
            for y in ys {
                for x in xs.clone() {
                    for &id in &self.vert_cells[y * self.nx + x] {
                        let id = id as usize;
                        if self.vert_mark[id] == mark {
                            continue;
                        }
                        self.vert_mark[id] = mark;
                        let (w, ws) = self.verts[id];
                        let r = self.gap + s + ws - SHRINK;
                        let swept = Rect::from_points(a, b).union(Rect::from_points(a + d * best, b + d * best));
                        if !in_box(&swept.inflate(r, r), w) {
                            continue;
                        }
                        if let Some(t) = hit_capsule(w, -d, a, b, r)
                            && t < best
                        {
                            best = t;
                            if best <= 0.0 {
                                return 0.0;
                            }
                        }
                    }
                }
            }
        }
        (best - BACK).max(0.0)
    }
}
