//! Marlin laser firmware (GCODE-02), serial.
//!
//! From the public Marlin documentation:
//! * `ok` per line; the command queue holds a few lines, `busy: processing` lines mean a long
//!   command is running;
//! * power: `M3 S<n>` / `M4 S<n>` (constant / dynamic), inline mode with `M3 I` / `M4 I` where
//!   `S` rides on `G1` moves, or a fan output (`M106 P<tool> S<0-255>`, `M107 P<tool>`);
//! * `M114` reports the position, `M115` the firmware, `M503` prints the settings, `M500`
//!   stores and `M501` loads them, `M92`/`M203`/`M201`/`M206` set steps, speeds, acceleration
//!   and home offset, `M211` the soft endstops;
//! * `M410` is the quick stop (drops the planner without halting), `M112` is the emergency
//!   stop: Marlin prints `Error:Printer halted. kill() called!` and stops answering until the
//!   board is reset. The driver therefore refuses everything after an emergency stop until the
//!   user reconnects;
//! * Marlin has no feed hold for a streamed job. Pause only stops the driver sending, so the
//!   moves already buffered still run.

use std::time::Duration;

use crate::detect::FirmwareGuess;
use crate::line::{Commands, LineConfig, PollMode, QuickStop, SettingsStyle};
use crate::machine::MachineFamily;
use crate::profile::{DeviceProfile, MarlinPowerMode, TransferMode};
use crate::smoothie::frame_options;
use crate::translate::home_command;

/// Fire and laser-off commands for a power mode.
pub fn power_commands(mode: MarlinPowerMode, tool: u8) -> (String, String, String) {
    match mode {
        MarlinPowerMode::M3 | MarlinPowerMode::Inline => ("M3 S{s}".into(), "M5".into(), "M5".into()),
        MarlinPowerMode::M4 => ("M4 S{s}".into(), "M5".into(), "M5".into()),
        MarlinPowerMode::Fan => (format!("M106 P{tool} S{{s}}"), format!("M107 P{tool}"), format!("M107 P{tool}")),
    }
}

/// Builds the driver configuration from a device profile.
pub fn config(p: &DeviceProfile) -> LineConfig {
    let g = &p.gcode;
    let mut c = LineConfig::plain(MachineFamily::Marlin);
    c.ident = Some("M115".into());
    c.accept = vec![FirmwareGuess::Marlin];
    (c.window_lines, c.window_bytes) = match g.transfer {
        TransferMode::Synchronous => (1, 96),
        TransferMode::Buffered => (4, 96),
    };
    c.poll = PollMode::M114;
    c.poll_interval = Duration::from_millis(500);
    c.s_max = g.s_max;
    c.travel = Some(p.travel_limits());
    c.max_fire_percent = g.max_fire_percent;
    c.max_fire_duration = p.fire_duration();
    c.quick_stop = QuickStop::Line("M410".into());
    c.settings = SettingsStyle::MarlinM503;
    c.halt_needs_power_cycle = true;
    c.default_name = "Marlin".into();
    let (fire_on, fire_off, laser_off) = power_commands(g.marlin_power, g.tool_index);
    c.commands = Commands {
        home: home_command(p),
        unlock: String::new(),
        pause: String::new(),
        resume: String::new(),
        fire_on,
        fire_off,
        air_on: g.air_assist.on.clone(),
        air_off: g.air_assist.off.clone(),
        laser_off,
        after_stop: String::new(),
        emergency: "M112".into(),
        set_origin: "G92 X0 Y0".into(),
        clear_origin: "G92.1".into(),
        feed_override: "M220 S{percent}".into(),
        persist: "M500".into(),
        jog: "G91\nG0 X{dx} Y{dy} Z{dz} F{feed}\nG90".into(),
    };
    if !g.fire_button {
        // The fire button is switched off in the device settings: the driver has no fire command.
        c.commands.fire_on.clear();
    }
    c.laser_frames = g.laser_on_when_framing;
    c.power = frame_options(p);
    c
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::DeviceKind;

    #[test]
    fn power_modes() {
        assert_eq!(power_commands(MarlinPowerMode::M3, 0).0, "M3 S{s}");
        assert_eq!(power_commands(MarlinPowerMode::M4, 0).0, "M4 S{s}");
        assert_eq!(power_commands(MarlinPowerMode::Fan, 2), ("M106 P2 S{s}".into(), "M107 P2".into(), "M107 P2".into()));
    }

    #[test]
    fn config_follows_the_profile() {
        let mut p = DeviceProfile::for_kind(DeviceKind::Marlin);
        p.gcode.marlin_power = MarlinPowerMode::Fan;
        p.gcode.tool_index = 1;
        p.gcode.transfer = TransferMode::Buffered;
        let c = config(&p);
        assert_eq!(c.window_lines, 4);
        assert_eq!(c.commands.fire_off, "M107 P1");
        assert_eq!(c.commands.home, "G28 X Y");
        assert!(c.halt_needs_power_cycle);
    }
}
