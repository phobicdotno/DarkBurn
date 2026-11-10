//! The job compiler: a neutral job description in millimetres in, list packets out.
//!
//! A [`Job`] is a stack of [`Layer`]s (line, fill, flood fill or 3D sliced image), each with its
//! own frequency / Q-pulse / power / speed / timings, plus job wide options: repeat marking with
//! an external axis, a rotary runner (by slice or by shape) and door-protect stages.
//! [`compile`] turns it into a [`CompiledJob`]: a list of [`Step`]s (list packets, axis moves,
//! door and foot-switch waits) and a time estimate. The driver ([`crate::GalvoMachine`]) plays the
//! steps.
//!
//! Interpretations that are DarkBurn's own (they have no public specification) are marked in the
//! item docs and listed in `docs/hardware-checks-galvo.md`.

use std::fmt;

use kurbo::{BezPath, Point, Rect};

use crate::codec::{ListCmd, ListPacket, packetize, unpacketize};
use crate::field::{Field, OutOfField};
use crate::geom::{Poly, Wobble, clip_polyline_y, convex_hull, flatten_path, hatch, order_paths, polys_bounds, wobble_polyline};
use crate::settings::{DeviceSettings, LaserSource, MarkParams, Timings, power_cmds, speed_word};

/// How the fill angle changes from pass to pass.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AngleMode {
    /// Same angle every pass.
    Fixed,
    /// Add `increment_deg` every pass ("angle increment per pass").
    PerPass { increment_deg: f64 },
    /// Spread the passes evenly over 180 degrees (DarkBurn's reading of "auto-rotate").
    AutoRotate,
}

impl AngleMode {
    /// Scan angle of pass `pass` (0 based) out of `total`.
    pub fn angle(&self, base: f64, pass: u32, total: u32) -> f64 {
        match *self {
            Self::Fixed => base,
            Self::PerPass { increment_deg } => base + increment_deg * pass as f64,
            Self::AutoRotate => base + 180.0 * pass as f64 / total.max(1) as f64,
        }
    }
}

/// Whether shapes are hatched together or one by one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FillGrouping {
    /// All shapes of the layer together, even-odd (holes work across shapes).
    #[default]
    AllAtOnce,
    /// Each shape (one `BezPath`) on its own.
    Individually,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FillSettings {
    pub interval_mm: f64,
    pub angle_deg: f64,
    pub angle_mode: AngleMode,
    pub bidirectional: bool,
    /// Add a second set of lines at +90 degrees in every pass.
    pub cross_hatch: bool,
    pub grouping: FillGrouping,
}

impl Default for FillSettings {
    fn default() -> Self {
        Self {
            interval_mm: 0.05,
            angle_deg: 0.0,
            angle_mode: AngleMode::Fixed,
            bidirectional: true,
            cross_hatch: false,
            grouping: FillGrouping::AllAtOnce,
        }
    }
}

/// A grey-scale depth image marked as stacked slices ("3D sliced image").
///
/// `depth` holds one byte per pixel, row 0 at the top, larger = deeper. Slice `k` (1..=slices)
/// marks every pixel deeper than `255 * (k - 1) / slices`; between slices the focus moves by
/// `z_step_mm`. The optional cleanup pass marks every non-zero pixel once more with its own
/// parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct SlicedImage {
    pub width_px: u32,
    pub height_px: u32,
    pub depth: Vec<u8>,
    /// Lower-left corner and size in mm.
    pub origin: Point,
    pub size_mm: (f64, f64),
    pub slices: u32,
    pub z_step_mm: f64,
    pub interval_mm: f64,
    pub bidirectional: bool,
    pub cleanup: Option<MarkParams>,
}

/// Drill mode: instead of tracing an outline, fire in place at points along it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DrillSettings {
    /// How long the laser fires at each point, in milliseconds.
    pub dwell_ms: f64,
    /// Distance between points along the outline in mm. 0 drills only the outline's own
    /// points (the corners of a polygon, the ends of a line).
    pub spacing_mm: f64,
}

impl Default for DrillSettings {
    fn default() -> Self {
        Self { dwell_ms: 1.0, spacing_mm: 0.5 }
    }
}

/// The longest dwell one `LaserOnPoint` carries, in its 10 microsecond units; longer drills
/// are sent as several in a row.
pub const MAX_POINT_DWELL: u16 = 60_000;

/// The points to drill along one polyline: its own points when `spacing` is 0, otherwise
/// evenly spread about `spacing` apart (both ends of an open line are always drilled).
pub fn drill_points(pts: &[Point], closed: bool, spacing: f64) -> Vec<Point> {
    let mut line: Vec<Point> = pts.to_vec();
    line.dedup_by(|a, b| (a.x - b.x).abs() < 1e-9 && (a.y - b.y).abs() < 1e-9);
    if closed && line.len() > 1 && (line[0] - line[line.len() - 1]).hypot() < 1e-9 {
        line.pop();
    }
    if line.len() < 2 || !(spacing.is_finite() && spacing > 0.0) {
        return line;
    }
    if closed {
        line.push(line[0]);
    }
    let length: f64 = line.windows(2).map(|w| (w[1] - w[0]).hypot()).sum();
    let steps = ((length / spacing).round() as usize).max(1);
    let count = if closed { steps } else { steps + 1 };
    let mut out = Vec::with_capacity(count);
    let (mut seg, mut walked) = (0usize, 0.0f64);
    for i in 0..count {
        let target = length * i as f64 / steps as f64;
        while seg + 2 < line.len() && walked + (line[seg + 1] - line[seg]).hypot() < target {
            walked += (line[seg + 1] - line[seg]).hypot();
            seg += 1;
        }
        let d = (line[seg + 1] - line[seg]).hypot();
        let t = if d > 0.0 { ((target - walked) / d).clamp(0.0, 1.0) } else { 0.0 };
        out.push(line[seg].lerp(line[seg + 1], t));
    }
    out
}

#[derive(Clone, Debug, PartialEq)]
pub enum LayerMode {
    /// Trace the outlines.
    Line,
    /// Fire in place at points along the outlines.
    Drill(DrillSettings),
    /// Hatch closed shapes.
    Fill(FillSettings),
    /// Hatch with continuous sweeps: gaps between segments of one scan line are crossed with
    /// fast jumps (no settle delay, no end delay), shapes are not grouped.
    Flood(FillSettings),
    /// 3D sliced image (geometry is ignored).
    Sliced(SlicedImage),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Layer {
    pub name: String,
    pub mode: LayerMode,
    pub params: MarkParams,
    pub passes: u32,
    pub geometry: Vec<BezPath>,
    pub wobble: Option<Wobble>,
    pub enabled: bool,
}

impl Layer {
    pub fn new(name: &str, mode: LayerMode, params: MarkParams, geometry: Vec<BezPath>) -> Self {
        Self { name: name.to_string(), mode, params, passes: 1, geometry, wobble: None, enabled: true }
    }
}

/// Repeat marking: the whole job `count` times, optionally moving an external axis between
/// repeats (conveyor, indexer).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RepeatMarking {
    pub count: u32,
    /// `(axis index, mm to advance before every repeat after the first)`.
    pub axis_step: Option<(u8, f64)>,
    /// Wait for the foot switch before every repeat after the first.
    pub wait_foot_each: bool,
}

impl Default for RepeatMarking {
    fn default() -> Self {
        Self { count: 1, axis_step: None, wait_foot_each: false }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RotaryMode {
    /// Cut the design into bands of at most `max_size_mm` along the rotating direction.
    BySlice,
    /// Keep shapes whole; neighbouring shapes share a window up to `max_size_mm`. Shapes taller
    /// than the window are sliced.
    ByShape,
}

/// Rotary runner. The design's Y direction is the surface travel of the rotary.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RotaryConfig {
    pub mode: RotaryMode,
    /// Longest band (mm of surface) marked in one position; at most the field size.
    pub max_size_mm: f64,
    /// External axis that carries the rotary.
    pub axis: u8,
}

/// The neutral job description.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Job {
    pub layers: Vec<Layer>,
    pub repeat: RepeatMarking,
    pub rotary: Option<RotaryConfig>,
    /// One stage per layer pass with a door-closed wait before each.
    pub door_protect: bool,
    /// Order open paths by nearest neighbour.
    pub optimize: bool,
}

impl Job {
    pub fn new() -> Self {
        Self { layers: Vec::new(), repeat: RepeatMarking::default(), rotary: None, door_protect: false, optimize: true }
    }

    /// Bounds of everything that will be marked, in mm.
    pub fn bounds(&self, tolerance_mm: f64) -> Option<Rect> {
        let mut out: Option<Rect> = None;
        let mut add = |r: Rect| out = Some(out.map_or(r, |o| o.union(r)));
        for l in self.layers.iter().filter(|l| l.enabled) {
            match &l.mode {
                LayerMode::Sliced(s) => add(Rect::new(s.origin.x, s.origin.y, s.origin.x + s.size_mm.0, s.origin.y + s.size_mm.1)),
                _ => {
                    for g in &l.geometry {
                        if let Some(r) = polys_bounds(&flatten_path(g, tolerance_mm)) {
                            add(r);
                        }
                    }
                }
            }
        }
        out
    }

    /// Convex hull of everything that will be marked (for hull framing).
    pub fn hull(&self, tolerance_mm: f64) -> Vec<Point> {
        let mut pts = Vec::new();
        for l in self.layers.iter().filter(|l| l.enabled) {
            if let LayerMode::Sliced(s) = &l.mode {
                let (o, w, h) = (s.origin, s.size_mm.0, s.size_mm.1);
                pts.extend([o, Point::new(o.x + w, o.y), Point::new(o.x + w, o.y + h), Point::new(o.x, o.y + h)]);
            } else {
                for g in &l.geometry {
                    for p in flatten_path(g, tolerance_mm) {
                        pts.extend(p.pts);
                    }
                }
            }
        }
        convex_hull(&pts)
    }
}

/// Compiler options.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CompileOptions {
    /// Curve flattening tolerance in mm.
    pub tolerance_mm: f64,
    /// Skip nearest-neighbour ordering above this many paths.
    pub max_paths_to_order: usize,
    /// Refuse rotary jobs with more windows than this.
    pub max_rotary_windows: usize,
}

impl Default for CompileOptions {
    fn default() -> Self {
        Self { tolerance_mm: 0.01, max_paths_to_order: 5000, max_rotary_windows: 2000 }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum CompileError {
    OutOfField(OutOfField),
    NothingToMark,
    Invalid(String),
}

impl fmt::Display for CompileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutOfField(e) => write!(f, "{e}"),
            Self::NothingToMark => write!(f, "There is nothing to mark."),
            Self::Invalid(m) => write!(f, "{m}"),
        }
    }
}
impl std::error::Error for CompileError {}

impl From<OutOfField> for CompileError {
    fn from(e: OutOfField) -> Self {
        Self::OutOfField(e)
    }
}

/// One thing the driver does, in order.
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    /// Run these list packets to completion.
    List(Vec<ListPacket>),
    /// Move an external axis to an absolute position in mm.
    MoveAxis { axis: u8, mm: f64 },
    /// Move Z to an offset in mm from where the job started.
    MoveZ { mm: f64 },
    /// Wait until the door is closed.
    WaitDoorClosed,
    /// Wait for the foot switch.
    WaitFootSwitch,
}

/// Time estimate in seconds.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Estimate {
    pub mark_s: f64,
    pub jump_s: f64,
    pub delay_s: f64,
    /// Axis and Z moves.
    pub motion_s: f64,
}

impl Estimate {
    pub fn total_s(&self) -> f64 {
        self.mark_s + self.jump_s + self.delay_s + self.motion_s
    }
    fn add(&mut self, o: &Estimate) {
        self.mark_s += o.mark_s;
        self.jump_s += o.jump_s;
        self.delay_s += o.delay_s;
        self.motion_s += o.motion_s;
    }
}

/// Fixed time allowed per axis or Z move in the estimate.
pub const MOTION_STEP_S: f64 = 0.5;

#[derive(Clone, Debug, PartialEq)]
pub struct CompiledJob {
    pub steps: Vec<Step>,
    /// Bounds of the marked area in mm.
    pub bounds_mm: Rect,
    pub estimate: Estimate,
    /// Identifies the framed area (see [`bounds_key`]).
    pub frame_key: u64,
}

impl CompiledJob {
    /// All list commands of all list steps (for inspection and tests).
    pub fn list_commands(&self) -> Vec<ListCmd> {
        self.steps.iter().filter_map(|s| if let Step::List(p) = s { Some(unpacketize(p)) } else { None }).flatten().collect()
    }

    pub fn packet_count(&self) -> usize {
        self.steps.iter().map(|s| if let Step::List(p) = s { p.len() } else { 0 }).sum()
    }

    /// True if any list step contains a command that can fire the laser.
    pub fn emits(&self) -> bool {
        self.list_commands().iter().any(|c| c.emits())
    }
}

/// Key that ties a frame to a job: both hash the bounds rounded to micrometres.
pub fn bounds_key(r: Rect) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for v in [r.x0, r.y0, r.x1, r.y1] {
        for b in ((v * 1000.0).round() as i64).to_le_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
    }
    h
}

// ---------------------------------------------------------------------------------------------
// list writer

struct Writer<'a> {
    dev: &'a DeviceSettings,
    field: &'a Field,
    cmds: Vec<ListCmd>,
    last: Option<(u16, u16)>,
    est: Estimate,
    timings: Timings,
    // cached state
    cur_params: Option<MarkParams>,
    cur_jump_delay: Option<i32>,
    upmm: f64,
    jump_w: f64,
    mark_w: f64,
}

impl<'a> Writer<'a> {
    fn new(dev: &'a DeviceSettings, field: &'a Field) -> Self {
        let mut w = Self {
            dev,
            field,
            cmds: Vec::new(),
            last: None,
            est: Estimate::default(),
            timings: dev.timings,
            cur_params: None,
            cur_jump_delay: None,
            upmm: field.units_per_mm(),
            jump_w: 1.0,
            mark_w: 1.0,
        };
        w.cmds.push(ListCmd::ReadyMark);
        w.cmds.push(ListCmd::WritePort(1u16 << dev.ports.mark_bit));
        if dev.source.is_fiber() && dev.open_mo_delay_units > 0 {
            w.cmds.push(ListCmd::DelayTime { units: dev.open_mo_delay_units });
            w.est.delay_s += dev.open_mo_delay_units as f64 * 10e-6;
        }
        w
    }

    fn set_params(&mut self, p: &MarkParams) {
        if self.cur_params.as_ref() == Some(p) {
            return;
        }
        let first = self.cur_params.is_none();
        let old = self.cur_params.clone();
        let t = p.timings.unwrap_or(self.dev.timings);
        self.timings = t;
        let jw = speed_word(t.jump_speed_mm_s, self.upmm);
        if first || old.as_ref().map(|o| o.timings.unwrap_or(self.dev.timings).jump_speed_mm_s) != Some(t.jump_speed_mm_s) {
            self.cmds.push(ListCmd::JumpSpeed(jw));
        }
        self.jump_w = jw as f64;
        if first || old.as_ref().is_some_and(|o| (o.frequency_khz, o.power_pct) != (p.frequency_khz, p.power_pct)) {
            self.cmds.extend(power_cmds(self.dev.source, p.power_pct, p.frequency_khz, self.dev.power_invert));
        }
        if self.dev.source == LaserSource::Mopa
            && let Some(ns) = p.q_pulse_ns
            && (first || old.as_ref().map(|o| o.q_pulse_ns) != Some(Some(ns)))
        {
            self.cmds.push(ListCmd::FiberYlpmPulseWidth(ns.min(0xFFFF) as u16));
        }
        let mw = speed_word(p.speed_mm_s, self.upmm);
        self.cmds.push(ListCmd::MarkSpeed(mw));
        self.mark_w = mw as f64;
        self.cmds.push(ListCmd::LaserOnDelay(t.laser_on_tc));
        self.cmds.push(ListCmd::LaserOffDelay(t.laser_off_tc));
        self.cmds.push(ListCmd::PolygonDelay((t.polygon_tc / 10).clamp(0, 65535) as u16));
        self.cur_jump_delay = None;
        self.cur_params = Some(p.clone());
    }

    fn goto(&self, p: Point) -> Result<(u16, u16), CompileError> {
        Ok(self.field.mm_to_galvo(p)?)
    }

    fn dist(&self, a: (u16, u16), b: (u16, u16)) -> u16 {
        self.field.galvo_distance(a, b)
    }

    fn jump_to(&mut self, p: Point, delay_override: Option<i32>) -> Result<(), CompileError> {
        let g = self.goto(p)?;
        let from = self.last.unwrap_or((32768, 32768));
        let d = self.dist(from, g);
        if self.last == Some(g) {
            return Ok(());
        }
        let delay = delay_override.unwrap_or(if d as f64 / self.upmm > self.timings.jump_distance_limit_mm {
            self.timings.max_jump_delay
        } else {
            self.timings.min_jump_delay
        });
        if self.cur_jump_delay != Some(delay) {
            self.cmds.push(ListCmd::JumpDelay(delay));
            self.cur_jump_delay = Some(delay);
        }
        self.cmds.push(ListCmd::JumpTo { x: g.0, y: g.1, angle: 0, distance: d });
        self.est.jump_s += d as f64 / self.jump_w / 1000.0;
        self.est.delay_s += delay.max(0) as f64 * 1e-6;
        self.last = Some(g);
        Ok(())
    }

    fn mark_to(&mut self, p: Point, vertex: bool) -> Result<(), CompileError> {
        let g = self.goto(p)?;
        if self.last == Some(g) {
            return Ok(());
        }
        let from = self.last.unwrap_or(g);
        let d = self.dist(from, g);
        self.cmds.push(ListCmd::MarkTo { x: g.0, y: g.1, angle: 0, distance: d });
        self.est.mark_s += d as f64 / self.mark_w / 1000.0;
        if vertex {
            self.est.delay_s += self.timings.polygon_tc.max(0) as f64 * 1e-6;
        }
        self.last = Some(g);
        Ok(())
    }

    fn end_delay(&mut self) {
        let e = self.timings.end_tc;
        if e > 0 {
            self.cmds.push(ListCmd::DelayTime { units: (e / 10).clamp(0, 65535) as u16 });
            self.est.delay_s += e as f64 * 1e-6;
        }
        self.est.delay_s += (self.timings.laser_on_tc.max(0) + self.timings.laser_off_tc.max(0)) as f64 * 1e-6;
    }

    /// Jump to the start, mark through the points, optionally close, then the end delay.
    fn path(&mut self, pts: &[Point], closed: bool) -> Result<(), CompileError> {
        let Some(&first) = pts.first() else { return Ok(()) };
        self.jump_to(first, None)?;
        if pts.len() == 1 {
            self.cmds.push(ListCmd::LaserOnPoint { dwell: 1 });
            self.end_delay();
            return Ok(());
        }
        for (i, &q) in pts.iter().enumerate().skip(1) {
            self.mark_to(q, i + 1 < pts.len() || closed)?;
        }
        if closed {
            self.mark_to(first, false)?;
        }
        self.end_delay();
        Ok(())
    }

    /// Jump to `p` and fire in place for `dwell_ms`.
    fn drill(&mut self, p: Point, dwell_ms: f64) -> Result<(), CompileError> {
        let g = self.goto(p)?;
        if self.last != Some(g) {
            self.jump_to(p, None)?;
        }
        // 10 microsecond units, as the board's point command counts them.
        let mut units = (dwell_ms * 100.0).round().max(1.0) as u64;
        while units > 0 {
            let part = units.min(MAX_POINT_DWELL as u64);
            self.cmds.push(ListCmd::LaserOnPoint { dwell: part as u16 });
            units -= part;
        }
        self.est.mark_s += dwell_ms.max(0.01) / 1000.0;
        self.est.delay_s += (self.timings.laser_on_tc.max(0) + self.timings.laser_off_tc.max(0)) as f64 * 1e-6;
        Ok(())
    }

    /// One scan line of a flood fill: segments joined by delay-free jumps.
    fn flood_line(&mut self, segs: &[(Point, Point)]) -> Result<(), CompileError> {
        for (i, &(a, b)) in segs.iter().enumerate() {
            if i == 0 {
                self.jump_to(a, None)?;
            } else {
                self.jump_to(a, Some(0))?;
            }
            self.mark_to(b, false)?;
        }
        self.end_delay();
        Ok(())
    }

    fn finish(mut self) -> (Vec<ListPacket>, Estimate) {
        self.cmds.push(ListCmd::WritePort(0));
        (packetize(&self.cmds), self.est)
    }

    fn is_empty(&self) -> bool {
        !self.cmds.iter().any(|c| c.emits())
    }
}

// ---------------------------------------------------------------------------------------------
// windows (rotary)

#[derive(Clone, Debug)]
struct Window {
    /// Rotary position (mm) and local origin shift; `None` without rotary.
    center_y: Option<f64>,
    band: Option<(f64, f64)>,
    /// `(layer, geometry index)` allowed in this window; `None` = all.
    shapes: Option<Vec<(usize, usize)>>,
    /// The lowest band of its design (or shape): the only one that owns its lower edge.
    first_band: bool,
    /// The highest band of its design (or shape): the only one that owns its upper edge.
    last_band: bool,
}

/// Takes the parts lying on the lower edge `y0` of a band out of a clipped piece. Such a part
/// belongs to the band below, which marks it; marking it here as well would burn it twice.
fn without_lower_edge(piece: Vec<Point>, y0: f64) -> Vec<Vec<Point>> {
    let on_edge = |p: &Point| (p.y - y0).abs() < 1e-9;
    let mut out: Vec<Vec<Point>> = Vec::new();
    let mut cur: Vec<Point> = Vec::new();
    for w in piece.windows(2) {
        if on_edge(&w[0]) && on_edge(&w[1]) {
            if cur.len() > 1 {
                out.push(std::mem::take(&mut cur));
            }
            cur.clear();
            continue;
        }
        if cur.is_empty() {
            cur.push(w[0]);
        }
        cur.push(w[1]);
    }
    if cur.len() > 1 {
        out.push(cur);
    }
    out
}

/// The pieces of an open polyline this window marks.
fn band_pieces(pts: &[Point], win: &Window) -> Option<Vec<Vec<Point>>> {
    let (y0, y1) = win.band?;
    let pieces = clip_polyline_y(pts, y0, y1);
    Some(if win.first_band { pieces } else { pieces.into_iter().flat_map(|p| without_lower_edge(p, y0)).collect() })
}

fn plan_windows(job: &Job, rc: &RotaryConfig, field_size: f64, opt: &CompileOptions) -> Result<Vec<Window>, CompileError> {
    if rc.max_size_mm <= 0.0 || rc.max_size_mm > field_size {
        return Err(CompileError::Invalid(format!("The rotary band size must be between 0 and the field size ({field_size} mm).")));
    }
    let mut shapes: Vec<((usize, usize), Rect)> = Vec::new();
    for (li, l) in job.layers.iter().enumerate().filter(|(_, l)| l.enabled) {
        if matches!(l.mode, LayerMode::Sliced(_)) {
            return Err(CompileError::Invalid("A 3D sliced image can't run on the rotary.".into()));
        }
        for (gi, g) in l.geometry.iter().enumerate() {
            if let Some(r) = polys_bounds(&flatten_path(g, opt.tolerance_mm)) {
                shapes.push(((li, gi), r));
            }
        }
    }
    if shapes.is_empty() {
        return Err(CompileError::NothingToMark);
    }
    let ymin = shapes.iter().map(|s| s.1.y0).fold(f64::MAX, f64::min);
    let ymax = shapes.iter().map(|s| s.1.y1).fold(f64::MIN, f64::max);
    let mut windows = Vec::new();
    let bands = |a: f64, b: f64, shapes: Option<Vec<(usize, usize)>>, out: &mut Vec<Window>| {
        let n = (((b - a) / rc.max_size_mm).ceil() as usize).max(1);
        let h = (b - a) / n as f64;
        for k in 0..n {
            let (y0, y1) = (a + h * k as f64, a + h * (k + 1) as f64);
            out.push(Window {
                center_y: Some((y0 + y1) / 2.0),
                band: Some((y0, y1)),
                shapes: shapes.clone(),
                first_band: k == 0,
                last_band: k + 1 == n,
            });
        }
    };
    match rc.mode {
        RotaryMode::BySlice => bands(ymin, ymax, None, &mut windows),
        RotaryMode::ByShape => {
            shapes.sort_by(|a, b| a.1.y0.total_cmp(&b.1.y0));
            let mut group: Vec<(usize, usize)> = Vec::new();
            let (mut g0, mut g1) = (0.0, 0.0);
            let close = |group: &mut Vec<(usize, usize)>, g0: f64, g1: f64, out: &mut Vec<Window>| {
                if !group.is_empty() {
                    out.push(Window {
                        center_y: Some((g0 + g1) / 2.0),
                        band: None,
                        shapes: Some(std::mem::take(group)),
                        first_band: true,
                        last_band: true,
                    });
                }
            };
            for (id, r) in shapes {
                if r.height() > rc.max_size_mm {
                    close(&mut group, g0, g1, &mut windows);
                    bands(r.y0, r.y1, Some(vec![id]), &mut windows);
                    continue;
                }
                if group.is_empty() {
                    (g0, g1) = (r.y0, r.y1);
                } else if r.y1.max(g1) - g0 > rc.max_size_mm {
                    close(&mut group, g0, g1, &mut windows);
                    (g0, g1) = (r.y0, r.y1);
                } else {
                    g1 = g1.max(r.y1);
                }
                group.push(id);
            }
            close(&mut group, g0, g1, &mut windows);
        }
    }
    if windows.len() > opt.max_rotary_windows {
        return Err(CompileError::Invalid(format!(
            "The rotary job needs {} positions, which is more than the limit of {}.",
            windows.len(),
            opt.max_rotary_windows
        )));
    }
    Ok(windows)
}

// ---------------------------------------------------------------------------------------------
// layer emission

fn shift(p: Point, dy: f64) -> Point {
    Point::new(p.x, p.y - dy)
}

fn layer_polys(job_layer: &Layer, li: usize, win: &Window, opt: &CompileOptions) -> Vec<Vec<Poly>> {
    job_layer
        .geometry
        .iter()
        .enumerate()
        .filter(|(gi, _)| win.shapes.as_ref().is_none_or(|s| s.contains(&(li, *gi))))
        .map(|(_, g)| flatten_path(g, opt.tolerance_mm))
        .collect()
}

/// Drill mode: the outlines become rows of points, each fired at in place. Wobble does not
/// apply. With a rotary, each window drills the points inside its band.
fn emit_drill_layer(w: &mut Writer, shapes: &[Vec<Poly>], d: &DrillSettings, win: &Window) -> Result<(), CompileError> {
    let dy = win.center_y.unwrap_or(0.0);
    for poly in shapes.iter().flatten() {
        for p in drill_points(&poly.pts, poly.closed, d.spacing_mm) {
            // A point on the line between two bands belongs to the upper one, so it is drilled once.
            if let Some((a, b)) = win.band
                && !(p.y >= a && (p.y < b || (win.last_band && p.y <= b + 1e-9)))
            {
                continue;
            }
            w.drill(shift(p, dy), d.dwell_ms)?;
        }
    }
    Ok(())
}

fn emit_line_layer(w: &mut Writer, layer: &Layer, shapes: &[Vec<Poly>], win: &Window, job: &Job, opt: &CompileOptions) -> Result<(), CompileError> {
    let mut paths: Vec<(Vec<Point>, bool)> = Vec::new();
    for poly in shapes.iter().flatten() {
        let mut pts = poly.pts.clone();
        if poly.closed && pts.len() > 1 {
            pts.push(pts[0]);
        }
        let pieces = match band_pieces(&pts, win) {
            Some(pieces) => pieces.into_iter().map(|p| (p, false)).collect::<Vec<_>>(),
            None => vec![(pts, poly.closed)],
        };
        for (p, closed) in pieces {
            let p = match &layer.wobble {
                Some(wb) => wobble_polyline(&p, wb),
                None => p,
            };
            paths.push((p, closed));
        }
    }
    // closed unwobbled paths are marked as polylines that already end at the start
    for (p, closed) in &mut paths {
        if *closed && p.len() > 1 && p.first() == p.last() {
            p.pop();
        }
    }
    let dy = win.center_y.unwrap_or(0.0);
    let local: Vec<(Vec<Point>, bool)> = paths.into_iter().map(|(p, c)| (p.into_iter().map(|q| shift(q, dy)).collect(), c)).collect();
    let order: Vec<(usize, bool)> = if job.optimize && local.len() > 1 && local.len() <= opt.max_paths_to_order {
        let se: Vec<(Point, Point, bool)> = local.iter().map(|(p, c)| (p[0], if *c { p[0] } else { *p.last().expect("non-empty") }, *c)).collect();
        order_paths(&se, Point::ORIGIN)
    } else {
        (0..local.len()).map(|i| (i, false)).collect()
    };
    for (i, rev) in order {
        let (p, closed) = &local[i];
        if p.is_empty() {
            continue;
        }
        if rev {
            let r: Vec<Point> = p.iter().rev().copied().collect();
            w.path(&r, *closed)?;
        } else {
            w.path(p, *closed)?;
        }
    }
    Ok(())
}

fn fill_segments(shapes: &[Vec<Poly>], fs: &FillSettings, pass: u32, total: u32, grouping: FillGrouping) -> Vec<Vec<(Point, Point)>> {
    let angle = fs.angle_mode.angle(fs.angle_deg, pass, total);
    let mut angles = vec![angle];
    if fs.cross_hatch {
        angles.push(angle + 90.0);
    }
    let mut out = Vec::new();
    for a in angles {
        match grouping {
            FillGrouping::AllAtOnce => {
                let polys: Vec<Poly> = shapes.iter().flatten().filter(|p| p.closed).cloned().collect();
                out.extend(hatch(&polys, fs.interval_mm, a, fs.bidirectional));
            }
            FillGrouping::Individually => {
                for s in shapes {
                    let polys: Vec<Poly> = s.iter().filter(|p| p.closed).cloned().collect();
                    out.extend(hatch(&polys, fs.interval_mm, a, fs.bidirectional));
                }
            }
        }
    }
    out
}

fn emit_fill_layer(
    w: &mut Writer,
    layer: &Layer,
    shapes: &[Vec<Poly>],
    fs: &FillSettings,
    flood: bool,
    pass: u32,
    win: &Window,
) -> Result<(), CompileError> {
    let grouping = if flood { FillGrouping::AllAtOnce } else { fs.grouping };
    let lines = fill_segments(shapes, fs, pass, layer.passes, grouping);
    let dy = win.center_y.unwrap_or(0.0);
    for line in lines {
        // pieces of this scan line after clipping to the rotary band
        let mut pieces: Vec<(Point, Point)> = Vec::new();
        for (a, b) in line {
            match band_pieces(&[a, b], win) {
                Some(inside) => pieces.extend(inside.into_iter().map(|p| (p[0], p[p.len() - 1]))),
                None => pieces.push((a, b)),
            }
        }
        if flood {
            let segs: Vec<(Point, Point)> = pieces.iter().map(|&(a, b)| (shift(a, dy), shift(b, dy))).collect();
            w.flood_line(&segs)?;
        } else {
            for (a, b) in pieces {
                let pts = match &layer.wobble {
                    Some(wb) => wobble_polyline(&[a, b], wb),
                    None => vec![a, b],
                };
                let shifted: Vec<Point> = pts.iter().map(|&q| shift(q, dy)).collect();
                w.path(&shifted, false)?;
            }
        }
    }
    Ok(())
}

fn sliced_masks(s: &SlicedImage, k: u32) -> Vec<Vec<(Point, Point)>> {
    let thr = (255.0 * (k - 1) as f64 / s.slices.max(1) as f64) as u16;
    mask_lines(s, |v| v as u16 > thr)
}

fn mask_lines(s: &SlicedImage, keep: impl Fn(u8) -> bool) -> Vec<Vec<(Point, Point)>> {
    let (w, h) = (s.width_px as usize, s.height_px as usize);
    let (pw, ph) = (s.size_mm.0 / w as f64, s.size_mm.1 / h as f64);
    let row_step = ((s.interval_mm / ph).round() as usize).max(1);
    let mut out = Vec::new();
    let mut n = 0usize;
    let mut row = 0usize;
    while row < h {
        let y = s.origin.y + s.size_mm.1 - (row as f64 + 0.5) * ph;
        let mut segs = Vec::new();
        let mut x = 0usize;
        while x < w {
            if keep(s.depth[row * w + x]) {
                let start = x;
                while x < w && keep(s.depth[row * w + x]) {
                    x += 1;
                }
                segs.push((Point::new(s.origin.x + start as f64 * pw, y), Point::new(s.origin.x + x as f64 * pw, y)));
            } else {
                x += 1;
            }
        }
        if !segs.is_empty() {
            if s.bidirectional && n % 2 == 1 {
                segs.reverse();
                for sg in &mut segs {
                    *sg = (sg.1, sg.0);
                }
            }
            n += 1;
            out.push(segs);
        }
        row += row_step;
    }
    out
}

fn validate(job: &Job) -> Result<(), CompileError> {
    for l in job.layers.iter().filter(|l| l.enabled) {
        if l.passes == 0 {
            return Err(CompileError::Invalid(format!("Layer '{}' has zero passes.", l.name)));
        }
        if !(0.0..=100.0).contains(&l.params.power_pct) || l.params.frequency_khz <= 0.0 || l.params.speed_mm_s <= 0.0 {
            return Err(CompileError::Invalid(format!("Layer '{}' has invalid power, frequency or speed.", l.name)));
        }
        match &l.mode {
            LayerMode::Drill(d) if !(d.dwell_ms > 0.0 && d.dwell_ms <= 60_000.0 && d.spacing_mm >= 0.0) => {
                return Err(CompileError::Invalid(format!(
                    "Layer '{}' needs a drill time above zero (at most 60 s) and a point spacing of zero or more.",
                    l.name
                )));
            }
            LayerMode::Fill(f) | LayerMode::Flood(f) if f.interval_mm <= 0.0 => {
                return Err(CompileError::Invalid(format!("Layer '{}' needs a line interval above zero.", l.name)));
            }
            LayerMode::Sliced(s)
                if s.depth.len() != (s.width_px * s.height_px) as usize
                    || s.width_px == 0
                    || s.height_px == 0
                    || s.slices == 0
                    || s.interval_mm <= 0.0 =>
            {
                return Err(CompileError::Invalid(format!("Layer '{}' has an invalid 3D image.", l.name)));
            }
            _ => {}
        }
    }
    if job.repeat.count == 0 {
        return Err(CompileError::Invalid("Repeat count must be at least 1.".into()));
    }
    Ok(())
}

fn flush<'a>(w: &mut Writer<'a>, dev: &'a DeviceSettings, field: &'a Field, body: &mut Vec<Step>, est: &mut Estimate) {
    let done = std::mem::replace(w, Writer::new(dev, field));
    if !done.is_empty() {
        let (p, e) = done.finish();
        est.add(&e);
        body.push(Step::List(p));
    }
}

/// Compiles a job.
pub fn compile(job: &Job, dev: &DeviceSettings, opt: &CompileOptions) -> Result<CompiledJob, CompileError> {
    validate(job)?;
    let field = Field::new(dev.field.clone());
    let bounds = job.bounds(opt.tolerance_mm).ok_or(CompileError::NothingToMark)?;
    let windows = match &job.rotary {
        Some(rc) => plan_windows(job, rc, dev.field.size_mm, opt)?,
        None => vec![Window { center_y: None, band: None, shapes: None, first_band: true, last_band: true }],
    };

    // one repeat body
    let mut body: Vec<Step> = Vec::new();
    let mut est = Estimate::default();
    for win in &windows {
        if let (Some(rc), Some(cy)) = (&job.rotary, win.center_y) {
            body.push(Step::MoveAxis { axis: rc.axis, mm: cy });
            est.motion_s += MOTION_STEP_S;
        }
        let mut w = Writer::new(dev, &field);
        for (li, layer) in job.layers.iter().enumerate().filter(|(_, l)| l.enabled) {
            if let LayerMode::Sliced(s) = &layer.mode {
                flush(&mut w, dev, &field, &mut body, &mut est);
                emit_sliced(dev, &field, layer, s, job.door_protect, &mut body, &mut est)?;
                continue;
            }
            let shapes = layer_polys(layer, li, win, opt);
            if shapes.iter().all(|s| s.is_empty()) {
                continue;
            }
            for pass in 0..layer.passes {
                if job.door_protect {
                    flush(&mut w, dev, &field, &mut body, &mut est);
                    body.push(Step::WaitDoorClosed);
                }
                w.set_params(&layer.params);
                match &layer.mode {
                    LayerMode::Line => emit_line_layer(&mut w, layer, &shapes, win, job, opt)?,
                    LayerMode::Drill(d) => emit_drill_layer(&mut w, &shapes, d, win)?,
                    LayerMode::Fill(fs) => emit_fill_layer(&mut w, layer, &shapes, fs, false, pass, win)?,
                    LayerMode::Flood(fs) => emit_fill_layer(&mut w, layer, &shapes, fs, true, pass, win)?,
                    LayerMode::Sliced(_) => unreachable!("handled above"),
                }
                if job.door_protect {
                    flush(&mut w, dev, &field, &mut body, &mut est);
                }
            }
        }
        flush(&mut w, dev, &field, &mut body, &mut est);
    }
    if !body.iter().any(|s| matches!(s, Step::List(_))) {
        return Err(CompileError::NothingToMark);
    }

    // repeats
    let mut steps = Vec::new();
    let mut total = Estimate::default();
    for r in 0..job.repeat.count {
        if r > 0 {
            if job.repeat.wait_foot_each {
                steps.push(Step::WaitFootSwitch);
            }
            if let Some((axis, step)) = job.repeat.axis_step {
                steps.push(Step::MoveAxis { axis, mm: step * r as f64 });
                total.motion_s += MOTION_STEP_S;
            }
        }
        steps.extend(body.iter().cloned());
        total.add(&est);
    }
    Ok(CompiledJob { steps, bounds_mm: bounds, estimate: total, frame_key: bounds_key(bounds) })
}

fn emit_sliced(
    dev: &DeviceSettings,
    field: &Field,
    layer: &Layer,
    s: &SlicedImage,
    door_protect: bool,
    body: &mut Vec<Step>,
    est: &mut Estimate,
) -> Result<(), CompileError> {
    let run = |lines: Vec<Vec<(Point, Point)>>, params: &MarkParams, z: f64, body: &mut Vec<Step>, est: &mut Estimate| -> Result<(), CompileError> {
        if lines.is_empty() {
            return Ok(());
        }
        // Every slice is a stage of its own, so each gets the door check a layer pass gets.
        if door_protect {
            body.push(Step::WaitDoorClosed);
        }
        body.push(Step::MoveZ { mm: z });
        est.motion_s += MOTION_STEP_S;
        let mut w = Writer::new(dev, field);
        w.set_params(params);
        for l in lines {
            w.flood_line(&l)?;
        }
        let (p, e) = w.finish();
        est.add(&e);
        body.push(Step::List(p));
        Ok(())
    };
    for _ in 0..layer.passes {
        for k in 1..=s.slices {
            run(sliced_masks(s, k), &layer.params, s.z_step_mm * (k - 1) as f64, body, est)?;
        }
        if let Some(c) = &s.cleanup {
            run(mask_lines(s, |v| v > 0), c, s.z_step_mm * (s.slices - 1) as f64, body, est)?;
        }
    }
    body.push(Step::MoveZ { mm: 0.0 });
    est.motion_s += MOTION_STEP_S;
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// red-light framing

/// A red-light frame: the outline is traced with the laser off and the red pointer on.
#[derive(Clone, Debug, PartialEq)]
pub struct FrameSpec {
    /// Closed outline in mm.
    pub outline: Vec<Point>,
    /// Shifts the outline (arrow keys in the framing window).
    pub nudge_mm: (f64, f64),
    /// Scales the outline about its centre (PgUp / PgDn); 100 = as is.
    pub scale_pct: f64,
    pub speed_mm_s: Option<f64>,
    pub delay_dark: Option<i32>,
    pub delay_light: Option<i32>,
    /// Ties this frame to a job ([`CompiledJob::frame_key`]); derived from the outline when
    /// built by [`FrameSpec::from_rect`].
    pub key: u64,
}

impl FrameSpec {
    pub fn from_rect(r: Rect) -> Self {
        Self {
            outline: vec![Point::new(r.x0, r.y0), Point::new(r.x1, r.y0), Point::new(r.x1, r.y1), Point::new(r.x0, r.y1)],
            nudge_mm: (0.0, 0.0),
            scale_pct: 100.0,
            speed_mm_s: None,
            delay_dark: None,
            delay_light: None,
            key: bounds_key(r),
        }
    }

    /// Bounding rectangle of everything the job marks.
    pub fn from_job_bounds(job: &Job, tolerance_mm: f64) -> Option<Self> {
        job.bounds(tolerance_mm).map(Self::from_rect)
    }

    /// Convex hull of everything the job marks (tighter than the rectangle). The key is the
    /// bounds key, so a hull frame also satisfies "require framing".
    pub fn from_job_hull(job: &Job, tolerance_mm: f64) -> Option<Self> {
        let b = job.bounds(tolerance_mm)?;
        let hull = job.hull(tolerance_mm);
        (hull.len() >= 3).then(|| Self { outline: hull, key: bounds_key(b), ..Self::from_rect(b) })
    }

    /// Outline after scale and nudge.
    pub fn final_outline(&self) -> Vec<Point> {
        let n = self.outline.len().max(1) as f64;
        let (sx, sy) = self.outline.iter().fold((0.0, 0.0), |a, p| (a.0 + p.x, a.1 + p.y));
        let (cx, cy) = (sx / n, sy / n);
        let k = self.scale_pct / 100.0;
        self.outline.iter().map(|p| Point::new(cx + (p.x - cx) * k + self.nudge_mm.0, cy + (p.y - cy) * k + self.nudge_mm.1)).collect()
    }
}

/// A compiled frame, ready to run in a loop.
#[derive(Clone, Debug, PartialEq)]
pub struct CompiledFrame {
    pub packets: Vec<ListPacket>,
    pub bounds_mm: Rect,
    pub key: u64,
}

/// Compiles a frame. The list contains jumps only: it can never fire the laser.
pub fn compile_frame(spec: &FrameSpec, dev: &DeviceSettings) -> Result<CompiledFrame, CompileError> {
    let outline = spec.final_outline();
    if outline.len() < 2 {
        return Err(CompileError::NothingToMark);
    }
    if !(1.0..=1000.0).contains(&spec.scale_pct) {
        return Err(CompileError::Invalid("Frame scale must be between 1 and 1000 percent.".into()));
    }
    let field = Field::new(dev.field.clone());
    let upmm = field.units_per_mm();
    let mut cmds = vec![ListCmd::ReadyMark, ListCmd::WritePort(1u16 << dev.ports.light_bit)];
    cmds.push(ListCmd::JumpSpeed(speed_word(spec.speed_mm_s.unwrap_or(dev.light.speed_mm_s), upmm)));
    cmds.push(ListCmd::JumpDelay(spec.delay_light.unwrap_or(dev.light.delay_light)));
    let mut last = (32768u16, 32768u16);
    let mut first = true;
    for p in outline.iter().chain(std::iter::once(&outline[0])) {
        let g = field.mm_to_galvo(*p)?;
        if first {
            cmds.push(ListCmd::JumpDelay(spec.delay_dark.unwrap_or(dev.light.delay_dark)));
            first = false;
        }
        cmds.push(ListCmd::JumpTo { x: g.0, y: g.1, angle: 0, distance: field.galvo_distance(last, g) });
        last = g;
    }
    debug_assert!(!cmds.iter().any(|c| c.emits()));
    let mut r = Rect::new(outline[0].x, outline[0].y, outline[0].x, outline[0].y);
    for p in &outline {
        r = r.union_pt(*p);
    }
    // A shifted or scaled frame shows another place than the job marks, so it carries the key of
    // what it showed, not the job's: it must not count for "require framing".
    let as_is = spec.nudge_mm == (0.0, 0.0) && spec.scale_pct == 100.0;
    Ok(CompiledFrame { packets: packetize(&cmds), bounds_mm: r, key: if as_is { spec.key } else { bounds_key(r) } })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::field::{FieldConfig, FieldOrigin};

    fn dev() -> DeviceSettings {
        DeviceSettings { field: FieldConfig { size_mm: 100.0, origin: FieldOrigin::Center, ..Default::default() }, ..Default::default() }
    }

    fn rect_path(x: f64, y: f64, w: f64, h: f64) -> BezPath {
        let mut p = BezPath::new();
        p.move_to((x, y));
        p.line_to((x + w, y));
        p.line_to((x + w, y + h));
        p.line_to((x, y + h));
        p.close_path();
        p
    }

    fn line_layer(g: Vec<BezPath>) -> Layer {
        Layer::new("L", LayerMode::Line, MarkParams::default(), g)
    }

    fn marks(c: &CompiledJob) -> usize {
        c.list_commands().iter().filter(|c| matches!(c, ListCmd::MarkTo { .. })).count()
    }

    #[test]
    fn square_outline_compiles() {
        let mut job = Job::new();
        job.layers.push(line_layer(vec![rect_path(-10.0, -10.0, 20.0, 20.0)]));
        let c = compile(&job, &dev(), &CompileOptions::default()).unwrap();
        let cmds = c.list_commands();
        assert_eq!(cmds[0], ListCmd::ReadyMark);
        assert!(cmds.iter().any(|c| matches!(c, ListCmd::QSwitchPeriod(667))));
        assert!(cmds.iter().any(|c| matches!(c, ListCmd::MarkCurrent(2048))));
        assert_eq!(marks(&c), 4);
        assert_eq!(cmds.iter().filter(|c| matches!(c, ListCmd::JumpTo { .. })).count(), 1);
        assert_eq!(cmds.last(), Some(&ListCmd::WritePort(0)));
        assert_eq!(c.bounds_mm, Rect::new(-10.0, -10.0, 10.0, 10.0));
        assert!(c.emits());
        // 80 mm of marking at 500 mm/s = 0.16 s
        assert!((c.estimate.mark_s - 0.16).abs() < 0.01, "{:?}", c.estimate);
        assert!(c.estimate.total_s() > c.estimate.mark_s);
    }

    #[test]
    fn out_of_field_is_an_error() {
        let mut job = Job::new();
        job.layers.push(line_layer(vec![rect_path(0.0, 0.0, 120.0, 10.0)]));
        assert!(matches!(compile(&job, &dev(), &CompileOptions::default()), Err(CompileError::OutOfField(_))));
    }

    #[test]
    fn empty_and_invalid_jobs() {
        let job = Job::new();
        assert_eq!(compile(&job, &dev(), &CompileOptions::default()), Err(CompileError::NothingToMark));
        let mut j2 = Job::new();
        let mut l = line_layer(vec![rect_path(0.0, 0.0, 5.0, 5.0)]);
        l.params.power_pct = 150.0;
        j2.layers.push(l);
        assert!(matches!(compile(&j2, &dev(), &CompileOptions::default()), Err(CompileError::Invalid(_))));
    }

    #[test]
    fn params_per_layer_and_mopa_pulse_width() {
        let mut d = dev();
        d.source = LaserSource::Mopa;
        let mut job = Job::new();
        let mut a = line_layer(vec![rect_path(-10.0, -10.0, 5.0, 5.0)]);
        a.params = MarkParams { frequency_khz: 100.0, q_pulse_ns: Some(60), power_pct: 20.0, speed_mm_s: 1000.0, timings: None };
        let mut b = line_layer(vec![rect_path(5.0, 5.0, 5.0, 5.0)]);
        b.params = MarkParams { frequency_khz: 50.0, q_pulse_ns: Some(200), power_pct: 80.0, speed_mm_s: 250.0, timings: None };
        job.layers.extend([a, b]);
        let cmds = compile(&job, &d, &CompileOptions::default()).unwrap().list_commands();
        assert!(cmds.contains(&ListCmd::FiberYlpmPulseWidth(60)) && cmds.contains(&ListCmd::FiberYlpmPulseWidth(200)));
        assert!(cmds.contains(&ListCmd::QSwitchPeriod(200)) && cmds.contains(&ListCmd::QSwitchPeriod(400)));
        let speeds: Vec<_> = cmds.iter().filter_map(|c| if let ListCmd::MarkSpeed(s) = c { Some(*s) } else { None }).collect();
        assert_eq!(speeds.len(), 2);
        assert!(speeds[0] > speeds[1]);
    }

    #[test]
    fn timings_are_emitted_and_overridable() {
        let mut job = Job::new();
        let mut l = line_layer(vec![rect_path(-10.0, -10.0, 5.0, 5.0)]);
        l.params.timings = Some(Timings {
            jump_speed_mm_s: 4000.0,
            laser_on_tc: -40,
            laser_off_tc: 60,
            end_tc: 500,
            polygon_tc: 250,
            min_jump_delay: 3,
            max_jump_delay: 90,
            jump_distance_limit_mm: 1.0,
        });
        job.layers.push(l);
        let cmds = compile(&job, &dev(), &CompileOptions::default()).unwrap().list_commands();
        assert!(cmds.contains(&ListCmd::LaserOnDelay(-40)));
        assert!(cmds.contains(&ListCmd::LaserOffDelay(60)));
        assert!(cmds.contains(&ListCmd::PolygonDelay(25)));
        assert!(cmds.contains(&ListCmd::DelayTime { units: 50 }));
        // first jump comes from the field centre, further than 1 mm: max delay
        assert!(cmds.contains(&ListCmd::JumpDelay(90)));
    }

    #[test]
    fn short_and_long_jumps_pick_delays() {
        let mut job = Job::new();
        job.optimize = false;
        job.layers.push(line_layer(vec![rect_path(-40.0, -40.0, 1.0, 1.0), rect_path(-38.0, -40.0, 1.0, 1.0), rect_path(30.0, 30.0, 1.0, 1.0)]));
        let cmds = compile(&job, &dev(), &CompileOptions::default()).unwrap().list_commands();
        let delays: Vec<_> = cmds.iter().filter_map(|c| if let ListCmd::JumpDelay(d) = c { Some(*d) } else { None }).collect();
        assert_eq!(delays, vec![200, 8, 200]);
    }

    #[test]
    fn hatch_fill_marks_the_area() {
        let mut job = Job::new();
        job.layers.push(Layer::new(
            "F",
            LayerMode::Fill(FillSettings { interval_mm: 1.0, ..Default::default() }),
            MarkParams::default(),
            vec![rect_path(-5.0, -5.0, 10.0, 10.0)],
        ));
        let c = compile(&job, &dev(), &CompileOptions::default()).unwrap();
        assert_eq!(marks(&c), 10);
        assert_eq!(c.list_commands().iter().filter(|c| matches!(c, ListCmd::JumpTo { .. })).count(), 10);
    }

    #[test]
    fn flood_fill_has_fewer_delays_than_fill() {
        let hole = rect_path(-5.0, -5.0, 10.0, 10.0);
        let inner = rect_path(-2.0, -2.0, 4.0, 4.0);
        let mut both = hole.clone();
        both.extend(inner.iter());
        let fs = FillSettings { interval_mm: 1.0, ..Default::default() };
        let mk = |mode| {
            let mut job = Job::new();
            job.layers.push(Layer::new("F", mode, MarkParams::default(), vec![both.clone()]));
            compile(&job, &dev(), &CompileOptions::default()).unwrap()
        };
        let fill = mk(LayerMode::Fill(fs));
        let flood = mk(LayerMode::Flood(fs));
        assert_eq!(marks(&fill), marks(&flood));
        assert!(flood.estimate.delay_s < fill.estimate.delay_s);
        let jd = |c: &CompiledJob| c.list_commands().iter().filter(|c| matches!(c, ListCmd::JumpDelay(0))).count();
        assert!(jd(&flood) > 0 && jd(&fill) == 0);
    }

    #[test]
    fn angle_per_pass_and_auto_rotate() {
        assert_eq!(AngleMode::Fixed.angle(10.0, 3, 4), 10.0);
        assert_eq!(AngleMode::PerPass { increment_deg: 15.0 }.angle(0.0, 2, 4), 30.0);
        assert_eq!(AngleMode::AutoRotate.angle(0.0, 1, 4), 45.0);
        let mut job = Job::new();
        let mut l = Layer::new(
            "F",
            LayerMode::Fill(FillSettings {
                interval_mm: 2.0,
                angle_mode: AngleMode::PerPass { increment_deg: 90.0 },
                bidirectional: false,
                ..Default::default()
            }),
            MarkParams::default(),
            vec![rect_path(-5.0, -5.0, 10.0, 10.0)],
        );
        l.passes = 2;
        job.layers.push(l);
        let c = compile(&job, &dev(), &CompileOptions::default()).unwrap();
        let cmds = c.list_commands();
        let first_mark = cmds.iter().position(|c| matches!(c, ListCmd::MarkTo { .. })).unwrap();
        let ListCmd::MarkTo { x: x1, y: y1, .. } = cmds[first_mark] else { unreachable!() };
        let ListCmd::JumpTo { x: jx, y: jy, .. } = cmds[..first_mark].iter().rev().find(|c| matches!(c, ListCmd::JumpTo { .. })).copied().unwrap()
        else {
            unreachable!()
        };
        assert_eq!(jy, y1, "pass 1 is horizontal");
        assert_ne!(jx, x1);
        // pass 2 vertical: find the first mark after half the marks
        let all: Vec<_> = cmds.to_vec();
        let mid = all.iter().enumerate().filter(|(_, c)| matches!(c, ListCmd::MarkTo { .. })).map(|(i, _)| i).nth(5).unwrap();
        let ListCmd::MarkTo { x: x2, .. } = all[mid] else { unreachable!() };
        let ListCmd::JumpTo { x: jx2, .. } = all[..mid].iter().rev().find(|c| matches!(c, ListCmd::JumpTo { .. })).copied().unwrap() else {
            unreachable!()
        };
        assert_eq!(jx2, x2, "pass 2 is vertical");
    }

    #[test]
    fn wobble_adds_marks() {
        let mut plain = Job::new();
        plain.layers.push(line_layer(vec![rect_path(-10.0, -10.0, 20.0, 20.0)]));
        let mut wob = plain.clone();
        wob.layers[0].wobble = Some(Wobble::default());
        let a = compile(&plain, &dev(), &CompileOptions::default()).unwrap();
        let b = compile(&wob, &dev(), &CompileOptions::default()).unwrap();
        assert!(marks(&b) > marks(&a) * 20);
    }

    #[test]
    fn passes_repeat_the_layer() {
        let mut job = Job::new();
        let mut l = line_layer(vec![rect_path(-10.0, -10.0, 20.0, 20.0)]);
        l.passes = 3;
        job.layers.push(l);
        let c = compile(&job, &dev(), &CompileOptions::default()).unwrap();
        assert_eq!(marks(&c), 12);
    }

    #[test]
    fn repeat_marking_with_axis() {
        let mut job = Job::new();
        job.layers.push(line_layer(vec![rect_path(-5.0, -5.0, 10.0, 10.0)]));
        job.repeat = RepeatMarking { count: 3, axis_step: Some((1, 25.0)), wait_foot_each: true };
        let c = compile(&job, &dev(), &CompileOptions::default()).unwrap();
        let axis: Vec<f64> = c.steps.iter().filter_map(|s| if let Step::MoveAxis { mm, .. } = s { Some(*mm) } else { None }).collect();
        assert_eq!(axis, vec![25.0, 50.0]);
        assert_eq!(c.steps.iter().filter(|s| matches!(s, Step::List(_))).count(), 3);
        assert_eq!(c.steps.iter().filter(|s| matches!(s, Step::WaitFootSwitch)).count(), 2);
        assert_eq!(marks(&c), 12);
        let mut one = job.clone();
        one.repeat.count = 1;
        let single = compile(&one, &dev(), &CompileOptions::default()).unwrap();
        assert!((c.estimate.total_s() - 3.0 * single.estimate.total_s() - 2.0 * MOTION_STEP_S).abs() < 1e-9);
    }

    #[test]
    fn door_protect_makes_stages() {
        let mut job = Job::new();
        job.door_protect = true;
        let mut l = line_layer(vec![rect_path(-5.0, -5.0, 10.0, 10.0)]);
        l.passes = 2;
        job.layers.push(l);
        job.layers.push(line_layer(vec![rect_path(10.0, 10.0, 5.0, 5.0)]));
        let c = compile(&job, &dev(), &CompileOptions::default()).unwrap();
        let lists = c.steps.iter().filter(|s| matches!(s, Step::List(_))).count();
        let waits = c.steps.iter().filter(|s| matches!(s, Step::WaitDoorClosed)).count();
        assert_eq!((lists, waits), (3, 3));
        assert!(matches!(c.steps[0], Step::WaitDoorClosed));
        let mut plain = job.clone();
        plain.door_protect = false;
        let p = compile(&plain, &dev(), &CompileOptions::default()).unwrap();
        assert_eq!(p.steps.len(), 1);
    }

    fn drill_layer(g: Vec<BezPath>, dwell_ms: f64, spacing_mm: f64) -> Layer {
        Layer::new("D", LayerMode::Drill(DrillSettings { dwell_ms, spacing_mm }), MarkParams::default(), g)
    }

    /// Where each shot lands (mm) and its total dwell in 10 microsecond units.
    fn shots(c: &CompiledJob) -> Vec<(Point, u32)> {
        let field = Field::new(dev().field);
        let mut at = Point::ORIGIN;
        let mut out: Vec<(Point, u32)> = Vec::new();
        let mut fresh = true;
        for cmd in c.list_commands() {
            match cmd {
                ListCmd::JumpTo { x, y, .. } => {
                    at = field.galvo_to_mm(x as f64, y as f64);
                    fresh = true;
                }
                ListCmd::LaserOnPoint { dwell } => {
                    if fresh {
                        out.push((at, dwell as u32));
                        fresh = false;
                    } else {
                        out.last_mut().unwrap().1 += dwell as u32;
                    }
                }
                _ => {}
            }
        }
        out
    }

    #[test]
    fn drill_points_are_the_corners_or_evenly_spread() {
        let sq = [Point::new(0.0, 0.0), Point::new(10.0, 0.0), Point::new(10.0, 10.0), Point::new(0.0, 10.0)];
        assert_eq!(drill_points(&sq, true, 0.0), sq, "spacing 0: the corners");
        // A closed square with a repeated first point is still four corners.
        let mut again = sq.to_vec();
        again.push(sq[0]);
        assert_eq!(drill_points(&again, true, 0.0).len(), 4);
        // 40 mm round, every 5 mm: eight points, none twice.
        let eight = drill_points(&sq, true, 5.0);
        assert_eq!(eight.len(), 8);
        assert_eq!(eight[0], Point::new(0.0, 0.0));
        assert!((eight[1] - Point::new(5.0, 0.0)).hypot() < 1e-9 && (eight[3] - Point::new(10.0, 5.0)).hypot() < 1e-9);
        assert!((eight[7] - Point::new(0.0, 5.0)).hypot() < 1e-9);
        // An open 10 mm line every 3 mm: both ends and two between, evenly.
        let line = [Point::new(0.0, 0.0), Point::new(10.0, 0.0)];
        let four = drill_points(&line, false, 3.0);
        assert_eq!(four.len(), 4);
        assert!((four[3] - line[1]).hypot() < 1e-9 && (four[1].x - 10.0 / 3.0).abs() < 1e-9);
        // A spacing longer than the line: just its ends. A single point stays one point.
        assert_eq!(drill_points(&line, false, 50.0), line);
        assert_eq!(drill_points(&line[..1], false, 1.0), line[..1]);
        assert!(drill_points(&[], true, 1.0).is_empty());
    }

    #[test]
    fn drill_layer_fires_in_place_and_never_marks_a_line() {
        let mut job = Job::new();
        job.layers.push(drill_layer(vec![rect_path(-10.0, -10.0, 20.0, 20.0)], 2.5, 0.0));
        let c = compile(&job, &dev(), &CompileOptions::default()).unwrap();
        assert_eq!(marks(&c), 0, "no line is drawn");
        let s = shots(&c);
        assert_eq!(s.len(), 4, "one shot per corner: {s:?}");
        for (p, dwell) in &s {
            assert!((p.x.abs() - 10.0).abs() < 0.05 && (p.y.abs() - 10.0).abs() < 0.05, "{p:?}");
            assert_eq!(*dwell, 250, "2.5 ms in 10 microsecond units");
        }
        // The power and frequency are set as for any layer.
        assert!(c.list_commands().iter().any(|c| matches!(c, ListCmd::QSwitchPeriod(667))));
        assert!(c.estimate.mark_s > 0.0099 && c.estimate.mark_s < 0.0101, "four shots of 2.5 ms: {}", c.estimate.mark_s);
        assert!((c.bounds_mm.width() - 20.0).abs() < 1e-6);
    }

    #[test]
    fn drill_spacing_passes_and_long_dwells() {
        let mut job = Job::new();
        let mut l = drill_layer(vec![rect_path(0.0, 0.0, 10.0, 10.0)], 1.0, 2.0);
        l.passes = 3;
        job.layers.push(l);
        let c = compile(&job, &dev(), &CompileOptions::default()).unwrap();
        assert_eq!(shots(&c).len(), 20 * 3, "40 mm round every 2 mm, three passes");
        // A dwell longer than one command carries is sent as several, adding up.
        let mut long = Job::new();
        let mut line = BezPath::new();
        line.move_to((0.0, 0.0));
        line.line_to((5.0, 0.0));
        long.layers.push(drill_layer(vec![line], 1500.0, 0.0));
        let c = compile(&long, &dev(), &CompileOptions::default()).unwrap();
        let s = shots(&c);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].1, 150_000);
        assert!(c.list_commands().iter().all(|c| !matches!(c, ListCmd::LaserOnPoint { dwell } if *dwell > MAX_POINT_DWELL)));
        assert_eq!(c.list_commands().iter().filter(|c| matches!(c, ListCmd::LaserOnPoint { .. })).count(), 6);
    }

    #[test]
    fn drill_settings_are_checked() {
        for (dwell, spacing) in [(0.0, 1.0), (-1.0, 1.0), (f64::NAN, 1.0), (70_000.0, 1.0), (1.0, -0.5)] {
            let mut job = Job::new();
            job.layers.push(drill_layer(vec![rect_path(0.0, 0.0, 5.0, 5.0)], dwell, spacing));
            assert!(matches!(compile(&job, &dev(), &CompileOptions::default()), Err(CompileError::Invalid(_))), "{dwell} {spacing}");
        }
    }

    #[test]
    fn drill_with_a_rotary_drills_each_point_once_in_its_window() {
        let mut job = Job::new();
        job.layers.push(drill_layer(vec![rect_path(-10.0, 0.0, 20.0, 90.0)], 1.0, 10.0));
        job.rotary = Some(RotaryConfig { mode: RotaryMode::BySlice, max_size_mm: 40.0, axis: 0 });
        let c = compile(&job, &dev(), &CompileOptions::default()).unwrap();
        assert_eq!(c.steps.iter().filter(|s| matches!(s, Step::MoveAxis { .. })).count(), 3);
        let s = shots(&c);
        assert_eq!(s.len(), 22, "220 mm round every 10 mm, none lost or doubled at the band edges");
        for (p, _) in &s {
            assert!(p.y.abs() <= 15.0 + 0.05, "inside the band: {p:?}");
        }
    }

    #[test]
    fn rotary_by_slice_splits_and_recentres() {
        let mut job = Job::new();
        job.layers.push(line_layer(vec![rect_path(-10.0, 0.0, 20.0, 90.0)]));
        job.rotary = Some(RotaryConfig { mode: RotaryMode::BySlice, max_size_mm: 40.0, axis: 0 });
        let c = compile(&job, &dev(), &CompileOptions::default()).unwrap();
        let moves: Vec<f64> = c.steps.iter().filter_map(|s| if let Step::MoveAxis { mm, .. } = s { Some(*mm) } else { None }).collect();
        assert_eq!(moves, vec![15.0, 45.0, 75.0]);
        assert_eq!(c.steps.iter().filter(|s| matches!(s, Step::List(_))).count(), 3);
        // every mark stays inside +-15 mm of the field centre in Y
        let field = Field::new(dev().field);
        for cmd in c.list_commands() {
            if let ListCmd::MarkTo { y, x, .. } | ListCmd::JumpTo { y, x, .. } = cmd {
                let p = field.galvo_to_mm(x as f64, y as f64);
                assert!(p.y.abs() <= 15.0 + 0.05, "{p:?}");
            }
        }
    }

    #[test]
    fn rotary_by_shape_keeps_shapes_whole() {
        let mut job = Job::new();
        job.layers.push(line_layer(vec![
            rect_path(0.0, 0.0, 10.0, 10.0),
            rect_path(0.0, 12.0, 10.0, 10.0),
            rect_path(0.0, 100.0, 10.0, 10.0),
            rect_path(0.0, 200.0, 10.0, 90.0),
        ]));
        job.rotary = Some(RotaryConfig { mode: RotaryMode::ByShape, max_size_mm: 40.0, axis: 0 });
        let c = compile(&job, &dev(), &CompileOptions::default()).unwrap();
        let moves: Vec<f64> = c.steps.iter().filter_map(|s| if let Step::MoveAxis { mm, .. } = s { Some(*mm) } else { None }).collect();
        // two small shapes share a window, one alone, the tall one is sliced into 3
        assert_eq!(moves.len(), 2 + 3);
        assert_eq!(moves[0], 11.0);
        assert_eq!(moves[1], 105.0);
    }

    /// Total length of everything marked, in mm.
    fn marked_mm(c: &CompiledJob) -> f64 {
        let field = Field::new(dev().field);
        let (mut at, mut sum) = (Point::ORIGIN, 0.0);
        for cmd in c.list_commands() {
            match cmd {
                ListCmd::JumpTo { x, y, .. } => at = field.galvo_to_mm(x as f64, y as f64),
                ListCmd::MarkTo { x, y, .. } => {
                    let to = field.galvo_to_mm(x as f64, y as f64);
                    sum += at.distance(to);
                    at = to;
                }
                _ => {}
            }
        }
        sum
    }

    #[test]
    fn rotary_marks_a_line_on_the_edge_between_two_bands_once() {
        // Bands 0..40 and 40..80: the top of the square lies exactly on the edge between them.
        let mut tick = BezPath::new();
        tick.move_to((20.0, 0.0));
        tick.line_to((20.0, 80.0));
        let mut job = Job::new();
        job.layers.push(line_layer(vec![rect_path(-10.0, 0.0, 20.0, 40.0), tick.clone()]));
        job.rotary = Some(RotaryConfig { mode: RotaryMode::BySlice, max_size_mm: 40.0, axis: 0 });
        let c = compile(&job, &dev(), &CompileOptions::default()).unwrap();
        let mm = marked_mm(&c);
        assert!((mm - 200.0).abs() < 0.2, "a 20 x 40 square and an 80 mm line are 200 mm, marked {mm}");

        // A fill whose scan line falls on the edge: 0.5..60.5 with 1 mm lines has one at y = 40.
        let mut job = Job::new();
        let fs = FillSettings { interval_mm: 1.0, bidirectional: false, ..Default::default() };
        job.layers.push(Layer::new("F", LayerMode::Fill(fs), MarkParams::default(), vec![rect_path(-10.0, 0.5, 20.0, 60.0)]));
        job.layers.push(line_layer(vec![tick]));
        job.rotary = Some(RotaryConfig { mode: RotaryMode::BySlice, max_size_mm: 40.0, axis: 0 });
        let c = compile(&job, &dev(), &CompileOptions::default()).unwrap();
        let mm = marked_mm(&c);
        assert!((mm - (60.0 * 20.0 + 80.0)).abs() < 0.5, "60 scan lines of 20 mm and the 80 mm line, marked {mm}");
    }

    #[test]
    fn rotary_by_shape_drills_the_top_of_a_sliced_shape() {
        // The tall square is cut into three bands; the small one comes after it in its own window.
        let mut job = Job::new();
        job.layers.push(drill_layer(vec![rect_path(-10.0, 0.0, 20.0, 90.0), rect_path(20.0, 5.0, 5.0, 5.0)], 1.0, 0.0));
        job.rotary = Some(RotaryConfig { mode: RotaryMode::ByShape, max_size_mm: 40.0, axis: 0 });
        let c = compile(&job, &dev(), &CompileOptions::default()).unwrap();
        assert_eq!(c.steps.iter().filter(|s| matches!(s, Step::MoveAxis { .. })).count(), 4);
        assert_eq!(shots(&c).len(), 8, "four corners each: {:?}", shots(&c));
    }

    fn step_name(s: &Step) -> &'static str {
        match s {
            Step::List(_) => "list",
            Step::MoveAxis { .. } => "axis",
            Step::MoveZ { .. } => "z",
            Step::WaitDoorClosed => "door",
            Step::WaitFootSwitch => "foot",
        }
    }

    #[test]
    fn door_protect_waits_before_every_stage_of_a_sliced_image() {
        let img = SlicedImage {
            width_px: 2,
            height_px: 1,
            depth: vec![100, 250],
            origin: Point::new(-1.0, -0.5),
            size_mm: (2.0, 1.0),
            slices: 2,
            z_step_mm: 0.1,
            interval_mm: 1.0,
            bidirectional: false,
            cleanup: Some(MarkParams::default()),
        };
        let mut job = Job::new();
        job.door_protect = true;
        job.layers.push(line_layer(vec![rect_path(-5.0, -5.0, 10.0, 10.0)]));
        job.layers.push(Layer::new("3D", LayerMode::Sliced(img), MarkParams::default(), vec![]));
        let c = compile(&job, &dev(), &CompileOptions::default()).unwrap();
        let names: Vec<&str> = c.steps.iter().map(step_name).collect();
        let mut waited = false;
        let mut lists = 0;
        for s in &c.steps {
            match s {
                Step::WaitDoorClosed => waited = true,
                Step::List(_) => {
                    assert!(waited, "a stage fires without a door check before it: {names:?}");
                    waited = false;
                    lists += 1;
                }
                _ => {}
            }
        }
        assert_eq!(lists, 4, "the outline, two slices and the cleanup");
    }

    #[test]
    fn rotary_sanity_checks() {
        let mut job = Job::new();
        job.layers.push(line_layer(vec![rect_path(0.0, 0.0, 10.0, 10.0)]));
        job.rotary = Some(RotaryConfig { mode: RotaryMode::BySlice, max_size_mm: 500.0, axis: 0 });
        assert!(matches!(compile(&job, &dev(), &CompileOptions::default()), Err(CompileError::Invalid(_))));
        job.rotary = Some(RotaryConfig { mode: RotaryMode::BySlice, max_size_mm: 1.0, axis: 0 });
        job.layers[0].geometry = vec![rect_path(0.0, 0.0, 10.0, 90.0)];
        let opt = CompileOptions { max_rotary_windows: 10, ..Default::default() };
        assert!(matches!(compile(&job, &dev(), &opt), Err(CompileError::Invalid(_))));
    }

    #[test]
    fn sliced_image_passes() {
        // 4x2 image: left half shallow (100), right half deep (250)
        let img = SlicedImage {
            width_px: 4,
            height_px: 2,
            depth: vec![100, 100, 250, 250, 100, 100, 250, 250],
            origin: Point::new(-2.0, -1.0),
            size_mm: (4.0, 2.0),
            slices: 2,
            z_step_mm: 0.1,
            interval_mm: 1.0,
            bidirectional: true,
            cleanup: Some(MarkParams { power_pct: 5.0, ..Default::default() }),
        };
        let mut job = Job::new();
        job.layers.push(Layer::new("3D", LayerMode::Sliced(img), MarkParams::default(), vec![]));
        let c = compile(&job, &dev(), &CompileOptions::default()).unwrap();
        let z: Vec<f64> = c.steps.iter().filter_map(|s| if let Step::MoveZ { mm } = s { Some(*mm) } else { None }).collect();
        assert_eq!(z.len(), 4);
        assert_eq!(z[0], 0.0);
        assert!((z[1] - 0.1).abs() < 1e-12);
        assert_eq!(*z.last().unwrap(), 0.0);
        let lists: Vec<_> = c.steps.iter().filter_map(|s| if let Step::List(p) = s { Some(unpacketize(p)) } else { None }).collect();
        assert_eq!(lists.len(), 3);
        let m = |l: &Vec<ListCmd>| l.iter().filter(|c| matches!(c, ListCmd::MarkTo { .. })).count();
        // slice 1: both halves (2 rows, 1 run each); slice 2: only the deep half; cleanup: all
        assert_eq!(m(&lists[0]), 2);
        assert_eq!(m(&lists[1]), 2);
        assert_eq!(m(&lists[2]), 2);
        assert_eq!(c.bounds_mm, Rect::new(-2.0, -1.0, 2.0, 1.0));
    }

    #[test]
    fn rotary_rejects_sliced_image() {
        let img = SlicedImage {
            width_px: 1,
            height_px: 1,
            depth: vec![255],
            origin: Point::ORIGIN,
            size_mm: (1.0, 1.0),
            slices: 1,
            z_step_mm: 0.0,
            interval_mm: 1.0,
            bidirectional: false,
            cleanup: None,
        };
        let mut job = Job::new();
        job.layers.push(Layer::new("3D", LayerMode::Sliced(img), MarkParams::default(), vec![]));
        job.layers.push(line_layer(vec![rect_path(0.0, 0.0, 5.0, 5.0)]));
        job.rotary = Some(RotaryConfig { mode: RotaryMode::BySlice, max_size_mm: 40.0, axis: 0 });
        assert!(matches!(compile(&job, &dev(), &CompileOptions::default()), Err(CompileError::Invalid(_))));
    }

    #[test]
    fn disabled_layers_are_skipped() {
        let mut job = Job::new();
        let mut l = line_layer(vec![rect_path(0.0, 0.0, 5.0, 5.0)]);
        l.enabled = false;
        job.layers.push(l);
        assert_eq!(compile(&job, &dev(), &CompileOptions::default()), Err(CompileError::NothingToMark));
    }

    #[test]
    fn long_jobs_span_many_packets() {
        let mut job = Job::new();
        job.layers.push(Layer::new(
            "F",
            LayerMode::Fill(FillSettings { interval_mm: 0.1, ..Default::default() }),
            MarkParams::default(),
            vec![rect_path(-20.0, -20.0, 40.0, 40.0)],
        ));
        let c = compile(&job, &dev(), &CompileOptions::default()).unwrap();
        assert!(c.packet_count() > 2);
    }

    #[test]
    fn frame_never_fires_and_applies_nudge_scale() {
        let mut spec = FrameSpec::from_rect(Rect::new(-10.0, -10.0, 10.0, 10.0));
        spec.nudge_mm = (5.0, -5.0);
        spec.scale_pct = 50.0;
        let f = compile_frame(&spec, &dev()).unwrap();
        let cmds = unpacketize(&f.packets);
        assert!(!cmds.iter().any(|c| c.emits()));
        assert!(cmds.contains(&ListCmd::WritePort(1 << 8)));
        assert_eq!(f.bounds_mm, Rect::new(0.0, -10.0, 10.0, 0.0));
        assert_eq!(cmds.iter().filter(|c| matches!(c, ListCmd::JumpTo { .. })).count(), 5);
        spec.scale_pct = 0.0;
        assert!(compile_frame(&spec, &dev()).is_err());
    }

    #[test]
    fn frame_key_matches_job_key() {
        let mut job = Job::new();
        job.layers.push(line_layer(vec![rect_path(-10.0, -10.0, 20.0, 5.0)]));
        let c = compile(&job, &dev(), &CompileOptions::default()).unwrap();
        let rect = FrameSpec::from_job_bounds(&job, 0.01).unwrap();
        assert_eq!(rect.key, c.frame_key);
        let mut tri = BezPath::new();
        tri.move_to((0.0, 0.0));
        tri.line_to((10.0, 0.0));
        tri.line_to((0.0, 10.0));
        tri.close_path();
        let mut j2 = Job::new();
        j2.layers.push(line_layer(vec![tri]));
        let hull = FrameSpec::from_job_hull(&j2, 0.01).unwrap();
        assert_eq!(hull.outline.len(), 3);
        assert_eq!(hull.key, compile(&j2, &dev(), &CompileOptions::default()).unwrap().frame_key);
    }
}
