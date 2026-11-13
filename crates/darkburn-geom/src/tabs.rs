//! Tabs (bridges), perforation and lead-in/lead-out.
//!
//! Distances are arc lengths along a contour measured from its start node. Curves are
//! kept exact: pieces are cut with `subsegment`, never flattened.

use crate::path::{Contour, DEFAULT_ACCURACY, SegParam, append_contour, contours, seg_tangent};
use kurbo::{Arc, BezPath, CubicBez, Line, ParamCurve, ParamCurveArclen, PathEl, PathSeg, Point, Vec2};

const EPS: f64 = 1e-9;

/// Length of each segment of a contour.
pub fn seg_lengths(c: &Contour) -> Vec<f64> {
    c.segs.iter().map(|s| s.arclen(DEFAULT_ACCURACY)).collect()
}

/// Location (segment and parameter) and position at distance `d` along a contour.
/// Closed contours wrap `d` around; open ones clamp it to `0..=length`.
pub fn point_at_distance(c: &Contour, d: f64) -> Option<(SegParam, Point)> {
    let lens = seg_lengths(c);
    locate(c, &lens, d)
}

fn locate(c: &Contour, lens: &[f64], d: f64) -> Option<(SegParam, Point)> {
    if c.segs.is_empty() {
        return None;
    }
    let total: f64 = lens.iter().sum();
    // A contour whose length is not a number (coordinates too large) has no place at a distance.
    if total.is_nan() {
        return None;
    }
    let mut d = if c.closed && total > 0.0 { d.rem_euclid(total) } else { d.clamp(0.0, total) };
    for (i, s) in c.segs.iter().enumerate() {
        if d <= lens[i] || i + 1 == c.segs.len() {
            let t = if lens[i] <= 0.0 { 0.0 } else { s.inv_arclen(d.min(lens[i]), DEFAULT_ACCURACY).clamp(0.0, 1.0) };
            return Some((SegParam { seg: i, t }, s.eval(t)));
        }
        d -= lens[i];
    }
    None
}

/// Extracts the part of a contour between distances `d0` and `d1` (`d0 < d1`) as exact
/// segments. On closed contours `d0` is wrapped into `0..length` and the range may run
/// past the start node (it wraps around, at most one full turn); open contours clamp.
pub fn extract(c: &Contour, d0: f64, d1: f64) -> Vec<PathSeg> {
    let lens = seg_lengths(c);
    extract_with(c, &lens, d0, d1)
}

fn extract_with(c: &Contour, lens: &[f64], d0: f64, d1: f64) -> Vec<PathSeg> {
    let total: f64 = lens.iter().sum();
    if c.segs.is_empty() || total <= 0.0 || d1 <= d0 {
        return Vec::new();
    }
    if c.closed {
        let span = (d1 - d0).min(total);
        let a = d0.rem_euclid(total);
        let b = a + span;
        if b > total + EPS {
            let mut out = extract_linear(c, lens, a, total);
            out.extend(extract_linear(c, lens, 0.0, b - total));
            return out;
        }
        extract_linear(c, lens, a, b.min(total))
    } else {
        extract_linear(c, lens, d0.max(0.0), d1.min(total))
    }
}

fn extract_linear(c: &Contour, lens: &[f64], a: f64, b: f64) -> Vec<PathSeg> {
    let mut out = Vec::new();
    let mut s0 = 0.0;
    for (i, seg) in c.segs.iter().enumerate() {
        let s1 = s0 + lens[i];
        let lo = a.max(s0);
        let hi = b.min(s1);
        if hi - lo > EPS && lens[i] > 0.0 {
            let t0 = if lo <= s0 { 0.0 } else { seg.inv_arclen(lo - s0, DEFAULT_ACCURACY).clamp(0.0, 1.0) };
            let t1 = if hi >= s1 { 1.0 } else { seg.inv_arclen(hi - s0, DEFAULT_ACCURACY).clamp(0.0, 1.0) };
            if t1 > t0 {
                out.push(if t0 == 0.0 && t1 == 1.0 { *seg } else { seg.subsegment(t0..t1) });
            }
        }
        s0 = s1;
    }
    out
}

/// Options for [`place_tabs`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TabOptions {
    /// Number of tabs per closed subpath when `spacing` is `None`.
    pub count: usize,
    /// When set, the count is `round(length / spacing)` per subpath.
    pub spacing: Option<f64>,
    /// Lower bound on the count (applied after `spacing`).
    pub min_count: usize,
    /// Upper bound on the count.
    pub max_count: usize,
    /// Gap length of each tab (mm along the path).
    pub size: f64,
    /// Subpaths shorter than this get no tabs.
    pub skip_shorter_than: Option<f64>,
    /// Shifts the first tab along the path (mm). With 0 the first tab centre sits half a
    /// spacing after the start node, so the start node is never in a gap.
    pub offset_from_start: f64,
}

impl Default for TabOptions {
    fn default() -> Self {
        Self { count: 4, spacing: None, min_count: 0, max_count: usize::MAX, size: 1.0, skip_shorter_than: None, offset_from_start: 0.0 }
    }
}

/// One tab: a gap left uncut.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Gap {
    /// Subpath index (in [`contours`] order).
    pub subpath: usize,
    /// Distance along the subpath where the gap starts (wrapped into `0..length`).
    pub start: f64,
    /// Gap length.
    pub len: f64,
    /// Position where the gap starts.
    pub from: Point,
    /// Position where the gap ends.
    pub to: Point,
}

/// Result of placing tabs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TabResult {
    /// The pieces still to cut. Subpaths without tabs are kept as they were.
    pub cut: BezPath,
    /// The gaps that were left.
    pub gaps: Vec<Gap>,
}

/// Places tabs evenly by arc length on every closed subpath of `path`. Open subpaths are
/// passed through unchanged. If the tabs would not fit (`count * size >= length`) the count
/// is reduced until they do.
pub fn place_tabs(path: &BezPath, opts: &TabOptions) -> TabResult {
    let cs = contours(path);
    let mut per: Vec<Vec<(f64, f64)>> = vec![Vec::new(); cs.len()];
    for (i, c) in cs.iter().enumerate() {
        if !c.closed || opts.size <= 0.0 {
            continue;
        }
        let l = c.length();
        if l <= EPS || opts.skip_shorter_than.is_some_and(|s| l < s) {
            continue;
        }
        let mut n = match opts.spacing {
            Some(s) if s > 0.0 => (l / s).round() as usize,
            _ => opts.count,
        };
        n = n.max(opts.min_count).min(opts.max_count);
        let fit = ((l - EPS) / opts.size).floor().max(0.0) as usize;
        if n >= fit {
            n = fit.saturating_sub(if fit as f64 * opts.size >= l - EPS { 1 } else { 0 });
        }
        if n == 0 {
            continue;
        }
        let step = l / n as f64;
        for k in 0..n {
            let centre = opts.offset_from_start + step * (k as f64 + 0.5);
            per[i].push((centre - opts.size * 0.5, opts.size));
        }
    }
    apply_gaps(&cs, per)
}

/// Places tabs of length `size` centred at the given distances, measured along the whole
/// path (subpaths one after the other, in order). Works on open and closed subpaths.
pub fn place_tabs_at(path: &BezPath, distances: &[f64], size: f64) -> TabResult {
    let cs = contours(path);
    let lens: Vec<f64> = cs.iter().map(Contour::length).collect();
    let mut per: Vec<Vec<(f64, f64)>> = vec![Vec::new(); cs.len()];
    if size > 0.0 {
        for &d in distances {
            let mut acc = 0.0;
            for (i, l) in lens.iter().enumerate() {
                if d <= acc + l || i + 1 == lens.len() {
                    per[i].push((d - acc - size * 0.5, size));
                    break;
                }
                acc += l;
            }
        }
    }
    apply_gaps(&cs, per)
}

/// Merges gap intervals `(start, len)` and cuts each contour accordingly.
fn apply_gaps(cs: &[Contour], per: Vec<Vec<(f64, f64)>>) -> TabResult {
    let mut res = TabResult::default();
    for (i, (c, gaps)) in cs.iter().zip(per).enumerate() {
        if gaps.is_empty() {
            append_contour(&mut res.cut, c);
            continue;
        }
        let lens = seg_lengths(c);
        let l: f64 = lens.iter().sum();
        // Normalise to (start, end) on the unrolled line.
        let mut iv: Vec<(f64, f64)> = gaps
            .iter()
            .map(|&(s, len)| {
                if c.closed {
                    let a = s.rem_euclid(l);
                    (a, a + len.min(l))
                } else {
                    (s.max(0.0), (s + len).min(l))
                }
            })
            .filter(|(a, b)| b > a)
            .collect();
        iv.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut merged: Vec<(f64, f64)> = Vec::new();
        for (a, b) in iv {
            match merged.last_mut() {
                Some(last) if a <= last.1 => last.1 = last.1.max(b),
                _ => merged.push((a, b)),
            }
        }
        if c.closed && merged.len() > 1 {
            // A gap running past the end may swallow gaps at the start.
            let wrap_end = merged.last().map(|g| g.1 - l).unwrap_or(0.0);
            while merged.len() > 1 && merged[0].0 <= wrap_end {
                let first = merged.remove(0);
                let last = merged.last_mut().expect("non-empty");
                last.1 = last.1.max(first.1 + l);
            }
        }
        for &(a, b) in &merged {
            let from = locate(c, &lens, a).map(|x| x.1).unwrap_or_default();
            let to = locate(c, &lens, b).map(|x| x.1).unwrap_or_default();
            res.gaps.push(Gap { subpath: i, start: a, len: b - a, from, to });
        }
        let mut pieces: Vec<(f64, f64)> = Vec::new();
        if c.closed {
            if merged.iter().map(|g| g.1 - g.0).sum::<f64>() < l - EPS {
                for k in 0..merged.len() {
                    let a = merged[k].1;
                    let b = if k + 1 < merged.len() { merged[k + 1].0 } else { merged[0].0 + l };
                    pieces.push((a, b));
                }
            }
        } else {
            let mut cur = 0.0;
            for &(a, b) in &merged {
                pieces.push((cur, a));
                cur = b;
            }
            pieces.push((cur, l));
        }
        for (a, b) in pieces {
            if b - a > EPS {
                let segs = extract_with(c, &lens, a, b);
                append_contour(&mut res.cut, &Contour::new(segs, false));
            }
        }
    }
    res
}

/// Perforation: cuts `cut_len`, skips `skip_len`, repeating along every subpath. The
/// pattern starts `start_offset` mm along each subpath (negative values start mid-dash).
/// Returns open dashes with curves kept. `cut_len <= 0` gives an empty path;
/// `skip_len <= 0` returns the path unchanged. Lengths or an offset that are not finite
/// numbers give an empty path, and a subpath whose length is not finite gets no dashes: there
/// is no telling where they would go.
pub fn perforate(path: &BezPath, cut_len: f64, skip_len: f64, start_offset: f64) -> BezPath {
    if cut_len <= 0.0 {
        return BezPath::new();
    }
    if skip_len <= 0.0 {
        return path.clone();
    }
    let period = cut_len + skip_len;
    // The dash before the start of the path. Past 2^53 dashes away a step of one dash is lost
    // in the rounding, and the loop below would stand still.
    let first = ((-start_offset) / period).floor() - 1.0;
    if !(period.is_finite() && start_offset.is_finite() && first.abs() < 9e15) {
        return BezPath::new();
    }
    let mut out = BezPath::new();
    for c in contours(path) {
        let lens = seg_lengths(&c);
        let l: f64 = lens.iter().sum();
        if l <= EPS || !l.is_finite() {
            continue;
        }
        let mut k = first;
        loop {
            let a = start_offset + k * period;
            if a >= l {
                break;
            }
            let (lo, hi) = (a.max(0.0), (a + cut_len).min(l));
            if hi - lo > EPS {
                let segs = extract_linear(&c, &lens, lo, hi);
                append_contour(&mut out, &Contour::new(segs, false));
            }
            k += 1.0;
        }
    }
    out
}

/// Shape of a lead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeadKind {
    Line,
    /// A circular arc tangent to the path at the point where it joins.
    Arc,
}

/// Which side of the path a lead lies on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeadSide {
    /// Outside the shape (decided by the contour's winding; open contours are closed
    /// implicitly for this, and a zero-area contour counts as counter-clockwise).
    Outside,
    /// Inside the shape.
    Inside,
    /// Left of the travel direction.
    Left,
    /// Right of the travel direction.
    Right,
}

/// A lead-in or lead-out.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lead {
    pub kind: LeadKind,
    /// Length of the lead (for arcs, the arc length).
    pub length: f64,
    /// For lines: angle between the lead and the path tangent (0 = along the tangent,
    /// 90 = perpendicular). For arcs: the swept angle (90 = arrives perpendicular).
    pub angle_deg: f64,
    pub side: LeadSide,
}

/// Lead-in and lead-out options.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LeadOptions {
    pub lead_in: Option<Lead>,
    pub lead_out: Option<Lead>,
}

fn side_normal(c: &Contour, tangent: Vec2, side: LeadSide) -> Vec2 {
    let left = Vec2::new(-tangent.y, tangent.x);
    let ccw = c.signed_area() >= 0.0;
    let use_left = match side {
        LeadSide::Left => true,
        LeadSide::Right => false,
        // Interior of a CCW contour is on the left.
        LeadSide::Inside => ccw,
        LeadSide::Outside => !ccw,
    };
    if use_left { left } else { -left }
}

/// Builds the lead segments. For a lead-in they run from the lead start to `at`; for a
/// lead-out from `at` outwards.
fn lead_segs(at: Point, tangent: Vec2, normal: Vec2, lead: &Lead, is_in: bool) -> Vec<PathSeg> {
    if lead.length <= EPS || tangent == Vec2::ZERO {
        return Vec::new();
    }
    let a = lead.angle_deg.to_radians();
    match lead.kind {
        LeadKind::Line => {
            if is_in {
                let p0 = at + lead.length * (-a.cos() * tangent + a.sin() * normal);
                vec![PathSeg::Line(Line::new(p0, at))]
            } else {
                let p1 = at + lead.length * (a.cos() * tangent + a.sin() * normal);
                vec![PathSeg::Line(Line::new(at, p1))]
            }
        }
        LeadKind::Arc => {
            if a.abs() <= 1e-6 {
                let l = Lead { kind: LeadKind::Line, angle_deg: 0.0, ..*lead };
                return lead_segs(at, tangent, normal, &l, is_in);
            }
            let r = lead.length / a.abs();
            let center = at + r * normal;
            // Centre on the left means counter-clockwise travel.
            let sgn = if tangent.cross(normal) > 0.0 { 1.0 } else { -1.0 };
            let theta = (at - center).atan2();
            let (start_angle, sweep) = if is_in { (theta - sgn * a, sgn * a) } else { (theta, sgn * a) };
            let arc = Arc::new(center, Vec2::new(r, r), start_angle, sweep, 0.0);
            let mut cur = if is_in { center + r * Vec2::new(start_angle.cos(), start_angle.sin()) } else { at };
            let mut segs = Vec::new();
            for el in arc.append_iter(1e-6) {
                if let PathEl::CurveTo(p1, p2, p3) = el {
                    segs.push(PathSeg::Cubic(CubicBez::new(cur, p1, p2, p3)));
                    cur = p3;
                }
            }
            // Snap the joint exactly onto the path.
            if is_in && let Some(PathSeg::Cubic(last)) = segs.last_mut() {
                last.p3 = at;
            }
            segs
        }
    }
}

/// Adds a lead-in before and a lead-out after every subpath. Closed subpaths become open
/// paths that start at the lead-in start, run once round the shape and leave by the
/// lead-out (no overcut). Empty subpaths are dropped.
pub fn lead_in_out(path: &BezPath, opts: &LeadOptions) -> BezPath {
    let mut out = BezPath::new();
    for c in contours(path) {
        let (Some(first), Some(last)) = (c.segs.first(), c.segs.last()) else { continue };
        let mut segs = Vec::new();
        if let Some(lead) = &opts.lead_in {
            let t = seg_tangent(first, 0.0);
            let n = side_normal(&c, t, lead.side);
            segs.extend(lead_segs(first.start(), t, n, lead, true));
        }
        segs.extend(c.segs.iter().copied());
        if let Some(lead) = &opts.lead_out {
            let t = seg_tangent(last, 1.0);
            let n = side_normal(&c, t, lead.side);
            segs.extend(lead_segs(last.end(), t, n, lead, false));
        }
        append_contour(&mut out, &Contour::new(segs, false));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path::{bounding_box, is_open, length, subpaths};
    use kurbo::{Circle, Rect, Shape};

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> BezPath {
        Rect::new(x0, y0, x1, y1).to_path(0.1)
    }

    #[test]
    fn distance_lookup_and_extract() {
        let c = contours(&rect(0.0, 0.0, 10.0, 10.0)).remove(0);
        let (_, p) = point_at_distance(&c, 15.0).unwrap();
        assert!(p.distance(Point::new(10.0, 5.0)) < 1e-9);
        // Wraps on closed contours.
        let (_, p) = point_at_distance(&c, 45.0).unwrap();
        assert!(p.distance(Point::new(5.0, 0.0)) < 1e-9);
        let segs = extract(&c, 35.0, 45.0);
        let l: f64 = segs.iter().map(|s| s.arclen(1e-9)).sum();
        assert!((l - 10.0).abs() < 1e-9);
        assert!(segs[0].start().distance(Point::new(0.0, 5.0)) < 1e-9);
        assert!(segs.last().unwrap().end().distance(Point::new(5.0, 0.0)) < 1e-9);
        // Continuity.
        for w in segs.windows(2) {
            assert!(w[0].end().distance(w[1].start()) < 1e-9);
        }
        assert!(extract(&c, 5.0, 5.0).is_empty());
        assert!(point_at_distance(&Contour::new(vec![], true), 1.0).is_none());
    }

    #[test]
    fn extract_on_curve_is_exact() {
        let c = contours(&Circle::new((0.0, 0.0), 10.0).to_path(1e-6)).remove(0);
        let l = c.length();
        let segs = extract(&c, l * 0.1, l * 0.35);
        let el: f64 = segs.iter().map(|s| s.arclen(1e-9)).sum();
        assert!((el - l * 0.25).abs() < 1e-4);
        for s in &segs {
            for i in 0..=4 {
                assert!((s.eval(i as f64 / 4.0).to_vec2().hypot() - 10.0).abs() < 1e-3);
            }
        }
    }

    #[test]
    fn four_tabs_on_square() {
        let p = rect(0.0, 0.0, 100.0, 100.0);
        let r = place_tabs(&p, &TabOptions { count: 4, size: 2.0, ..Default::default() });
        assert_eq!(r.gaps.len(), 4);
        assert!((length(&r.cut) - 392.0).abs() < 1e-6);
        assert_eq!(subpaths(&r.cut).len(), 4);
        assert!(is_open(&r.cut, 1e-9));
        // First tab centre half a spacing (50 mm) along: middle of the bottom edge.
        let g = r.gaps[0];
        assert!(g.from.distance(Point::new(49.0, 0.0)) < 1e-9, "{g:?}");
        assert!(g.to.distance(Point::new(51.0, 0.0)) < 1e-9);
    }

    #[test]
    fn tab_count_by_spacing_and_clamp() {
        let p = rect(0.0, 0.0, 100.0, 100.0);
        let r = place_tabs(&p, &TabOptions { spacing: Some(50.0), size: 1.0, ..Default::default() });
        assert_eq!(r.gaps.len(), 8);
        let r = place_tabs(&p, &TabOptions { spacing: Some(50.0), max_count: 3, size: 1.0, ..Default::default() });
        assert_eq!(r.gaps.len(), 3);
        let r = place_tabs(&p, &TabOptions { spacing: Some(1000.0), min_count: 2, size: 1.0, ..Default::default() });
        assert_eq!(r.gaps.len(), 2);
    }

    #[test]
    fn tabs_skip_short_open_and_degenerate() {
        let small = rect(0.0, 0.0, 1.0, 1.0);
        let r = place_tabs(&small, &TabOptions { skip_shorter_than: Some(10.0), ..Default::default() });
        assert!(r.gaps.is_empty());
        assert_eq!(r.cut, small);
        // Tabs larger than the shape: fewer, then none.
        let r = place_tabs(&small, &TabOptions { count: 4, size: 1.5, ..Default::default() });
        assert_eq!(r.gaps.len(), 2);
        let r = place_tabs(&small, &TabOptions { count: 4, size: 10.0, ..Default::default() });
        assert!(r.gaps.is_empty());
        // Open paths pass through.
        let mut line = BezPath::new();
        line.move_to((0.0, 0.0));
        line.line_to((50.0, 0.0));
        let r = place_tabs(&line, &TabOptions::default());
        assert!(r.gaps.is_empty());
        assert!((length(&r.cut) - 50.0).abs() < 1e-9);
        let r = place_tabs(&BezPath::new(), &TabOptions::default());
        assert!(r.cut.is_empty());
        // Zero-length contour.
        let mut z = BezPath::new();
        z.move_to((1.0, 1.0));
        z.line_to((1.0, 1.0));
        z.close_path();
        assert!(place_tabs(&z, &TabOptions::default()).gaps.is_empty());
    }

    #[test]
    fn tabs_on_outer_and_hole() {
        let mut p = rect(0.0, 0.0, 100.0, 100.0);
        p.extend(rect(40.0, 40.0, 60.0, 60.0));
        let r = place_tabs(&p, &TabOptions { count: 2, size: 2.0, ..Default::default() });
        assert_eq!(r.gaps.len(), 4);
        assert_eq!(r.gaps.iter().filter(|g| g.subpath == 1).count(), 2);
        assert!((length(&r.cut) - (400.0 + 80.0 - 8.0)).abs() < 1e-6);
    }

    #[test]
    fn tab_wrapping_over_start_and_manual_tabs() {
        let p = rect(0.0, 0.0, 10.0, 10.0);
        let r = place_tabs_at(&p, &[0.0], 2.0);
        assert_eq!(r.gaps.len(), 1);
        assert_eq!(subpaths(&r.cut).len(), 1);
        assert!((length(&r.cut) - 38.0).abs() < 1e-9);
        let start = crate::path::start_point(&r.cut).unwrap();
        assert!(start.distance(Point::new(1.0, 0.0)) < 1e-9);
        // Overlapping manual tabs merge.
        let r = place_tabs_at(&p, &[5.0, 6.0], 2.0);
        assert_eq!(r.gaps.len(), 1);
        assert!((length(&r.cut) - 37.0).abs() < 1e-9);
        // Distances spill into the second subpath.
        let mut two = rect(0.0, 0.0, 10.0, 10.0);
        two.extend(rect(20.0, 0.0, 30.0, 10.0));
        let r = place_tabs_at(&two, &[45.0], 2.0);
        assert_eq!(r.gaps[0].subpath, 1);
        assert!(place_tabs_at(&p, &[1.0], 0.0).gaps.is_empty());
    }

    #[test]
    fn tabs_on_self_intersecting_path() {
        let mut b = BezPath::new();
        b.move_to((0.0, 0.0));
        b.line_to((10.0, 10.0));
        b.line_to((10.0, 0.0));
        b.line_to((0.0, 10.0));
        b.close_path();
        let l = length(&b);
        let r = place_tabs(&b, &TabOptions { count: 3, size: 1.0, ..Default::default() });
        assert_eq!(r.gaps.len(), 3);
        assert!((length(&r.cut) - (l - 3.0)).abs() < 1e-6);
    }

    #[test]
    fn perforation_lengths() {
        let c = Circle::new((0.0, 0.0), 20.0).to_path(1e-6);
        let l = length(&c);
        let d = perforate(&c, 3.0, 1.0, 0.0);
        let dl = length(&d);
        assert!((dl - l * 0.75).abs() < 3.0, "{dl} vs {}", l * 0.75);
        let mut line = BezPath::new();
        line.move_to((0.0, 0.0));
        line.line_to((100.0, 0.0));
        let d = perforate(&line, 3.0, 2.0, 0.0);
        assert_eq!(subpaths(&d).len(), 20);
        assert!((length(&d) - 60.0).abs() < 1e-9);
        // Offset shifts the pattern: starts mid-dash.
        let d = perforate(&line, 3.0, 2.0, -1.0);
        assert!((crate::path::length(&subpaths(&d)[0]) - 2.0).abs() < 1e-9);
        assert!(perforate(&line, 0.0, 2.0, 0.0).is_empty());
        assert_eq!(perforate(&line, 3.0, 0.0, 0.0), line);
        assert!(perforate(&BezPath::new(), 1.0, 1.0, 0.0).is_empty());
    }

    #[test]
    fn line_leads_outside_square() {
        // CCW square starting at the origin, going +x.
        let p = rect(0.0, 0.0, 10.0, 10.0);
        let lead = Lead { kind: LeadKind::Line, length: 2.0, angle_deg: 90.0, side: LeadSide::Outside };
        let out = lead_in_out(&p, &LeadOptions { lead_in: Some(lead), lead_out: Some(lead) });
        let c = contours(&out).remove(0);
        assert!(!c.closed);
        // Outside of a CCW square at its bottom edge is -y.
        assert!(c.start().unwrap().distance(Point::new(0.0, -2.0)) < 1e-9, "{:?}", c.start());
        assert!(c.segs[0].end().distance(Point::ZERO) < 1e-9);
        // Lead-out leaves from the start node (end of the closing edge, going -y): outside is -x.
        assert!(c.end().unwrap().distance(Point::new(-2.0, 0.0)) < 1e-9, "{:?}", c.end());
        assert!((length(&out) - 44.0).abs() < 1e-9);
        let inside = Lead { side: LeadSide::Inside, ..lead };
        let out = lead_in_out(&p, &LeadOptions { lead_in: Some(inside), lead_out: None });
        assert!(crate::path::start_point(&out).unwrap().distance(Point::new(0.0, 2.0)) < 1e-9);
    }

    #[test]
    fn arc_lead_is_tangent_and_perpendicular_at_start() {
        let mut line = BezPath::new();
        line.move_to((0.0, 0.0));
        line.line_to((10.0, 0.0));
        let len = std::f64::consts::FRAC_PI_2 * 2.0; // radius 2 quarter arc
        let lead = Lead { kind: LeadKind::Arc, length: len, angle_deg: 90.0, side: LeadSide::Left };
        let out = lead_in_out(&line, &LeadOptions { lead_in: Some(lead), lead_out: Some(lead) });
        let c = contours(&out).remove(0);
        // Starts 2 mm left of the path, 2 mm before the start: (-2, 2).
        assert!(c.start().unwrap().distance(Point::new(-2.0, 2.0)) < 1e-6, "{:?}", c.start());
        // Initial direction is perpendicular (towards -y), arrival is tangent (+x).
        let t0 = seg_tangent(&c.segs[0], 0.0);
        assert!((t0 - Vec2::new(0.0, -1.0)).hypot() < 1e-3, "{t0:?}");
        let idx = c.segs.iter().position(|s| matches!(s, PathSeg::Line(_))).unwrap();
        let t_in = seg_tangent(&c.segs[idx - 1], 1.0);
        assert!((t_in - Vec2::new(1.0, 0.0)).hypot() < 1e-3);
        assert_eq!(c.segs[idx - 1].end(), Point::ZERO);
        // Lead-out ends 2 mm up from (12, 0).
        assert!(c.end().unwrap().distance(Point::new(12.0, 2.0)) < 1e-6, "{:?}", c.end());
        assert!((length(&out) - (10.0 + 2.0 * len)).abs() < 1e-4);
        let bb = bounding_box(&out).unwrap();
        assert!(bb.y0 > -1e-6);
    }

    #[test]
    fn leads_degenerate() {
        let lead = Lead { kind: LeadKind::Arc, length: 0.0, angle_deg: 45.0, side: LeadSide::Right };
        let p = rect(0.0, 0.0, 1.0, 1.0);
        let out = lead_in_out(&p, &LeadOptions { lead_in: Some(lead), lead_out: None });
        assert!((length(&out) - 4.0).abs() < 1e-9);
        assert!(lead_in_out(&BezPath::new(), &LeadOptions::default()).is_empty());
        // Arc with zero angle falls back to a straight tangent line.
        let lead = Lead { kind: LeadKind::Arc, length: 3.0, angle_deg: 0.0, side: LeadSide::Left };
        let out = lead_in_out(&p, &LeadOptions { lead_in: Some(lead), lead_out: None });
        assert!(crate::path::start_point(&out).unwrap().distance(Point::new(-3.0, 0.0)) < 1e-9);
    }
}
