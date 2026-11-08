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

/// Layer settings and cuts of `tp` as a Ruida job. `shift` moves head-relative toolpaths to
/// absolute controller coordinates. Arcs are cut as their chord (the planner flattens curves
/// before this, so arcs only appear when a flavor asks for them).
pub fn toolpath_to_rd(tp: &Toolpath, shift: Vec2) -> Result<ruida::RdJob, String> {
    let mut job = ruida::RdJob::default();
    let mut cur: Option<usize> = None;
    let mut speed = 100.0;
    let at = |p: Point| p + shift;
    for op in &tp.ops {
        match op {
            Op::LayerStart { layer } => {
                let l = tp.layers.get(*layer).ok_or("the job refers to a layer it does not have")?;
                let index = job.layers.len().min(127) as u8;
                speed = l.speed;
                job.layers.push(ruida::RdLayer {
                    settings: ruida::LayerSettings {
                        index,
                        speed_mm_s: l.speed.max(0.1),
                        min_power: [l.min_power, l.min_power],
                        max_power: [l.max_power, l.max_power],
                        color: 0,
                        frequency_hz: None,
                        air_assist: l.air,
                        dot_mode: None,
                    },
                    ops: Vec::new(),
                });
                cur = Some(job.layers.len() - 1);
            }
            Op::Rapid { to } | Op::Overscan { to, .. } => {
                let i = cur.ok_or("the job moves before it starts a layer")?;
                job.layers[i].ops.push(ruida::RdOp::Move(at(*to)));
            }
            Op::Cut { to, speed: s, power } | Op::Arc { to, speed: s, power, .. } => {
                let i = cur.ok_or("the job cuts before it starts a layer")?;
                let p = at(*to);
                job.layers[i].ops.push(if *power > 0.0 {
                    ruida::RdOp::Cut { to: p, power: Some(*power), speed: Some(*s) }
                } else {
                    ruida::RdOp::Move(p)
                });
                speed = *s;
            }
            Op::ZMove { z } => {
                if let Some(i) = cur {
                    job.layers[i].ops.push(ruida::RdOp::Z(*z));
                }
            }
            _ => {}
        }
    }
    let _ = speed;
    job.layers.retain(|l| !l.ops.is_empty());
    if job.layers.is_empty() {
        return Err("the job has nothing to send".into());
    }
    Ok(job)
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
            JobRequest::gcode(job_build::gcode_lines(&tp, &app.job.gcode))
        }
        Family::Ruida => {
            let shift = if tp.space == CoordSpace::RelativeToStart {
                let s = app.machine.handle.snapshot();
                Vec2::new(s.machine_pos[0], s.machine_pos[1])
            } else {
                Vec2::ZERO
            };
            let rd = toolpath_to_rd(&tp, shift)?;
            if send_only {
                let data = rd.to_rd_file(ruida::Magic::DEFAULT).map_err(|e| e.to_string())?;
                JobRequest::new(JobData::RdFile { name: name.clone(), data })
            } else {
                JobRequest::rd(rd)
            }
        }
        Family::DesignOnly => return Err("This device is design only; there is no laser to run a job on.".into()),
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

/// Window flow: build in the background, then act (progress and cancel come with the build).
pub fn start_later(app: &mut DarkBurn, action: Action) -> Result<Value, String> {
    if !app.machine.handle.is_connected() {
        return Err("The laser is not connected. Press Connect in the Laser panel first.".into());
    }
    job_cmds::start_build(app, After::Machine(action));
    Ok(json!({ "ok": true, "building": true }))
}

/// Headless flow: build now and act.
pub fn start_now(app: &mut DarkBurn, action: Action, allow_out_of_bounds: bool) -> Result<Value, String> {
    need_connection(app)?;
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
    let input = job_cmds::build_input(app);
    let tp = job_build::build_frame(&input, kind, 100.0, None).map_err(|e| e.to_string())?;
    let points = work_points(app, &tp);
    if points.len() < 2 {
        return Err("There is nothing to frame.".into());
    }
    let dev = app.machine.active();
    let laser = (dev.settings.gcode.laser_on_when_framing && dev.family() == Family::Grbl)
        .then(|| app.machine.fire_percent.min(dev.settings.fire_max_percent));
    app.machine.handle.frame_outline(points.clone(), FrameSpec { feed: 3000.0, laser_percent: laser }).map_err(|e| e.to_string())?;
    Ok(json!({ "ok": true, "points": points.len() }))
}

/// A machine origin was set or cleared: tell the job state (user origin in machine coordinates).
pub fn origin_changed(app: &mut DarkBurn, set: bool) {
    app.job.user_origin = if set { head(app) } else { None };
}
