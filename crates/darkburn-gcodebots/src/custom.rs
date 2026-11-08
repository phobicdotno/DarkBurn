//! The custom G-code device (GCODE-05): user-defined start and end scripts, units, S-max and
//! command templates for air, pause, home and jog, with plain `ok` streaming.
//!
//! Templates live in [`crate::profile::CustomTemplates`] and use `{name}` placeholders: jog
//! has `{dx} {dy} {dz} {feed}`, fire has `{s}` and `{percent}`. A custom device is assumed to
//! answer every line with `ok` (or `error...`). It has no banner and is not identified; if the
//! `status` template is set, that text is sent as raw bytes every poll and the reply must be a
//! `<State,MPos:x,y,z>` report that is not followed by `ok`.

use std::time::Duration;

use crate::line::{Commands, LineConfig, PollMode, QuickStop};
use crate::machine::MachineFamily;
use crate::profile::{DeviceProfile, TransferMode};
use crate::smoothie::frame_options;

/// Builds the driver configuration from a device profile.
pub fn config(p: &DeviceProfile) -> LineConfig {
    let g = &p.gcode;
    let t = &g.custom;
    let mut c = LineConfig::plain(MachineFamily::Custom);
    (c.window_lines, c.window_bytes) = match g.transfer {
        TransferMode::Synchronous => (1, 100),
        TransferMode::Buffered => (4, 100),
    };
    if !t.status.trim().is_empty() {
        c.poll = PollMode::Raw(t.status.clone().into_bytes());
        c.reports_state = true;
        c.poll_interval = Duration::from_millis(250);
    }
    c.s_max = g.s_max;
    c.travel = Some(p.travel_limits());
    c.max_fire_percent = g.max_fire_percent;
    c.max_fire_duration = p.fire_duration();
    c.finish_sync = t.finish_sync.trim().to_owned();
    c.default_name = "Custom G-code device".into();
    if !t.stop.trim().is_empty() {
        c.quick_stop = QuickStop::Line(t.stop.trim().to_owned());
    }
    c.commands = Commands {
        home: t.home.clone(),
        unlock: t.unlock.clone(),
        jog: t.jog.clone(),
        pause: t.pause.clone(),
        resume: t.resume.clone(),
        fire_on: if g.fire_button { t.fire_on.clone() } else { String::new() },
        fire_off: t.fire_off.clone(),
        air_on: g.air_assist.on.clone(),
        air_off: g.air_assist.off.clone(),
        laser_off: t.laser_off.clone(),
        after_stop: String::new(),
        emergency: t.stop.lines().next().unwrap_or("").trim().to_owned(),
        set_origin: "G92 X0 Y0".into(),
        clear_origin: "G92.1".into(),
        feed_override: String::new(),
        persist: String::new(),
    };
    c.power = frame_options(p);
    c
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::DeviceKind;

    #[test]
    fn templates_come_from_the_profile() {
        let mut p = DeviceProfile::for_kind(DeviceKind::Custom);
        p.gcode.custom.pause = "M0".into();
        p.gcode.custom.jog = "G91 G0 X{dx} Y{dy} F{feed}\nG90".into();
        p.gcode.custom.status = "?".into();
        let c = config(&p);
        assert_eq!(c.commands.pause, "M0");
        assert_eq!(c.poll, PollMode::Raw(b"?".to_vec()));
        assert!(c.reports_state);
        p.gcode.custom.status.clear();
        assert_eq!(config(&p).poll, PollMode::None);
    }
}
