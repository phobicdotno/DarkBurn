//! Turns a [`JobInput`] into a [`Toolpath`].

use kurbo::{Affine, Point, Rect};

use crate::fill::{self, RowEmit, ScanRow, ScanRun};
use crate::geom::{self, EPS, Poly};
use crate::input::{CutMode, CutSettings, ImageInput, JobInput, LayerInput};
use crate::ir::{CoordSpace, IrLayer, Op, Toolpath};
use crate::line::{self, PathContext};
use crate::origin::OriginSettings;
use crate::planner::{self, ItemGeom, PlanItem, Planned, PlannerSettings};
use crate::progress::{Cancelled, Progress};

/// Options for [`generate`].
#[derive(Clone, Debug, PartialEq)]
pub struct JobOptions {
    pub planner: PlannerSettings,
    /// Map to machine coordinates. `None` keeps design coordinates.
    pub origin: Option<OriginSettings>,
    /// Curve flattening tolerance, mm.
    pub tolerance: f64,
    /// Add comments (layer names, settings) to the toolpath.
    pub comments: bool,
}

impl Default for JobOptions {
    fn default() -> Self {
        Self { planner: PlannerSettings::default(), origin: None, tolerance: 0.01, comments: true }
    }
}

/// Why a job could not be generated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobError {
    Cancelled,
    /// Nothing to burn.
    Empty,
}

impl std::fmt::Display for JobError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JobError::Cancelled => f.write_str("job generation was cancelled"),
            JobError::Empty => f.write_str("nothing to burn"),
        }
    }
}

impl std::error::Error for JobError {}

impl From<Cancelled> for JobError {
    fn from(_: Cancelled) -> Self {
        JobError::Cancelled
    }
}

/// A block is one layer (or sub-layer) in one mode; Fill + Line makes two blocks.
struct Block {
    ir_layer: usize,
    settings: CutSettings,
    mode: CutMode,
    layer: usize,
}

enum UnitKind {
    /// `tabs`: the tab centres on this sub-path (manual or automatic).
    Path {
        closed: bool,
        shape: usize,
        depth: usize,
        tabs: Vec<Point>,
    },
    /// `scales`: the power scale of each polygon's shape.
    Fill {
        polys: Vec<Vec<Point>>,
        scales: Vec<f64>,
        reversible: bool,
    },
    Offset {
        polys: Vec<Vec<Point>>,
        scales: Vec<f64>,
    },
    Image {
        image: usize,
    },
}

struct Unit {
    block: usize,
    kind: UnitKind,
}

/// Generate the toolpath for a job.
pub fn generate(input: &JobInput, opts: &JobOptions, progress: &mut Progress) -> Result<Toolpath, JobError> {
    let tol = opts.tolerance.max(1e-4);
    // Layers in priority order.
    let mut order: Vec<usize> = (0..input.layers.len()).filter(|&i| input.layers[i].output).collect();
    order.sort_by_key(|&i| (input.layers[i].priority, i));

    // Flatten every shape once.
    let flat: Vec<Vec<Vec<Poly>>> = input.layers.iter().map(|l| l.shapes.iter().map(|s| geom::flatten(&s.path, tol)).collect()).collect();
    let job_bounds = job_bounds(input, &flat).ok_or(JobError::Empty)?;
    let start = opts.origin.map_or(Point::ZERO, |o| o.design_start(job_bounds));

    let mut tp = Toolpath { space: CoordSpace::Design, layers: Vec::new(), ops: Vec::new(), start };
    let mut blocks: Vec<Block> = Vec::new();
    for &li in &order {
        let layer = &input.layers[li];
        for (si, s) in layer.settings.iter().enumerate() {
            if !s.enabled || s.passes == 0 {
                continue;
            }
            let ir_layer = tp.layers.len();
            tp.layers.push(IrLayer {
                layer_index: layer.index,
                sublayer: si,
                name: layer.name.clone(),
                mode: s.mode,
                speed: s.speed,
                max_power: s.max_power,
                min_power: s.min_power,
                passes: s.passes,
                constant_power: s.constant_power,
                air: s.air,
                laser: s.laser,
                raster: matches!(s.mode, CutMode::Fill | CutMode::FillLine | CutMode::OffsetFill | CutMode::Image),
                dsp: s.dsp,
                dot: s.line.dot_mode.map(|d| (d.time_ms, d.spacing)),
            });
            let modes: &[CutMode] = match s.mode {
                CutMode::FillLine => &[CutMode::Fill, CutMode::Line],
                CutMode::Line => &[CutMode::Line],
                CutMode::Fill => &[CutMode::Fill],
                CutMode::OffsetFill => &[CutMode::OffsetFill],
                CutMode::Image => &[CutMode::Image],
            };
            for &mode in modes {
                blocks.push(Block { ir_layer, settings: s.clone(), mode, layer: li });
            }
        }
    }

    // Build plannable units.
    let mut units: Vec<Unit> = Vec::new();
    let mut items: Vec<PlanItem> = Vec::new();
    for (bi, b) in blocks.iter().enumerate() {
        let layer = &input.layers[b.layer];
        let polys = &flat[b.layer];
        match b.mode {
            CutMode::Line => line_units(bi, b, layer, polys, &opts.planner, &mut units, &mut items),
            CutMode::Fill | CutMode::FillLine | CutMode::OffsetFill => fill_units(bi, b, layer, polys, start, &mut units, &mut items),
            CutMode::Image => {
                for (ii, img) in layer.images.iter().enumerate() {
                    let ops = image_ops(img, &b.settings, false);
                    let Some((entry, exit, bounds)) = ends(&ops) else { continue };
                    units.push(Unit { block: bi, kind: UnitKind::Image { image: ii } });
                    let mut it = PlanItem::new(ItemGeom::Block { entry, exit, reversible: b.settings.image.bidirectional, bounds });
                    it.layer_rank = bi;
                    it.group = img.group;
                    it.priority = img.priority;
                    items.push(it);
                }
            }
        }
    }
    if units.is_empty() {
        return Err(JobError::Empty);
    }
    progress.set_span(0.0, 0.9);
    let planned = planner::plan(&items, &opts.planner, start, progress)?;
    progress.set_span(0.9, 1.0);

    // Emit runs of consecutive units from the same block, pass by pass.
    let mut cur_laser: Option<u8> = None;
    let mut cur_air = false;
    let mut i = 0;
    while i < planned.len() {
        let bi = units[planned[i].item].block;
        let mut j = i;
        while j < planned.len() && units[planned[j].item].block == bi {
            j += 1;
        }
        let b = &blocks[bi];
        let s = &b.settings;
        tp.ops.push(Op::LayerStart { layer: b.ir_layer });
        if opts.comments {
            tp.ops.push(Op::Comment { text: layer_comment(&input.layers[b.layer], s, b.mode) });
        }
        if cur_laser != Some(s.laser) {
            tp.ops.push(Op::Laser { index: s.laser });
            cur_laser = Some(s.laser);
        }
        if cur_air != s.air {
            tp.ops.push(Op::Air { on: s.air });
            cur_air = s.air;
        }
        for pass in 1..=s.passes {
            tp.ops.push(Op::PassStart { pass });
            if s.z.enabled {
                tp.ops.push(Op::ZMove { z: s.z.offset + s.z.step_per_pass * (pass - 1) as f64 });
            }
            for p in &planned[i..j] {
                emit_unit(&units[p.item], p, &items, b, &input.layers[b.layer], pass, &opts.planner, &mut tp.ops);
            }
        }
        progress.report(j as f32 / planned.len() as f32)?;
        i = j;
    }
    if cur_air {
        tp.ops.push(Op::Air { on: false });
    }
    progress.set_span(0.0, 1.0);
    progress.report(1.0)?;
    Ok(match &opts.origin {
        Some(o) => o.apply(&tp, job_bounds),
        None => tp,
    })
}

fn layer_comment(layer: &LayerInput, s: &CutSettings, mode: CutMode) -> String {
    format!(
        "Layer C{:02} {} - {:?} {} mm/s {}%{}",
        layer.index,
        layer.name,
        mode,
        fmt_num(s.speed),
        fmt_num(s.max_power),
        if s.passes > 1 { format!(" x{}", s.passes) } else { String::new() }
    )
}

fn fmt_num(v: f64) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.to_string() }
}

fn job_bounds(input: &JobInput, flat: &[Vec<Vec<Poly>>]) -> Option<Rect> {
    let mut r: Option<Rect> = None;
    let mut add = |p: Point| r = Some(r.map_or(Rect::from_points(p, p), |r| r.union_pt(p)));
    for (li, layer) in input.layers.iter().enumerate() {
        if !layer.output {
            continue;
        }
        for shape in &flat[li] {
            for poly in shape {
                poly.pts.iter().for_each(|p| add(*p));
            }
        }
        for img in &layer.images {
            for sl in &img.scanlines {
                for &(x0, x1, _) in &sl.runs {
                    add(img.transform * Point::new(x0, sl.y));
                    add(img.transform * Point::new(x1, sl.y));
                }
            }
        }
    }
    r
}

/// Distance from `p` to a sub-path.
fn poly_distance(poly: &Poly, p: Point) -> f64 {
    let mut pts = poly.pts.clone();
    if poly.closed {
        pts.push(poly.pts[0]);
    }
    geom::nearest_arclen(&pts, p).1
}

fn line_units(bi: usize, b: &Block, layer: &LayerInput, polys: &[Vec<Poly>], ps: &PlannerSettings, units: &mut Vec<Unit>, items: &mut Vec<PlanItem>) {
    let mut list: Vec<(Poly, usize)> = Vec::new();
    for (si, shape) in polys.iter().enumerate() {
        for p in shape {
            list.push((p.clone(), si));
        }
    }
    if ps.optimize && ps.remove_overlaps {
        list = planner::remove_overlaps(list, ps.overlap_distance.max(1e-6));
    }
    // Containment depth among closed paths (for tabs and lead side).
    let closed: Vec<(Rect, &Poly)> = list.iter().filter(|(p, _)| p.closed).map(|(p, _)| (geom::bbox(&p.pts), p)).collect();
    // The sub-paths of each shape, as indices into `list`.
    let mut subs: Vec<Vec<usize>> = vec![Vec::new(); layer.shapes.len()];
    for (li, (_, si)) in list.iter().enumerate() {
        subs[*si].push(li);
    }
    // A forced start point belongs to the closed sub-path of its shape that lies nearest to it
    // (a compound shape may have one for each sub-path); a sub-path takes its nearest one.
    // A manual tab belongs to the sub-path of its shape nearest to it, open or closed.
    let mut forced: Vec<Option<(Point, f64)>> = vec![None; list.len()];
    let mut manual: Vec<Vec<Point>> = vec![Vec::new(); list.len()];
    for (si, shape) in layer.shapes.iter().enumerate() {
        let nearest = |p: Point, closed_only: bool| {
            subs[si]
                .iter()
                .filter(|&&li| list[li].0.closed || !closed_only)
                .map(|&li| (li, poly_distance(&list[li].0, p)))
                .min_by(|a, b| a.1.total_cmp(&b.1))
        };
        for sp in shape.start_point.iter().chain(&shape.extra_start_points) {
            if let Some((li, d)) = nearest(*sp, true)
                && forced[li].is_none_or(|f| d < f.1)
            {
                forced[li] = Some((*sp, d));
            }
        }
        for t in &shape.tabs {
            if let Some((li, _)) = nearest(*t, false) {
                manual[li].push(*t);
            }
        }
    }
    for (li, (poly, si)) in list.iter().enumerate() {
        let shape = &layer.shapes[*si];
        let bb = geom::bbox(&poly.pts);
        let area = geom::signed_area(&poly.pts).abs();
        let depth = closed
            .iter()
            .filter(|(r, q)| {
                !std::ptr::eq(*q, poly)
                    && geom::rect_inside(bb, *r)
                    && (!poly.closed || geom::signed_area(&q.pts).abs() > area)
                    && geom::point_in_polygon(poly.pts[0], &q.pts)
                    && geom::point_in_polygon(poly.pts[poly.pts.len() / 2], &q.pts)
            })
            .count();
        let mut pts = poly.pts.clone();
        let mut forced_start = None;
        if poly.closed
            && let Some((sp, _)) = forced[li]
        {
            // Insert the forced start point on the polygon.
            let mut ring = pts.clone();
            ring.push(pts[0]);
            let (s, _) = geom::nearest_arclen(&ring, sp);
            let (p, _) = geom::point_at(&ring, s);
            match pts.iter().position(|q| q.distance(p) < 1e-6) {
                Some(k) => forced_start = Some(k),
                None => {
                    let mut acc = 0.0;
                    let mut at = pts.len();
                    for k in 0..pts.len() {
                        let l = ring[k].distance(ring[k + 1]);
                        if acc + l >= s {
                            at = k + 1;
                            break;
                        }
                        acc += l;
                    }
                    pts.insert(at, p);
                    forced_start = Some(at % pts.len());
                }
            }
        }
        let tabs = line::tab_centres(&poly.pts, poly.closed, depth, &b.settings.line.tabs, (!shape.tabs.is_empty()).then_some(manual[li].as_slice()));
        units.push(Unit { block: bi, kind: UnitKind::Path { closed: poly.closed, shape: *si, depth, tabs } });
        let mut it = PlanItem::new(if poly.closed { ItemGeom::Closed(pts) } else { ItemGeom::Open(pts) });
        it.layer_rank = bi;
        it.group = shape.group;
        it.priority = shape.priority;
        it.forced_start = forced_start;
        it.forced_direction = shape.direction;
        items.push(it);
    }
}

fn fill_units(bi: usize, b: &Block, layer: &LayerInput, polys: &[Vec<Poly>], start: Point, units: &mut Vec<Unit>, items: &mut Vec<PlanItem>) {
    let mut closed: Vec<Vec<Point>> = Vec::new();
    let mut shape_of: Vec<usize> = Vec::new();
    for (si, shape) in polys.iter().enumerate() {
        for p in shape.iter().filter(|p| p.closed) {
            closed.push(p.pts.clone());
            shape_of.push(si);
        }
    }
    let groups: Vec<Option<u32>> = layer.shapes.iter().map(|s| s.group).collect();
    let clusters = fill::fill_clusters(&closed, &shape_of, &groups, b.settings.fill.grouping);
    for c in clusters {
        let cpolys: Vec<Vec<Point>> = c.iter().map(|&i| closed[i].clone()).collect();
        let scales: Vec<f64> = c.iter().map(|&i| layer.shapes[shape_of[i]].power_scale).collect();
        let group = layer.shapes[shape_of[c[0]]].group;
        let priority = c.iter().map(|&i| layer.shapes[shape_of[i]].priority).min().unwrap_or(0);
        let kind = if b.mode == CutMode::OffsetFill {
            UnitKind::Offset { polys: cpolys, scales }
        } else {
            UnitKind::Fill { polys: cpolys, scales, reversible: b.settings.fill.bidirectional }
        };
        let unit = Unit { block: bi, kind };
        let mut ops = Vec::new();
        let dummy = Planned { item: 0, start: 0, reversed: false };
        expand_fill(&unit, &dummy, &b.settings, 1, start, &mut ops);
        let Some((entry, exit, bounds)) = ends(&ops) else { continue };
        let reversible = matches!(unit.kind, UnitKind::Fill { reversible: true, .. });
        units.push(unit);
        let mut it = PlanItem::new(ItemGeom::Block { entry, exit, reversible, bounds });
        it.layer_rank = bi;
        it.group = group;
        it.priority = priority;
        items.push(it);
    }
}

/// Entry (first rapid target), exit (last position) and bounds of an op list.
fn ends(ops: &[Op]) -> Option<(Point, Point, Rect)> {
    let pts: Vec<Point> = ops.iter().filter_map(|o| o.target()).collect();
    let first = *pts.first()?;
    Some((first, *pts.last()?, geom::bbox(&pts)))
}

#[allow(clippy::too_many_arguments)]
fn emit_unit(unit: &Unit, p: &Planned, items: &[PlanItem], b: &Block, layer: &LayerInput, pass: u32, ps: &PlannerSettings, out: &mut Vec<Op>) {
    let backlash = (ps.optimize && ps.hide_backlash).then_some(ps.backlash_approach);
    match &unit.kind {
        UnitKind::Path { closed, shape, depth, tabs } => {
            let pts = p.ordered_points(items);
            let sh = &layer.shapes[*shape];
            let ctx = PathContext { settings: &b.settings, power_scale: sh.power_scale, depth: *depth, tabs, backlash };
            line::emit_path(&pts, *closed, &ctx, out);
        }
        UnitKind::Fill { .. } | UnitKind::Offset { .. } => {
            let mut ops = Vec::new();
            let from = p.entry(items);
            expand_fill(unit, p, &b.settings, pass, from, &mut ops);
            if let (Some(d), Some(Op::Rapid { to })) = (backlash, ops.first()) {
                out.push(Op::Rapid { to: *to - kurbo::Vec2::new(d, d) });
            }
            out.extend(ops);
        }
        UnitKind::Image { image } => {
            let ops = image_ops(&layer.images[*image], &b.settings, p.reversed);
            if let (Some(d), Some(Op::Rapid { to })) = (backlash, ops.first()) {
                out.push(Op::Rapid { to: *to - kurbo::Vec2::new(d, d) });
            }
            out.extend(ops);
        }
    }
}

/// Expand a fill or offset fill unit for one pass.
fn expand_fill(unit: &Unit, p: &Planned, s: &CutSettings, pass: u32, from: Point, out: &mut Vec<Op>) {
    let f = &s.fill;
    match &unit.kind {
        UnitKind::Fill { polys, scales, .. } => {
            let powers: Vec<f64> = scales.iter().map(|k| s.max_power * k).collect();
            let refs: Vec<&[Point]> = polys.iter().map(|p| p.as_slice()).collect();
            let base = f.angle_deg + f.angle_step_per_pass * (pass - 1) as f64;
            let mut angles = vec![base];
            if f.cross_hatch {
                angles.push(base + 90.0);
            }
            for deg in angles {
                let a = deg.to_radians();
                let mut rows = fill::scan_polygons_powers(&refs, &powers, f.fill_rule, f.interval, a);
                fill::mark_ramps(&mut rows, f.ramp_outer_edge);
                let mut ordered = fill::order_rows(rows, f.bidirectional, f.flood_fill, f.interval);
                if p.reversed {
                    ordered.reverse();
                    if f.bidirectional {
                        for o in ordered.iter_mut() {
                            o.1 = !o.1;
                        }
                    }
                }
                let e = RowEmit {
                    frame: Affine::rotate(a),
                    speed: s.speed,
                    overscan: f.overscan.distance(s.speed),
                    rapid_gap: f.rapid_gap,
                    ramp: (f.ramp_length, s.min_power),
                };
                fill::emit_rows(&ordered, &e, out);
            }
        }
        UnitKind::Offset { polys, scales } => {
            let parents = fill::parents(polys);
            let mut pos = from;
            for (pi, poly) in polys.iter().enumerate() {
                // Rings of outer shapes only (holes are not avoided).
                let depth = std::iter::successors(parents[pi], |&q| parents[q]).count();
                if depth % 2 == 1 {
                    continue;
                }
                let power = s.max_power * scales[pi];
                let rings = fill::offset_rings(poly, f.interval);
                for (k, ring) in rings.iter().enumerate() {
                    let st = planner::choose_start(ring, pos, false);
                    let n = ring.len();
                    let first = ring[st];
                    if k == 0 {
                        out.push(Op::Rapid { to: first });
                    } else {
                        // Step inwards with the laser on: an inward spiral.
                        out.push(Op::Cut { to: first, speed: s.speed, power });
                    }
                    for m in 1..=n {
                        out.push(Op::Cut { to: ring[(st + m) % n], speed: s.speed, power });
                    }
                    pos = first;
                }
            }
        }
        _ => {}
    }
}

/// Operations for an image's scanlines.
fn image_ops(img: &ImageInput, s: &CutSettings, reversed: bool) -> Vec<Op> {
    let is = &s.image;
    let mut rows: Vec<ScanRow> = Vec::new();
    let mut sorted: Vec<&crate::input::Scanline> = img.scanlines.iter().collect();
    sorted.sort_by(|a, b| a.y.partial_cmp(&b.y).unwrap());
    for sl in sorted {
        let mut runs: Vec<ScanRun> = Vec::new();
        let mut raw: Vec<(f64, f64, f64)> =
            sl.runs.iter().map(|&(a, b, p)| (a.min(b), a.max(b), p.clamp(0.0, 1.0))).filter(|r| r.1 - r.0 > EPS).collect();
        raw.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        for (x0, x1, p) in raw {
            let power = if p > 0.0 { s.min_power + (s.max_power - s.min_power) * p } else { 0.0 };
            if is.skip_blanks && power <= 0.0 {
                continue;
            }
            match runs.last_mut() {
                Some(r) if (x0 - r.x1).abs() < EPS && (r.power - power).abs() < 1e-9 => r.x1 = x1,
                _ => runs.push(ScanRun { x0, x1, power, ramp_start: false, ramp_end: false }),
            }
        }
        if runs.is_empty() || (is.skip_blanks && runs.iter().all(|r| r.power <= 0.0)) {
            continue;
        }
        rows.push(ScanRow { y: sl.y, runs });
    }
    let mut ordered = fill::order_rows(rows, is.bidirectional, false, 0.0);
    if reversed {
        ordered.reverse();
        if is.bidirectional {
            for o in ordered.iter_mut() {
                o.1 = !o.1;
            }
        }
    }
    let half = img.direction_offset / 2.0;
    if is.bidirectional && half != 0.0 {
        for (row, forward) in ordered.iter_mut() {
            let d = if *forward { -half } else { half };
            for r in row.runs.iter_mut() {
                r.x0 += d;
                r.x1 += d;
            }
        }
    }
    let e = RowEmit {
        frame: img.transform,
        speed: s.speed,
        overscan: is.overscan.distance(s.speed),
        rapid_gap: if is.skip_blanks { is.rapid_gap } else { f64::INFINITY },
        ramp: (0.0, 0.0),
    };
    let mut out = Vec::new();
    fill::emit_rows(&ordered, &e, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{ShapeInput, ZSettings};
    use kurbo::{BezPath, Shape};

    fn rect_path(x: f64, y: f64, w: f64, h: f64) -> BezPath {
        kurbo::Rect::new(x, y, x + w, y + h).to_path(0.01)
    }

    #[test]
    fn layers_follow_priority_and_passes() {
        let mut a = LayerInput::new(0, "A", CutSettings::line(10.0, 50.0)).with_shape(ShapeInput::new(rect_path(0.0, 0.0, 10.0, 10.0)));
        a.priority = 5;
        a.settings[0].passes = 2;
        a.settings[0].z = ZSettings { enabled: true, offset: 1.0, step_per_pass: -0.5 };
        let b = LayerInput::new(1, "B", CutSettings::line(20.0, 30.0)).with_shape(ShapeInput::new(rect_path(20.0, 0.0, 10.0, 10.0)));
        let tp = generate(&JobInput { layers: vec![a, b] }, &JobOptions::default(), &mut Progress::none()).unwrap();
        assert_eq!(tp.layers[0].name, "B");
        let zs: Vec<f64> = tp.ops.iter().filter_map(|o| if let Op::ZMove { z } = o { Some(*z) } else { None }).collect();
        assert_eq!(zs, vec![1.0, 0.5]);
        let passes = tp.ops.iter().filter(|o| matches!(o, Op::PassStart { .. })).count();
        assert_eq!(passes, 3);
    }

    #[test]
    fn sublayers_run_in_order_and_disabled_skip() {
        let mut l = LayerInput::new(0, "A", CutSettings::line(10.0, 50.0)).with_shape(ShapeInput::new(rect_path(0.0, 0.0, 10.0, 10.0)));
        l.settings.push(CutSettings::fill(100.0, 20.0, 1.0));
        l.settings.push(CutSettings { enabled: false, ..CutSettings::line(5.0, 90.0) });
        let tp = generate(&JobInput { layers: vec![l] }, &JobOptions::default(), &mut Progress::none()).unwrap();
        assert_eq!(tp.layers.len(), 2);
        assert_eq!(tp.layers[1].sublayer, 1);
        let starts: Vec<usize> = tp.ops.iter().filter_map(|o| if let Op::LayerStart { layer } = o { Some(*layer) } else { None }).collect();
        assert_eq!(starts, vec![0, 1]);
    }

    #[test]
    fn fill_line_fills_then_outlines() {
        let mut s = CutSettings::fill(100.0, 20.0, 1.0);
        s.mode = CutMode::FillLine;
        let l = LayerInput::new(0, "A", s).with_shape(ShapeInput::new(rect_path(0.0, 0.0, 10.0, 10.0)));
        let tp = generate(&JobInput { layers: vec![l] }, &JobOptions::default(), &mut Progress::none()).unwrap();
        let cuts: Vec<&Op> = tp.ops.iter().filter(|o| matches!(o, Op::Cut { .. })).collect();
        assert_eq!(cuts.len(), 10 + 4);
    }

    #[test]
    fn offset_fill_spirals_inwards() {
        let mut s = CutSettings::fill(100.0, 20.0, 1.0);
        s.mode = CutMode::OffsetFill;
        let l = LayerInput::new(0, "A", s).with_shape(ShapeInput::new(rect_path(0.0, 0.0, 10.0, 10.0)));
        let tp = generate(&JobInput { layers: vec![l] }, &JobOptions::default(), &mut Progress::none()).unwrap();
        let rapids = tp.ops.iter().filter(|o| matches!(o, Op::Rapid { .. })).count();
        assert_eq!(rapids, 1);
        let cuts = tp.ops.iter().filter(|o| matches!(o, Op::Cut { .. })).count();
        assert_eq!(cuts, 5 * 4 + 4);
    }

    /// The powers the laser fires at left and right of x = 50.
    fn powers_by_side(tp: &Toolpath) -> (Vec<f64>, Vec<f64>) {
        let (mut left, mut right) = (Vec::new(), Vec::new());
        let mut pos = Point::ZERO;
        for op in &tp.ops {
            if let Op::Cut { to, power, .. } = op
                && *power > 0.0
            {
                let side = if pos.midpoint(*to).x < 50.0 { &mut left } else { &mut right };
                if !side.contains(power) {
                    side.push(*power);
                }
            }
            if let Some(t) = op.target() {
                pos = t;
            }
        }
        (left, right)
    }

    #[test]
    fn every_shape_of_a_fill_keeps_its_own_power_scale() {
        for mode in [CutMode::Fill, CutMode::OffsetFill] {
            let mut s = CutSettings::fill(100.0, 80.0, 1.0);
            s.mode = mode;
            let full = ShapeInput::new(rect_path(0.0, 0.0, 10.0, 10.0));
            let mut half = ShapeInput::new(rect_path(100.0, 0.0, 10.0, 10.0));
            half.power_scale = 0.5;
            let tp = run(LayerInput::new(0, "A", s).with_shape(full).with_shape(half), &JobOptions::default());
            assert_eq!(powers_by_side(&tp), (vec![80.0], vec![40.0]), "{mode:?}");
        }
    }

    #[test]
    fn a_hole_with_another_power_scale_stays_a_hole() {
        let outer = ShapeInput::new(rect_path(0.0, 0.0, 10.0, 10.0));
        let mut hole = ShapeInput::new(rect_path(3.0, 3.0, 4.0, 4.0));
        hole.power_scale = 0.5;
        let tp = run(LayerInput::new(0, "A", CutSettings::fill(100.0, 80.0, 1.0)).with_shape(outer).with_shape(hole), &JobOptions::default());
        let mut pos = Point::ZERO;
        let mut burnt = 0.0;
        for op in &tp.ops {
            if let Op::Cut { to, power, .. } = op
                && *power > 0.0
            {
                assert_eq!(*power, 80.0, "the ring burns at the outer shape's power");
                burnt += pos.distance(*to);
            }
            if let Some(t) = op.target() {
                pos = t;
            }
        }
        assert!((burnt - (100.0 - 16.0)).abs() < 1e-6, "{burnt}");
    }

    #[test]
    fn an_image_scan_offset_follows_the_direction_of_each_line() {
        // The burning moves of four 10 mm lines starting at `y0`, as (from, to).
        let burns = |offset: f64, bidirectional: bool, y0: f64| {
            let lines = (0..4).map(|k| crate::input::Scanline { y: y0 + k as f64, runs: vec![(0.0, 10.0, 1.0)] }).collect();
            let mut img = ImageInput::new(lines);
            img.direction_offset = offset;
            let mut s = CutSettings::image(100.0, 50.0, 0.0);
            s.image.bidirectional = bidirectional;
            let mut l = LayerInput::new(0, "A", s);
            l.images.push(img);
            let tp = run(l, &JobOptions::default());
            let mut pos = Point::ZERO;
            let mut out = Vec::new();
            for op in &tp.ops {
                if let Op::Cut { to, power, .. } = op
                    && *power > 0.0
                {
                    out.push((pos, *to));
                }
                if let Some(t) = op.target() {
                    pos = t;
                }
            }
            out
        };
        // Above the start the image is scanned bottom up, below it top down (every line then
        // runs the other way).
        for y0 in [5.0, -10.0] {
            let plain = burns(0.0, true, y0);
            let shifted = burns(0.4, true, y0);
            assert_eq!(plain.len(), 4);
            assert_eq!(plain[0].0.y < plain[3].0.y, y0 > 0.0, "scan order at {y0}");
            for (a, b) in plain.iter().zip(&shifted) {
                let want = if a.1.x > a.0.x { -0.2 } else { 0.2 };
                assert!((b.0.x - a.0.x - want).abs() < 1e-9 && (b.1.x - a.1.x - want).abs() < 1e-9, "at {y0}: {a:?} {b:?}");
            }
            assert_eq!(burns(0.4, false, y0), burns(0.0, false, y0), "a one-way scan is not shifted");
        }
    }

    #[test]
    fn empty_job() {
        let r = generate(&JobInput::default(), &JobOptions::default(), &mut Progress::none());
        assert_eq!(r, Err(JobError::Empty));
    }

    #[test]
    fn forced_start_point_inserted() {
        let mut sh = ShapeInput::new(rect_path(0.0, 0.0, 10.0, 10.0));
        sh.start_point = Some(Point::new(5.0, -1.0));
        let l = LayerInput::new(0, "A", CutSettings::line(10.0, 50.0)).with_shape(sh);
        let tp = generate(&JobInput { layers: vec![l] }, &JobOptions::default(), &mut Progress::none()).unwrap();
        let first = tp.ops.iter().find_map(|o| if let Op::Rapid { to } = o { Some(*to) } else { None }).unwrap();
        assert!(first.distance(Point::new(5.0, 0.0)) < 1e-9, "{first:?}");
    }

    /// The laser-off stretches of the cuts, as (from, to).
    fn gaps(tp: &Toolpath) -> Vec<(Point, Point)> {
        let mut pos = Point::ZERO;
        let mut out: Vec<(Point, Point)> = Vec::new();
        for op in &tp.ops {
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

    fn tab_settings(auto: bool) -> CutSettings {
        let mut s = CutSettings::line(10.0, 50.0);
        s.line.tabs.enabled = true;
        s.line.tabs.auto = auto;
        s.line.tabs.size = 2.0;
        s.line.tabs.spacing = 20.0;
        s.line.tabs.min_count = 1;
        s.line.tabs.max_count = 10;
        s
    }

    fn run(l: LayerInput, opts: &JobOptions) -> Toolpath {
        generate(&JobInput { layers: vec![l] }, opts, &mut Progress::none()).unwrap()
    }

    #[test]
    fn a_manual_tab_goes_on_the_sub_path_it_was_placed_on() {
        // A frame: outer 40 x 40 and an inner 20 x 20 hole, as one compound shape.
        let mut path = rect_path(0.0, 0.0, 40.0, 40.0);
        path.extend(rect_path(10.0, 10.0, 20.0, 20.0));
        path.move_to((50.0, 0.0));
        path.line_to((50.0, 40.0));
        let mut sh = ShapeInput::new(path);
        sh.tabs = vec![Point::new(20.0, 0.0), Point::new(30.0, 20.0), Point::new(50.0, 25.0)];
        for optimize in [true, false] {
            let opts = JobOptions { planner: PlannerSettings { optimize, ..Default::default() }, ..Default::default() };
            let tp = run(LayerInput::new(0, "A", tab_settings(false)).with_shape(sh.clone()), &opts);
            let g = gaps(&tp);
            assert_eq!(g.len(), 3, "one gap for each tab, none on the other sub-paths: {g:?}");
            let mid: Vec<Point> = g.iter().map(|(a, b)| a.midpoint(*b)).collect();
            for want in &sh.tabs {
                assert!(mid.iter().any(|m| m.distance(*want) < 1e-6), "a gap centred on {want:?}: {g:?}");
            }
            assert!(g.iter().all(|(a, b)| (a.distance(*b) - 2.0).abs() < 1e-6), "{g:?}");
        }
    }

    #[test]
    fn manual_tabs_replace_the_automatic_ones_on_their_shape_only() {
        let mut with = ShapeInput::new(rect_path(0.0, 0.0, 20.0, 20.0));
        with.tabs = vec![Point::new(10.0, 0.0)];
        let without = ShapeInput::new(rect_path(100.0, 0.0, 20.0, 20.0));
        let tp = run(LayerInput::new(0, "A", tab_settings(true)).with_shape(with).with_shape(without), &JobOptions::default());
        let g = gaps(&tp);
        assert_eq!(g.iter().filter(|(a, _)| a.x < 50.0).count(), 1, "{g:?}");
        assert_eq!(g.iter().filter(|(a, _)| a.x > 50.0).count(), 4, "80 mm at 20 mm spacing: {g:?}");
    }

    #[test]
    fn automatic_tabs_stay_put_when_the_start_point_moves() {
        let mids = |start: Option<Point>, direction| {
            let mut sh = ShapeInput::new(rect_path(0.0, 0.0, 30.0, 10.0));
            sh.start_point = start;
            sh.direction = direction;
            let tp = run(LayerInput::new(0, "A", tab_settings(true)).with_shape(sh), &JobOptions::default());
            let mut m: Vec<(i64, i64)> =
                gaps(&tp).iter().map(|(a, b)| a.midpoint(*b)).map(|p| ((p.x * 1e3).round() as i64, (p.y * 1e3).round() as i64)).collect();
            m.sort();
            m
        };
        let base = mids(None, None);
        assert_eq!(base.len(), 4);
        assert_eq!(mids(Some(Point::new(17.0, 10.0)), None), base);
        assert_eq!(mids(Some(Point::new(30.0, 3.0)), Some(crate::input::ForcedDirection::Clockwise)), base);
    }

    #[test]
    fn every_sub_path_of_a_compound_shape_can_have_its_own_start_point() {
        let mut path = rect_path(0.0, 0.0, 10.0, 10.0);
        path.extend(rect_path(50.0, 0.0, 10.0, 10.0));
        path.extend(rect_path(100.0, 0.0, 10.0, 10.0));
        let mut sh = ShapeInput::new(path);
        sh.start_point = Some(Point::new(5.0, 10.5));
        sh.extra_start_points = vec![Point::new(60.0, 4.0), Point::new(61.0, 6.0)];
        for optimize in [true, false] {
            let opts = JobOptions { planner: PlannerSettings { optimize, ..Default::default() }, ..Default::default() };
            let tp = run(LayerInput::new(0, "A", CutSettings::line(10.0, 50.0)).with_shape(sh.clone()), &opts);
            let rapids: Vec<Point> = tp.ops.iter().filter_map(|o| if let Op::Rapid { to } = o { Some(*to) } else { None }).collect();
            assert_eq!(rapids.len(), 3);
            assert!(rapids.iter().any(|p| p.distance(Point::new(5.0, 10.0)) < 1e-9), "{rapids:?}");
            assert!(rapids.iter().any(|p| p.distance(Point::new(60.0, 4.0)) < 1e-9), "the nearer of two points on one sub-path wins: {rapids:?}");
        }
    }

    #[test]
    fn a_forced_start_point_holds_with_lead_in_and_every_planner_option() {
        let mut s = CutSettings::line(10.0, 50.0);
        s.line.lead_in = Some(crate::input::Lead { length: 2.0, angle_deg: 90.0, style: crate::input::LeadStyle::Line });
        let mut sh = ShapeInput::new(rect_path(0.0, 0.0, 10.0, 10.0));
        sh.start_point = Some(Point::new(10.0, 4.0));
        sh.direction = Some(crate::input::ForcedDirection::Clockwise);
        let far = ShapeInput::new(rect_path(40.0, 40.0, 10.0, 10.0));
        let l = LayerInput::new(0, "A", s).with_shape(far).with_shape(sh);
        for bits in 0..32u32 {
            let on = |b: u32| bits & (1 << b) != 0;
            let planner = PlannerSettings {
                optimize: on(0),
                best_start: on(1),
                prefer_corners: on(2),
                best_direction: on(3),
                reduce_direction_changes: on(4),
                ..Default::default()
            };
            let tp = run(l.clone(), &JobOptions { planner, ..Default::default() });
            // The lead-in comes in from the waste side (outside, +X) and ends on the start point;
            // the first move of the outline then heads down (clockwise).
            let i = tp
                .ops
                .iter()
                .position(|o| matches!(o, Op::Rapid { to } if to.distance(Point::new(12.0, 4.0)) < 1e-9))
                .unwrap_or_else(|| panic!("lead-in start, options {bits:05b}: {:?}", tp.ops));
            assert_eq!(tp.ops[i + 1].target(), Some(Point::new(10.0, 4.0)), "options {bits:05b}");
            assert_eq!(tp.ops[i + 2].target(), Some(Point::new(10.0, 0.0)), "options {bits:05b}");
        }
    }

    #[test]
    fn serde_roundtrip() {
        let l = LayerInput::new(0, "A", CutSettings::line(10.0, 50.0)).with_shape(ShapeInput::new(rect_path(0.0, 0.0, 10.0, 10.0)));
        let tp = generate(&JobInput { layers: vec![l] }, &JobOptions::default(), &mut Progress::none()).unwrap();
        let json = serde_json::to_string(&tp).unwrap();
        let back: Toolpath = serde_json::from_str(&json).unwrap();
        assert_eq!(back, tp);
    }

    #[test]
    fn hide_backlash_approaches_from_one_side() {
        let l = LayerInput::new(0, "A", CutSettings::line(10.0, 50.0)).with_shape(ShapeInput::new(rect_path(5.0, 5.0, 10.0, 10.0)));
        let opts = JobOptions { planner: PlannerSettings { hide_backlash: true, ..Default::default() }, ..Default::default() };
        let tp = generate(&JobInput { layers: vec![l] }, &opts, &mut Progress::none()).unwrap();
        let rapids: Vec<Point> = tp.ops.iter().filter_map(|o| if let Op::Rapid { to } = o { Some(*to) } else { None }).collect();
        assert_eq!(rapids, vec![Point::new(4.5, 4.5), Point::new(5.0, 5.0)]);
    }
}
