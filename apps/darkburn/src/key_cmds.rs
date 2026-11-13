//! Keyboard-only commands [KEY-01, KEY-02, KEY-03]: step the head with the keys (arrows,
//! diagonals, Z, the number pad). They go through the same jog command the Move panel's buttons
//! use, so the origin corner, the step size and the speed are the panel's, and they are refused
//! exactly when those buttons are disabled.

use serde_json::json;

use crate::commands::Cmd;
use crate::machine_api::{Family, LaserState, Snapshot};
use crate::machine_ui::MachineUi;
use crate::shell::DarkBurn;

const fn cmd(id: &'static str, label: &'static str) -> Cmd {
    Cmd { id, label, key: "", icon: "", args: "" }
}

pub const COMMANDS: &[Cmd] = &[
    cmd("laser.jog_left", "Jog Left"),
    cmd("laser.jog_right", "Jog Right"),
    cmd("laser.jog_up", "Jog Up"),
    cmd("laser.jog_down", "Jog Down"),
    cmd("laser.jog_up_left", "Jog Up Left"),
    cmd("laser.jog_up_right", "Jog Up Right"),
    cmd("laser.jog_down_left", "Jog Down Left"),
    cmd("laser.jog_down_right", "Jog Down Right"),
    cmd("laser.jog_z_up", "Jog Z Up"),
    cmd("laser.jog_z_down", "Jog Z Down"),
];

pub fn works(id: &str) -> bool {
    COMMANDS.iter().any(|c| c.id == id)
}

/// Screen direction (x right, y up) or Z (1 up, -1 down).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Dir {
    Xy(i8, i8),
    Z(i8),
}

fn direction(id: &str) -> Option<Dir> {
    Some(match id {
        "laser.jog_left" => Dir::Xy(-1, 0),
        "laser.jog_right" => Dir::Xy(1, 0),
        "laser.jog_up" => Dir::Xy(0, 1),
        "laser.jog_down" => Dir::Xy(0, -1),
        "laser.jog_up_left" => Dir::Xy(-1, 1),
        "laser.jog_up_right" => Dir::Xy(1, 1),
        "laser.jog_down_left" => Dir::Xy(-1, -1),
        "laser.jog_down_right" => Dir::Xy(1, -1),
        "laser.jog_z_up" => Dir::Z(1),
        "laser.jog_z_down" => Dir::Z(-1),
        _ => return None,
    })
}

/// Whether the Move panel's jog buttons work: a connected laser that is idle (not running,
/// paused or framing a job, not in alarm).
pub fn jog_free(s: &Snapshot) -> bool {
    s.connected() && s.state == LaserState::Ready
}

/// Whether the Move panel shows Z buttons for this machine.
pub fn has_z(m: &MachineUi) -> bool {
    m.active().settings.z_enabled || crate::machine_ui::family_of(m) == Family::Ruida
}

/// Why a jog key does nothing now, or `None` when it jogs.
fn refusal(m: &MachineUi, z: bool) -> Option<&'static str> {
    let s = m.handle.snapshot();
    if !s.connected() {
        Some("The laser is not connected. Press Connect in the Laser panel first.")
    } else if !jog_free(s) {
        Some("The laser is busy. Jog keys work when it is ready.")
    } else if z && !has_z(m) {
        Some("This machine has no Z axis to jog. Switch it on in Device Settings.")
    } else {
        None
    }
}

/// Run a jog key. Returns false for ids that are not ours.
pub fn run(app: &mut DarkBurn, _ctx: &egui::Context, id: &str) -> bool {
    let Some(dir) = direction(id) else { return false };
    if let Some(why) = refusal(&app.machine, matches!(dir, Dir::Z(_))) {
        app.machine.fail(why);
        return true;
    }
    let d = app.machine.jog.distance;
    match dir {
        Dir::Xy(dx, dy) => {
            let (ax, ay) = crate::machine_ui::arrow_to_machine(dx, dy, app.machine.active(), app.machine.jog.invert_keypad);
            let feed = app.machine.jog.speed * 60.0;
            crate::machine_ui::run_with(app, "machine.jog", &json!({ "dx": f64::from(ax) * d, "dy": f64::from(ay) * d, "feed": feed }));
        }
        Dir::Z(dir) => {
            // As the panel's Z up and Z down buttons: reversed when the device says so.
            let rev = if app.machine.active().settings.z_reverse { -1.0 } else { 1.0 };
            let feed = app.machine.jog.z_speed * 60.0;
            crate::machine_ui::run_with(app, "machine.jog", &json!({ "dz": f64::from(dir) * rev * d, "feed": feed }));
        }
    }
    true
}

/// Whether a number pad key bound to `id` belongs to it now. A pad jog key only does so while a
/// laser is connected: without one, the pad's arrows (NumLock off) keep moving the selection as
/// they did before.
pub fn numpad_owns(app: &DarkBurn, id: &str) -> bool {
    !works(id) || app.machine.handle.snapshot().connected()
}

/// The command of a number pad key (NumLock off) pressed this frame; the press is used up so
/// the canvas does not also take it for an arrow. Not over a dialog or a menu, not with Ctrl or
/// Alt (those are the keys' ordinary shortcuts), and not in the node tool, which owns every
/// plain key (the pad's arrows nudge its nodes).
pub fn numpad_command(app: &DarkBurn, ctx: &egui::Context) -> Option<&'static str> {
    if app.tool == crate::tools::Tool::Nodes || crate::escape::window_open(ctx) {
        return None;
    }
    let pad = crate::keymap::numpad_table(&app.prefs.hotkeys);
    let owner = |key: &egui::Key, physical: &Option<egui::Key>| {
        let pressed = crate::keymap::numpad_key(*key, *physical)?;
        pad.iter().find(|(k, id)| *k == pressed && crate::commands::works(id) && numpad_owns(app, id)).map(|(_, id)| *id)
    };
    ctx.input_mut(|i| {
        let (at, id) = i.events.iter().enumerate().find_map(|(at, e)| match e {
            egui::Event::Key { key, physical_key, pressed: true, modifiers, .. } if !modifiers.command && !modifiers.alt => {
                owner(key, physical_key).map(|id| (at, id))
            }
            _ => None,
        })?;
        i.events.remove(at);
        Some(id)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_jog_command_has_a_direction_and_is_a_laser_command() {
        for c in COMMANDS {
            assert!(direction(c.id).is_some(), "{}", c.id);
            assert!(crate::commands::hardware(c.id), "{} must be gated as hardware", c.id);
        }
        assert_eq!(direction("laser.jog_up_left"), Some(Dir::Xy(-1, 1)));
        assert_eq!(direction("laser.jog_z_down"), Some(Dir::Z(-1)));
        assert_eq!(direction("laser.fire"), None);
    }
}
