//! Line mode: turns one planned path into operations (lead-in/out, pauses, overcut, tabs,
//! perforation, dot mode).

use std::f64::consts::FRAC_PI_2;

use kurbo::{Point, Vec2};

use crate::geom::{self, EPS};
use crate::input::{CutSettings, Lead, LeadStyle};
use crate::ir::Op;

/// Per-path context for [`emit_path`].
#[derive(Clone, Copy, Debug)]
pub struct PathContext<'a> {
    pub settings: &'a CutSettings,
    /// Shape power scale 0..=1.
    pub power_scale: f64,
    /// How many closed shapes this path lies inside (even = outer, odd = hole).
    pub depth: usize,
    /// Manually placed tabs.
    pub manual_tabs: &'a [Point],
    /// Backlash approach distance (hide backlash), if on.
    pub backlash: Option<f64>,
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
    if closed && ls.overcut > 0.0 {
        let oc = ls.overcut.min(perimeter);
        let pieces = geom::split_at(pts, &[oc]);
        path.extend(pieces[0].iter().skip(1));
    }
    let total = geom::polyline_length(&path);

    // Power profile: sorted breakpoints with the power of the piece after each one.
    let mut intervals: Vec<(f64, f64, f64)> = Vec::new(); // (from, to, power)
    let tabs = &ls.tabs;
    if closed && tabs.enabled && !(tabs.skip_inner && ctx.depth > 0) && perimeter > EPS {
        let tab_power = tabs.tab_power.unwrap_or(0.0).min(power);
        let mut centers: Vec<f64> = if ctx.manual_tabs.is_empty() && !tabs.auto {
            Vec::new()
        } else if ctx.manual_tabs.is_empty() {
            let count = if tabs.spacing > 0.0 { (perimeter / tabs.spacing).round() as u32 } else { tabs.min_count };
            let count = count.clamp(tabs.min_count, tabs.max_count.max(tabs.min_count));
            (0..count).map(|k| (k as f64 + 0.5) * perimeter / count as f64).collect()
        } else {
            ctx.manual_tabs.iter().map(|p| geom::nearest_arclen(pts, *p).0).collect()
        };
        centers.sort_by(|a, b| a.partial_cmp(b).unwrap());
        for c in centers {
            for base in [0.0, perimeter] {
                let a = (base + c - tabs.size / 2.0).max(0.0);
                let b = (base + c + tabs.size / 2.0).min(total);
                if b > a {
                    intervals.push((a, b, tab_power));
                }
            }
        }
    }
    if let Some(pf) = ls.perforation
        && pf.cut > 0.0
        && pf.skip > 0.0
        && total > pf.cut
    {
        let period = pf.cut + pf.skip;
        let mut a = pf.cut;
        while a < total {
            intervals.push((a, (a + pf.skip).min(total), 0.0));
            a += period;
        }
    }
    let power_at = |s_mid: f64| -> f64 { intervals.iter().filter(|(a, b, _)| s_mid > *a && s_mid < *b).fold(power, |p, (_, _, q)| p.min(*q)) };
    let mut breaks: Vec<f64> = intervals.iter().flat_map(|(a, b, _)| [*a, *b]).filter(|v| *v > EPS && *v < total - EPS).collect();
    breaks.sort_by(|a, b| a.partial_cmp(b).unwrap());
    breaks.dedup_by(|a, b| (*a - *b).abs() < EPS);

    // Lead-in start.
    let lead_in = ls.lead_in.filter(|l| l.length > EPS);
    let lead_out = ls.lead_out.filter(|l| l.length > EPS);
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
        let t_end = if closed && ls.overcut > 0.0 { geom::point_at(&path, total).1 } else { t1 };
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
        PathContext { settings: s, power_scale: 1.0, depth: 0, manual_tabs: &[], backlash: None }
    }

    #[test]
    fn tabs_leave_gaps() {
        let mut s = CutSettings::line(10.0, 50.0);
        s.line.tabs.enabled = true;
        s.line.tabs.spacing = 10.0;
        s.line.tabs.min_count = 4;
        s.line.tabs.size = 1.0;
        let mut ops = Vec::new();
        emit_path(&square(), true, &ctx(&s), &mut ops);
        assert!((cut_len(&ops, false) - 4.0).abs() < 1e-9);
        assert!((cut_len(&ops, true) - 36.0).abs() < 1e-9);
        // Skip inner shapes.
        s.line.tabs.skip_inner = true;
        let mut ops = Vec::new();
        emit_path(&square(), true, &PathContext { depth: 1, ..ctx(&s) }, &mut ops);
        assert!((cut_len(&ops, true) - 40.0).abs() < 1e-9);
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
