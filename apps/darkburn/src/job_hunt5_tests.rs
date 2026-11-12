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
        if let Op::Cut { to, power, .. } | Op::Arc { to, power, .. } = op
            && *power > 0.0
        {
            let mid = pos.midpoint(*to);
            let cell = cell_of(mid, shift).unwrap_or(usize::MAX);
            *map.entry((cell, layer.0, layer.1)).or_insert(0.0) += pos.distance(*to);
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
        if let Op::Cut { to, power, .. } = op
            && on
            && *power > 0.0
        {
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
        if let Op::Cut { power, speed, to } | Op::Arc { power, speed, to, .. } = op
            && (!(0.0..=100.0).contains(power) || !(*speed > 0.0) || !to.x.is_finite() || !to.y.is_finite())
        {
            return Err(format!("op {i} is not sane: {op:?}"));
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

#[test]
fn tool_layer_frame_follows_the_print_and_cut_alignment() {
    use darkburn_core::layer::T1;
    // The job is moved 20 mm to the right by a print-and-cut alignment. A tool layer shape that
    // is framed with the job moves with it.
    let mut doc = crate::job_field_tests::line_doc(20.0);
    doc.layer_mut(T1).frame = true;
    doc.add(Shape::rect(Rect::new(50.0, 50.0, 60.0, 60.0), 0.0, T1));
    let plain = input(doc.clone());
    let frame = build_frame(&plain, FrameKind::Rectangle, 50.0, None).unwrap();
    assert!(close(bbox_of(&frame), Rect::new(10.0, 10.0, 60.0, 60.0), 1e-6), "{:?}", bbox_of(&frame));

    let d = [Point::new(0.0, 0.0), Point::new(100.0, 0.0), Point::new(0.0, 100.0)];
    let m: Vec<Point> = d.iter().map(|p| *p + darkburn_core::kurbo::Vec2::new(20.0, 0.0)).collect();
    let mut vs = crate::vision_state::VisionState::default();
    vs.pnc.fit = Some(darkburn_vision::printcut::fit_registration(&d, &m, darkburn_vision::printcut::PnCMode::Scaled).unwrap());
    doc.vision_settings = Some(vs.to_value());
    let moved = input(doc);
    let job = build_job_simple(&moved).unwrap();
    assert!(close(job.bounds, Rect::new(30.0, 10.0, 50.0, 30.0), 1e-6), "{:?}", job.bounds);
    for kind in [FrameKind::Rectangle, FrameKind::RubberBand] {
        let frame = build_frame(&moved, kind, 50.0, None).unwrap();
        assert!(close(bbox_of(&frame), Rect::new(30.0, 10.0, 80.0, 60.0), 0.05), "{kind:?}: {:?}", bbox_of(&frame));
    }
}

// ----- The estimate and the G-code against the planned job --------------------------------------

/// What a G-code program does, read the way the controller reads it: the length burned, the
/// length moved with the laser off, and the rapid moves made with the laser still on.
#[derive(Debug, Default)]
struct Ran {
    burn: f64,
    off: f64,
    hot_rapids: usize,
    end: Point,
}

fn run_gcode(text: &str, dev: &DeviceGcodeSettings, start: Point) -> Ran {
    use darkburn_job::gcode::{Flavor, MarlinPower};
    let mut r = Ran::default();
    let (mut pos, mut relative, mut motion) = (start, false, 0u32);
    let (mut s, mut spindle, mut fan, mut q) = (0.0f64, false, 0.0f64, 0.0f64);
    for line in text.lines() {
        let line = line.split(';').next().unwrap_or("");
        let line = if line.trim_start().starts_with('(') { "" } else { line };
        let mut words: Vec<(char, f64)> = Vec::new();
        let chars: Vec<char> = line.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i].to_ascii_uppercase();
            i += 1;
            if !c.is_ascii_alphabetic() {
                continue;
            }
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_digit() || matches!(chars[i], '.' | '-' | '+')) {
                i += 1;
            }
            if let Ok(v) = chars[start..i].iter().collect::<String>().parse::<f64>() {
                words.push((c, v));
            }
        }
        let get = |k: char| words.iter().find(|w| w.0 == k).map(|w| w.1);
        let m = get('M').map(|v| v as i64);
        match m {
            Some(3) | Some(4) => spindle = true,
            Some(5) => spindle = false,
            Some(106) => fan = get('S').unwrap_or(255.0),
            Some(107) => fan = 0.0,
            Some(67) | Some(68) => q = get('Q').unwrap_or(0.0),
            _ => {}
        }
        if m != Some(106)
            && let Some(v) = get('S')
        {
            s = v;
        }
        for w in &words {
            if w.0 == 'G' {
                match w.1 as i64 {
                    0..=3 => motion = w.1 as u32,
                    90 => relative = false,
                    91 => relative = true,
                    _ => {}
                }
            }
        }
        let (x, y) = (get('X'), get('Y'));
        if (x.is_none() && y.is_none()) || m.is_some() || get('G').is_some_and(|g| g as i64 > 3) {
            continue;
        }
        let to = if relative {
            Point::new(pos.x + x.unwrap_or(0.0), pos.y + y.unwrap_or(0.0))
        } else {
            Point::new(x.unwrap_or(pos.x), y.unwrap_or(pos.y))
        };
        let len = pos.distance(to);
        let on = match (dev.flavor, dev.marlin_power) {
            (Flavor::LinuxCnc, _) => q > 0.0,
            (Flavor::Marlin, MarlinPower::M106) => fan > 0.0,
            (Flavor::Smoothieware, _) => s > 0.0,
            _ => spindle && s > 0.0,
        };
        // GRBL (laser mode), Smoothieware and Marlin's inline power switch the laser off in a
        // rapid themselves; the others burn whenever the output is on.
        let firmware_blanks =
            !matches!((dev.flavor, dev.marlin_power), (Flavor::LinuxCnc, _) | (Flavor::Marlin, MarlinPower::M3 | MarlinPower::M106));
        if motion == 0 {
            if on && !firmware_blanks && len > 1e-9 {
                r.hot_rapids += 1;
            }
            r.off += len;
        } else if on {
            r.burn += len;
        } else {
            r.off += len;
        }
        pos = to;
    }
    r.end = pos;
    r
}

fn check_outputs(seed: u64) -> Result<(), String> {
    use darkburn_job::gcode::MarlinPower;
    let case = make_case(seed, false);
    let b = build_job_simple(&case.input).map_err(|e| format!("no job: {e}"))?;
    let ms = segs(&b.toolpath);
    if ms.iter().any(|s| s.kind == 'A') {
        return Ok(());
    }
    let burn: f64 = ms.iter().filter(|s| burning(s)).map(|s| s.from.distance(s.to)).sum();
    let all: f64 = ms.iter().map(|s| s.from.distance(s.to)).sum();

    // The estimate walks the same moves.
    let sim = b.simulate(&case.input.motion);
    if (sim.cut_distance - burn).abs() > 1e-6 * (1.0 + burn) {
        return Err(format!("estimate cuts {} mm, the job {burn} mm", sim.cut_distance));
    }
    if (sim.cut_distance + sim.scan_distance + sim.rapid_distance - all).abs() > 1e-6 * (1.0 + all) {
        return Err("estimate and job move different distances".into());
    }
    let mut t = 0.0;
    let mut slowest = 0.0;
    for s in &sim.segments {
        if (s.t0 - t).abs() > 1e-6 || !(s.duration >= 0.0) {
            return Err(format!("estimate has a gap or a negative time at op {}", s.op_index));
        }
        t = s.t0 + s.duration;
        if let Some(Op::Cut { speed, .. }) = b.design.ops.get(s.op_index) {
            slowest += s.length / speed.max(1e-9);
            if s.duration < s.length / speed.max(1e-9) - 1e-6 && *speed <= case.input.motion.motion().max_speed {
                return Err(format!("op {} is estimated faster than its speed allows", s.op_index));
            }
        }
    }
    if (sim.total_time - t).abs() > 1e-6 || sim.total_time < slowest - 1e-3 {
        return Err("estimate total is not the end of its last move".into());
    }

    // The G-code burns what the job burns, for every flavour.
    for flavor in 0..6 {
        for mp in [MarlinPower::Inline, MarlinPower::M3, MarlinPower::M106] {
            if flavor != 3 && mp != MarlinPower::Inline {
                continue;
            }
            let mut dev = DeviceGcodeSettings::default();
            dev.set_flavor_index(flavor);
            dev.marlin_power = mp;
            dev.overscan_as_g0 = seed.is_multiple_of(3);
            dev.s_every_g1 = seed.is_multiple_of(5);
            let text = gcode_text(&b, None, &dev);
            let ran = run_gcode(&text, &dev, b.toolpath.start);
            let name = DeviceGcodeSettings::flavor_names()[flavor];
            if ran.hot_rapids > 0 {
                return Err(format!("{name} {mp:?}: {} rapid moves with the laser on", ran.hot_rapids));
            }
            let slack = 0.002 * ms.len() as f64 + 0.001 * burn;
            if (ran.burn - burn).abs() > slack {
                return Err(format!("{name} {mp:?}: the G-code burns {:.3} mm, the job {burn:.3} mm", ran.burn));
            }
            if (ran.burn + ran.off - all).abs() > 0.002 * ms.len() as f64 + 0.001 * all {
                return Err(format!("{name} {mp:?}: the G-code moves {:.3} mm, the job {all:.3} mm", ran.burn + ran.off));
            }
        }
    }
    Ok(())
}

#[test]
fn estimate_and_gcode_do_what_the_planned_job_does() {
    let mut failures = Vec::new();
    for seed in 0..120u64 {
        if let Err(e) = check_outputs(seed) {
            failures.push(format!("seed {seed}: {e}"));
        }
    }
    assert!(failures.is_empty(), "{} of 120 generated jobs:\n{}", failures.len(), failures.join("\n"));
}

// ----- Framing on the simulators -----------------------------------------------------------------

mod on_machine {
    use std::time::{Duration, Instant};

    use serde_json::{Value, json};

    use crate::api::execute_headless;
    use crate::devices::DeviceType;
    use crate::machine_api::LaserState;
    use crate::prefs::Prefs;
    use crate::shell::DarkBurn;

    fn run(app: &mut DarkBurn, id: &str, args: Value) -> Result<Value, String> {
        execute_headless(app, id, &args)
    }

    fn wait(app: &mut DarkBurn, what: &str, mut f: impl FnMut(&DarkBurn) -> bool) {
        let end = Instant::now() + Duration::from_secs(20);
        loop {
            app.machine.tick();
            if f(app) {
                return;
            }
            let s = app.machine.handle.snapshot();
            assert!(Instant::now() < end, "timed out waiting for {what}; state {:?}, head {:?}", s.state, s.machine_pos);
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn ready(app: &DarkBurn) -> bool {
        app.machine.handle.snapshot().state == LaserState::Ready
    }

    fn head(app: &DarkBurn) -> (f64, f64) {
        let s = app.machine.handle.snapshot();
        (s.machine_pos[0], s.machine_pos[1])
    }

    /// Frame a job that starts from the current position, anchored at its centre, and say where
    /// the head rests afterwards.
    fn frame_from_the_centre(kind: DeviceType, command: &str) -> (f64, f64) {
        let mut a = DarkBurn::new(Prefs::default());
        let id = a.machine.devices.devices.iter().find(|d| d.kind == kind).unwrap().id;
        run(&mut a, "device.select", json!({ "id": id })).unwrap();
        run(&mut a, "machine.connect", json!({})).unwrap();
        wait(&mut a, "connected", ready);
        run(&mut a, "shape.add_rect", json!({ "x": 10, "y": 10, "width": 20, "height": 20, "layer": 0 })).unwrap();
        run(&mut a, "job.set_origin", json!({ "start_from": "current_position", "anchor": "centre" })).unwrap();
        run(&mut a, "machine.move_to", json!({ "x": 100, "y": 100, "machine_coords": true })).unwrap();
        wait(&mut a, "the move to the start", |a| ready(a) && (head(a).0 - 100.0).abs() < 1e-3 && (head(a).1 - 100.0).abs() < 1e-3);
        let sent = run(&mut a, command, Value::Null).unwrap();
        assert!(sent["points"].as_u64().unwrap() >= 5, "the frame goes round the square: {sent}");
        // Let the frame get going (the Ruida simulator moves at once, GRBL takes its time).
        let going = Instant::now() + Duration::from_millis(400);
        while Instant::now() < going {
            a.machine.tick();
            std::thread::sleep(Duration::from_millis(5));
        }
        wait(&mut a, "the frame to end", ready);
        // Let a last position report in.
        let end = Instant::now() + Duration::from_millis(600);
        while Instant::now() < end {
            a.machine.tick();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(ready(&a));
        head(&a)
    }

    /// The job starts where the head is. A frame that leaves the head on a corner of the frame
    /// makes the job that follows burn somewhere else than what was framed.
    #[test]
    fn frame_from_the_current_position_brings_the_head_back_grbl() {
        for command in ["job.frame", "job.frame_rubber"] {
            let (x, y) = frame_from_the_centre(DeviceType::SimulatorGrbl, command);
            assert!((x - 100.0).abs() < 0.01 && (y - 100.0).abs() < 0.01, "{command}: the head rests at {x:.2}, {y:.2}, it started at 100, 100");
        }
    }

    #[test]
    fn frame_from_the_current_position_brings_the_head_back_ruida() {
        for command in ["job.frame", "job.frame_rubber"] {
            let (x, y) = frame_from_the_centre(DeviceType::SimulatorRuida, command);
            assert!((x - 100.0).abs() < 0.01 && (y - 100.0).abs() < 0.01, "{command}: the head rests at {x:.2}, {y:.2}, it started at 100, 100");
        }
    }

    /// A connected simulator of `kind` with the job set to start from the current position and
    /// the head at 100, 100.
    fn at_100_from_the_current_position(kind: DeviceType) -> DarkBurn {
        let mut a = DarkBurn::new(Prefs::default());
        let id = run(&mut a, "device.add", json!({ "type": format!("{kind:?}") })).unwrap()["id"].as_u64().unwrap();
        run(&mut a, "device.select", json!({ "id": id })).unwrap();
        run(&mut a, "machine.connect", json!({})).unwrap();
        wait(&mut a, "connected", ready);
        run(&mut a, "job.set_origin", json!({ "start_from": "current_position", "anchor": "centre" })).unwrap();
        run(&mut a, "machine.move_to", json!({ "x": 100, "y": 100, "machine_coords": kind != DeviceType::SimulatorMarlin })).unwrap();
        wait(&mut a, "the move to the start", |a| ready(a) && (head(a).0 - 100.0).abs() < 1e-3 && (head(a).1 - 100.0).abs() < 1e-3);
        a
    }

    /// Wait for a frame that was just sent to end, and say where the head rests.
    fn rest_after_the_frame(a: &mut DarkBurn) -> (f64, f64) {
        let going = Instant::now() + Duration::from_millis(400);
        while Instant::now() < going {
            a.machine.tick();
            std::thread::sleep(Duration::from_millis(5));
        }
        let end = Instant::now() + Duration::from_secs(20);
        while !ready(a) {
            a.machine.tick();
            assert!(Instant::now() < end, "the frame did not end");
            std::thread::sleep(Duration::from_millis(5));
        }
        let end = Instant::now() + Duration::from_millis(600);
        while Instant::now() < end {
            a.machine.tick();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(ready(a));
        head(a)
    }

    const FRAME_KINDS: [DeviceType; 3] = [DeviceType::SimulatorGrbl, DeviceType::SimulatorRuida, DeviceType::SimulatorMarlin];

    /// The material test is burned from the current position like any job: its frame must leave
    /// the head where it was.
    #[test]
    fn the_material_test_frame_from_the_current_position_brings_the_head_back() {
        let mut wrong = Vec::new();
        for kind in FRAME_KINDS {
            let mut a = at_100_from_the_current_position(kind);
            crate::material_test::frame_test(&mut a).unwrap();
            let (x, y) = rest_after_the_frame(&mut a);
            if (x - 100.0).abs() > 0.01 || (y - 100.0).abs() > 0.01 {
                wrong.push(format!("{kind:?}: the head rests at {x:.2}, {y:.2}, it started at 100, 100"));
            }
        }
        assert!(wrong.is_empty(), "{wrong:#?}");
    }

    /// `machine.frame` goes round a rectangle given by numbers. With the job set to start from
    /// the current position the head must rest where it was, or the job burns from a corner of
    /// that rectangle.
    #[test]
    fn machine_frame_by_numbers_from_the_current_position_brings_the_head_back() {
        let mut wrong = Vec::new();
        for kind in FRAME_KINDS {
            let mut a = at_100_from_the_current_position(kind);
            run(&mut a, "machine.frame", json!({ "x": 20, "y": 30, "width": 40, "height": 25 })).unwrap();
            let (x, y) = rest_after_the_frame(&mut a);
            if (x - 100.0).abs() > 0.01 || (y - 100.0).abs() > 0.01 {
                wrong.push(format!("{kind:?}: the head rests at {x:.2}, {y:.2}, it started at 100, 100"));
            }
        }
        assert!(wrong.is_empty(), "{wrong:#?}");
    }

    /// With an absolute job the frame by numbers ends on its first corner, as before.
    #[test]
    fn machine_frame_by_numbers_with_an_absolute_job_stays_on_the_first_corner() {
        for kind in FRAME_KINDS {
            let mut a = at_100_from_the_current_position(kind);
            run(&mut a, "job.set_origin", json!({ "start_from": "absolute" })).unwrap();
            run(&mut a, "machine.frame", json!({ "x": 20, "y": 30, "width": 40, "height": 25 })).unwrap();
            let (x, y) = rest_after_the_frame(&mut a);
            assert!((x - 20.0).abs() < 0.01 && (y - 30.0).abs() < 0.01, "{kind:?}: the head rests at {x:.2}, {y:.2}");
        }
    }
}
