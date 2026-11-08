//! Slot detection and resizing: rectangular and round-ended notches (and, optionally,
//! the matching protruding tabs) along a path, for finger joints and press-fit parts.

use crate::path::{Contour, append_contour, contours, seg_tangent};
use kurbo::{Arc, BezPath, CubicBez, Line, ParamCurve, PathEl, PathSeg, Point, Vec2};

/// What to look for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SlotQuery {
    pub min_width: f64,
    pub max_width: f64,
    pub min_depth: f64,
    pub max_depth: f64,
    /// Length tolerance (mm): the two side walls must match within this, and round ends
    /// must stay this close to a true semicircle.
    pub tol: f64,
    /// Angle tolerance (degrees) for parallel and perpendicular tests.
    pub angle_tol_deg: f64,
    /// Also report protruding tabs (convex rectangles/rounds sticking out of the shape).
    pub include_tabs: bool,
}

impl Default for SlotQuery {
    fn default() -> Self {
        Self {
            min_width: 0.0,
            max_width: f64::INFINITY,
            min_depth: 0.0,
            max_depth: f64::INFINITY,
            tol: 0.01,
            angle_tol_deg: 1.0,
            include_tabs: false,
        }
    }
}

/// Shape of a slot's bottom.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotKind {
    /// Flat bottom: wall, bottom line, wall.
    Rect,
    /// Semicircular bottom between the two walls.
    Round,
}

/// A detected slot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Slot {
    /// Subpath index (in [`contours`] order).
    pub subpath: usize,
    /// Index of the first wall segment.
    pub first_seg: usize,
    /// Number of segments from the first wall to the second wall inclusive.
    pub seg_count: usize,
    /// Distance between the walls.
    pub width: f64,
    /// Distance from the mouth to the deepest point (for round slots this includes the
    /// radius of the end).
    pub depth: f64,
    pub kind: SlotKind,
    /// Centre of the mouth (between the open ends of the walls).
    pub mouth: Point,
    /// Unit direction from the mouth into the slot.
    pub axis: Vec2,
    /// True for a protruding tab rather than a notch.
    pub protruding: bool,
}

fn dir(s: &PathSeg) -> Vec2 {
    let d = s.end() - s.start();
    if d.hypot2() > 0.0 { d.normalize() } else { Vec2::ZERO }
}

fn is_line(s: &PathSeg) -> bool {
    matches!(s, PathSeg::Line(_))
}

/// Finds slots on every subpath of `path`.
pub fn find_slots(path: &BezPath, q: &SlotQuery) -> Vec<Slot> {
    let mut out = Vec::new();
    for (ci, c) in contours(path).iter().enumerate() {
        let n = c.segs.len();
        if n < 3 {
            continue;
        }
        let orient = if c.signed_area() >= 0.0 { 1.0 } else { -1.0 };
        let par = q.angle_tol_deg.to_radians().sin();
        let starts = if c.closed { n } else { n - 2 };
        for i in 0..starts {
            let seg = |k: usize| -> Option<&PathSeg> { if c.closed { Some(&c.segs[(i + k) % n]) } else { c.segs.get(i + k) } };
            let Some(a) = seg(0) else { continue };
            if !is_line(a) {
                continue;
            }
            // Bottom: one line, or a run of curves for a round end.
            let Some(b0) = seg(1) else { continue };
            let (kind, bottom_len) = if is_line(b0) {
                (SlotKind::Rect, 1)
            } else {
                let mut k = 1;
                while k < n - 1 && seg(k).is_some_and(|s| !is_line(s)) && k <= 8 {
                    k += 1;
                }
                (SlotKind::Round, k - 1)
            };
            let count = bottom_len + 2;
            if count > n || (!c.closed && i + count > n) {
                continue;
            }
            let Some(cw) = seg(count - 1) else { continue };
            if !is_line(cw) {
                continue;
            }
            let da = dir(a);
            let dc = dir(cw);
            if da == Vec2::ZERO || dc == Vec2::ZERO || (da + dc).hypot() > 2.0 * par {
                continue;
            }
            let chord = cw.start() - a.end();
            let width = chord.hypot();
            if width <= 0.0 {
                continue;
            }
            let dchord = chord / width;
            if da.dot(dchord).abs() > par || dc.dot(dchord).abs() > par {
                continue;
            }
            let la = a.end().distance(a.start());
            let lc = cw.end().distance(cw.start());
            if (la - lc).abs() > q.tol {
                continue;
            }
            if kind == SlotKind::Round {
                let r = width * 0.5;
                let k = a.end().midpoint(cw.start());
                let mut ok = (seg_tangent(b0, 0.0) - da).hypot() < 2.0 * par;
                for j in 1..=bottom_len {
                    let s = seg(j).expect("in range");
                    for m in 0..=4 {
                        let p = s.eval(m as f64 / 4.0);
                        if (p.distance(k) - r).abs() > q.tol || (p - k).dot(da) < -q.tol {
                            ok = false;
                        }
                    }
                }
                // The apex must reach r beyond the chord.
                let apex = (1..=bottom_len)
                    .flat_map(|j| (0..=8).map(move |m| (j, m as f64 / 8.0)))
                    .map(|(j, t)| (seg(j).expect("in range").eval(t) - k).dot(da))
                    .fold(f64::MIN, f64::max);
                if !ok || (apex - r).abs() > q.tol {
                    continue;
                }
            }
            // Turning from the wall into the bottom: right turns on a CCW contour go into
            // the material (a notch); left turns stick out (a tab).
            let turn = da.cross(dchord) * orient;
            let protruding = turn > 0.0;
            if protruding && !q.include_tabs {
                continue;
            }
            // The neighbour before the first wall must turn the other way, otherwise this
            // is just three sides of a plain rectangle.
            let prev = if c.closed { Some(&c.segs[(i + n - 1) % n]) } else { i.checked_sub(1).map(|p| &c.segs[p]) };
            if let Some(p) = prev {
                let pt = seg_tangent(p, 1.0);
                if pt.cross(da) * orient * turn.signum() >= 0.0 {
                    continue;
                }
            }
            let next = if c.closed { Some(&c.segs[(i + count) % n]) } else { c.segs.get(i + count) };
            if let Some(nx) = next {
                let nt = seg_tangent(nx, 0.0);
                if dc.cross(nt) * orient * turn.signum() >= 0.0 {
                    continue;
                }
            }
            let depth = match kind {
                SlotKind::Rect => la,
                SlotKind::Round => la + width * 0.5,
            };
            if width < q.min_width || width > q.max_width || depth < q.min_depth || depth > q.max_depth {
                continue;
            }
            out.push(Slot {
                subpath: ci,
                first_seg: i,
                seg_count: count,
                width,
                depth,
                kind,
                mouth: a.start().midpoint(cw.end()),
                axis: da,
                protruding,
            });
        }
    }
    out
}

fn set_end(s: &mut PathSeg, p: Point) {
    match s {
        PathSeg::Line(l) => l.p1 = p,
        PathSeg::Quad(q) => q.p2 = p,
        PathSeg::Cubic(c) => {
            let d = p - c.p3;
            c.p2 += d;
            c.p3 = p;
        }
    }
}

fn set_start(s: &mut PathSeg, p: Point) {
    match s {
        PathSeg::Line(l) => l.p0 = p,
        PathSeg::Quad(q) => q.p0 = p,
        PathSeg::Cubic(c) => {
            let d = p - c.p0;
            c.p1 += d;
            c.p0 = p;
        }
    }
}

fn slot_geometry(slot: &Slot, width: f64, depth: f64, wall_dir: Vec2) -> (Vec<PathSeg>, Point, Point) {
    let u = slot.axis;
    // `v` points from the first wall to the second.
    let v = wall_dir;
    let a0 = slot.mouth - v * (width * 0.5);
    let c0 = slot.mouth + v * (width * 0.5);
    match slot.kind {
        SlotKind::Rect => {
            let a1 = a0 + u * depth;
            let c1 = c0 + u * depth;
            (vec![PathSeg::Line(Line::new(a0, a1)), PathSeg::Line(Line::new(a1, c1)), PathSeg::Line(Line::new(c1, c0))], a0, c0)
        }
        SlotKind::Round => {
            let r = width * 0.5;
            let straight = (depth - r).max(0.0);
            let a1 = a0 + u * straight;
            let c1 = c0 + u * straight;
            let k = slot.mouth + u * straight;
            let start = (-v).atan2();
            let sweep = if (-v).cross(u) > 0.0 { std::f64::consts::PI } else { -std::f64::consts::PI };
            let mut segs = Vec::new();
            if straight > 0.0 {
                segs.push(PathSeg::Line(Line::new(a0, a1)));
            }
            let mut cur = a1;
            let arc = Arc::new(k, Vec2::new(r, r), start, sweep, 0.0);
            for el in arc.append_iter(1e-6) {
                if let PathEl::CurveTo(p1, p2, p3) = el {
                    segs.push(PathSeg::Cubic(CubicBez::new(cur, p1, p2, p3)));
                    cur = p3;
                }
            }
            if let Some(PathSeg::Cubic(last)) = segs.last_mut() {
                last.p3 = c1;
            }
            if straight > 0.0 {
                segs.push(PathSeg::Line(Line::new(c1, c0)));
            }
            (segs, a0, c0)
        }
    }
}

/// Resizes the given slots (as returned by [`find_slots`] for this same `path`). The width
/// changes symmetrically about the slot axis, moving the side walls and the ends of the
/// neighbouring mouth segments; the depth moves the bottom. `None` keeps the current
/// value. Round slots keep a semicircular end with the new diameter (if the new depth is
/// less than the radius the walls vanish and only the semicircle remains).
///
/// A closed subpath whose start node lies inside a slot gets the first new wall as its
/// new start node.
pub fn resize_slots(path: &BezPath, slots: &[Slot], new_width: Option<f64>, new_depth: Option<f64>) -> BezPath {
    let mut cs = contours(path);
    for (ci, c) in cs.iter_mut().enumerate() {
        let mut mine: Vec<Slot> = slots.iter().copied().filter(|s| s.subpath == ci).collect();
        if mine.is_empty() {
            continue;
        }
        let n0 = c.segs.len();
        // Non-wrapping slots from the back first so earlier indices stay valid; wrapping
        // ones last, shifted by the change in segment count before them.
        mine.sort_by(|a, b| {
            let wa = a.first_seg + a.seg_count > n0;
            let wb = b.first_seg + b.seg_count > n0;
            wa.cmp(&wb).then(b.first_seg.cmp(&a.first_seg))
        });
        let mut delta: isize = 0;
        for s in mine {
            let n = c.segs.len();
            let wraps = s.first_seg + s.seg_count > n0;
            let k = if wraps { (s.first_seg as isize + delta).rem_euclid(n as isize) as usize } else { s.first_seg };
            if s.seg_count > n || (!c.closed && k + s.seg_count > n) {
                continue;
            }
            let first = c.segs[k];
            let last = c.segs[(k + s.seg_count - 1) % n];
            let chord = last.start() - first.end();
            if chord.hypot2() == 0.0 {
                continue;
            }
            let (new, a0, c0) = slot_geometry(&s, new_width.unwrap_or(s.width), new_depth.unwrap_or(s.depth), chord.normalize());
            let added = new.len() as isize - s.seg_count as isize;
            splice(c, k, s.seg_count, new, a0, c0, !wraps);
            if !wraps {
                delta += added;
            }
        }
    }
    let mut out = BezPath::new();
    for c in &cs {
        append_contour(&mut out, c);
    }
    out
}

fn splice(c: &mut Contour, k: usize, cnt: usize, new: Vec<PathSeg>, a0: Point, c0: Point, restore_start: bool) {
    let n = c.segs.len();
    let cnt2 = new.len();
    if c.closed {
        let mut t = c.segs.clone();
        t.rotate_left(k);
        t.splice(0..cnt, new);
        let len = t.len();
        if len > cnt2 {
            set_end(&mut t[len - 1], a0);
            set_start(&mut t[cnt2], c0);
        }
        if restore_start && k + cnt <= n {
            let pos = (n - k - cnt + cnt2) % len;
            t.rotate_left(pos);
        }
        c.segs = t;
    } else {
        c.segs.splice(k..k + cnt, new);
        if k > 0 {
            set_end(&mut c.segs[k - 1], a0);
        }
        if k + cnt2 < c.segs.len() {
            set_start(&mut c.segs[k + cnt2], c0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path::{signed_area, start_point};

    /// 100 x 50 plate (CCW) with a notch cut up from the bottom edge.
    fn plate_with_slot(x: f64, w: f64, d: f64) -> BezPath {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((x, 0.0));
        p.line_to((x, d));
        p.line_to((x + w, d));
        p.line_to((x + w, 0.0));
        p.line_to((100.0, 0.0));
        p.line_to((100.0, 50.0));
        p.line_to((0.0, 50.0));
        p.close_path();
        p
    }

    fn round_slot(x: f64, w: f64, wall: f64) -> BezPath {
        let r = w / 2.0;
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((x, 0.0));
        p.line_to((x, wall));
        // Semicircle bulging up (+y), from (x, wall) to (x + w, wall), clockwise.
        let arc = Arc::new((x + r, wall), (r, r), std::f64::consts::PI, -std::f64::consts::PI, 0.0);
        for el in arc.append_iter(1e-6) {
            p.push(el);
        }
        p.line_to((x + w, 0.0));
        p.line_to((100.0, 0.0));
        p.line_to((100.0, 50.0));
        p.line_to((0.0, 50.0));
        p.close_path();
        p
    }

    #[test]
    fn detects_rect_slot() {
        let p = plate_with_slot(40.0, 6.0, 20.0);
        let s = find_slots(&p, &SlotQuery::default());
        assert_eq!(s.len(), 1, "{s:?}");
        let s = s[0];
        assert_eq!(s.kind, SlotKind::Rect);
        assert!((s.width - 6.0).abs() < 1e-9 && (s.depth - 20.0).abs() < 1e-9);
        assert_eq!(s.first_seg, 1);
        assert!(s.mouth.distance(Point::new(43.0, 0.0)) < 1e-9);
        assert!((s.axis - Vec2::new(0.0, 1.0)).hypot() < 1e-9);
        assert!(!s.protruding);
        // Filters.
        assert!(find_slots(&p, &SlotQuery { max_width: 5.0, ..Default::default() }).is_empty());
        assert!(find_slots(&p, &SlotQuery { min_depth: 25.0, ..Default::default() }).is_empty());
    }

    #[test]
    fn plain_rectangle_has_no_slots() {
        let r = kurbo::Shape::to_path(&kurbo::Rect::new(0.0, 0.0, 10.0, 5.0), 0.1);
        assert!(find_slots(&r, &SlotQuery { include_tabs: true, ..Default::default() }).is_empty());
        assert!(find_slots(&BezPath::new(), &SlotQuery::default()).is_empty());
    }

    #[test]
    fn same_slot_on_clockwise_contour() {
        let p = crate::path::reverse(&plate_with_slot(40.0, 6.0, 20.0));
        let s = find_slots(&p, &SlotQuery::default());
        assert_eq!(s.len(), 1);
        assert!((s[0].width - 6.0).abs() < 1e-9);
    }

    #[test]
    fn uneven_walls_rejected() {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((40.0, 0.0));
        p.line_to((40.0, 20.0));
        p.line_to((46.0, 20.0));
        p.line_to((46.0, 5.0));
        p.line_to((100.0, 5.0));
        p.line_to((100.0, 50.0));
        p.line_to((0.0, 50.0));
        p.close_path();
        assert!(find_slots(&p, &SlotQuery::default()).is_empty());
    }

    #[test]
    fn resize_rect_slot_width_and_depth() {
        let p = plate_with_slot(40.0, 6.0, 20.0);
        let s = find_slots(&p, &SlotQuery::default());
        let r = resize_slots(&p, &s, Some(8.0), Some(25.0));
        assert!((signed_area(&r) - (5000.0 - 8.0 * 25.0)).abs() < 1e-9);
        let s2 = find_slots(&r, &SlotQuery::default());
        assert_eq!(s2.len(), 1);
        assert!((s2[0].width - 8.0).abs() < 1e-9 && (s2[0].depth - 25.0).abs() < 1e-9);
        assert!(s2[0].mouth.distance(Point::new(43.0, 0.0)) < 1e-9);
        assert_eq!(start_point(&r), Some(Point::ZERO));
        // Width only.
        let r = resize_slots(&p, &s, Some(4.0), None);
        assert!((signed_area(&r) - (5000.0 - 4.0 * 20.0)).abs() < 1e-9);
    }

    #[test]
    fn slot_wrapping_over_start_node() {
        // Start the contour on the notch's bottom so the slot wraps.
        let p = plate_with_slot(40.0, 6.0, 20.0);
        let q = crate::path::set_start_point(&p, Point::new(40.0, 20.0), true);
        let s = find_slots(&q, &SlotQuery::default());
        assert_eq!(s.len(), 1);
        let r = resize_slots(&q, &s, Some(10.0), None);
        assert!((signed_area(&r) - (5000.0 - 200.0)).abs() < 1e-9);
    }

    #[test]
    fn two_slots_resized() {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        for x in [20.0, 60.0] {
            p.line_to((x, 0.0));
            p.line_to((x, 10.0));
            p.line_to((x + 5.0, 10.0));
            p.line_to((x + 5.0, 0.0));
        }
        p.line_to((100.0, 0.0));
        p.line_to((100.0, 50.0));
        p.line_to((0.0, 50.0));
        p.close_path();
        let s = find_slots(&p, &SlotQuery::default());
        assert_eq!(s.len(), 2);
        let r = resize_slots(&p, &s, Some(3.0), Some(12.0));
        assert!((signed_area(&r) - (5000.0 - 2.0 * 36.0)).abs() < 1e-9);
    }

    #[test]
    fn round_slot_detect_and_resize() {
        let p = round_slot(40.0, 6.0, 10.0);
        let s = find_slots(&p, &SlotQuery { tol: 1e-3, ..Default::default() });
        assert_eq!(s.len(), 1, "{s:?}");
        assert_eq!(s[0].kind, SlotKind::Round);
        assert!((s[0].width - 6.0).abs() < 1e-6);
        assert!((s[0].depth - 13.0).abs() < 1e-6);
        let a0 = 5000.0 - (6.0 * 10.0 + std::f64::consts::PI * 9.0 / 2.0);
        assert!((signed_area(&p) - a0).abs() < 1e-3);
        let r = resize_slots(&p, &s, Some(8.0), Some(14.0));
        let a1 = 5000.0 - (8.0 * 10.0 + std::f64::consts::PI * 16.0 / 2.0);
        assert!((signed_area(&r) - a1).abs() < 1e-3, "{} vs {a1}", signed_area(&r));
        let s2 = find_slots(&r, &SlotQuery { tol: 1e-3, ..Default::default() });
        assert_eq!(s2.len(), 1);
        assert!((s2[0].width - 8.0).abs() < 1e-6 && (s2[0].depth - 14.0).abs() < 1e-6);
    }

    #[test]
    fn protruding_tab_optional() {
        // Tab sticking down out of the bottom edge.
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((40.0, 0.0));
        p.line_to((40.0, -5.0));
        p.line_to((50.0, -5.0));
        p.line_to((50.0, 0.0));
        p.line_to((100.0, 0.0));
        p.line_to((100.0, 50.0));
        p.line_to((0.0, 50.0));
        p.close_path();
        assert!(find_slots(&p, &SlotQuery::default()).is_empty());
        let s = find_slots(&p, &SlotQuery { include_tabs: true, ..Default::default() });
        assert_eq!(s.len(), 1);
        assert!(s[0].protruding);
        let r = resize_slots(&p, &s, Some(12.0), None);
        assert!((signed_area(&r) - (5000.0 + 60.0)).abs() < 1e-9);
    }

    #[test]
    fn open_path_slot() {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((40.0, 0.0));
        p.line_to((40.0, 10.0));
        p.line_to((46.0, 10.0));
        p.line_to((46.0, 0.0));
        p.line_to((100.0, 0.0));
        let s = find_slots(&p, &SlotQuery { include_tabs: true, ..Default::default() });
        assert_eq!(s.len(), 1);
        let r = resize_slots(&p, &s, Some(10.0), None);
        let c = contours(&r).remove(0);
        assert_eq!(c.segs[0].end(), Point::new(38.0, 0.0));
        assert_eq!(c.segs[4].start(), Point::new(48.0, 0.0));
        assert_eq!(c.end(), Some(Point::new(100.0, 0.0)));
    }
}
