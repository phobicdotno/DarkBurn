//! Bug hunt 5: the job as a whole, checked by invariants over generated documents instead of
//! by examples. Every document is made from a seed, so a failure names the seed to rerun.

use std::collections::BTreeMap;

use darkburn_core::kurbo::{BezPath, ParamCurveArclen, Point, Rect, Shape as _};
use darkburn_core::layer::Mode;
use darkburn_core::{Document, Shape, ShapeId};
use darkburn_job::{Corner, FinishPosition, JobAnchor, Op, Toolpath};

use crate::job_build::*;
use crate::job_field_tests::{Seg, input, segs};

// ----- A small deterministic generator ----------------------------------------------------------

pub(crate) struct Rng(u64);

impl Rng {
    pub(crate) fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xD1B5_4A32_D192_ED03)
    }

    fn next(&mut self) -> u64 {
        // splitmix64
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    pub(crate) fn pick(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    pub(crate) fn f(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * ((self.next() >> 11) as f64 / (1u64 << 53) as f64)
    }

    pub(crate) fn chance(&mut self, p: f64) -> bool {
        self.f(0.0, 1.0) < p
    }
}

/// Side of one grid cell: every generated shape lives in a cell of its own, so what burns in a
/// cell belongs to that shape.
const CELL: f64 = 40.0;
const COLS: usize = 5;
const LAYERS: usize = 4;

#[derive(Clone)]
struct Planned {
    cell: usize,
    shape: Shape,
    /// A point in the middle of a straight edge (rectangles and polygons).
    edge_mid: Option<Point>,
}

fn cell_rect(cell: usize, shift: Point) -> Rect {
    let (cx, cy) = ((cell % COLS) as f64, (cell / COLS) as f64);
    Rect::new(cx * CELL + shift.x, cy * CELL + shift.y, (cx + 1.0) * CELL + shift.x, (cy + 1.0) * CELL + shift.y)
}

fn cell_of(p: Point, shift: Point) -> Option<usize> {
    let (x, y) = ((p.x - shift.x) / CELL, (p.y - shift.y) / CELL);
    if x < 0.0 || y < 0.0 || x >= COLS as f64 || y >= COLS as f64 {
        return None;
    }
    Some(y.floor() as usize * COLS + x.floor() as usize)
}

/// The shapes of a document: one per cell, well inside it.
fn plan_shapes(rng: &mut Rng, shift: Point) -> Vec<Planned> {
    let n = 1 + rng.pick(9);
    let mut cells: Vec<usize> = (0..COLS * COLS).collect();
    let mut out = Vec::new();
    for _ in 0..n {
        let cell = cells.swap_remove(rng.pick(cells.len()));
        let c = cell_rect(cell, shift);
        let layer = rng.pick(LAYERS);
        let (w, h) = (rng.f(15.0, 26.0), rng.f(15.0, 26.0));
        let (x, y) = (c.x0 + rng.f(7.0, CELL - 7.0 - w), c.y0 + rng.f(7.0, CELL - 7.0 - h));
        let r = Rect::new(x, y, x + w, y + h);
        let (shape, edge_mid) = match rng.pick(4) {
            0 | 1 => (Shape::rect(r, 0.0, layer), Some(Point::new(x + w / 2.0, y))),
            2 => (Shape::ellipse(r, layer), None),
            _ => {
                let mut p = BezPath::new();
                p.move_to((x, y));
                p.line_to((x + w, y));
                p.line_to((x + w, y + h));
                p.line_to((x + w / 2.0, y + h * 0.6));
                p.line_to((x, y + h));
                p.close_path();
                (Shape::path(p, layer), Some(Point::new(x + w / 2.0, y)))
            }
        };
        out.push(Planned { cell, shape, edge_mid });
    }
    out
}

/// Random cut settings on the layers the shapes use. Returns the document without shapes.
fn plan_layers(rng: &mut Rng) -> Document {
    let mut d = Document::new();
    for li in 0..LAYERS {
        let subs = if rng.chance(0.2) { 2 } else { 1 };
        if subs == 2 {
            d.layer_mut(li).add_sub_layer();
        }
        for si in 0..subs {
            let s = &mut d.layer_mut(li).settings[si];
            s.enabled = true;
            s.mode = match rng.pick(10) {
                0..=4 => Mode::Line,
                5 | 6 => Mode::Fill,
                7 => Mode::FillLine,
                _ => Mode::OffsetFill,
            };
            s.speed = rng.f(10.0, 200.0);
            s.max_power = rng.f(5.0, 100.0);
            s.min_power = s.max_power * rng.f(0.0, 1.0);
            s.passes = 1 + rng.pick(3) as u32;
            s.interval = rng.f(0.3, 0.6);
            s.bidirectional = rng.chance(0.7);
            s.cross_hatch = rng.chance(0.2);
            s.overscan = if rng.chance(0.4) { rng.f(0.5, 4.0) } else { 0.0 };
            s.overscan_percent = false;
            s.scan_angle = if rng.chance(0.3) { rng.f(0.0, 180.0) } else { 0.0 };
            if rng.chance(0.2) {
                s.kerf = rng.f(0.05, 0.3);
            }
            if rng.chance(0.15) {
                s.tabs.enabled = true;
                s.tabs.auto_spacing = true;
                s.tabs.spacing = rng.f(10.0, 30.0);
                s.tabs.size = rng.f(0.3, 1.0);
            }
            if rng.chance(0.15) {
                s.lead_in.enabled = true;
                s.lead_in.length = rng.f(1.0, 4.0);
            }
            if rng.chance(0.1) {
                s.overcut = rng.f(0.5, 3.0);
            }
            if rng.chance(0.1) {
                s.perforation = true;
                s.perf_cut = rng.f(0.5, 2.0);
                s.perf_skip = rng.f(0.5, 2.0);
            }
        }
    }
    d
}

struct Case {
    input: BuildInput,
    shapes: Vec<Planned>,
    ids: Vec<ShapeId>,
    shift: Point,
}

fn make_case(seed: u64, reverse_creation: bool) -> Case {
    let mut rng = Rng::new(seed);
    // Most documents sit inside the bed; some hang over an edge.
    let shift = if rng.chance(0.2) { Point::new(rng.f(-12.0, 12.0), rng.f(-12.0, 12.0)) } else { Point::ZERO };
    let mut doc = plan_layers(&mut rng);
    let shapes = plan_shapes(&mut rng, shift);
    let mut ids = vec![ShapeId(0); shapes.len()];
    let order: Vec<usize> = if reverse_creation { (0..shapes.len()).rev().collect() } else { (0..shapes.len()).collect() };
    for i in order {
        ids[i] = doc.add(shapes[i].shape.clone());
    }
    let mut inp = input(doc);
    inp.device = JobDevice::new(COLS as f64 * CELL, COLS as f64 * CELL);
    inp.device.origin = [Corner::BottomLeft, Corner::BottomRight, Corner::TopLeft, Corner::TopRight][rng.pick(4)];
    inp.device.negative_coords = rng.chance(0.15);
    inp.device.swap_xy = rng.chance(0.15);
    let st = &mut inp.settings;
    st.start_from = [StartMode::Absolute, StartMode::Absolute, StartMode::CurrentPosition, StartMode::UserOrigin][rng.pick(4)];
    st.anchor = [
        JobAnchor::TopLeft,
        JobAnchor::Top,
        JobAnchor::TopRight,
        JobAnchor::Left,
        JobAnchor::Center,
        JobAnchor::Right,
        JobAnchor::BottomLeft,
        JobAnchor::Bottom,
        JobAnchor::BottomRight,
    ][rng.pick(9)];
    st.finish = if rng.chance(0.3) { FinishPosition::StartPosition } else { FinishPosition::Stay };
    let p = &mut st.planner;
    p.optimize = rng.chance(0.85);
    p.inner_first = rng.chance(0.7);
    p.reduce_travel = rng.chance(0.8);
    p.best_start = rng.chance(0.7);
    p.prefer_corners = rng.chance(0.7);
    p.best_direction = rng.chance(0.7);
    p.hide_backlash = rng.chance(0.1);
    p.reduce_direction_changes = rng.chance(0.2);
    let area = inp.device.geometry().work_area();
    inp.head = Some(Point::new(rng.f(area.x0, area.x1), rng.f(area.y0, area.y1)));
    inp.user_origin = Some(Point::new(rng.f(area.x0, area.x1), rng.f(area.y0, area.y1)));
    if rng.chance(0.25) {
        inp.settings.cut_selected = true;
        inp.selection = ids.iter().copied().filter(|_| rng.chance(0.6)).collect();
        if inp.selection.is_empty() {
            inp.selection.push(ids[0]);
        }
        inp.settings.use_selection_origin = rng.chance(0.5);
    }
    Case { input: inp, shapes, ids, shift }
}

/// Burn length per (cell, IR layer, sub-layer), design coordinates.
fn burn_by_cell(design: &Toolpath, shift: Point) -> BTreeMap<(usize, usize, usize), f64> {
    let mut map = BTreeMap::new();
    let mut pos = design.start;
    let mut layer = (usize::MAX, 0usize);
    for op in &design.ops {
        if let Op::LayerStart { layer: l } = op {
            layer = (design.layers[*l].layer_index, design.layers[*l].sublayer);
        }
        if let Op::Cut { to, power, .. } | Op::Arc { to, power, .. } = op {
            if *power > 0.0 {
                let mid = pos.midpoint(*to);
                let cell = cell_of(mid, shift).unwrap_or(usize::MAX);
                *map.entry((cell, layer.0, layer.1)).or_insert(0.0) += pos.distance(*to);
            }
        }
        if let Some(t) = op.target() {
            pos = t;
        }
    }
    map
}

fn perimeter(s: &Shape) -> f64 {
    s.outline().segments().map(|seg| seg.arclen(1e-4)).sum()
}

/// How many burning moves of sub-layer (`li`, `si`) pass over `p`.
fn burns_over(design: &Toolpath, li: usize, si: usize, p: Point) -> usize {
    let mut pos = design.start;
    let mut on = false;
    let mut n = 0;
    for op in &design.ops {
        if let Op::LayerStart { layer: l } = op {
            on = design.layers[*l].layer_index == li && design.layers[*l].sublayer == si;
        }
        if let Op::Cut { to, power, .. } = op {
            if on && *power > 0.0 {
                let (a, b) = (pos, *to);
                let ab = b - a;
                let len2 = ab.hypot2();
                if len2 > 1e-12 {
                    let t = (p - a).dot(ab) / len2;
                    let foot = a + ab * t.clamp(0.0, 1.0);
                    if foot.distance(p) < 1e-4 && t > -1e-9 && t < 1.0 - 1e-9 {
                        n += 1;
                    }
                }
            }
        }
        if let Some(t) = op.target() {
            pos = t;
        }
    }
    n
}

fn burning(s: &Seg) -> bool {
    matches!(s.kind, 'C' | 'A') && s.power > 0.0
}

/// Every invariant of one built case. Returns a description of the first one broken.
fn check_case(seed: u64) -> Result<(), String> {
    let case = make_case(seed, false);
    let inp = &case.input;
    let st = &inp.settings;
    let b = match build_job_simple(inp) {
        Ok(b) => b,
        Err(e) => return Err(format!("no job: {e}")),
    };

    // (e) The same input gives the same job.
    let again = build_job_simple(inp).map_err(|e| format!("second build: {e}"))?;
    if again.toolpath != b.toolpath {
        return Err("built twice, the two jobs differ".into());
    }

    // (a) Travel moves carry no power, and nothing has a power that is not a number in 0..100.
    for (i, op) in b.toolpath.ops.iter().enumerate() {
        if let Op::Cut { power, speed, to } | Op::Arc { power, speed, to, .. } = op {
            if !(0.0..=100.0).contains(power) || !(*speed > 0.0) || !to.x.is_finite() || !to.y.is_finite() {
                return Err(format!("op {i} is not sane: {op:?}"));
            }
        }
    }

    // The design copy and the machine copy are the same moves.
    let t = b.origin.transform(b.bounds);
    let (ds, ms) = (segs(&b.design), segs(&b.toolpath));
    if ds.len() != ms.len() {
        return Err("design and machine toolpaths have different lengths".into());
    }
    for (d, m) in ds.iter().zip(&ms) {
        if (t * d.to).distance(m.to) > 1e-6 || d.power != m.power {
            return Err(format!("design move {d:?} is not machine move {m:?}"));
        }
    }

    // (b) The work-area report is right: it flags the job exactly when a move leaves the area.
    let area = inp.device.geometry().work_area();
    let base = match b.toolpath.space {
        darkburn_job::CoordSpace::RelativeToStart => inp.head.unwrap().to_vec2(),
        _ => darkburn_core::kurbo::Vec2::ZERO,
    };
    let outside = ms.iter().any(|s| {
        let p = s.to + base;
        p.x < area.x0 - 1e-6 || p.x > area.x1 + 1e-6 || p.y < area.y0 - 1e-6 || p.y > area.y1 + 1e-6
    });
    if outside != b.out_of_bounds() {
        return Err(format!("a move leaves the work area: {outside}, but the job says {}", b.out_of_bounds()));
    }
    if outside && !b.warnings.iter().any(|w| w.contains("leaves the work area")) {
        return Err("out of bounds without a warning".into());
    }
    if st.start_from == StartMode::Absolute {
        let bed = inp.device.area();
        let design_outside = ds.iter().any(|s| s.to.x < -1e-6 || s.to.x > bed.x1 + 1e-6 || s.to.y < -1e-6 || s.to.y > bed.y1 + 1e-6);
        if design_outside != outside {
            return Err(format!("absolute job: design leaves the bed {design_outside}, machine leaves the area {outside}"));
        }
    }

    // (c) Every shape that outputs is burned the number of passes asked for.
    let by_cell = burn_by_cell(&b.design, case.shift);
    let mut expected_cells = Vec::new();
    for (i, pl) in case.shapes.iter().enumerate() {
        let selected = !st.cut_selected || inp.selection.contains(&case.ids[i]);
        let li = pl.shape.layer;
        let layer = inp.doc.layer(li);
        for (si, s) in layer.settings.iter().enumerate() {
            let got = by_cell.get(&(pl.cell, li, si)).copied().unwrap_or(0.0);
            if !selected {
                if got > 0.0 {
                    return Err(format!("shape {i} is not selected but burns {got} mm"));
                }
                continue;
            }
            expected_cells.push((pl.cell, li, si));
            let passes = s.passes as f64;
            let per = perimeter(&pl.shape);
            let plain = !s.tabs.enabled && !s.perforation && s.overcut == 0.0;
            match s.mode {
                Mode::Line if plain => {
                    let want = passes * per;
                    let slack = passes * (0.02 * per + 8.0 * s.kerf + if s.lead_in.enabled { s.lead_in.length + 0.5 } else { 0.0 });
                    if (got - want).abs() > slack {
                        return Err(format!("shape {i} layer {li}.{si} line: burned {got:.3} mm, {passes} passes of {per:.3} mm asked"));
                    }
                    if let (Some(mid), true) = (pl.edge_mid, s.kerf == 0.0) {
                        let n = burns_over(&b.design, li, si, mid);
                        if n != s.passes as usize {
                            return Err(format!("shape {i} layer {li}.{si}: an edge is burned {n} times, {passes} passes asked"));
                        }
                    }
                }
                Mode::Fill => {
                    let area = pl.shape.outline().area().abs();
                    let want = passes * area / s.interval * if s.cross_hatch { 2.0 } else { 1.0 };
                    let slack = passes * (CELL * 2.0 + 0.03 * want / passes) * if s.cross_hatch { 2.0 } else { 1.0 };
                    if (got - want).abs() > slack {
                        return Err(format!("shape {i} layer {li}.{si} fill: burned {got:.1} mm, {want:.1} expected ({passes} passes)"));
                    }
                }
                _ => {
                    if got <= 0.0 {
                        return Err(format!("shape {i} layer {li}.{si} ({:?}) does not burn", s.mode));
                    }
                }
            }
        }
    }
    for (key, len) in &by_cell {
        if !expected_cells.contains(key) && *len > 1e-6 {
            return Err(format!("{len:.3} mm burned where no shape of that layer is: cell/layer/sub {key:?}"));
        }
    }

    // The burn stays inside the bounds the job reports (frames and origins are built on them),
    // lead-ins aside.
    let leads = inp.doc.layers.iter().take(LAYERS).any(|l| l.settings.iter().any(|s| s.lead_in.enabled));
    if !leads {
        for s in ds.iter().filter(|s| burning(s)) {
            let r = b.bounds.inflate(0.02, 0.02);
            if !r.contains(s.to) || !r.contains(s.from) {
                return Err(format!("burn {s:?} is outside the job bounds {:?}", b.bounds));
            }
        }
    }

    // (d) The order the shapes were created in does not change what is burned.
    let rev = make_case(seed, true);
    let rb = build_job_simple(&rev.input).map_err(|e| format!("reversed build: {e}"))?;
    let rev_cells = burn_by_cell(&rb.design, rev.shift);
    if rev_cells.len() != by_cell.len() || rev_cells.iter().zip(&by_cell).any(|((k1, v1), (k2, v2))| k1 != k2 || (v1 - v2).abs() > 1e-6) {
        return Err(format!("creation order changes the burn: {by_cell:?} against {rev_cells:?}"));
    }
    if (rb.bounds.x0 - b.bounds.x0).abs() > 1e-9 || (rb.bounds.y1 - b.bounds.y1).abs() > 1e-9 {
        return Err("creation order changes the job bounds".into());
    }
    Ok(())
}

#[test]
fn generated_jobs_keep_their_invariants() {
    let mut failures = Vec::new();
    for seed in 0..300u64 {
        if let Err(e) = check_case(seed) {
            failures.push(format!("seed {seed}: {e}"));
        }
    }
    assert!(failures.is_empty(), "{} of 300 generated jobs broke an invariant:\n{}", failures.len(), failures.join("\n"));
}

// ----- Frames against what the job burns ---------------------------------------------------------

fn bbox_of(tp: &Toolpath) -> Rect {
    let pts: Vec<Point> = tp.ops.iter().filter_map(|o| o.target()).collect();
    pts.iter().fold(Rect::from_points(pts[0], pts[0]), |r, p| r.union_pt(*p))
}

fn close(a: Rect, b: Rect, tol: f64) -> bool {
    (a.x0 - b.x0).abs() < tol && (a.y0 - b.y0).abs() < tol && (a.x1 - b.x1).abs() < tol && (a.y1 - b.y1).abs() < tol
}

#[test]
fn rubber_band_frame_leaves_out_a_picture_that_is_not_burned() {
    use crate::images::testutil::{add_image, png};
    // A square that is cut, and a picture on a layer that is not in Image mode: the job says
    // the picture is left out, so no frame may go round it.
    let mut doc = crate::job_field_tests::line_doc(20.0);
    doc.layer_mut(1).main_mut().mode = Mode::Line;
    add_image(&mut doc, png(4, 4, |_, _| 0), (4, 4), (20.0, 20.0), (150.0, 150.0), 1);
    let inp = input(doc);
    let job = build_job_simple(&inp).unwrap();
    assert!(job.warnings.iter().any(|w| w.contains("left out")), "{:?}", job.warnings);
    assert!(close(job.bounds, Rect::new(10.0, 10.0, 30.0, 30.0), 1e-6), "{:?}", job.bounds);
    let rect = build_frame(&inp, FrameKind::Rectangle, 50.0, None).unwrap();
    let band = build_frame(&inp, FrameKind::RubberBand, 50.0, None).unwrap();
    assert!(close(bbox_of(&rect), job.bounds, 1e-6), "rectangle frame {:?}", bbox_of(&rect));
    assert!(close(bbox_of(&band), job.bounds, 0.05), "rubber band frame {:?} against the job {:?}", bbox_of(&band), job.bounds);
}
