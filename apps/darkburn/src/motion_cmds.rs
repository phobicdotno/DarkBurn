//! Move window commands that sit beside the `machine.*` set in `machine_ui.rs`: Focus Z, the
//! finish position, jogging and test-turning the rotary, and taking a print and cut target from
//! the laser head. `machine_ui::dispatch` hands them to [`run`].
//!
//! Every id here starts with `machine.`, so all of them count as hardware commands: an agent
//! reaches the simulators only, and a real machine is refused by `machine_ui::dispatch` before
//! [`run`] (the finish position commands move nothing, but they decide where a later job ends).

use darkburn_core::kurbo::Point;
use darkburn_vision::rotary::{RotaryAxis, RotaryParams};
use serde_json::{Value, json};

use crate::commands::Cmd;
use crate::machine_api::{Family, JogSpec, MachineCommand, MachineError, MoveSpec};
use crate::machine_ui::Source;
use crate::shell::DarkBurn;

const fn a(id: &'static str, label: &'static str, args: &'static str) -> Cmd {
    Cmd { id, label, key: "", icon: "", args }
}

pub const COMMANDS: &[Cmd] = &[
    a(
        "machine.focus_z",
        "Focus Z",
        r#"{"type":"object","properties":{"z":{"type":"number","description":"Z height in mm for controllers without a focus routine"},"feed":{"type":"number","description":"mm/min"},"machine_coords":{"type":"boolean"}}}"#,
    ),
    a(
        "machine.set_finish",
        "Set Finish Position",
        r#"{"type":"object","properties":{"x":{"type":"number","description":"mm from machine zero; leave x and y out to use the head's position"},"y":{"type":"number"}}}"#,
    ),
    a("machine.clear_finish", "Clear Finish Position", ""),
    a(
        "machine.jog_rotary",
        "Turn the Rotary",
        r#"{"type":"object","properties":{"distance":{"type":"number","description":"mm along the object's surface; negative turns back"},"feed":{"type":"number","description":"mm/min"}},"required":["distance"]}"#,
    ),
    a(
        "machine.rotary_test",
        "Run a Rotary Test Rotation",
        r#"{"type":"object","properties":{"revolutions":{"type":"number"},"feed":{"type":"number","description":"mm/min"},"axis":{"enum":["Y","A","B","C","U","V"]},"kind":{"enum":["chuck","roller"]},"steps_per_rotation":{"type":"number"},"axis_steps_per_mm":{"type":"number"},"roller_diameter":{"type":"number"},"diameter":{"type":"number"},"circumference":{"type":"number"},"mirror":{"type":"boolean"}}}"#,
    ),
];

pub fn handles(id: &str) -> bool {
    COMMANDS.iter().any(|c| c.id == id)
}

pub fn checklist(id: &str) -> &'static [&'static str] {
    match id {
        "machine.focus_z" => &["MOVE-12"],
        "machine.set_finish" | "machine.clear_finish" => &["MOVE-14"],
        "machine.jog_rotary" => &["MOVE-03"],
        "machine.rotary_test" => &["ROT-06"],
        _ => &[],
    }
}

fn num(a: &Value, k: &str) -> Option<f64> {
    a.get(k).and_then(Value::as_f64).filter(|v| v.is_finite())
}

fn mach(r: Result<(), MachineError>) -> Result<Value, String> {
    r.map(|_| json!({ "ok": true })).map_err(|e| e.to_string())
}

/// Run command `id`. None when it is not one of ours.
pub fn run(app: &mut DarkBurn, id: &str, a: &Value, _src: Source) -> Option<Result<Value, String>> {
    if !handles(id) {
        return None;
    }
    let null = json!({});
    let a = if a.is_null() { &null } else { a };
    Some(match id {
        "machine.focus_z" => focus_z(app, a),
        "machine.set_finish" => set_finish(app, a),
        "machine.clear_finish" => {
            app.machine.jog.finish = None;
            Ok(json!({ "finish": null }))
        }
        "machine.jog_rotary" => jog_rotary(app, a),
        "machine.rotary_test" => rotary_test(app, a),
        _ => Err(format!("{id} is not a motion command")),
    })
}

/// Focus Z [MOVE-12]. A DSP controller runs its own focus routine and stops at the focus
/// distance of its Z settings. A G-code controller has none: its Z axis moves to the height `z`.
fn focus_z(app: &mut DarkBurn, a: &Value) -> Result<Value, String> {
    let family = app.machine.handle.snapshot().family;
    if family == Family::Ruida {
        return mach(app.machine.handle.command(MachineCommand::FocusZ)).map(|_| json!({ "ok": true, "routine": "controller" }));
    }
    let z = num(a, "z").ok_or("this controller has no focus routine: give the Z height to move to (argument z)")?;
    let spec = MoveSpec {
        x: None,
        y: None,
        z: Some(z),
        machine_coords: a.get("machine_coords").and_then(Value::as_bool).unwrap_or(false),
        feed: num(a, "feed").unwrap_or(app.machine.jog.z_speed * 60.0),
    };
    mach(app.machine.handle.move_to(spec)).map(|_| json!({ "ok": true, "z": z }))
}

/// Set Finish Position [MOVE-14]: jobs end at this place on the bed. Without x and y it is where
/// the head stands now. Kept in machine coordinates, so setting or clearing the origin later does
/// not move it.
fn set_finish(app: &mut DarkBurn, a: &Value) -> Result<Value, String> {
    let p = match (num(a, "x"), num(a, "y")) {
        (Some(x), Some(y)) => (x, y),
        (None, None) => {
            let s = app.machine.handle.snapshot();
            if !s.has_position {
                return Err("the laser has not reported where it is yet; connect it or press Get position".into());
            }
            (s.machine_pos[0], s.machine_pos[1])
        }
        _ => return Err("give both x and y, or neither to use the head's position".into()),
    };
    on_bed(app, p)?;
    app.machine.jog.finish = Some(p);
    Ok(json!({ "finish": { "x": p.0, "y": p.1 } }))
}

/// Refuse a finish position off the active device's bed: every later job would end with a rapid
/// move there, outside what the job's own bounds check looks at. The bed is the work area in
/// machine coordinates, so a machine with negative coordinates has it below zero.
fn on_bed(app: &DarkBurn, (x, y): (f64, f64)) -> Result<(), String> {
    let area = app.machine.active().job_device().geometry().work_area();
    if !(area.width() > 0.0 && area.height() > 0.0) {
        return Err("this device has no bed size; set it in Device Settings first".into());
    }
    const SLACK: f64 = 1e-6;
    if x < area.x0 - SLACK || x > area.x1 + SLACK || y < area.y0 - SLACK || y > area.y1 + SLACK {
        return Err(format!(
            "the finish position X {x:.2}, Y {y:.2} is off the bed (X {:.2} to {:.2}, Y {:.2} to {:.2})",
            area.x0, area.x1, area.y0, area.y1
        ));
    }
    Ok(())
}

/// The finish position a job ends at, checked against the active device's bed when the job is
/// made: one kept from before (a saved device state, or another bed size since) may be off it,
/// and then the job is refused rather than ending with a move off the bed.
pub fn finish_for_job(app: &DarkBurn) -> Result<Option<(f64, f64)>, String> {
    match app.machine.jog.finish {
        None => Ok(None),
        Some(p) => on_bed(app, p).map(|_| Some(p)).map_err(|e| format!("{e}. Clear it or set it again in the Move panel.")),
    }
}

/// The jog for `units` on the rotary axis: Y (any controller) or U (DSP controllers). The job
/// output turns the rotary on the Y axis only, so the other letters are refused here as well.
fn rotary_jog(axis: RotaryAxis, units: f64, feed: f64) -> Result<JogSpec, String> {
    let mut j = JogSpec { dx: 0.0, dy: 0.0, dz: 0.0, du: 0.0, feed };
    match axis {
        RotaryAxis::Y => j.dy = units,
        RotaryAxis::U => j.du = units,
        other => {
            return Err(format!("the rotary is set to axis {}; DarkBurn turns a rotary on the Y or U axis only", other.letter()));
        }
    }
    Ok(j)
}

/// The project's rotary setup, which must be switched on.
fn project_rotary(app: &mut DarkBurn) -> Result<RotaryParams, String> {
    crate::vision_cmds::sync(app);
    let r = app.vision.state.rotary;
    if !r.enabled {
        return Err("switch the rotary on in Rotary Setup first".into());
    }
    r.validate().map_err(|e| e.to_string())?;
    Ok(r)
}

/// Turn the rotary by `distance` mm along the object's surface [MOVE-03]. The distance is turned
/// into axis units with the rotary setup (object size, steps, roller), so the jog moves the
/// surface by what was asked, whatever the axis is set up for.
fn jog_rotary(app: &mut DarkBurn, a: &Value) -> Result<Value, String> {
    let d = num(a, "distance").ok_or("argument distance must be a number")?;
    if d == 0.0 {
        return Err("the distance must not be zero".into());
    }
    let r = project_rotary(app)?;
    let units = r.y_to_axis(d).map_err(|e| e.to_string())?;
    let feed = num(a, "feed").unwrap_or(app.machine.jog.speed * 60.0);
    let j = rotary_jog(r.axis, units, feed)?;
    mach(app.machine.handle.jog_step(j)).map(|_| json!({ "ok": true, "axis": r.axis.letter().to_string(), "units": units }))
}

/// Run a test rotation [ROT-06]: turn the object by the plan and back again, so the user can
/// check it returns to where it started. The plan comes from the settings in `a` laid over the
/// project's (the Rotary Setup window sends what it shows).
fn rotary_test(app: &mut DarkBurn, a: &Value) -> Result<Value, String> {
    let revs = num(a, "revolutions").unwrap_or(1.0);
    let v = crate::vision_cmds::rotary_plan_for(app, a, revs)?;
    let plan: darkburn_vision::rotary::TestRotationPlan = serde_json::from_value(v["plan"].clone()).map_err(|e| e.to_string())?;
    let feed = num(a, "feed").unwrap_or(app.machine.jog.speed * 60.0);
    let steps = plan.moves.iter().map(|m| rotary_jog(m.axis, m.distance, feed)).collect::<Result<Vec<_>, _>>()?;
    // As one sequence: a DSP controller works every turn out from one position reading, so the
    // turn back does not start from a position read while the first turn is still under way.
    mach(app.machine.handle.jog_sequence(steps))?;
    Ok(json!({ "plan": plan, "sent": true }))
}

/// Where a print and cut target is when the laser stands on it now [LT-01, LT-02]: the head's
/// position from machine zero (targets are kept in machine coordinates). With `pointer`, the
/// head was placed with the red dot, which sits at the device's pointer offset from the beam.
pub fn head_target(app: &DarkBurn, pointer: bool) -> Result<Point, String> {
    if !app.machine.handle.is_connected() {
        return Err("connect the laser and put it on the target first".into());
    }
    let s = app.machine.handle.snapshot();
    if !s.has_position {
        return Err("the laser has not reported where it is yet; press Get position".into());
    }
    let mut p = Point::new(s.machine_pos[0], s.machine_pos[1]);
    if pointer {
        let d = &app.machine.active().settings;
        p.x += d.red_dot_x;
        p.y += d.red_dot_y;
    }
    Ok(p)
}
