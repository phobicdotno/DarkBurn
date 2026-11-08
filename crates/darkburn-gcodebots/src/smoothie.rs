//! Smoothieware (GCODE-01), serial or TCP.
//!
//! From the public Smoothieware documentation:
//! * flow control is `ok` per line; there is no character counting, so the driver keeps one or
//!   two lines in flight;
//! * `?` returns a status report (`<Idle,MPos:...,WPos:...>`) and is not followed by `ok`;
//! * `M114` returns `ok C: X:... Y:... Z:...`;
//! * `fire <percent>` / `fire off` is the laser module's manual fire command;
//! * `suspend` / `resume` pause and continue a job;
//! * `M106`/`M107` switch the fan output (used for air assist), `M7`/`M8`/`M9` are the
//!   alternative the profile can choose;
//! * the configuration is the text file `/sd/config.txt` (`cat /sd/config.txt`), and
//!   `config-set sd <key> <value>` changes a value;
//! * `M112` halts, `M999` (or `$X`) clears the halt;
//! * the laser's `S` word runs from 0.0 to 1.0 unless `laser_module_max_power` says otherwise.

use std::time::Duration;

use crate::detect::FirmwareGuess;
use crate::line::{Commands, LineConfig, PollMode, QuickStop, SettingsStyle};
use crate::machine::MachineFamily;
use crate::profile::{DeviceProfile, TransferMode};
use crate::translate::TranslateOptions;

/// Builds the driver configuration from a device profile.
pub fn config(p: &DeviceProfile) -> LineConfig {
    let g = &p.gcode;
    let mut c = LineConfig::plain(MachineFamily::Smoothie);
    c.ident = Some("version".into());
    c.accept = vec![FirmwareGuess::Smoothie];
    (c.window_lines, c.window_bytes) = match g.transfer {
        TransferMode::Synchronous => (1, 100),
        TransferMode::Buffered => (2, 120),
    };
    if g.reduce_smoothie_pause_delay {
        // Smoothieware reads lines in order, so a pause waits behind every line already sent.
        // One line in flight keeps that wait short.
        c.window_lines = 1;
    }
    c.poll = PollMode::Raw(b"?".to_vec());
    c.poll_interval = Duration::from_millis(200);
    c.reports_state = true;
    c.s_max = g.s_max;
    c.travel = Some(p.travel_limits());
    c.max_fire_percent = g.max_fire_percent;
    c.max_fire_duration = p.fire_duration();
    c.quick_stop = QuickStop::Realtime(0x18);
    c.settings = SettingsStyle::SmoothieConfig;
    c.supports_g53 = true;
    c.default_name = "Smoothieware".into();
    c.commands = Commands {
        home: "$H".into(),
        unlock: "M999".into(),
        pause: "suspend".into(),
        resume: "resume".into(),
        fire_on: "fire {percent}".into(),
        fire_off: "fire off".into(),
        air_on: g.air_assist.on.clone(),
        air_off: g.air_assist.off.clone(),
        laser_off: "M5".into(),
        after_stop: "M999".into(),
        emergency: "M112".into(),
        set_origin: "G92 X0 Y0".into(),
        clear_origin: "G92.1".into(),
        feed_override: "M220 S{percent}".into(),
        persist: String::new(),
        jog: "G91\nG0 X{dx} Y{dy} Z{dz} F{feed}\nG90".into(),
    };
    c.power = frame_options(p);
    c
}

/// Options for framing moves: the job translation without scripts, homing or clustering.
pub fn frame_options(p: &DeviceProfile) -> TranslateOptions {
    TranslateOptions {
        start_script: String::new(),
        end_script: String::new(),
        home_command: None,
        cluster: false,
        fast_whitespace: None,
        ..TranslateOptions::for_profile(p)
    }
}

/// Options for translating a job for this profile (scripts, homing and clustering included).
pub fn job_options(p: &DeviceProfile) -> TranslateOptions {
    TranslateOptions::for_profile(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::DeviceKind;

    #[test]
    fn config_follows_the_profile() {
        let mut p = DeviceProfile::for_kind(DeviceKind::Smoothie);
        let c = config(&p);
        assert_eq!(c.window_lines, 1, "the preset is synchronous");
        p.gcode.transfer = TransferMode::Buffered;
        assert_eq!(config(&p).window_lines, 2);
        assert_eq!(c.commands.air_on, "M106");
        assert_eq!(c.s_max, 1.0);
        assert!(c.travel.is_some());
    }
}
