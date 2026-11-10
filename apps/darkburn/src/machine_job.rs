//! The seam between the job builder (`job_build.rs`, `job_cmds.rs`) and the machine
//! (`machine_api.rs`): a built job becomes G-code lines for a GRBL machine or an `RdJob` for a
//! Ruida controller, framing walks the job's outline, and the machine's position feeds back into
//! the job state (head, user origin).
//!
//! The commands are the job builder's registered ids (`job.start`, `job.frame`, ...). They are
//! `hardware: true`: an agent reaches the simulators only, a real machine only runs a job a
//! human started from the window (after the REAL MACHINE confirmation on Connect).

#![allow(dead_code, reason = "some helpers are used by tests and the panels only")]

use std::sync::Arc;

use darkburn_core::kurbo::{Point, Vec2};
use darkburn_job::{CoordSpace, Op, Toolpath};
use darkburn_ruida as ruida;
use serde_json::{Value, json};

use crate::job_build::{self, BuiltJob, FrameKind, JobStats};
use crate::job_cmds::{self, After};
use crate::machine_api::{Family, FrameSpec, JobData, JobRequest, LaserState};
use crate::shell::DarkBurn;

/// The job commands that reach the machine.
pub fn handles(id: &str) -> bool {
    job_cmds::hardware(id)
}

/// What to do with a job once it is built.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// Run it now (from operation `from_op` when set).
    Start { from_op: Option<usize> },
    /// Store it on the controller without running it (DSP controllers).
    Send { from_op: Option<usize> },
}

/// The machine's position as the job state keeps it.
pub fn head(app: &DarkBurn) -> Option<Point> {
    let s = app.machine.handle.snapshot();
    (s.connected() && s.has_position).then(|| Point::new(s.machine_pos[0], s.machine_pos[1]))
}

/// Each frame: where the head is, for jobs that start from the current position.
pub fn update_head(app: &mut DarkBurn) {
    // Also each frame: "Start here" does not outlive the design it was picked in.
    job_cmds::start_op(app);
    if let Some(h) = head(app) {
        app.job.head = Some(h);
    } else if !app.machine.handle.is_connected() {
        app.job.head = None;
    }
}

/// Offset that turns machine coordinates into the controller's work coordinates (GRBL: the work
/// offset; Ruida has none).
fn work_offset(app: &DarkBurn) -> Vec2 {
    let s = app.machine.handle.snapshot();
    if s.family == Family::Grbl { Vec2::new(s.machine_pos[0] - s.work_pos[0], s.machine_pos[1] - s.work_pos[1]) } else { Vec2::ZERO }
}

/// The points a toolpath visits, as work coordinates for the connected controller.
pub fn work_points(app: &DarkBurn, tp: &Toolpath) -> Vec<Point> {
    let s = app.machine.handle.snapshot();
    let shift = match tp.space {
        // Head-relative: add where the head is now (work coordinates for GRBL).
        CoordSpace::RelativeToStart => {
            if s.family == Family::Grbl {
                Vec2::new(s.work_pos[0], s.work_pos[1])
            } else {
                Vec2::new(s.machine_pos[0], s.machine_pos[1])
            }
        }
        _ => -work_offset(app),
    };
    tp.ops.iter().filter_map(Op::target).map(|p| p + shift).collect()
}

// ----- Building the machine's job -----------------------------------------------------------------

/// How a toolpath becomes a Ruida job.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RdOptions {
    /// Let the controller run past the ends of scan lines itself (RUIDA-12): scanning layers
    /// along X or Y get their hardware overscan mode and the job's own overscan moves are left
    /// out. Scans at other angles keep their overscan moves.
    pub hardware_overscan: bool,
    /// What the coordinates are measured from.
    pub reference: ruida::RefPoint,
}

impl RdOptions {
    /// The options the active device asks for.
    pub fn of(dev: &crate::devices::Device) -> Self {
        RdOptions { hardware_overscan: dev.settings.dsp.hardware_overscan, reference: ruida::RefPoint::MachineZero }
    }
}

/// The hardware overscan mode of each layer block of `tp`, in the order the blocks start.
/// A block gets a mode when it is a scanning layer whose lines all run along X or all along Y.
pub fn hardware_overscan_modes(tp: &Toolpath) -> Vec<ruida::OverscanMode> {
    fn mode_of(raster: bool, leads: &[Vec2]) -> ruida::OverscanMode {
        if !raster || leads.is_empty() {
            return ruida::OverscanMode::Off;
        }
        let horizontal = leads.iter().all(|d| d.y.abs() < 1e-6 && d.x.abs() > 1e-6);
        let vertical = leads.iter().all(|d| d.x.abs() < 1e-6 && d.y.abs() > 1e-6);
        if !horizontal && !vertical {
            return ruida::OverscanMode::Off;
        }
        let forward = |d: &Vec2| if horizontal { d.x > 0.0 } else { d.y > 0.0 };
        let both_ways = leads.iter().any(forward) && leads.iter().any(|d| !forward(d));
        ruida::OverscanMode::for_scan(horizontal, both_ways)
    }
    let mut out = Vec::new();
    let mut block: Option<(bool, Vec<Vec2>)> = None;
    let mut rapid: Option<Point> = None;
    for op in &tp.ops {
        match op {
            Op::LayerStart { layer } => {
                if let Some((raster, leads)) = block.take() {
                    out.push(mode_of(raster, &leads));
                }
                block = Some((tp.layers.get(*layer).is_some_and(|l| l.raster), Vec::new()));
                rapid = None;
            }
            Op::Rapid { to } => rapid = Some(*to),
            // The run-up to a scan line: from the end of a rapid to the start of the line.
            Op::Overscan { to, .. } => {
                if let (Some(from), Some((_, leads))) = (rapid.take(), block.as_mut()) {
                    leads.push(*to - from);
                }
            }
            _ => rapid = None,
        }
    }
    if let Some((raster, leads)) = block.take() {
        out.push(mode_of(raster, &leads));
    }
    out
}

/// Layer settings and cuts of `tp` as a Ruida job. `shift` moves head-relative toolpaths to
/// absolute controller coordinates. Arcs are cut as their chord (the planner flattens curves
/// before this, so arcs only appear when a flavor asks for them).
pub fn toolpath_to_rd(tp: &Toolpath, shift: Vec2) -> Result<ruida::RdJob, String> {
    toolpath_to_rd_with(tp, shift, RdOptions::default())
}

/// As [`toolpath_to_rd`] with the device's options.
pub fn toolpath_to_rd_with(tp: &Toolpath, shift: Vec2, options: RdOptions) -> Result<ruida::RdJob, String> {
    let mut job = ruida::RdJob { reference: options.reference, ..Default::default() };
    let mut cur: Option<usize> = None;
    let mut speed = 100.0;
    let mut laser2_only = false;
    let mut dot_layer = false;
    let mut pos = tp.start;
    let at = |p: Point| p + shift;
    let modes = if options.hardware_overscan { hardware_overscan_modes(tp) } else { Vec::new() };
    let mut block = 0usize;
    let mut hardware = false;
    // With hardware overscan the rapid to the run-up point is dropped: the head goes straight
    // to the start of the line and the controller adds the run-up and the run-out.
    let mut held: Option<Point> = None;
    for op in &tp.ops {
        if !matches!(op, Op::Rapid { .. } | Op::Overscan { .. })
            && let (Some(p), Some(i)) = (held.take(), cur)
        {
            job.layers[i].ops.push(ruida::RdOp::Move(at(p)));
        }
        match op {
            Op::LayerStart { layer } => {
                let l = tp.layers.get(*layer).ok_or("the job refers to a layer it does not have")?;
                let overscan = modes.get(block).copied().unwrap_or_default();
                block += 1;
                hardware = overscan != ruida::OverscanMode::Off;
                let [r, g, b] = darkburn_core::layer::layer_colour(l.layer_index);
                let index = job.layers.len().min(127) as u8;
                speed = l.speed;
                // Laser 1 / laser 2 / both tubes: a tube that is not used gets no power.
                let (min_power, max_power) = match (l.laser, l.dsp.power2) {
                    (2, Some([lo, hi])) => ([0.0, lo], [0.0, hi]),
                    (_, Some([lo, hi])) => ([l.min_power, lo], [l.max_power, hi]),
                    _ => ([l.min_power, 0.0], [l.max_power, 0.0]),
                };
                job.layers.push(ruida::RdLayer {
                    settings: ruida::LayerSettings {
                        index,
                        speed_mm_s: l.speed.max(0.1),
                        min_power,
                        max_power,
                        color: (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b),
                        frequency_hz: l.dsp.pwm_hz,
                        air_assist: l.air,
                        dot_mode: l.dot.map(|(ms, spacing)| ruida::DotMode { dot_time_ms: ms, spacing_mm: spacing }),
                        overscan,
                    },
                    ops: Vec::new(),
                });
                // The layer's U axis offset is applied before its first move.
                if l.dsp.u_offset != 0.0
                    && let Some(layer) = job.layers.last_mut()
                {
                    layer.ops.push(ruida::RdOp::U(l.dsp.u_offset));
                }
                laser2_only = l.laser == 2;
                dot_layer = l.dot.is_some();
                cur = Some(job.layers.len() - 1);
            }
            Op::Rapid { to } if hardware => {
                cur.ok_or("the job moves before it starts a layer")?;
                held = Some(*to);
            }
            Op::Overscan { to, .. } if hardware => {
                let i = cur.ok_or("the job moves before it starts a layer")?;
                // A run-up ends at the start of a line: go there. A run-out is the controller's.
                if held.take().is_some() {
                    job.layers[i].ops.push(ruida::RdOp::Move(at(*to)));
                }
            }
            Op::Rapid { to } | Op::Overscan { to, .. } => {
                let i = cur.ok_or("the job moves before it starts a layer")?;
                job.layers[i].ops.push(ruida::RdOp::Move(at(*to)));
            }
            Op::Cut { to, speed: s, power } | Op::Arc { to, speed: s, power, .. } => {
                let i = cur.ok_or("the job cuts before it starts a layer")?;
                let p = at(*to);
                job.layers[i].ops.push(if *power > 0.0 {
                    // The per-cut power override is for laser 1; laser 2 layers use the layer's power.
                    ruida::RdOp::Cut { to: p, power: (!laser2_only).then_some(*power), speed: Some(*s) }
                } else {
                    ruida::RdOp::Move(p)
                });
                speed = *s;
            }
            // Dot mode: the controller fires the dots itself, so each dot is a zero-length cut.
            Op::Dwell { power, .. } if dot_layer && *power > 0.0 => {
                if let Some(i) = cur {
                    job.layers[i].ops.push(ruida::RdOp::Cut { to: pos + shift, power: (!laser2_only).then_some(*power), speed: Some(speed) });
                }
            }
            Op::ZMove { z } => {
                if let Some(i) = cur {
                    job.layers[i].ops.push(ruida::RdOp::Z(*z));
                }
            }
            _ => {}
        }
        if let Some(t) = op.target() {
            pos = t;
        }
    }
    let _ = speed;
    if let (Some(p), Some(i)) = (held.take(), cur) {
        job.layers[i].ops.push(ruida::RdOp::Move(at(p)));
    }
    job.layers.retain(|l| !l.ops.is_empty());
    if job.layers.is_empty() {
        return Err("the job has nothing to send".into());
    }
    Ok(job)
}

/// What [`toolpath_to_rd_with`] leaves out of `tp`, one line per layer: a Ruida job has no
/// command for standing still with the laser on or off, so start and end pauses and cut-through
/// are not sent (the dots of dot mode are: the controller times those itself).
pub fn rd_dropped_pauses(tp: &Toolpath) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur: Option<&darkburn_job::IrLayer> = None;
    let mut said: Vec<(usize, usize)> = Vec::new();
    for op in &tp.ops {
        match op {
            Op::LayerStart { layer } => cur = tp.layers.get(*layer),
            Op::Dwell { ms, power } if *ms > 0.0 => {
                let Some(l) = cur else { continue };
                if (l.dot.is_some() && *power > 0.0) || said.contains(&(l.layer_index, l.sublayer)) {
                    continue;
                }
                said.push((l.layer_index, l.sublayer));
                out.push(format!(
                    "Layer {}: start and end pauses and cut-through are not sent to a Ruida controller; the cut runs without them.",
                    l.name
                ));
            }
            _ => {}
        }
    }
    out
}

/// Why `dev` has no machine file DarkBurn can write, if so (FILE-09).
pub fn machine_file_unsupported(dev: &crate::devices::Device) -> Option<String> {
    use crate::devices::DeviceType as T;
    match dev.kind {
        T::Trocen => Some(
            "Trocen / AWC controllers are not supported yet: the .ud5 file format is not public, so DarkBurn cannot write one. Use the controller's own software for now."
                .into(),
        ),
        T::TopWisdom => Some(
            "TopWisdom controllers are not supported yet: the .out file format is not public, so DarkBurn cannot write one. Use the controller's own software for now."
                .into(),
        ),
        _ => None,
    }
}

/// The kind of machine file the active device takes: its name, file extensions and the label of
/// the save command (FILE-09, LASER-06).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MachineFileKind {
    pub label: &'static str,
    pub filter: &'static str,
    pub extensions: &'static [&'static str],
    pub rd: bool,
}

pub fn machine_file_kind(dev: &crate::devices::Device) -> MachineFileKind {
    if dev.family() == Family::Ruida {
        MachineFileKind { label: "Save RD file", filter: "Ruida job", extensions: &[crate::rd_import::EXTENSION], rd: true }
    } else {
        MachineFileKind { label: "Save G-code", filter: "G-code", extensions: job_build::GCODE_EXTENSIONS, rd: false }
    }
}

/// The bytes of an `.rd` file for `job`: the command stream the controller would be sent, with
/// the file scrambling (RUIDA-13). A job that starts from the current position is written to be
/// measured from where the head is; every other job in bed coordinates.
pub fn rd_file_bytes(app: &DarkBurn, job: &BuiltJob, from_op: Option<usize>) -> Result<Vec<u8>, String> {
    let tp = match from_op {
        Some(op) => job.toolpath_from(op),
        None => job.toolpath.clone(),
    };
    let mut options = RdOptions::of(app.machine.active());
    if tp.space == CoordSpace::RelativeToStart {
        options.reference = ruida::RefPoint::CurrentPosition;
    }
    let rd = toolpath_to_rd_with(&tp, Vec2::ZERO, options)?;
    rd.to_rd_file(ruida::Magic::DEFAULT).map_err(|e| e.to_string())
}

/// The request for the connected machine's family.
pub fn request_for(app: &DarkBurn, job: &BuiltJob, from_op: Option<usize>, send_only: bool) -> Result<JobRequest, String> {
    let tp = match from_op {
        Some(op) => job.toolpath_from(op),
        None => job.toolpath.clone(),
    };
    let family = app.machine.handle.snapshot().family;
    let name = app.project_path.as_deref().and_then(|p| p.file_stem()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "DARKBURN".into());
    let sim = job.simulate(&app.job.additional);
    let seconds = JobStats::of(job, &sim).seconds;
    let estimated = (!app.machine.handle.is_simulator() && seconds.is_finite() && seconds > 0.0).then(|| std::time::Duration::from_secs_f64(seconds));
    let mut req = match family {
        Family::Grbl => {
            if send_only {
                return Err("A G-code machine runs a job as it streams it. Use Start (or Save G-code).".into());
            }
            // An absolute job is in machine coordinates, but the controller reads its moves in
            // work coordinates: they differ by the work offset once an origin is set. The frame
            // is sent the same way (`work_points`).
            let offset = work_offset(app);
            let tp =
                if tp.space == CoordSpace::MachineAbsolute && offset != Vec2::ZERO { darkburn_job::origin::pointer_offset(&tp, -offset) } else { tp };
            JobRequest::gcode(crate::gcode_machines::job_lines(app.machine.active(), &tp, &app.job.gcode))
        }
        Family::Ruida => {
            let shift = if tp.space == CoordSpace::RelativeToStart {
                let s = app.machine.handle.snapshot();
                Vec2::new(s.machine_pos[0], s.machine_pos[1])
            } else {
                Vec2::ZERO
            };
            let rd = toolpath_to_rd_with(&tp, shift, RdOptions::of(app.machine.active()))?;
            if send_only {
                let data = rd.to_rd_file(ruida::Magic::DEFAULT).map_err(|e| e.to_string())?;
                JobRequest::new(JobData::RdFile { name: name.clone(), data })
            } else {
                JobRequest::rd(rd)
            }
        }
        Family::DesignOnly => return Err("This device is design only; there is no laser to run a job on.".into()),
        Family::FileOnly => return Err("This device is a file workflow; use the Gweike window to export files.".into()),
        Family::Galvo => {
            if send_only {
                return Err("A galvo marks as it is sent. Use Start.".into());
            }
            if from_op.is_some_and(|op| op > 0) {
                // The galvo job is made from the shapes, not from the preview's moves.
                return Err("A galvo job is marked whole: it cannot start part way. Use Start (and clear Start here in the Preview).".into());
            }
            crate::galvo_job::request(&crate::galvo_job::job_document(app)?, app.machine.active(), false, false)?.0
        }
    };
    req.name = name;
    req.estimated = estimated;
    req.start_immediately = !send_only;
    req.auto_increment_name = app.machine.active().settings.dsp.filename_increment;
    Ok(req)
}

/// A job was built: do `action` with it. Records it as the last job.
pub fn after_build(app: &mut DarkBurn, job: Arc<BuiltJob>, action: Action) -> Result<Value, String> {
    if !app.machine.handle.is_connected() {
        return Err("The laser is not connected. Press Connect in the Laser panel first.".into());
    }
    let state = app.machine.handle.snapshot().state;
    if state != LaserState::Ready {
        return Err(format!("The laser is {}; wait until it is ready.", state.label().to_lowercase()));
    }
    let (from_op, send) = match action {
        Action::Start { from_op } => (from_op, false),
        Action::Send { from_op } => (from_op, true),
    };
    let req = request_for(app, &job, from_op, send)?;
    let out = if send {
        app.machine.handle.start_job(req).map_err(|e| e.to_string())?;
        json!({ "ok": true, "sent": true })
    } else {
        match app.machine.begin_start(req)? {
            crate::machine_ui::StartOutcome::Started => json!({ "ok": true }),
            crate::machine_ui::StartOutcome::AskChecklist => json!({ "ok": true, "waiting_for_checklist": true }),
        }
    };
    job_cmds::record_last_job(app, &job);
    app.job.start_op = None;
    Ok(out)
}

fn need_connection(app: &DarkBurn) -> Result<(), String> {
    if app.machine.handle.is_connected() { Ok(()) } else { Err("The laser is not connected. Press Connect in the Laser panel first.".into()) }
}

/// A galvo job is compiled from the shapes, not from the preview's moves, so it can only be
/// marked whole: "from here" is refused instead of quietly marking everything [PREV-09].
fn galvo_start(app: &mut DarkBurn, action: Action) -> Result<Value, String> {
    let (from_op, send) = match action {
        Action::Start { from_op } => (from_op, false),
        Action::Send { from_op } => (from_op, true),
    };
    if !send && from_op.is_some_and(|op| op > 0) {
        return Err("A galvo job is marked whole: it cannot start part way. Use Start (and clear Start here in the Preview).".into());
    }
    crate::machine_ext::galvo_start(app, send)
}

/// Window flow: build in the background, then act (progress and cancel come with the build).
pub fn start_later(app: &mut DarkBurn, action: Action) -> Result<Value, String> {
    if !app.machine.handle.is_connected() {
        return Err("The laser is not connected. Press Connect in the Laser panel first.".into());
    }
    if app.machine.handle.snapshot().family == Family::Galvo {
        return galvo_start(app, action);
    }
    job_cmds::start_build(app, After::Machine(action));
    Ok(json!({ "ok": true, "building": true }))
}

/// Headless flow: build now and act.
pub fn start_now(app: &mut DarkBurn, action: Action, allow_out_of_bounds: bool) -> Result<Value, String> {
    need_connection(app)?;
    if app.machine.handle.snapshot().family == Family::Galvo {
        return galvo_start(app, action);
    }
    let job = job_cmds::build_now(app)?;
    if job.out_of_bounds() && !allow_out_of_bounds {
        return Err(format!(
            "the job leaves the work area ({}); pass allow_out_of_bounds or turn on discard_out_of_bounds",
            job.warnings.last().cloned().unwrap_or_default()
        ));
    }
    after_build(app, job, action)
}

// ----- Framing [LASER-04, LASER-05] --------------------------------------------------------------

/// Frame the job outline: rectangle, rubber band or the selection.
pub fn frame(app: &mut DarkBurn, kind: FrameKind) -> Result<Value, String> {
    need_connection(app)?;
    if app.machine.handle.snapshot().family == Family::Galvo {
        // The galvo frames with the red light, one lap, laser off.
        return crate::machine_ext::run(app, "machine.galvo_frame", &json!({ "repeat": false }), crate::machine_ui::Source::Api)
            .unwrap_or_else(|| Err("no galvo frame".into()));
    }
    let mut input = job_cmds::build_input(app);
    let dev = app.machine.active();
    let laser = (dev.settings.gcode.laser_on_when_framing && dev.family() == Family::Grbl)
        .then(|| app.machine.fire_percent.min(dev.settings.fire_max_percent));
    let check_bounds = dev.settings.dsp.check_bounds_when_framing && dev.family() == Family::Ruida;
    if laser.is_some() {
        // The laser itself walks the frame: no red-dot offset.
        input.device.pointer_offset = [0.0, 0.0];
    }
    let tp = job_build::build_frame(&input, kind, 100.0, None).map_err(|e| e.to_string())?;
    if check_bounds {
        job_build::check_frame_bounds(&input, &tp)?;
    }
    let points = work_points(app, &tp);
    if points.len() < 2 {
        return Err("There is nothing to frame.".into());
    }
    app.machine.handle.frame_outline(points.clone(), FrameSpec { feed: 3000.0, laser_percent: laser }).map_err(|e| e.to_string())?;
    Ok(json!({ "ok": true, "points": points.len() }))
}

/// A machine origin was set or cleared: tell the job state (user origin in machine coordinates).
pub fn origin_changed(app: &mut DarkBurn, set: bool) {
    app.job.user_origin = if set { head(app) } else { None };
}
