//! Toolpaths written as EGV and read back by the emulator: vectors, rasters, every board and
//! many speeds, directly and through packets sent to the in-memory board.

use std::collections::BTreeSet;

use darkburn_job::fill::{RowEmit, emit_rows, order_rows, scan_polygons};
use darkburn_job::{CoordSpace, CutMode, FillRule, IrLayer, Op, Toolpath};
use darkburn_lihuiyu::egv::octant_runs;
use darkburn_lihuiyu::toolpath::{K40Settings, mils_to_mm, min_speed, mm_to_mils, toolpath_to_egv};
use darkburn_lihuiyu::{Board, EgvReader, MemoryTransport, SendOptions, egv_body, send, write_egv};
use kurbo::{Affine, Point};

fn layer(raster: bool, speed: f64) -> IrLayer {
    IrLayer {
        layer_index: 0,
        sublayer: 0,
        name: "test".into(),
        mode: if raster { CutMode::Fill } else { CutMode::Line },
        speed,
        max_power: 40.0,
        min_power: 10.0,
        passes: 1,
        constant_power: false,
        air: false,
        laser: 0,
        raster,
        dsp: Default::default(),
        dot: None,
    }
}

fn tp(raster: bool, speed: f64, ops: Vec<Op>) -> Toolpath {
    let mut all = vec![Op::LayerStart { layer: 0 }, Op::PassStart { pass: 1 }];
    all.extend(ops);
    Toolpath { space: CoordSpace::MachineAbsolute, layers: vec![layer(raster, speed)], ops: all, start: Point::ZERO }
}

/// A small deterministic random source.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Step {
    /// One whole travel move.
    Travel((i64, i64)),
    /// One mil (or one diagonal mil) with the laser on or off.
    Unit((i64, i64), bool),
}

/// The path the reader saw, travel moves whole and program moves in single steps.
fn read_steps(r: &EgvReader) -> Vec<Step> {
    let mut out = Vec::new();
    for m in &r.moves {
        if !m.program {
            out.push(Step::Travel(m.to));
            continue;
        }
        let (dx, dy) = (m.to.0 - m.from.0, m.to.1 - m.from.1);
        assert!(dx == 0 || dy == 0 || dx.abs() == dy.abs(), "program move {m:?} is not a straight or 45 degree move");
        let n = dx.abs().max(dy.abs());
        for i in 1..=n {
            out.push(Step::Unit((m.from.0 + dx.signum() * i, m.from.1 + dy.signum() * i), m.laser));
        }
    }
    out
}

/// The path a vector toolpath asks for, stepped the way a line is stepped.
fn expected_steps(t: &Toolpath) -> Vec<Step> {
    let mut out = Vec::new();
    let mut pos = (mm_to_mils(t.start.x), mm_to_mils(t.start.y));
    for op in &t.ops {
        let (to, on) = match op {
            Op::Rapid { to } => {
                let to = (mm_to_mils(to.x), mm_to_mils(to.y));
                if to != pos {
                    out.push(Step::Travel(to));
                }
                pos = to;
                continue;
            }
            Op::Cut { to, power, .. } => (*to, *power > 0.0),
            Op::Overscan { to, .. } => (*to, false),
            _ => continue,
        };
        let to = (mm_to_mils(to.x), mm_to_mils(to.y));
        if to == pos {
            continue;
        }
        let mut last = (0, 0);
        for run in octant_runs(to.0 - pos.0, to.1 - pos.1) {
            let (dx, dy) = (run.0 - last.0, run.1 - last.1);
            for i in 1..=dx.abs().max(dy.abs()) {
                out.push(Step::Unit((pos.0 + last.0 + dx.signum() * i, pos.1 + last.1 + dy.signum() * i), on));
            }
            last = run;
        }
        pos = to;
    }
    out
}

/// Burned cells of horizontal burns (half-open in x), from the reader.
fn read_cells(r: &EgvReader) -> BTreeSet<(i64, i64)> {
    let mut cells = BTreeSet::new();
    for m in r.moves.iter().filter(|m| m.burns()) {
        assert_eq!(m.from.1, m.to.1, "raster burn {m:?} is not horizontal");
        for x in m.from.0.min(m.to.0)..m.from.0.max(m.to.0) {
            cells.insert((x, m.from.1));
        }
    }
    cells
}

/// Burned cells a raster toolpath asks for.
fn expected_cells(t: &Toolpath) -> BTreeSet<(i64, i64)> {
    let mut cells = BTreeSet::new();
    let mut pos = (mm_to_mils(t.start.x), mm_to_mils(t.start.y));
    for op in &t.ops {
        let Some(to) = op.target() else { continue };
        let to = (mm_to_mils(to.x), mm_to_mils(to.y));
        if let Op::Cut { power, .. } = op
            && *power > 0.0
        {
            assert_eq!(pos.1, to.1);
            for x in pos.0.min(to.0)..pos.0.max(to.0) {
                cells.insert((x, to.1));
            }
        }
        pos = to;
    }
    cells
}

fn polygon_ops(rng: &mut Lcg, speed: f64) -> Vec<Op> {
    let mut ops = Vec::new();
    for _ in 0..6 {
        let c = Point::new(20.0 + rng.next() * 260.0, 20.0 + rng.next() * 170.0);
        ops.push(Op::Rapid { to: c });
        let n = 3 + (rng.next() * 6.0) as usize;
        for _ in 0..n {
            let p = Point::new(c.x + (rng.next() - 0.5) * 30.0, c.y + (rng.next() - 0.5) * 30.0);
            let power = if rng.next() < 0.2 { 0.0 } else { 50.0 };
            ops.push(Op::Cut { to: p, speed, power });
        }
        ops.push(Op::Cut { to: c, speed, power: 50.0 });
    }
    ops
}

fn read(board: Board, text: &str) -> EgvReader {
    EgvReader::read(board, text)
}

#[test]
fn vectors_round_trip_on_every_board() {
    let mut rng = Lcg(7);
    for board in Board::ALL {
        for speed in [1.0, 5.0, 10.0, 15.0, 25.0, 40.0, 80.0, 140.0] {
            let t = tp(false, speed, polygon_ops(&mut rng, speed));
            let s = K40Settings { board, ..Default::default() };
            let job = toolpath_to_egv(&t, &s).unwrap();
            assert!(job.warnings.is_empty(), "{board} {speed}: {:?}", job.warnings);
            let r = read(board, &job.text);
            assert_eq!(read_steps(&r), expected_steps(&t), "{board} {speed}");
            assert_eq!(r.position(), job.end);
            assert!(r.is_finished() || !r.in_program());
            assert_eq!(r.speeds.len(), job.program_blocks);
            for seen in &r.speeds {
                let got = seen.speed.unwrap_or_else(|| panic!("{board}: {} did not read", seen.code));
                assert!((got - speed).abs() <= speed / 100.0, "{board} {speed}: {} reads as {got}", seen.code);
            }
        }
    }
}

#[test]
fn diagonal_and_steep_lines_step_within_a_mil() {
    for (dx, dy) in [(1000, 3), (3, 1000), (-777, 500), (250, -251), (-1, -999), (640, 640)] {
        let mut last = (0i64, 0i64);
        let mut steps = vec![(0i64, 0i64)];
        for run in octant_runs(dx, dy) {
            let (a, b) = (run.0 - last.0, run.1 - last.1);
            for i in 1..=a.abs().max(b.abs()) {
                steps.push((last.0 + a.signum() * i, last.1 + b.signum() * i));
            }
            last = run;
        }
        let len = ((dx * dx + dy * dy) as f64).sqrt();
        for (x, y) in steps {
            let off = ((x * dy - y * dx) as f64).abs() / len;
            assert!(off <= 1.0, "{dx},{dy}: {x},{y} is {off} mils off the line");
        }
    }
}

#[test]
fn arcs_round_trip_as_lines() {
    let c = Point::new(100.0, 100.0);
    let ops = vec![
        Op::Rapid { to: Point::new(120.0, 100.0) },
        Op::Arc { to: Point::new(100.0, 120.0), center: c, clockwise: false, speed: 20.0, power: 30.0 },
        Op::Arc { to: Point::new(100.0, 80.0), center: c, clockwise: false, speed: 20.0, power: 30.0 },
    ];
    let t = tp(false, 20.0, ops);
    let job = toolpath_to_egv(&t, &K40Settings::default()).unwrap();
    let r = read(Board::M2, &job.text);
    assert_eq!(r.position(), (mm_to_mils(100.0), mm_to_mils(80.0)));
    for m in r.moves.iter().filter(|m| m.burns()) {
        for p in [m.from, m.to] {
            let d = Point::new(mils_to_mm(p.0), mils_to_mm(p.1)).distance(c);
            assert!((d - 20.0).abs() < 0.06, "{p:?} is {d} mm from the centre");
        }
    }
    // Three quarters of a circle of radius 20 mm, in mils.
    let want = 0.75 * std::f64::consts::TAU * 20.0 / 0.0254;
    assert!((r.burn_length() / want - 1.0).abs() < 0.08, "{} vs {want}", r.burn_length());
}

fn fill_ops(interval: f64, overscan: f64, bidirectional: bool, angle: f64, speed: f64, y_flip: bool) -> Vec<Op> {
    let star: Vec<Point> = (0..10)
        .map(|i| {
            let a = i as f64 * std::f64::consts::TAU / 10.0;
            let r = if i % 2 == 0 { 15.0 } else { 6.0 };
            Point::new(r * a.cos(), r * a.sin())
        })
        .collect();
    let hole: Vec<Point> = vec![Point::new(-2.0, -2.0), Point::new(2.0, -2.0), Point::new(2.0, 2.0), Point::new(-2.0, 2.0)];
    let rows = scan_polygons(&[&star, &hole], FillRule::EvenOdd, interval, angle, 60.0);
    let mut rows = order_rows(rows, bidirectional, false, interval);
    if y_flip {
        rows.reverse();
        for (i, r) in rows.iter_mut().enumerate() {
            r.1 = !bidirectional || i % 2 == 0;
        }
    }
    let frame = Affine::translate((60.0, 50.0)) * Affine::rotate(angle);
    let e = RowEmit { frame, speed, overscan, rapid_gap: 1.0, ramp: (0.0, 0.0) };
    let mut ops = Vec::new();
    emit_rows(&rows, &e, &mut ops);
    ops
}

#[test]
fn rasters_round_trip_in_raster_mode() {
    for board in Board::ALL {
        for (interval, overscan, speed, flip) in
            [(0.0762, 2.0, 100.0, false), (0.1, 1.0, 300.0, false), (0.1, 0.0, 15.0, true), (0.25, 3.0, 700.0, true), (0.05, 0.5, 50.0, false)]
        {
            let t = tp(true, speed, fill_ops(interval, overscan, true, 0.0, speed, flip));
            let s = K40Settings { board, ..Default::default() };
            let job = toolpath_to_egv(&t, &s).unwrap();
            assert_eq!(job.raster_blocks, 1, "{board} {interval} {speed}");
            assert_eq!(job.program_blocks, 0);
            let r = read(board, &job.text);
            let cells = expected_cells(&t);
            assert!(cells.len() > 1000);
            assert_eq!(read_cells(&r), cells, "{board} {interval} {speed}");
            assert_eq!(r.position(), job.end);
            assert_eq!(r.speeds.len(), 1);
            let step = mm_to_mils(interval).max(1) as u32;
            let got = r.speeds[0].raster_step.unwrap();
            assert!(matches!(got, darkburn_lihuiyu::speed::RasterStep::Bidirectional(g) if g == step || g + 1 == step), "{interval}: {got:?}");
            let read_speed = r.speeds[0].speed.unwrap();
            assert!((read_speed - speed).abs() <= speed / 50.0, "{board} {speed}: {} reads as {read_speed}", r.speeds[0].code);
        }
    }
}

#[test]
fn rows_the_raster_mode_cannot_follow_fall_back_to_lines() {
    // Rotated rows and one-way rows.
    for (angle, bidirectional) in [(0.5, true), (0.0, false)] {
        let t = tp(true, 60.0, fill_ops(0.2, 1.0, bidirectional, angle, 60.0, false));
        let job = toolpath_to_egv(&t, &K40Settings::default()).unwrap();
        assert_eq!(job.raster_blocks, 0);
        assert!(job.program_blocks >= 1);
        let r = read(Board::M2, &job.text);
        assert_eq!(read_steps(&r), expected_steps(&t), "{angle} {bidirectional}");
    }
    // Raster mode switched off.
    let t = tp(true, 60.0, fill_ops(0.2, 1.0, true, 0.0, 60.0, false));
    let job = toolpath_to_egv(&t, &K40Settings { raster: false, ..Default::default() }).unwrap();
    assert_eq!(job.raster_blocks, 0);
    assert_eq!(read_steps(&read(Board::M2, &job.text)), expected_steps(&t));
}

#[test]
fn slow_rasters_are_raised_to_the_slowest_raster_speed() {
    let t = tp(true, 2.0, fill_ops(0.1, 0.0, true, 0.0, 2.0, false));
    let job = toolpath_to_egv(&t, &K40Settings::default()).unwrap();
    assert_eq!(job.warnings.len(), 1, "{:?}", job.warnings);
    let r = read(Board::M2, &job.text);
    let got = r.speeds[0].speed.unwrap();
    let min = min_speed(Board::M2, true, false);
    assert!(got >= min * 0.99, "{got} < {min}");
}

#[test]
fn mixed_jobs_send_through_packets_like_they_read() {
    let mut ops = fill_ops(0.1, 1.0, true, 0.0, 200.0, false);
    ops.extend(polygon_ops(&mut Lcg(3), 20.0));
    let mut t = tp(true, 200.0, Vec::new());
    t.layers.push(layer(false, 20.0));
    let split = ops.iter().position(|o| matches!(o, Op::Cut { speed, .. } if *speed == 20.0)).unwrap() - 1;
    let (raster_ops, vector_ops) = ops.split_at(split);
    t.ops.extend(raster_ops.iter().cloned());
    t.ops.push(Op::LayerStart { layer: 1 });
    t.ops.extend(vector_ops.iter().cloned());
    let job = toolpath_to_egv(&t, &K40Settings::default()).unwrap();
    assert_eq!(job.raster_blocks, 1);
    assert!(job.program_blocks >= 1);
    let direct = read(Board::M2, &job.text);

    let mut link = MemoryTransport::new(Board::M2);
    let report = send(&mut link, job.text.as_bytes(), &SendOptions::default()).unwrap();
    assert_eq!(report.unsent, 0);
    assert!(report.finish_waits >= 2);
    let board = link.board.lock().unwrap();
    assert_eq!(board.reader.moves, direct.moves);
    assert_eq!(board.rejected, 0);

    // And through a .egv file.
    let file = write_egv(&job.text);
    let again = read(Board::M2, std::str::from_utf8(egv_body(file.as_bytes())).unwrap());
    assert_eq!(again.moves, direct.moves);
}

#[test]
fn several_passes_each_raster_on_their_own() {
    let rows = fill_ops(0.1, 1.0, true, 0.0, 100.0, false);
    let mut t = tp(true, 100.0, rows.clone());
    t.ops.push(Op::PassStart { pass: 2 });
    t.ops.extend(rows);
    let job = toolpath_to_egv(&t, &K40Settings::default()).unwrap();
    assert_eq!(job.raster_blocks, 2);
    let r = read(Board::M2, &job.text);
    assert_eq!(read_cells(&r), expected_cells(&t));
}
