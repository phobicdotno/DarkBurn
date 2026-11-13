//! Line mode: turns one planned path into operations (lead-in/out, pauses, overcut, tabs,
//! perforation, dot mode).

use std::f64::consts::FRAC_PI_2;

use kurbo::{Point, Vec2};

use crate::geom::{self, EPS};
use crate::input::{CutSettings, Lead, LeadStyle, TabSettings};
use crate::ir::Op;

/// Per-path context for [`emit_path`].
#[derive(Clone, Copy, Debug)]
pub struct PathContext<'a> {
    pub settings: &'a CutSettings,
    /// Shape power scale 0..=1.
    pub power_scale: f64,
    /// How many closed shapes this path lies inside (even = outer, odd = hole).
    pub depth: usize,
    /// Tab centres on this path (see [`tab_centres`]). Each snaps to the nearest path point.
    pub tabs: &'a [Point],
    /// Backlash approach distance (hide backlash), if on.
    pub backlash: Option<f64>,
}

/// Where the tabs of one sub-path go, as points on it. `pts` is the sub-path as drawn (closed:
/// the first vertex is not repeated). `manual` holds the manually placed tabs that belong to
/// this sub-path when its shape has any (they replace the automatic ones on that shape);
/// `None` asks for the automatic tabs, which are spaced evenly from the drawn start, so they
/// stay put whatever start point the planner picks.
pub fn tab_centres(pts: &[Point], closed: bool, depth: usize, tabs: &TabSettings, manual: Option<&[Point]>) -> Vec<Point> {
    if !tabs.enabled || pts.len() < 2 {
        return Vec::new();
    }
    if let Some(m) = manual {
        return m.to_vec();
    }
    if !closed || !tabs.auto || (tabs.skip_inner && depth > 0) {
        return Vec::new();
    }
    let mut ring = pts.to_vec();
    ring.push(pts[0]);
    let perimeter = geom::polyline_length(&ring);
    if perimeter <= EPS {
        return Vec::new();
    }
    let count = if tabs.spacing > 0.0 { (perimeter / tabs.spacing).round() as u32 } else { tabs.min_count };
    let count = count.clamp(tabs.min_count, tabs.max_count.max(tabs.min_count));
    (0..count).map(|k| geom::point_at(&ring, (k as f64 + 0.5) * perimeter / count as f64).0).collect()
}

/// Add one tab over `a..b` to the power profile: laser off, reduced power, or (partly cut tabs
/// with a pulse width) full-power pulses with pauses between them.
fn push_tab(intervals: &mut Vec<(f64, f64, f64)>, a: f64, b: f64, tab_power: f64, power: f64, pulse_width: f64) {
    if tab_power > 0.0 && tab_power < power && pulse_width > EPS {
        let duty = tab_power / power;
        let n = ((b - a) * duty / pulse_width).round().max(1.0);
        let period = (b - a) / n;
        for k in 0..n as usize {
            let from = a + k as f64 * period;
            intervals.push((from, from + period * (1.0 - duty), 0.0));
        }
    } else {
        intervals.push((a, b, tab_power));
    }
}

/// Emit a planned path. `pts` are in cut order; closed paths repeat the start point at the end.
pub fn emit_path(pts: &[Point], closed: bool, ctx: &PathContext, out: &mut Vec<Op>) {
    if pts.len() < 2 {
        return;
    }
    let s = ctx.settings;
    let ls = &s.line;
    let speed = s.speed;
    let power = s.max_power * ctx.power_scale;
    let start = pts[0];
    let end = *pts.last().unwrap();
    let t0 = geom::unit(pts[1] - pts[0]);
    let t1 = geom::unit(end - pts[pts.len() - 2]);

    // Waste side normal: outer shapes have waste outside, holes inside.
    let area = if closed { geom::signed_area(&pts[..pts.len() - 1]) } else { 0.0 };
    let interior_left = area > 0.0;
    let waste_left = if closed { interior_left == (ctx.depth % 2 == 1) } else { true };
    let normal = |t: Vec2| if waste_left { Vec2::new(-t.y, t.x) } else { Vec2::new(t.y, -t.x) };

    // Path with overcut.
    let perimeter = geom::polyline_length(pts);
    let mut path = pts.to_vec();
    // Overcut closes a cut line over its own start. Dots have no line to close: going on past
    // the start would only fire the first dots again, or new ones between them.
    let dots = ls.dot_mode.is_some_and(|d| d.spacing > EPS);
    let overcut = if dots { 0.0 } else { ls.overcut };
    if closed && overcut > 0.0 {
        let oc = overcut.min(perimeter);
        let pieces = geom::split_at(pts, &[oc]);
        path.extend(pieces[0].iter().skip(1));
    }
    let total = geom::polyline_length(&path);

    // Power profile: sorted breakpoints with the power of the piece after each one.
    let mut intervals: Vec<(f64, f64, f64)> = Vec::new(); // (from, to, power)
    let tabs = &ls.tabs;
    if tabs.enabled && tabs.size > 0.0 && perimeter > EPS {
        let tab_power = tabs.tab_power.unwrap_or(0.0).min(power);
        let mut centers: Vec<f64> = ctx.tabs.iter().map(|p| geom::nearest_arclen(pts, *p).0).collect();
        centers.sort_by(|a, b| a.total_cmp(b));
        // Tabs hold the part; they never take more than half of its outline, or a small shape
        // (or a tab longer than the shape) would not be cut at all.
        let size = if centers.is_empty() { tabs.size } else { tabs.size.min(perimeter / (2.0 * centers.len() as f64)) };
        // A tab next to the start of a closed path wraps around it (and shows again in the overcut).
        let turns: &[f64] = if closed { &[-1.0, 0.0, 1.0] } else { &[0.0] };
        for c in centers {
            for turn in turns {
                let a = (turn * perimeter + c - size / 2.0).max(0.0);
                let b = (turn * perimeter + c + size / 2.0).min(total);
                if b > a + EPS {
                    push_tab(&mut intervals, a, b, tab_power, power, tabs.pulse_width);
                }
            }
        }
    }
    if let Some(pf) = ls.perforation
        && pf.cut > 0.0
        && pf.skip > 0.0
        && total > EPS
    {
        if total < pf.cut + pf.skip {
            // Too short for a whole cut and skip: it still gets one skip, at its end and no
            // longer than half of it, or a small part would be cut free.
            intervals.push((total - pf.skip.min(total / 2.0), total, 0.0));
        } else {
            let period = pf.cut + pf.skip;
            let mut a = pf.cut;
            while a < total {
                intervals.push((a, (a + pf.skip).min(total), 0.0));
                a += period;
            }
        }
    }
    let power_at = |s_mid: f64| -> f64 { intervals.iter().filter(|(a, b, _)| s_mid > *a && s_mid < *b).fold(power, |p, (_, _, q)| p.min(*q)) };
    let mut breaks: Vec<f64> = intervals.iter().flat_map(|(a, b, _)| [*a, *b]).filter(|v| *v > EPS && *v < total - EPS).collect();
    breaks.sort_by(|a, b| a.partial_cmp(b).unwrap());
    breaks.dedup_by(|a, b| (*a - *b).abs() < EPS);

    // Lead-in start. Leads lie on the waste side, and only a closed shape has one.
    let lead_in = ls.lead_in.filter(|l| closed && l.length > EPS);
    let lead_out = ls.lead_out.filter(|l| closed && l.length > EPS);
    let lead_in_op = lead_in.map(|l| lead_geometry(l, start, t0, normal(t0), true, speed, power));
    let entry = lead_in_op.as_ref().map_or(start, |(p, _)| *p);

    // Approach.
    if let Some(d) = ctx.backlash {
        out.push(Op::Rapid { to: entry - Vec2::new(d, d) });
    }
    out.push(Op::Rapid { to: entry });
    if ls.start_pause_ms > 0.0 {
        out.push(Op::Dwell { ms: ls.start_pause_ms, power: if ls.cut_through_start { ls.pause_power } else { 0.0 } });
    }
    if let Some((_, op)) = lead_in_op {
        out.push(op);
    }

    if let Some(dm) = s.line.dot_mode.filter(|d| d.spacing > EPS) {
        let mut d = 0.0;
        let mut first = true;
        while d <= total + EPS {
            let (p, _) = geom::point_at(&path, d);
            let pw = power_at(d);
            let repeat = !first && closed && p.distance(start) < EPS;
            if pw > 0.0 && !repeat {
                if !first || p.distance(entry) > EPS {
                    out.push(Op::Rapid { to: p });
                }
                out.push(Op::Dwell { ms: dm.time_ms, power: pw });
            }
            first = false;
            d += dm.spacing;
        }
    } else {
        let pieces = geom::split_at(&path, &breaks);
        let mut s_acc = 0.0;
        for piece in pieces {
            let len = geom::polyline_length(&piece);
            let pw = power_at(s_acc + len / 2.0);
            for p in piece.iter().skip(1) {
                out.push(Op::Cut { to: *p, speed, power: pw });
            }
            s_acc += len;
        }
    }

    let path_end = *path.last().unwrap();
    if ls.end_pause_ms > 0.0 {
        out.push(Op::Dwell { ms: ls.end_pause_ms, power: if ls.cut_through_end { ls.pause_power } else { 0.0 } });
    }
    if let Some(l) = lead_out {
        let t_end = if closed && overcut > 0.0 { geom::point_at(&path, total).1 } else { t1 };
        let (_, op) = lead_geometry(l, path_end, t_end, normal(t_end), false, speed, power);
        out.push(op);
    }
}

/// Lead-in (returns its start point and the op that ends at `at`) or lead-out (starts at `at`,
/// returns its end point and op). `n` is the unit normal towards the waste side.
fn lead_geometry(l: Lead, at: Point, t: Vec2, n: Vec2, is_in: bool, speed: f64, power: f64) -> (Point, Op) {
    match l.style {
        LeadStyle::Line => {
            let a = l.angle_deg.to_radians();
            let off = n * (l.length * a.sin());
            if is_in {
                let p = at - t * (l.length * a.cos()) + off;
                (p, Op::Cut { to: at, speed, power })
            } else {
                let p = at + t * (l.length * a.cos()) + off;
                (p, Op::Cut { to: p, speed, power })
            }
        }
        LeadStyle::Arc => {
            // Quarter arc of the given length, tangent to the path, centre on the waste side.
            let r = l.length / FRAC_PI_2;
            let c = at + n * r;
            let u = at - c;
            // Turning towards the centre: centre on the left of travel = counter-clockwise.
            let ccw = n.cross(t) < 0.0;
            if is_in {
                let p = c + geom::rotate(u, if ccw { -FRAC_PI_2 } else { FRAC_PI_2 });
                (p, Op::Arc { to: at, center: c, clockwise: !ccw, speed, power })
            } else {
                let p = c + geom::rotate(u, if ccw { FRAC_PI_2 } else { -FRAC_PI_2 });
                (p, Op::Arc { to: p, center: c, clockwise: !ccw, speed, power })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{DotMode, Perforation};

    fn square() -> Vec<Point> {
        vec![Point::new(0.0, 0.0), Point::new(10.0, 0.0), Point::new(10.0, 10.0), Point::new(0.0, 10.0), Point::new(0.0, 0.0)]
    }

    fn cut_len(ops: &[Op], powered: bool) -> f64 {
        let mut pos = Point::ZERO;
        let mut l = 0.0;
        for op in ops {
            if let Op::Cut { to, power, .. } = op
                && (*power > 0.0) == powered
            {
                l += pos.distance(*to);
            }
            if let Some(t) = op.target() {
                pos = t;
            }
        }
        l
    }

    fn ctx(s: &CutSettings) -> PathContext<'_> {
        PathContext { settings: s, power_scale: 1.0, depth: 0, tabs: &[], backlash: None }
    }

    #[test]
    fn tabs_leave_gaps() {
        let mut s = CutSettings::line(10.0, 50.0);
        s.line.tabs.enabled = true;
        s.line.tabs.spacing = 10.0;
        s.line.tabs.min_count = 4;
        s.line.tabs.size = 1.0;
        let drawn = &square()[..4];
        let auto = tab_centres(drawn, true, 0, &s.line.tabs, None);
        assert_eq!(auto.len(), 4);
        assert!(auto[0].distance(Point::new(5.0, 0.0)) < 1e-9, "half a spacing after the drawn start: {auto:?}");
        let mut ops = Vec::new();
        emit_path(&square(), true, &PathContext { tabs: &auto, ..ctx(&s) }, &mut ops);
        assert!((cut_len(&ops, false) - 4.0).abs() < 1e-9);
        assert!((cut_len(&ops, true) - 36.0).abs() < 1e-9);
        // Skip inner shapes: no automatic tabs there, but manual ones stay.
        s.line.tabs.skip_inner = true;
        assert!(tab_centres(drawn, true, 1, &s.line.tabs, None).is_empty());
        assert_eq!(tab_centres(drawn, true, 1, &s.line.tabs, Some(&[Point::new(5.0, 0.0)])).len(), 1);
        // Open paths get no automatic tabs; a shape with manual tabs gets only those.
        assert!(tab_centres(drawn, false, 0, &s.line.tabs, None).is_empty());
        assert!(tab_centres(drawn, true, 0, &s.line.tabs, Some(&[])).is_empty());
        // Manual-only layers and layers without tabs.
        s.line.tabs.auto = false;
        assert!(tab_centres(drawn, true, 0, &s.line.tabs, None).is_empty());
        s.line.tabs.enabled = false;
        assert!(tab_centres(drawn, true, 0, &s.line.tabs, Some(&[Point::new(5.0, 0.0)])).is_empty());
        let mut ops = Vec::new();
        emit_path(&square(), true, &PathContext { tabs: &auto, ..ctx(&s) }, &mut ops);
        assert!((cut_len(&ops, true) - 40.0).abs() < 1e-9, "tabs off: a solid cut");
    }

    /// Tabs hold a part in place; they must leave something of it to cut.
    #[test]
    fn tabs_never_take_the_whole_outline_of_a_small_shape() {
        let small = [Point::new(0.0, 0.0), Point::new(1.0, 0.0), Point::new(1.0, 1.0), Point::new(0.0, 1.0), Point::new(0.0, 0.0)];
        let mut s = CutSettings::line(10.0, 50.0);
        s.line.tabs.enabled = true;
        // one manual tab, longer than the 4 mm of outline
        s.line.tabs.size = 10.0;
        let manual = [Point::new(0.5, 0.0)];
        let mut ops = Vec::new();
        emit_path(&small, true, &PathContext { tabs: &manual, ..ctx(&s) }, &mut ops);
        assert!((cut_len(&ops, true) - 2.0).abs() < 1e-9, "half is cut: {}", cut_len(&ops, true));
        assert!((cut_len(&ops, false) - 2.0).abs() < 1e-9, "half is tab: {}", cut_len(&ops, false));
        // four automatic tabs of 2 mm
        s.line.tabs.size = 2.0;
        s.line.tabs.min_count = 4;
        let auto = tab_centres(&small[..4], true, 0, &s.line.tabs, None);
        assert_eq!(auto.len(), 4);
        let mut ops = Vec::new();
        emit_path(&small, true, &PathContext { tabs: &auto, ..ctx(&s) }, &mut ops);
        assert!((cut_len(&ops, true) - 2.0).abs() < 1e-9, "{}", cut_len(&ops, true));
        // an open line with a tab longer than itself
        s.line.tabs.size = 10.0;
        let mut ops = Vec::new();
        emit_path(&small[..2], false, &PathContext { tabs: &manual, ..ctx(&s) }, &mut ops);
        assert!((cut_len(&ops, true) - 0.5).abs() < 1e-9, "{}", cut_len(&ops, true));
    }

    /// The laser-off stretches of a cut as (from, to).
    fn gaps(ops: &[Op]) -> Vec<(Point, Point)> {
        let mut pos = Point::ZERO;
        let mut out: Vec<(Point, Point)> = Vec::new();
        for op in ops {
            if let Op::Cut { to, power, .. } = op
                && *power == 0.0
            {
                match out.last_mut() {
                    Some(g) if g.1.distance(pos) < 1e-9 => g.1 = *to,
                    _ => out.push((pos, *to)),
                }
            }
            if let Some(t) = op.target() {
                pos = t;
            }
        }
        out
    }

    fn manual(size: f64) -> CutSettings {
        let mut s = CutSettings::line(10.0, 50.0);
        s.line.tabs.enabled = true;
        s.line.tabs.auto = false;
        s.line.tabs.size = size;
        s
    }

    #[test]
    fn a_manual_tab_is_a_gap_centred_on_its_point() {
        let s = manual(2.0);
        let tabs = [Point::new(10.0, 4.0)];
        let mut ops = Vec::new();
        emit_path(&square(), true, &PathContext { tabs: &tabs, ..ctx(&s) }, &mut ops);
        let g = gaps(&ops);
        assert_eq!(g.len(), 1, "{g:?}");
        assert!(g[0].0.distance(Point::new(10.0, 3.0)) < 1e-9 && g[0].1.distance(Point::new(10.0, 5.0)) < 1e-9, "{g:?}");
        assert!((cut_len(&ops, true) - 38.0).abs() < 1e-9);
        // The same tab when the cut runs the other way round from another start.
        let rev = vec![Point::new(10.0, 10.0), Point::new(10.0, 0.0), Point::new(0.0, 0.0), Point::new(0.0, 10.0), Point::new(10.0, 10.0)];
        let mut ops = Vec::new();
        emit_path(&rev, true, &PathContext { tabs: &tabs, ..ctx(&s) }, &mut ops);
        let g = gaps(&ops);
        assert_eq!(g.len(), 1, "{g:?}");
        assert!(g[0].0.distance(Point::new(10.0, 5.0)) < 1e-9 && g[0].1.distance(Point::new(10.0, 3.0)) < 1e-9, "{g:?}");
    }

    #[test]
    fn a_tab_at_the_start_point_wraps_around_it() {
        let s = manual(2.0);
        for tab in [Point::new(0.0, 0.0), Point::new(0.0, 0.5), Point::new(0.5, 0.0)] {
            let mut ops = Vec::new();
            emit_path(&square(), true, &PathContext { tabs: &[tab], ..ctx(&s) }, &mut ops);
            assert!((cut_len(&ops, false) - 2.0).abs() < 1e-9, "the whole tab is left at {tab:?}: {}", cut_len(&ops, false));
            assert!((cut_len(&ops, true) - 38.0).abs() < 1e-9);
        }
    }

    #[test]
    fn manual_tabs_work_on_open_paths() {
        let s = manual(2.0);
        let line = [Point::new(0.0, 0.0), Point::new(20.0, 0.0), Point::new(20.0, 10.0)];
        let mut ops = Vec::new();
        emit_path(&line, false, &PathContext { tabs: &[Point::new(8.0, 0.3), Point::new(20.0, 10.0)], ..ctx(&s) }, &mut ops);
        let g = gaps(&ops);
        assert_eq!(g.len(), 2, "{g:?}");
        assert!(g[0].0.distance(Point::new(7.0, 0.0)) < 1e-9 && g[0].1.distance(Point::new(9.0, 0.0)) < 1e-9, "{g:?}");
        assert!(g[1].0.distance(Point::new(20.0, 9.0)) < 1e-9 && g[1].1.distance(Point::new(20.0, 10.0)) < 1e-9, "an end tab is cut short: {g:?}");
    }

    #[test]
    fn partly_cut_tabs_use_tab_power_or_pulses() {
        let mut s = manual(2.0);
        s.line.tabs.tab_power = Some(10.0);
        let tabs = [Point::new(5.0, 0.0)];
        let mut ops = Vec::new();
        emit_path(&square(), true, &PathContext { tabs: &tabs, ..ctx(&s) }, &mut ops);
        let low: Vec<&Op> = ops.iter().filter(|o| matches!(o, Op::Cut { power, .. } if *power == 10.0)).collect();
        assert_eq!(low.len(), 1, "one reduced-power piece");
        assert_eq!(cut_len(&ops, false), 0.0);
        // With a pulse width the tab is full-power pulses: 20 % of 50 % power = 1 on, 4 off.
        s.line.tabs.pulse_width = 0.1;
        let mut ops = Vec::new();
        emit_path(&square(), true, &PathContext { tabs: &tabs, ..ctx(&s) }, &mut ops);
        assert!(ops.iter().all(|o| !matches!(o, Op::Cut { power, .. } if *power == 10.0)), "no reduced-power moves");
        let g = gaps(&ops);
        assert_eq!(g.len(), 4, "2 mm of 0.5 mm periods: {g:?}");
        for (a, b) in &g {
            assert!((a.distance(*b) - 0.4).abs() < 1e-9, "{g:?}");
        }
        assert!((g[0].0.x - 4.0).abs() < 1e-9 && (g[1].0.x - 4.5).abs() < 1e-9, "{g:?}");
        assert!((cut_len(&ops, true) - (38.0 + 0.4)).abs() < 1e-9, "the pulses burn a fifth of the tab");
        // A tab shorter than one period still gets one pulse.
        s.line.tabs.size = 0.2;
        let mut ops = Vec::new();
        emit_path(&square(), true, &PathContext { tabs: &tabs, ..ctx(&s) }, &mut ops);
        assert_eq!(gaps(&ops).len(), 1);
        assert!((cut_len(&ops, false) - 0.16).abs() < 1e-9);
    }

    #[test]
    fn tabs_show_again_in_the_overcut_and_keep_the_lead_in() {
        let mut s = manual(1.0);
        s.line.overcut = 5.0;
        s.line.lead_in = Some(Lead { length: 2.0, angle_deg: 90.0, style: LeadStyle::Line });
        let mut ops = Vec::new();
        emit_path(&square(), true, &PathContext { tabs: &[Point::new(3.0, 0.0)], ..ctx(&s) }, &mut ops);
        let g = gaps(&ops);
        assert_eq!(g.len(), 2, "once on the way round and once in the overcut: {g:?}");
        assert!(g.iter().all(|(a, b)| (a.x - 2.5).abs() < 1e-9 && (b.x - 3.5).abs() < 1e-9 && a.y == 0.0), "{g:?}");
        let Op::Rapid { to } = ops[0] else { panic!() };
        assert!(to.distance(Point::new(0.0, -2.0)) < 1e-9, "the lead-in is still there: {to:?}");
    }

    /// Perforation always leaves at least one bridge, or a small part would be cut free.
    #[test]
    fn a_shape_shorter_than_the_cut_length_still_gets_one_perforation() {
        let small = [Point::new(0.0, 0.0), Point::new(1.0, 0.0), Point::new(1.0, 1.0), Point::new(0.0, 1.0), Point::new(0.0, 0.0)];
        let mut s = CutSettings::line(10.0, 50.0);
        // 4 mm of outline, 5 mm cuts with 1 mm skips
        s.line.perforation = Some(Perforation { cut: 5.0, skip: 1.0 });
        let mut ops = Vec::new();
        emit_path(&small, true, &ctx(&s), &mut ops);
        assert!((cut_len(&ops, false) - 1.0).abs() < 1e-9, "one skip: {}", cut_len(&ops, false));
        assert!((cut_len(&ops, true) - 3.0).abs() < 1e-9, "{}", cut_len(&ops, true));
        // a skip longer than the shape takes half of it at most
        s.line.perforation = Some(Perforation { cut: 5.0, skip: 10.0 });
        let mut ops = Vec::new();
        emit_path(&small, true, &ctx(&s), &mut ops);
        assert!((cut_len(&ops, false) - 2.0).abs() < 1e-9, "{}", cut_len(&ops, false));
        assert!((cut_len(&ops, true) - 2.0).abs() < 1e-9, "{}", cut_len(&ops, true));
        // a shape a hair longer than the cut length gets a whole skip, not a hair of one
        s.line.perforation = Some(Perforation { cut: 3.99, skip: 1.0 });
        let mut ops = Vec::new();
        emit_path(&small, true, &ctx(&s), &mut ops);
        assert!((cut_len(&ops, false) - 1.0).abs() < 1e-9, "{}", cut_len(&ops, false));
    }

    #[test]
    fn perforation_and_overcut() {
        let mut s = CutSettings::line(10.0, 50.0);
        s.line.perforation = Some(Perforation { cut: 3.0, skip: 1.0 });
        let mut ops = Vec::new();
        emit_path(&square(), true, &ctx(&s), &mut ops);
        assert!((cut_len(&ops, true) - 30.0).abs() < 1e-9);
        let mut s = CutSettings::line(10.0, 50.0);
        s.line.overcut = 2.5;
        let mut ops = Vec::new();
        emit_path(&square(), true, &ctx(&s), &mut ops);
        assert!((cut_len(&ops, true) - 42.5).abs() < 1e-9);
        assert_eq!(ops.last().unwrap().target(), Some(Point::new(2.5, 0.0)));
    }

    #[test]
    fn line_lead_in_on_waste_side() {
        let mut s = CutSettings::line(10.0, 50.0);
        s.line.lead_in = Some(Lead { length: 2.0, angle_deg: 90.0, style: LeadStyle::Line });
        let mut ops = Vec::new();
        emit_path(&square(), true, &ctx(&s), &mut ops);
        // CCW outer square starting at origin heading +X: waste is below (y < 0).
        let Op::Rapid { to } = ops[0] else { panic!() };
        assert!((to.x).abs() < 1e-9 && (to.y + 2.0).abs() < 1e-9, "{to:?}");
        // Hole: lead-in goes inside.
        let mut ops = Vec::new();
        emit_path(&square(), true, &PathContext { depth: 1, ..ctx(&s) }, &mut ops);
        let Op::Rapid { to } = ops[0] else { panic!() };
        assert!((to.y - 2.0).abs() < 1e-9, "{to:?}");
    }

    /// Leads go on the waste side of a closed shape. An open line has no waste side: it is cut
    /// as drawn, from its first point to its last, with nothing burned beside it.
    #[test]
    fn an_open_path_gets_no_lead_in_or_lead_out() {
        for style in [LeadStyle::Line, LeadStyle::Arc] {
            let mut s = CutSettings::line(10.0, 50.0);
            s.line.lead_in = Some(Lead { length: 2.0, angle_deg: 90.0, style });
            s.line.lead_out = Some(Lead { length: 2.0, angle_deg: 90.0, style });
            let line = [Point::new(0.0, 0.0), Point::new(20.0, 0.0)];
            let mut ops = Vec::new();
            emit_path(&line, false, &ctx(&s), &mut ops);
            assert_eq!(
                ops,
                vec![Op::Rapid { to: Point::new(0.0, 0.0) }, Op::Cut { to: Point::new(20.0, 0.0), speed: 10.0, power: 50.0 }],
                "{style:?}"
            );
        }
    }

    #[test]
    fn arc_lead_in_is_tangent() {
        let mut s = CutSettings::line(10.0, 50.0);
        s.line.lead_in = Some(Lead { length: FRAC_PI_2, angle_deg: 0.0, style: LeadStyle::Arc });
        s.line.lead_out = Some(Lead { length: FRAC_PI_2, angle_deg: 0.0, style: LeadStyle::Arc });
        let mut ops = Vec::new();
        emit_path(&square(), true, &ctx(&s), &mut ops);
        // r = 1, centre (0, -1), start (-1, -1), arc counter-clockwise? Travel +X with centre to the
        // right (below) means a clockwise turn.
        let Op::Rapid { to } = ops[0] else { panic!() };
        assert!(to.distance(Point::new(-1.0, -1.0)) < 1e-9, "{to:?}");
        let Op::Arc { center, clockwise, to, .. } = ops[1] else { panic!() };
        assert!(center.distance(Point::new(0.0, -1.0)) < 1e-9);
        assert!(clockwise);
        assert_eq!(to, Point::ZERO);
        // Lead-out: end at origin heading -Y, waste on the right of travel (-X side).
        let Op::Arc { to, center, .. } = ops.last().unwrap() else { panic!() };
        assert!(center.distance(Point::new(-1.0, 0.0)) < 1e-9, "{center:?}");
        assert!(to.distance(Point::new(-1.0, -1.0)) < 1e-9, "{to:?}");
    }

    #[test]
    fn dot_mode_and_pauses() {
        let mut s = CutSettings::line(10.0, 50.0);
        s.line.dot_mode = Some(DotMode { time_ms: 5.0, spacing: 2.0 });
        let mut ops = Vec::new();
        emit_path(&square(), true, &ctx(&s), &mut ops);
        let dots = ops.iter().filter(|o| matches!(o, Op::Dwell { .. })).count();
        assert_eq!(dots, 20);
        // Overcut does not fire dots a second time, on the first ones or between them.
        for spacing in [2.0, 3.0] {
            s.line.dot_mode = Some(DotMode { time_ms: 5.0, spacing });
            s.line.overcut = 0.0;
            let mut plain = Vec::new();
            emit_path(&square(), true, &ctx(&s), &mut plain);
            s.line.overcut = 10.0;
            let mut with = Vec::new();
            emit_path(&square(), true, &ctx(&s), &mut with);
            assert_eq!(with, plain, "spacing {spacing}");
        }
        let mut s = CutSettings::line(10.0, 50.0);
        s.line.start_pause_ms = 100.0;
        s.line.cut_through_start = true;
        s.line.pause_power = 80.0;
        s.line.end_pause_ms = 50.0;
        let mut ops = Vec::new();
        emit_path(&square(), true, &ctx(&s), &mut ops);
        assert_eq!(ops[1], Op::Dwell { ms: 100.0, power: 80.0 });
        assert_eq!(*ops.last().unwrap(), Op::Dwell { ms: 50.0, power: 0.0 });
    }
}
