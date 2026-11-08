//! Keyboard-only commands [KEY-02, KEY-03]: step the head with the arrow keys. They go through
//! the same jog command the Move panel's arrows use, so the origin corner, the step size and the
//! speed are the panel's.

use serde_json::json;

use crate::commands::Cmd;
use crate::shell::DarkBurn;

const fn cmd(id: &'static str, label: &'static str) -> Cmd {
    Cmd { id, label, key: "", icon: "", args: "" }
}

pub const COMMANDS: &[Cmd] =
    &[cmd("laser.jog_left", "Jog Left"), cmd("laser.jog_right", "Jog Right"), cmd("laser.jog_up", "Jog Up"), cmd("laser.jog_down", "Jog Down")];

pub fn works(id: &str) -> bool {
    COMMANDS.iter().any(|c| c.id == id)
}

fn direction(id: &str) -> Option<(i8, i8)> {
    Some(match id {
        "laser.jog_left" => (-1, 0),
        "laser.jog_right" => (1, 0),
        "laser.jog_up" => (0, 1),
        "laser.jog_down" => (0, -1),
        _ => return None,
    })
}

/// Run a jog key. Returns false for ids that are not ours.
pub fn run(app: &mut DarkBurn, _ctx: &egui::Context, id: &str) -> bool {
    let Some((dx, dy)) = direction(id) else { return false };
    let origin = app.machine.active().origin;
    let (ax, ay) = crate::machine_ui::arrow_to_axes(dx, dy, origin, app.machine.jog.invert_keypad);
    let d = app.machine.jog.distance;
    let feed = app.machine.jog.speed * 60.0;
    crate::machine_ui::run_with(app, "machine.jog", &json!({ "dx": f64::from(ax) * d, "dy": f64::from(ay) * d, "feed": feed }));
    true
}
