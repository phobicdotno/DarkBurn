//! Turning a DarkBurn toolpath into EGV.
//!
//! The toolpath must be in machine coordinates with the origin in the back left corner (the
//! K40 home): `+x` right, `+y` towards the front, millimetres. That is
//! [`CoordSpace::MachineAbsolute`] with a top-left machine origin, or
//! [`CoordSpace::RelativeToStart`] (the job then runs from wherever the head is).
//!
//! The stock boards switch the laser only on and off; any power above zero is "on". The tube
//! power is set by hand on the machine's panel.
//!
//! * Travel (`Rapid`) becomes a rapid move.
//! * Cuts, overscans and arcs become program (vector) blocks. A block lasts while the speed
//!   stays the same; each block is left through rapid mode, so the head comes to a stop
//!   between blocks.
//! * A raster layer whose rows are horizontal, alternate in direction and step by about the
//!   same distance becomes one raster block per pass, where the board steps between rows by
//!   itself. Anything else (rotated rows, one-way rows, uneven steps) is written as program
//!   blocks instead, which is slower but burns the same lines.
//! * Dwells that fire, Z moves, air and second-laser switches have no EGV form; they are
//!   skipped with a warning.

use std::fmt;

use darkburn_job::{CoordSpace, Op, Toolpath};
use kurbo::Point;

use crate::egv::{EgvWriter, MAX_RASTER_CORRECTION, Mode, WriterError, WriterOptions};
use crate::speed::{Board, equation};

/// Millimetres per mil.
pub const MM_PER_MIL: f64 = 0.0254;

/// Millimetres to the nearest mil.
pub fn mm_to_mils(mm: f64) -> i64 {
    (mm / MM_PER_MIL).round() as i64
}

/// Mils to millimetres.
pub fn mils_to_mm(mils: i64) -> f64 {
    mils as f64 * MM_PER_MIL
}

/// The slowest speed (mm/s) a speed code can hold for this board and mode. Slower requests are
/// written at this speed.
pub fn min_speed(board: Board, raster: bool, fix_speeds: bool) -> f64 {
    // Low speeds always use acceleration 1; vector blocks use the suffix-C form.
    let (b, m) = equation(board, 1, !raster, fix_speeds);
    25.4 * m / (65536.0 - b)
}

/// Settings for one K40 machine.
#[derive(Clone, Debug, PartialEq)]
pub struct K40Settings {
    pub board: Board,
    /// Keep the motors on after a travel move.
    pub autolock: bool,
    /// Correct speed codes so real M2 speeds match (measured on one machine; off by default).
    pub fix_speeds: bool,
    /// Bed size in mm (width, depth); absolute jobs outside it are refused. `None` skips the
    /// check.
    pub bed_mm: Option<(f64, f64)>,
    /// Fastest vector speed, mm/s; faster requests are slowed down to it.
    pub max_vector_speed: f64,
    /// Fastest raster speed, mm/s.
    pub max_raster_speed: f64,
    /// Use raster mode for raster layers where the rows allow it.
    pub raster: bool,
    /// Largest raster row step (mils) raster mode is used for.
    pub max_raster_step: u32,
    /// Largest distance (mm) a flattened arc may stray from the true arc.
    pub arc_tolerance_mm: f64,
}

impl Default for K40Settings {
    fn default() -> Self {
        K40Settings {
            board: Board::M2,
            autolock: true,
            fix_speeds: false,
            bed_mm: Some((310.0, 210.0)),
            max_vector_speed: 140.0,
            max_raster_speed: 750.0,
            raster: true,
            max_raster_step: 100,
            arc_tolerance_mm: 0.01,
        }
    }
}

/// Why a toolpath could not be written.
#[derive(Clone, Debug, PartialEq)]
pub enum ConvertError {
    /// The toolpath is still in design coordinates.
    DesignCoordinates,
    /// A point (mm) lies outside the bed.
    OutsideBed { x: f64, y: f64 },
    /// A point that is not a number.
    NotANumber,
    /// The writer refused a step (a bug in the conversion).
    Writer(WriterError),
}

impl fmt::Display for ConvertError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConvertError::DesignCoordinates => f.write_str("the toolpath is in design coordinates; place it on the machine first"),
            ConvertError::OutsideBed { x, y } => write!(f, "the job reaches {x:.2}, {y:.2} mm, outside the bed"),
            ConvertError::NotANumber => f.write_str("the toolpath has a point that is not a number"),
            ConvertError::Writer(e) => write!(f, "EGV writer: {e}"),
        }
    }
}

impl std::error::Error for ConvertError {}

impl From<WriterError> for ConvertError {
    fn from(e: WriterError) -> Self {
        ConvertError::Writer(e)
    }
}

/// A written job.
#[derive(Clone, Debug, PartialEq)]
pub struct EgvJob {
    /// The EGV commands (no file header; see [`crate::egv_file`]).
    pub text: String,
    /// Things that were changed or left out.
    pub warnings: Vec<String>,
    /// Where the head starts and ends, mils (relative jobs start at 0,0).
    pub start: (i64, i64),
    pub end: (i64, i64),
    /// Program (vector) blocks written.
    pub program_blocks: usize,
    /// Raster blocks written.
    pub raster_blocks: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Seg {
    Travel { to: (i64, i64) },
    Burn { to: (i64, i64), speed: f64, on: bool },
}

impl Seg {
    fn to(&self) -> (i64, i64) {
        match *self {
            Seg::Travel { to } | Seg::Burn { to, .. } => to,
        }
    }
}

#[derive(Clone, Debug, Default)]
struct Chunk {
    raster: bool,
    segs: Vec<Seg>,
}

struct Warnings(Vec<String>);

impl Warnings {
    fn add(&mut self, w: String) {
        if !self.0.contains(&w) {
            self.0.push(w);
        }
    }
}

fn check_point(p: Point, space: CoordSpace, bed: Option<(f64, f64)>) -> Result<(), ConvertError> {
    if !p.x.is_finite() || !p.y.is_finite() {
        return Err(ConvertError::NotANumber);
    }
    if space == CoordSpace::MachineAbsolute {
        let tol = MM_PER_MIL / 2.0;
        let outside = p.x < -tol || p.y < -tol || bed.is_some_and(|(w, h)| p.x > w + tol || p.y > h + tol);
        if outside {
            return Err(ConvertError::OutsideBed { x: p.x, y: p.y });
        }
    }
    Ok(())
}

/// The points that replace an arc from `from` (excluding it), ending exactly at `to`.
fn flatten_arc(from: Point, to: Point, center: Point, clockwise: bool, tolerance: f64) -> Vec<Point> {
    const EPS: f64 = 1e-9;
    let r = from.distance(center);
    if r < EPS {
        return vec![to];
    }
    let a0 = (from - center).atan2();
    let a1 = (to - center).atan2();
    let mut sweep = a1 - a0;
    if clockwise {
        if sweep >= -EPS {
            sweep -= std::f64::consts::TAU;
        }
    } else if sweep <= EPS {
        sweep += std::f64::consts::TAU;
    }
    let tol = tolerance.max(1e-4).min(r);
    let max_step = 2.0 * (1.0 - tol / r).clamp(-1.0, 1.0).acos();
    let n = ((sweep.abs() / max_step.max(1e-3)).ceil() as usize).clamp(1, 100_000);
    let mut out = Vec::with_capacity(n);
    for i in 1..n {
        let a = a0 + sweep * i as f64 / n as f64;
        out.push(Point::new(center.x + r * a.cos(), center.y + r * a.sin()));
    }
    out.push(to);
    out
}

/// Splits the toolpath into chunks (one per layer pass) of moves in mils.
fn chunks(tp: &Toolpath, s: &K40Settings, warnings: &mut Warnings) -> Result<Vec<Chunk>, ConvertError> {
    let space = tp.space;
    let start = if space == CoordSpace::RelativeToStart { Point::ZERO } else { tp.start };
    check_point(start, space, s.bed_mm)?;
    let mut pos = start;
    let mut mils = (mm_to_mils(start.x), mm_to_mils(start.y));
    let mut out: Vec<Chunk> = vec![Chunk::default()];
    let mut raster = false;
    let push = |out: &mut Vec<Chunk>, seg: Seg, mils: &mut (i64, i64)| {
        if seg.to() == *mils {
            return;
        }
        *mils = seg.to();
        if let Some(c) = out.last_mut() {
            c.segs.push(seg);
        }
    };
    for op in &tp.ops {
        match op {
            Op::LayerStart { layer } => {
                raster = tp.layers.get(*layer).is_some_and(|l| l.raster);
                out.push(Chunk { raster, segs: Vec::new() });
            }
            Op::PassStart { .. } => out.push(Chunk { raster, segs: Vec::new() }),
            Op::Rapid { to } => {
                check_point(*to, space, s.bed_mm)?;
                push(&mut out, Seg::Travel { to: (mm_to_mils(to.x), mm_to_mils(to.y)) }, &mut mils);
                pos = *to;
            }
            Op::Cut { to, speed, power } => {
                check_point(*to, space, s.bed_mm)?;
                push(&mut out, Seg::Burn { to: (mm_to_mils(to.x), mm_to_mils(to.y)), speed: *speed, on: *power > 0.0 }, &mut mils);
                pos = *to;
            }
            Op::Overscan { to, speed } => {
                check_point(*to, space, s.bed_mm)?;
                push(&mut out, Seg::Burn { to: (mm_to_mils(to.x), mm_to_mils(to.y)), speed: *speed, on: false }, &mut mils);
                pos = *to;
            }
            Op::Arc { to, center, clockwise, speed, power } => {
                if !center.x.is_finite() || !center.y.is_finite() {
                    return Err(ConvertError::NotANumber);
                }
                for p in flatten_arc(pos, *to, *center, *clockwise, s.arc_tolerance_mm) {
                    check_point(p, space, s.bed_mm)?;
                    push(&mut out, Seg::Burn { to: (mm_to_mils(p.x), mm_to_mils(p.y)), speed: *speed, on: *power > 0.0 }, &mut mils);
                }
                pos = *to;
            }
            Op::Dwell { power, .. } => {
                if *power > 0.0 {
                    warnings.add("Dwells that fire the laser (dot mode) cannot be sent to a K40 board; they were left out".into());
                }
            }
            Op::ZMove { .. } => warnings.add("Z moves were left out (the K40 board has no Z axis)".into()),
            Op::Laser { index } if *index > 1 => warnings.add("A second laser was asked for; the K40 board has one".into()),
            Op::Air { .. } | Op::Laser { .. } | Op::Comment { .. } => {}
        }
    }
    out.retain(|c| !c.segs.is_empty());
    Ok(out)
}

fn clamp_speed(speed: f64, raster: bool, s: &K40Settings, warnings: &mut Warnings) -> f64 {
    let min = min_speed(s.board, raster, s.fix_speeds);
    let max = if raster { s.max_raster_speed } else { s.max_vector_speed }.max(min);
    let kind = if raster { "raster" } else { "vector" };
    if !speed.is_finite() || speed < min {
        warnings.add(format!("{kind} speeds below {min:.2} mm/s were raised to {min:.2} mm/s (the slowest the board can go)"));
        min
    } else if speed > max {
        warnings.add(format!("{kind} speeds above {max:.0} mm/s were lowered to {max:.0} mm/s"));
        max
    } else {
        speed
    }
}

/// One raster row as raster mode writes it.
#[derive(Clone, Debug)]
struct Row {
    y: i64,
    /// +1 right, -1 left.
    dir: i64,
    /// Where the row starts and ends (the furthest points visited, in the row's direction).
    start_x: i64,
    end_x: i64,
    /// Burning stretches `(from, to)`, in order along the row.
    burns: Vec<(i64, i64)>,
}

/// A row as found in the toolpath, before its direction is settled.
#[derive(Clone, Debug)]
struct RawRow {
    y: i64,
    xs: Vec<i64>,
    burns: Vec<(i64, i64)>,
    /// Direction of the first move along the row (0 for a row with no moves).
    first_dir: i64,
}

#[derive(Clone, Debug)]
struct RasterPlan {
    before: Vec<(i64, i64)>,
    rows: Vec<Row>,
    step: u32,
    speed: f64,
    after: Vec<(i64, i64)>,
}

/// Fits a chunk to raster mode, or `None` when its rows do not allow it.
///
/// Rows are runs of horizontal moves on one line; the moves between rows must not burn. Moves
/// along a row that do not burn only set how far the row reaches (so overscan is kept, and
/// travel that doubles back between two burns is simply left out). The burning stretches of
/// a row must all run the same way, rows must alternate in direction and step the same way
/// by about the same amount.
fn plan_raster(start: (i64, i64), segs: &[Seg], max_step: u32) -> Option<RasterPlan> {
    let first_burn = segs.iter().position(|s| matches!(s, Seg::Burn { .. }))?;
    let last_burn = segs.iter().rposition(|s| matches!(s, Seg::Burn { .. }))?;
    let before: Vec<_> = segs[..first_burn].iter().map(Seg::to).collect();
    let after: Vec<_> = segs[last_burn + 1..].iter().map(Seg::to).collect();
    let mut pos = before.last().copied().unwrap_or(start);
    let mut raw: Vec<RawRow> = Vec::new();
    let mut open = false;
    let mut speed: Option<f64> = None;
    for seg in &segs[first_burn..=last_burn] {
        let to = seg.to();
        let on = matches!(seg, Seg::Burn { on: true, .. });
        if let Seg::Burn { speed: v, .. } = seg {
            match speed {
                None => speed = Some(*v),
                Some(old) if (old - v).abs() > 1e-9 => return None,
                Some(_) => {}
            }
        }
        if to.1 == pos.1 {
            if !open {
                raw.push(RawRow { y: pos.1, xs: vec![pos.0], burns: Vec::new(), first_dir: (to.0 - pos.0).signum() });
                open = true;
            }
            let row = raw.last_mut()?;
            row.xs.push(to.0);
            if on && to.0 != pos.0 {
                row.burns.push((pos.0, to.0));
            }
        } else {
            if on {
                return None;
            }
            // A row whose every move was rounded away: the head stops on its line between
            // two connecting moves. It stays as an empty row so the rows still alternate.
            if !open && raw.last().is_some_and(|r| r.y != pos.1) {
                raw.push(RawRow { y: pos.1, xs: vec![pos.0], burns: Vec::new(), first_dir: 0 });
            }
            open = false;
        }
        pos = to;
    }
    let mut rows: Vec<Row> = Vec::with_capacity(raw.len());
    for r in raw {
        let mut dir = r.burns.first().map_or(r.first_dir, |b| (b.1 - b.0).signum());
        if dir == 0 {
            dir = -rows.last()?.dir;
        }
        let mut reached = None;
        for &(a, b) in &r.burns {
            if (b - a).signum() != dir || reached.is_some_and(|e: i64| dir * (a - e) < 0) {
                return None;
            }
            reached = Some(b);
        }
        let (lo, hi) = (*r.xs.iter().min()?, *r.xs.iter().max()?);
        let (start_x, end_x) = if dir > 0 { (lo, hi) } else { (hi, lo) };
        rows.push(Row { y: r.y, dir, start_x, end_x, burns: r.burns });
    }
    if rows.len() < 2 {
        return None;
    }
    let mut steps = Vec::with_capacity(rows.len() - 1);
    for w in rows.windows(2) {
        if w[0].dir == w[1].dir {
            return None;
        }
        steps.push(w[1].y - w[0].y);
    }
    let sign = steps[0].signum();
    if sign == 0 || steps.iter().any(|d| d.signum() != sign) {
        return None;
    }
    let g = steps.iter().map(|d| d.abs()).min()?;
    let most = steps.iter().map(|d| d.abs()).max()?;
    if g < 1 || g > i64::from(max_step.min(999)) || most - g > MAX_RASTER_CORRECTION {
        return None;
    }
    Some(RasterPlan { before, rows, step: g as u32, speed: speed?, after })
}

fn write_raster(w: &mut EgvWriter, plan: &RasterPlan, speed: f64) -> Result<(), ConvertError> {
    for &p in &plan.before {
        w.move_to(p.0, p.1);
    }
    let first = &plan.rows[0];
    w.move_to(first.start_x, first.y);
    let topward = plan.rows[1].y < first.y;
    w.raster_mode(speed, plan.step, first.dir < 0, topward)?;
    for (i, row) in plan.rows.iter().enumerate() {
        w.raster_run_to(row.start_x, false)?;
        for &(a, b) in &row.burns {
            w.raster_run_to(a, false)?;
            w.raster_run_to(b, true)?;
        }
        w.raster_run_to(row.end_x, false)?;
        let Some(next) = plan.rows.get(i + 1) else { break };
        let step = i64::from(plan.step);
        let step_amount = if topward { -step } else { step };
        let correction = ((next.y - w.position().1) - step_amount).abs();
        // Reverse far enough out that the next row's start lies behind the turn.
        let turn_at = next.start_x - row.dir * correction;
        if row.dir * (turn_at - w.position().0) > 0 {
            w.raster_run_to(turn_at, false)?;
        }
        w.raster_next_row(next.y)?;
    }
    for &p in &plan.after {
        w.move_to(p.0, p.1);
    }
    Ok(())
}

fn write_vector(w: &mut EgvWriter, segs: &[Seg], s: &K40Settings, warnings: &mut Warnings) -> Result<usize, ConvertError> {
    let mut blocks = 0;
    let mut speed = f64::NAN;
    for seg in segs {
        match *seg {
            Seg::Travel { to } => w.move_to(to.0, to.1),
            Seg::Burn { to, speed: v, on } => {
                let v = clamp_speed(v, false, s, warnings);
                if w.mode() != Mode::Program || v != speed {
                    let p = w.position();
                    w.program_mode(v, (to.0 - p.0, to.1 - p.1));
                    speed = v;
                    blocks += 1;
                }
                w.line_to(to.0, to.1, on)?;
            }
        }
    }
    Ok(blocks)
}

/// Writes a toolpath as EGV.
pub fn toolpath_to_egv(tp: &Toolpath, s: &K40Settings) -> Result<EgvJob, ConvertError> {
    if tp.space == CoordSpace::Design {
        return Err(ConvertError::DesignCoordinates);
    }
    let mut warnings = Warnings(Vec::new());
    let chunks = chunks(tp, s, &mut warnings)?;
    let start = if tp.space == CoordSpace::RelativeToStart { (0, 0) } else { (mm_to_mils(tp.start.x), mm_to_mils(tp.start.y)) };
    let opts = WriterOptions { board: s.board, autolock: s.autolock, fix_speeds: s.fix_speeds };
    let mut w = EgvWriter::starting_at(opts, start);
    let (mut program_blocks, mut raster_blocks) = (0, 0);
    for chunk in &chunks {
        let plan = if chunk.raster && s.raster { plan_raster(w.position(), &chunk.segs, s.max_raster_step) } else { None };
        match plan {
            Some(plan) => {
                let speed = clamp_speed(plan.speed, true, s, &mut warnings);
                write_raster(&mut w, &plan, speed)?;
                raster_blocks += 1;
            }
            None => program_blocks += write_vector(&mut w, &chunk.segs, s, &mut warnings)?,
        }
    }
    let end = w.position();
    Ok(EgvJob { text: w.finish(), warnings: warnings.0, start, end, program_blocks, raster_blocks })
}

#[cfg(test)]
mod tests {
    use super::*;
    use darkburn_job::IrLayer;

    fn layer(raster: bool) -> IrLayer {
        IrLayer {
            layer_index: 0,
            sublayer: 0,
            name: "test".into(),
            mode: if raster { darkburn_job::CutMode::Fill } else { darkburn_job::CutMode::Line },
            speed: 15.0,
            max_power: 50.0,
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

    fn mm(mils: i64) -> f64 {
        mils_to_mm(mils)
    }

    fn square() -> Toolpath {
        let p = |x: i64, y: i64| Point::new(mm(x), mm(y));
        Toolpath {
            space: CoordSpace::MachineAbsolute,
            layers: vec![layer(false)],
            ops: vec![
                Op::LayerStart { layer: 0 },
                Op::Rapid { to: p(787, 787) },
                Op::Cut { to: p(1181, 787), speed: 15.0, power: 30.0 },
                Op::Cut { to: p(1181, 1181), speed: 15.0, power: 30.0 },
                Op::Cut { to: p(787, 1181), speed: 15.0, power: 30.0 },
                Op::Cut { to: p(787, 787), speed: 15.0, power: 30.0 },
            ],
            start: Point::ZERO,
        }
    }

    #[test]
    fn mm_and_mils() {
        assert_eq!(mm_to_mils(25.4), 1000);
        assert_eq!(mm_to_mils(20.0), 787);
        assert_eq!(mm_to_mils(-0.0127), -1);
        assert!((mils_to_mm(1000) - 25.4).abs() < 1e-9);
    }

    #[test]
    fn square_matches_the_reference_driver() {
        let job = toolpath_to_egv(&square(), &K40Settings::default()).unwrap();
        assert_eq!(job.text, "IBzzzvRzzzvS1P\nICV2490731016000027CNLBS1EDz139Rz139Tz139Lz139FNSE-\n");
        assert_eq!(job.program_blocks, 1);
        assert_eq!(job.end, (787, 787));
        assert!(job.warnings.is_empty());
    }

    #[test]
    fn design_coordinates_are_refused() {
        let mut tp = square();
        tp.space = CoordSpace::Design;
        assert_eq!(toolpath_to_egv(&tp, &K40Settings::default()), Err(ConvertError::DesignCoordinates));
    }

    #[test]
    fn points_outside_the_bed_are_refused() {
        let mut tp = square();
        tp.ops.push(Op::Rapid { to: Point::new(400.0, 10.0) });
        assert!(matches!(toolpath_to_egv(&tp, &K40Settings::default()), Err(ConvertError::OutsideBed { .. })));
        let mut tp = square();
        tp.ops.push(Op::Rapid { to: Point::new(-1.0, 10.0) });
        assert!(matches!(toolpath_to_egv(&tp, &K40Settings::default()), Err(ConvertError::OutsideBed { .. })));
        let mut tp = square();
        tp.ops.push(Op::Rapid { to: Point::new(f64::NAN, 10.0) });
        assert_eq!(toolpath_to_egv(&tp, &K40Settings::default()), Err(ConvertError::NotANumber));
        // No bed: only negative positions are refused.
        let mut tp = square();
        tp.ops.push(Op::Rapid { to: Point::new(400.0, 10.0) });
        assert!(toolpath_to_egv(&tp, &K40Settings { bed_mm: None, ..Default::default() }).is_ok());
    }

    #[test]
    fn relative_jobs_start_at_zero_and_may_go_negative() {
        let mut tp = square();
        tp.space = CoordSpace::RelativeToStart;
        tp.start = Point::new(100.0, 100.0);
        tp.ops.push(Op::Rapid { to: Point::new(-5.0, -5.0) });
        let job = toolpath_to_egv(&tp, &K40Settings::default()).unwrap();
        assert_eq!(job.start, (0, 0));
        assert_eq!(job.end, (mm_to_mils(-5.0), mm_to_mils(-5.0)));
    }

    #[test]
    fn speeds_are_clamped_with_a_warning() {
        let mut tp = square();
        for op in &mut tp.ops {
            if let Op::Cut { speed, .. } = op {
                *speed = 500.0;
            }
        }
        let job = toolpath_to_egv(&tp, &K40Settings::default()).unwrap();
        assert_eq!(job.warnings.len(), 1);
        assert!(job.warnings[0].contains("140"));
        let slow = min_speed(Board::M2, false, false);
        assert!(slow > 0.3 && slow < 0.5, "{slow}");
        assert!(min_speed(Board::M2, true, false) > 5.0);
    }

    #[test]
    fn unsupported_ops_warn_once() {
        let mut tp = square();
        tp.ops.push(Op::Dwell { ms: 5.0, power: 50.0 });
        tp.ops.push(Op::Dwell { ms: 5.0, power: 50.0 });
        tp.ops.push(Op::Dwell { ms: 5.0, power: 0.0 });
        tp.ops.push(Op::ZMove { z: 1.0 });
        tp.ops.push(Op::Air { on: true });
        tp.ops.push(Op::Comment { text: "x".into() });
        let job = toolpath_to_egv(&tp, &K40Settings::default()).unwrap();
        assert_eq!(job.warnings.len(), 2, "{:?}", job.warnings);
    }

    #[test]
    fn speed_changes_start_new_blocks() {
        let mut tp = square();
        if let Op::Cut { speed, .. } = &mut tp.ops[3] {
            *speed = 30.0;
        }
        let job = toolpath_to_egv(&tp, &K40Settings::default()).unwrap();
        assert_eq!(job.program_blocks, 3);
        assert_eq!(job.text.matches("FNSE-").count(), 3);
    }

    #[test]
    fn arcs_flatten_within_tolerance() {
        let c = Point::new(50.0, 50.0);
        let pts = flatten_arc(Point::new(60.0, 50.0), Point::new(60.0, 50.0), c, false, 0.01);
        assert!(pts.len() > 20);
        assert_eq!(*pts.last().unwrap(), Point::new(60.0, 50.0));
        for p in &pts {
            assert!((p.distance(c) - 10.0).abs() < 1e-9);
        }
        // Clockwise quarter: from (60,50) to (50,40) going through negative angles.
        let q = flatten_arc(Point::new(60.0, 50.0), Point::new(50.0, 40.0), c, true, 0.01);
        assert!(q.iter().all(|p| p.x >= 50.0 - 1e-9 && p.y <= 50.0 + 1e-9));
        assert_eq!(flatten_arc(c, Point::new(1.0, 1.0), c, false, 0.01), vec![Point::new(1.0, 1.0)]);
    }
}
