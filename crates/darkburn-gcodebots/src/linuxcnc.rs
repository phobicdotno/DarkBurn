//! LinuxCNC (GCODE-03): **file output only**.
//!
//! LinuxCNC can be driven live over its `linuxcncrsh` telnet interface, but that interface
//! needs a login handshake and a running LinuxCNC session with a matching HAL configuration,
//! and a mistake would start a machine that DarkBurn cannot see. Without a real machine to
//! check against, this module only writes an `.ngc` program that the user opens in LinuxCNC
//! (AXIS, Gmoccapy, ...) and starts there.
//!
//! Laser power uses `M67 E<channel> Q<value>` (synchronized analog output, documented in the
//! LinuxCNC G-code reference); the HAL configuration connects `motion.analog-out-NN` to the
//! laser driver. A spindle-style `M3 S` variant is offered for configurations that wire the
//! laser to the spindle PWM.

use crate::profile::DeviceProfile;
use crate::translate::{PowerStyle, TranslateOptions, translate};

/// How power is written in the program.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinuxCncPower {
    /// `M67 E<channel> Q<0..max_q>`.
    AnalogM67 { channel: u8 },
    /// `M3 S<n>` / `M5` on the spindle output.
    Spindle,
}

#[derive(Clone, Debug)]
pub struct LinuxCncOptions {
    pub power: LinuxCncPower,
    /// Analog value that means full power (`Q`), 100 by default; for `Spindle` the S max.
    pub max_q: f64,
    /// Path blending tolerance for `G64 P`; `None` leaves exact-path mode off.
    pub blend_tolerance_mm: Option<f64>,
    /// Digital output for air assist (`M64 P<n>` / `M65 P<n>`); `None` drops air commands.
    pub air_output: Option<u8>,
    /// Comment written at the top of the file.
    pub title: String,
}

impl Default for LinuxCncOptions {
    fn default() -> Self {
        Self {
            power: LinuxCncPower::AnalogM67 { channel: 0 },
            max_q: 100.0,
            blend_tolerance_mm: Some(0.05),
            air_output: Some(0),
            title: "DarkBurn laser job".into(),
        }
    }
}

impl LinuxCncOptions {
    pub fn for_profile(p: &DeviceProfile) -> Self {
        let mut o = Self { max_q: p.gcode.s_max, ..Self::default() };
        o.power = if p.gcode.constant_power { LinuxCncPower::Spindle } else { LinuxCncPower::AnalogM67 { channel: 0 } };
        o
    }
}

/// Writes an `.ngc` program from a GRBL-style job (`M3`/`M4`/`M5`, `S` 0..1000, `M8`/`M9`).
/// Every line of the profile's start and end scripts is kept.
pub fn write_program(job_lines: &[String], p: &DeviceProfile, o: &LinuxCncOptions) -> String {
    let style = match o.power {
        LinuxCncPower::AnalogM67 { channel } => PowerStyle::Command { set: format!("M67 E{channel} Q{{value}}"), off: format!("M67 E{channel} Q0") },
        LinuxCncPower::Spindle => PowerStyle::Passthrough,
    };
    let (air_on, air_off) = match o.air_output {
        Some(n) => (format!("M64 P{n}"), format!("M65 P{n}")),
        None => (String::new(), String::new()),
    };
    let blend = o.blend_tolerance_mm.map_or(String::new(), |t| format!("\nG64 P{}", darkburn_grbl::protocol::fmt_num(t)));
    let opts = TranslateOptions {
        style,
        in_s_max: 1000.0,
        out_s_max: o.max_q,
        air_on,
        air_off,
        swap_xy: p.gcode.swap_xy,
        s_with_every_g1: false,
        g0_for_overscan: p.gcode.g0_for_overscan,
        fast_whitespace: None,
        cluster: false,
        inches: p.gcode.units == crate::profile::Units::Inches,
        start_script: format!("G17 G21 G90 G94 G54{blend}\n{}", p.gcode.start_script),
        end_script: format!(
            "{}\n{}",
            p.gcode.end_script,
            match o.power {
                LinuxCncPower::AnalogM67 { channel } => format!("M67 E{channel} Q0"),
                LinuxCncPower::Spindle => "M5".to_owned(),
            }
        ),
        home_command: None,
    };
    let body = translate(job_lines.iter().map(String::as_str), &opts);
    let mut out = format!("({})\n", o.title.replace(['(', ')', '\n'], " "));
    for l in body {
        out.push_str(&l);
        out.push('\n');
    }
    out.push_str("M2\n");
    out
}

/// Problems found in a program text: power left on at the end, no program end.
pub fn check_program(text: &str, o: &LinuxCncOptions) -> Vec<String> {
    let mut problems = Vec::new();
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    if lines.last().is_none_or(|l| *l != "M2" && *l != "M30") {
        problems.push("The program does not end with M2.".to_owned());
    }
    let off_marker = match o.power {
        LinuxCncPower::AnalogM67 { channel } => format!("M67 E{channel} Q0"),
        LinuxCncPower::Spindle => "M5".to_owned(),
    };
    let last_power = lines.iter().rposition(|l| l.starts_with("M67") || l.starts_with("M3") || l.starts_with("M4") || l.starts_with("M5"));
    match last_power {
        Some(i) if lines[i] == off_marker || lines[i] == "M5" => {}
        _ => problems.push("The laser may still be on at the end of the program.".to_owned()),
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::DeviceKind;

    fn job() -> Vec<String> {
        ["G21", "G90", "M8", "M4 S0", "G0 X10 Y10", "G1 X20 S500 F1200", "G1 X30 S500", "G0 X0 Y0", "M5", "M9"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    #[test]
    fn analog_program() {
        let p = DeviceProfile::for_kind(DeviceKind::LinuxCnc);
        let o = LinuxCncOptions::for_profile(&p);
        let t = write_program(&job(), &p, &o);
        assert!(t.starts_with("(DarkBurn laser job)\nG17 G21 G90 G94 G54\nG64 P0.05\n"), "{t}");
        assert!(t.contains("M64 P0\n"));
        assert!(t.contains("M67 E0 Q50\nG1 X20 F1200\n"), "{t}");
        // Power goes off before the rapid.
        assert!(t.contains("G1 X30\nM67 E0 Q0\nG0 X0 Y0\n"), "{t}");
        assert!(t.ends_with("M2\n"));
        assert!(check_program(&t, &o).is_empty(), "{:?}", check_program(&t, &o));
    }

    #[test]
    fn spindle_program_and_checker() {
        let p = DeviceProfile::for_kind(DeviceKind::LinuxCnc);
        let o = LinuxCncOptions { power: LinuxCncPower::Spindle, max_q: 100.0, ..Default::default() };
        let t = write_program(&job(), &p, &o);
        assert!(t.contains("M4 S0\n"));
        assert!(t.contains("G1 X20 S50 F1200\n"), "{t}");
        let bad = "G1 X1\nM67 E0 Q40\nG1 X2\n";
        let probs = check_program(bad, &LinuxCncOptions::default());
        assert_eq!(probs.len(), 2);
    }
}
