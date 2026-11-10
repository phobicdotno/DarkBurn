//! Scanline fill, offset fill and raster row emission.

use kurbo::{Affine, Point, Rect};

use crate::geom::{self, EPS};
use crate::input::{FillGrouping, FillRule};
use crate::ir::Op;

/// A run of constant power on a scan row, in the scan frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScanRun {
    pub x0: f64,
    pub x1: f64,
    /// Percent.
    pub power: f64,
    /// Ramp power up at `x0` / down at `x1` (when ramps are on).
    pub ramp_start: bool,
    pub ramp_end: bool,
}

/// One scan row: runs sorted by `x0`, not overlapping.
#[derive(Clone, Debug, PartialEq)]
pub struct ScanRow {
    pub y: f64,
    pub runs: Vec<ScanRun>,
}

/// Scan closed polygons with horizontal lines in a frame rotated by `angle` (radians).
/// Rows lie on the global grid `y = (k + 0.5) * interval` of the rotated frame, so separate
/// scans line up. Returns rows in the scan frame; map back with `Affine::rotate(angle)`.
pub fn scan_polygons(polys: &[&[Point]], rule: FillRule, interval: f64, angle: f64, power: f64) -> Vec<ScanRow> {
    scan_polygons_powers(polys, &vec![power; polys.len()], rule, interval, angle)
}

/// [`scan_polygons`] with a power for each polygon: a filled stretch burns at the power of the
/// smallest polygon it lies in (the shape itself, not the shapes around it).
pub fn scan_polygons_powers(polys: &[&[Point]], powers: &[f64], rule: FillRule, interval: f64, angle: f64) -> Vec<ScanRow> {
    let interval = interval.max(1e-3);
    let areas: Vec<f64> = polys.iter().map(|p| geom::signed_area(p).abs()).collect();
    let mut edges: Vec<(Point, Point, i32, usize)> = Vec::new();
    for (pi, poly) in polys.iter().enumerate() {
        let n = poly.len();
        for i in 0..n {
            let a = geom::rotate_point(poly[i], -angle);
            let b = geom::rotate_point(poly[(i + 1) % n], -angle);
            if (a.y - b.y).abs() < 1e-12 {
                continue;
            }
            if a.y < b.y { edges.push((a, b, 1, pi)) } else { edges.push((b, a, -1, pi)) }
        }
    }
    if edges.is_empty() {
        return Vec::new();
    }
    edges.sort_by(|a, b| a.0.y.total_cmp(&b.0.y));
    let ymin = edges[0].0.y;
    let ymax = edges.iter().map(|e| e.1.y).fold(f64::MIN, f64::max);
    let k0 = (ymin / interval - 0.5).ceil() as i64;
    let k1 = (ymax / interval - 0.5).floor() as i64;
    let mut rows = Vec::new();
    let mut active: Vec<usize> = Vec::new();
    let mut next = 0;
    let mut xs: Vec<(f64, i32, usize)> = Vec::new();
    // With one power for all there is no need to know which polygon a stretch lies in.
    let uniform = powers.windows(2).all(|w| w[0] == w[1]);
    // Winding of each polygon at the current x, and the polygons the current x lies in.
    let mut winds: Vec<i32> = if uniform { Vec::new() } else { vec![0; polys.len()] };
    let mut open: Vec<usize> = Vec::new();
    for k in k0..=k1 {
        let y = (k as f64 + 0.5) * interval;
        while next < edges.len() && edges[next].0.y <= y {
            active.push(next);
            next += 1;
        }
        active.retain(|&e| edges[e].1.y > y);
        xs.clear();
        for &e in &active {
            let (a, b, w, pi) = edges[e];
            if y >= a.y && y < b.y {
                xs.push((a.x + (y - a.y) * (b.x - a.x) / (b.y - a.y), w, pi));
            }
        }
        xs.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut runs: Vec<ScanRun> = Vec::new();
        let mut wind = 0;
        let mut inside = false;
        let mut x_start = 0.0;
        for p in open.drain(..) {
            winds[p] = 0;
        }
        for &(x, w, pi) in &xs {
            if inside && x - x_start > EPS {
                // The stretch up to this crossing lies in the `open` polygons.
                let power =
                    if uniform { powers[0] } else { open.iter().copied().min_by(|a, b| areas[*a].total_cmp(&areas[*b])).map_or(0.0, |p| powers[p]) };
                match runs.last_mut() {
                    // Merge runs that touch.
                    Some(m) if x_start - m.x1 < EPS && m.power == power => m.x1 = x,
                    _ => runs.push(ScanRun { x0: x_start, x1: x, power, ramp_start: true, ramp_end: true }),
                }
            }
            wind += w;
            if !uniform {
                let was = winds[pi];
                winds[pi] += w;
                if was == 0 {
                    open.push(pi);
                } else if winds[pi] == 0 {
                    open.retain(|p| *p != pi);
                }
            }
            inside = match rule {
                FillRule::EvenOdd => !inside,
                FillRule::NonZero => wind != 0,
            };
            x_start = x;
        }
        if !runs.is_empty() {
            rows.push(ScanRow { y, runs });
        }
    }
    rows
}

/// Mark which run ends may ramp: when `outer_edge` is false the first start and last end of
/// every row (the outer edge of the scan) are not ramped.
pub fn mark_ramps(rows: &mut [ScanRow], outer_edge: bool) {
    for row in rows {
        let n = row.runs.len();
        for (i, r) in row.runs.iter_mut().enumerate() {
            r.ramp_start = outer_edge || i > 0;
            r.ramp_end = outer_edge || i + 1 < n;
        }
    }
}

/// Order rows for emission: `(row, forward)` pairs. Flood fill splits rows into single runs and
/// follows connected runs upwards before moving on.
pub fn order_rows(rows: Vec<ScanRow>, bidirectional: bool, flood: bool, interval: f64) -> Vec<(ScanRow, bool)> {
    let mut out = Vec::new();
    if !flood {
        for (i, r) in rows.into_iter().enumerate() {
            out.push((r, !bidirectional || i.is_multiple_of(2)));
        }
        return out;
    }
    let mut pieces: Vec<Vec<Option<ScanRun>>> = rows.iter().map(|r| r.runs.iter().copied().map(Some).collect()).collect();
    let ys: Vec<f64> = rows.iter().map(|r| r.y).collect();
    let mut count = 0usize;
    while let Some((mut ri, mut ki)) = pieces.iter().enumerate().find_map(|(ri, row)| row.iter().position(|p| p.is_some()).map(|ki| (ri, ki))) {
        loop {
            let run = pieces[ri][ki].take().unwrap();
            out.push((ScanRow { y: ys[ri], runs: vec![run] }, !bidirectional || count.is_multiple_of(2)));
            count += 1;
            // Next row must be the adjacent scan line and overlap this run.
            let Some(nri) = (ri + 1 < ys.len()).then_some(ri + 1) else { break };
            if ys[nri] - ys[ri] > interval * 1.5 {
                break;
            }
            let Some(nki) = pieces[nri].iter().position(|p| p.is_some_and(|q| q.x0 < run.x1 && q.x1 > run.x0)) else {
                break;
            };
            ri = nri;
            ki = nki;
        }
    }
    out
}

/// Parameters for [`emit_rows`].
#[derive(Clone, Copy, Debug)]
pub struct RowEmit {
    pub frame: Affine,
    pub speed: f64,
    pub overscan: f64,
    pub rapid_gap: f64,
    /// `(ramp length, min power)`; length 0 = no ramps.
    pub ramp: (f64, f64),
}

/// Emit ordered rows as operations.
pub fn emit_rows(rows: &[(ScanRow, bool)], e: &RowEmit, out: &mut Vec<Op>) {
    let f = |x: f64, y: f64| e.frame * Point::new(x, y);
    let os = e.overscan;
    for (row, forward) in rows {
        if row.runs.is_empty() {
            continue;
        }
        let dir = if *forward { 1.0 } else { -1.0 };
        let runs: Vec<ScanRun> = if *forward {
            row.runs.clone()
        } else {
            row.runs.iter().rev().map(|r| ScanRun { x0: r.x1, x1: r.x0, power: r.power, ramp_start: r.ramp_end, ramp_end: r.ramp_start }).collect()
        };
        let y = row.y;
        let first = runs[0].x0;
        if os > 0.0 {
            out.push(Op::Rapid { to: f(first - dir * os, y) });
            out.push(Op::Overscan { to: f(first, y), speed: e.speed });
        } else {
            out.push(Op::Rapid { to: f(first, y) });
        }
        for (i, r) in runs.iter().enumerate() {
            emit_run(r, dir, y, e, &f, out);
            if let Some(n) = runs.get(i + 1) {
                let gap = (n.x0 - r.x1).abs();
                if gap < EPS {
                    continue;
                }
                if gap > e.rapid_gap {
                    if os > 0.0 {
                        out.push(Op::Overscan { to: f(r.x1 + dir * os, y), speed: e.speed });
                        out.push(Op::Rapid { to: f(n.x0 - dir * os, y) });
                        out.push(Op::Overscan { to: f(n.x0, y), speed: e.speed });
                    } else {
                        out.push(Op::Rapid { to: f(n.x0, y) });
                    }
                } else {
                    out.push(Op::Cut { to: f(n.x0, y), speed: e.speed, power: 0.0 });
                }
            }
        }
        if os > 0.0 {
            let last = runs.last().unwrap().x1;
            out.push(Op::Overscan { to: f(last + dir * os, y), speed: e.speed });
        }
    }
}

fn emit_run(r: &ScanRun, dir: f64, y: f64, e: &RowEmit, f: &dyn Fn(f64, f64) -> Point, out: &mut Vec<Op>) {
    let len = (r.x1 - r.x0).abs();
    let (ramp_len, min_power) = e.ramp;
    let min_power = min_power.min(r.power);
    let ramp_in = ramp_len > 0.0 && r.ramp_start && r.power > 0.0;
    let ramp_out = ramp_len > 0.0 && r.ramp_end && r.power > 0.0;
    let ends = ramp_in as u32 + ramp_out as u32;
    if ends == 0 {
        out.push(Op::Cut { to: f(r.x1, y), speed: e.speed, power: r.power });
        return;
    }
    let rl = ramp_len.min(len / ends as f64);
    let steps = ((rl / 0.25).ceil() as usize).clamp(1, 10);
    let lerp = |t: f64| min_power + (r.power - min_power) * t;
    if ramp_in {
        for k in 0..steps {
            let x = r.x0 + dir * rl * (k + 1) as f64 / steps as f64;
            out.push(Op::Cut { to: f(x, y), speed: e.speed, power: lerp((k as f64 + 0.5) / steps as f64) });
        }
    }
    let body_end = if ramp_out { r.x1 - dir * rl } else { r.x1 };
    if (body_end - r.x0) * dir > (if ramp_in { rl } else { 0.0 }) + EPS {
        out.push(Op::Cut { to: f(body_end, y), speed: e.speed, power: r.power });
    }
    if ramp_out {
        for k in 0..steps {
            let x = body_end + dir * rl * (k + 1) as f64 / steps as f64;
            out.push(Op::Cut { to: f(x, y), speed: e.speed, power: lerp(1.0 - (k as f64 + 0.5) / steps as f64) });
        }
    }
}

/// Group closed polygons into fill clusters. `shape_of[i]` is the shape of polygon `i`,
/// `group_of_shape[s]` the shape's group.
pub fn fill_clusters(polys: &[Vec<Point>], shape_of: &[usize], group_of_shape: &[Option<u32>], grouping: FillGrouping) -> Vec<Vec<usize>> {
    if polys.is_empty() {
        return Vec::new();
    }
    if grouping == FillGrouping::AllAtOnce {
        return vec![(0..polys.len()).collect()];
    }
    let parent = parents(polys);
    let root = |mut i: usize| {
        while let Some(p) = parent[i] {
            i = p;
        }
        i
    };
    let mut keys: Vec<(i64, Vec<usize>)> = Vec::new();
    for i in 0..polys.len() {
        let r = root(i);
        let key = match (grouping, group_of_shape[shape_of[r]]) {
            (FillGrouping::GroupsTogether, Some(g)) => -(g as i64) - 1,
            _ => r as i64,
        };
        match keys.iter_mut().find(|(k, _)| *k == key) {
            Some((_, v)) => v.push(i),
            None => keys.push((key, vec![i])),
        }
    }
    keys.into_iter().map(|(_, v)| v).collect()
}

/// Smallest polygon containing each polygon, if any.
pub fn parents(polys: &[Vec<Point>]) -> Vec<Option<usize>> {
    let boxes: Vec<Rect> = polys.iter().map(|p| geom::bbox(p)).collect();
    let areas: Vec<f64> = polys.iter().map(|p| geom::signed_area(p).abs()).collect();
    (0..polys.len())
        .map(|i| {
            let mut best: Option<usize> = None;
            for j in 0..polys.len() {
                if i == j || areas[j] <= areas[i] || !geom::rect_inside(boxes[i], boxes[j]) {
                    continue;
                }
                let p = &polys[i];
                if geom::point_in_polygon(p[0], &polys[j])
                    && geom::point_in_polygon(p[p.len() / 2], &polys[j])
                    && best.is_none_or(|b| areas[j] < areas[b])
                {
                    best = Some(j);
                }
            }
            best
        })
        .collect()
}

/// The rings of an offset fill of the area inside `polys` (even-odd, so a shape inside another
/// is a hole), level by level: level `k` holds the closed rings `interval / 2 + k * interval`
/// inside the edge. A shape with arms, a neck or holes has as many rings on a level as it has
/// parts left at that depth; rings run around holes too.
pub fn offset_levels(polys: &[&[Point]], interval: f64) -> Vec<Vec<Vec<Point>>> {
    use darkburn_geom::offset::{Join, OffsetOptions, OffsetSide, offset};
    let interval = interval.max(1e-3);
    let mut path = kurbo::BezPath::new();
    for poly in polys.iter().filter(|p| p.len() >= 3) {
        path.move_to(poly[0]);
        poly[1..].iter().for_each(|p| path.line_to(*p));
        path.close_path();
    }
    let tol = (interval / 10.0).clamp(0.002, 0.02);
    let mut levels = Vec::new();
    let mut count = 0usize;
    let mut d = interval / 2.0;
    while count < 100_000 {
        let opts = OffsetOptions {
            distance: d,
            side: OffsetSide::Inward,
            join: Join::Round,
            fill_rule: darkburn_geom::FillRule::EvenOdd,
            tol,
            ..Default::default()
        };
        let rings: Vec<Vec<Point>> = offset(std::slice::from_ref(&path), &opts)
            .inward
            .iter()
            .flat_map(|p| geom::flatten(p, tol))
            .filter(|p| p.closed && p.pts.len() >= 3 && geom::signed_area(&p.pts).abs() > 1e-9)
            .map(|p| p.pts)
            .collect();
        if rings.is_empty() {
            break;
        }
        count += rings.len();
        levels.push(rings);
        d += interval;
    }
    levels
}

/// The rings of one shape for offset fill, outermost first (see [`offset_levels`]).
pub fn offset_rings(poly: &[Point], interval: f64) -> Vec<Vec<Point>> {
    offset_levels(&[poly], interval).into_iter().flatten().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(x: f64, y: f64, s: f64) -> Vec<Point> {
        vec![Point::new(x, y), Point::new(x + s, y), Point::new(x + s, y + s), Point::new(x, y + s)]
    }

    #[test]
    fn square_fill_rows() {
        let sq = square(0.0, 0.0, 10.0);
        let rows = scan_polygons(&[&sq], FillRule::EvenOdd, 1.0, 0.0, 50.0);
        assert_eq!(rows.len(), 10);
        assert_eq!(rows[0].y, 0.5);
        assert_eq!(rows[0].runs, vec![ScanRun { x0: 0.0, x1: 10.0, power: 50.0, ramp_start: true, ramp_end: true }]);
    }

    #[test]
    fn hole_even_odd_vs_nonzero() {
        let outer = square(0.0, 0.0, 10.0);
        let inner_same_dir = square(3.0, 3.0, 4.0);
        let eo = scan_polygons(&[&outer, &inner_same_dir], FillRule::EvenOdd, 1.0, 0.0, 50.0);
        assert_eq!(eo[5].runs.len(), 2);
        let nz = scan_polygons(&[&outer, &inner_same_dir], FillRule::NonZero, 1.0, 0.0, 50.0);
        assert_eq!(nz[5].runs.len(), 1);
        let inner_rev: Vec<Point> = inner_same_dir.iter().rev().copied().collect();
        let nz = scan_polygons(&[&outer, &inner_rev], FillRule::NonZero, 1.0, 0.0, 50.0);
        assert_eq!(nz[5].runs.len(), 2);
    }

    #[test]
    fn angled_scan_covers_area() {
        let sq = square(0.0, 0.0, 10.0);
        for deg in [0.0, 30.0, 45.0, 90.0, 137.0] {
            let rows = scan_polygons(&[&sq], FillRule::EvenOdd, 0.1, f64::to_radians(deg), 50.0);
            let area: f64 = rows.iter().flat_map(|r| &r.runs).map(|r| (r.x1 - r.x0) * 0.1).sum();
            assert!((area - 100.0).abs() < 1.0, "{deg}: {area}");
        }
    }

    #[test]
    fn flood_fill_follows_columns() {
        // Two separate columns: flood fill finishes the left one before the right one.
        let a = square(0.0, 0.0, 2.0);
        let b = square(5.0, 0.0, 2.0);
        let rows = scan_polygons(&[&a, &b], FillRule::EvenOdd, 1.0, 0.0, 50.0);
        let ordered = order_rows(rows, true, true, 1.0);
        let xs: Vec<f64> = ordered.iter().map(|(r, _)| r.runs[0].x0).collect();
        assert_eq!(xs, vec![0.0, 0.0, 5.0, 5.0]);
        assert_eq!(ordered.iter().map(|o| o.1).collect::<Vec<_>>(), vec![true, false, true, false]);
    }

    #[test]
    fn clusters_by_containment() {
        let polys = vec![square(0.0, 0.0, 10.0), square(2.0, 2.0, 2.0), square(20.0, 0.0, 5.0)];
        let c = fill_clusters(&polys, &[0, 1, 2], &[None, None, None], FillGrouping::Individually);
        assert_eq!(c, vec![vec![0, 1], vec![2]]);
        let c = fill_clusters(&polys, &[0, 1, 2], &[Some(1), None, Some(1)], FillGrouping::GroupsTogether);
        assert_eq!(c, vec![vec![0, 1, 2]]);
    }

    #[test]
    fn rings_shrink() {
        let rings = offset_rings(&square(0.0, 0.0, 10.0), 1.0);
        assert_eq!(rings.len(), 5);
        assert!(rings[0].iter().any(|p| p.distance(Point::new(0.5, 0.5)) < 1e-9));
        // L shape (concave).
        let l = vec![
            Point::new(0.0, 0.0),
            Point::new(10.0, 0.0),
            Point::new(10.0, 4.0),
            Point::new(4.0, 4.0),
            Point::new(4.0, 10.0),
            Point::new(0.0, 10.0),
        ];
        let rings = offset_rings(&l, 1.0);
        assert!(rings.len() >= 2);
        for r in &rings {
            for p in r {
                assert!(geom::point_in_polygon(*p, &l));
            }
        }
    }

    #[test]
    fn rings_split_where_a_neck_closes_and_run_around_holes() {
        let p = |x: f64, y: f64| Point::new(x, y);
        // Two 10 mm squares joined by a neck 2 mm wide.
        let dumbbell = vec![
            p(0.0, 0.0),
            p(10.0, 0.0),
            p(10.0, 4.0),
            p(20.0, 4.0),
            p(20.0, 0.0),
            p(30.0, 0.0),
            p(30.0, 10.0),
            p(20.0, 10.0),
            p(20.0, 6.0),
            p(10.0, 6.0),
            p(10.0, 10.0),
            p(0.0, 10.0),
        ];
        let levels = offset_levels(&[&dumbbell], 1.0);
        assert_eq!(levels.len(), 5, "both squares are filled to their middle");
        assert_eq!(levels.iter().map(Vec::len).collect::<Vec<_>>(), vec![1, 2, 2, 2, 2], "one ring through the neck, then one for each square");
        // A plate with a hole: one ring inside the edge and one around the hole.
        let plate = square(0.0, 0.0, 20.0);
        let hole = square(5.0, 5.0, 10.0);
        let levels = offset_levels(&[&plate, &hole], 1.0);
        assert_eq!(levels.len(), 3, "5 mm of plate between edge and hole");
        assert_eq!(levels[0].len(), 2);
        for ring in levels.iter().flatten() {
            for q in ring {
                assert!(geom::point_in_polygon(*q, &plate) && !geom::point_in_polygon(*q, &hole), "{q:?}");
            }
        }
    }

    /// Every ring of an offset fill lies inside the shape, at its distance from the edge.
    #[test]
    fn rings_of_shapes_with_arms_stay_inside() {
        let p = |x: f64, y: f64| Point::new(x, y);
        let shapes: Vec<(&str, Vec<Point>)> = vec![
            ("U", vec![p(0.0, 0.0), p(30.0, 0.0), p(30.0, 40.0), p(20.0, 40.0), p(20.0, 20.0), p(10.0, 20.0), p(10.0, 40.0), p(0.0, 40.0)]),
            ("T", vec![p(10.0, 0.0), p(20.0, 0.0), p(20.0, 30.0), p(30.0, 30.0), p(30.0, 36.0), p(0.0, 36.0), p(0.0, 30.0), p(10.0, 30.0)]),
            (
                "E",
                vec![
                    p(0.0, 0.0),
                    p(20.0, 0.0),
                    p(20.0, 4.0),
                    p(8.0, 4.0),
                    p(8.0, 10.0),
                    p(16.0, 10.0),
                    p(16.0, 14.0),
                    p(8.0, 14.0),
                    p(8.0, 20.0),
                    p(20.0, 20.0),
                    p(20.0, 24.0),
                    p(0.0, 24.0),
                ],
            ),
        ];
        for (name, poly) in shapes {
            let mut closed = poly.clone();
            closed.push(poly[0]);
            let levels = offset_levels(&[&poly], 1.0);
            assert!(levels.len() >= 2, "{name}");
            for (k, ring) in levels.iter().enumerate().flat_map(|(k, l)| l.iter().map(move |r| (k, r))) {
                let d = 0.5 + k as f64;
                let n = ring.len();
                for i in 0..n {
                    for q in [ring[i], ring[i].midpoint(ring[(i + 1) % n])] {
                        let dist = geom::nearest_arclen(&closed, q).1;
                        assert!(geom::point_in_polygon(q, &poly), "{name}: ring {k} leaves the shape at {q:?}");
                        assert!(dist > d - 0.03, "{name}: ring {k} at {q:?} is {dist} mm from the edge, not {d}");
                    }
                }
            }
        }
    }

    #[test]
    fn ramps_and_overscan() {
        let rows = vec![(ScanRow { y: 0.0, runs: vec![ScanRun { x0: 0.0, x1: 10.0, power: 100.0, ramp_start: true, ramp_end: true }] }, true)];
        let e = RowEmit { frame: Affine::IDENTITY, speed: 100.0, overscan: 2.0, rapid_gap: 10.0, ramp: (1.0, 0.0) };
        let mut ops = Vec::new();
        emit_rows(&rows, &e, &mut ops);
        assert_eq!(ops[0], Op::Rapid { to: Point::new(-2.0, 0.0) });
        assert_eq!(ops[1], Op::Overscan { to: Point::new(0.0, 0.0), speed: 100.0 });
        assert_eq!(*ops.last().unwrap(), Op::Overscan { to: Point::new(12.0, 0.0), speed: 100.0 });
        let powers: Vec<f64> = ops.iter().filter_map(|o| if let Op::Cut { power, .. } = o { Some(*power) } else { None }).collect();
        assert_eq!(powers, vec![12.5, 37.5, 62.5, 87.5, 100.0, 87.5, 62.5, 37.5, 12.5]);
    }
}
