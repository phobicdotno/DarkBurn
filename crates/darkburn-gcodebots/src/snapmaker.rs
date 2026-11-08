//! Snapmaker (GCODE-04): the file header, focus and material height, and basic serial control.
//!
//! Snapmaker 2.0 machines run a Marlin-derived firmware, so the live driver is the Marlin one
//! with the laser on a fan-style output (`M106`/`M107`). The file header (`;Header Start`
//! ... `;Header End`) is the block Snapmaker's own software writes in front of laser files so
//! the touchscreen can show size, speed and time. The field names here come from public
//! Snapmaker forum and wiki descriptions of that block and have to be confirmed on a machine
//! (`docs/hardware-checks-gcodebots.md`).
//!
//! Focus: the laser module is focused with the material height. DarkBurn raises Z to
//! `focus_height_mm + material_height_mm` before the job (`G0 Z...`).

use crate::detect::FirmwareGuess;
use crate::line::LineConfig;
use crate::machine::MachineFamily;
use crate::marlin;
use crate::profile::DeviceProfile;
use kurbo::Rect;

/// Driver configuration: Marlin's, accepting a Snapmaker banner.
pub fn config(p: &DeviceProfile) -> LineConfig {
    let mut c = marlin::config(p);
    c.family = MachineFamily::Snapmaker;
    c.accept = vec![FirmwareGuess::Snapmaker, FirmwareGuess::Marlin];
    c.default_name = "Snapmaker".into();
    c
}

/// Facts about the job that go into the header.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct JobInfo {
    /// Bounds of the job in mm.
    pub bounds: Rect,
    /// Work speed in mm/min (the main cut speed).
    pub work_speed: f64,
    /// Estimated time in seconds.
    pub estimated_seconds: f64,
    /// Power in percent.
    pub power_percent: f64,
}

/// The `;Header Start` block.
pub fn header(p: &DeviceProfile, info: &JobInfo) -> String {
    let s = &p.gcode.snapmaker;
    let b = info.bounds;
    let z = s.focus_height_mm + s.material_height_mm;
    let mut h = String::new();
    h.push_str(";Header Start\n");
    h.push_str(";header_type: laser\n");
    h.push_str(&format!(";tool_head: {}\n", s.tool_head));
    h.push_str(&format!(";machine: {}\n", s.model));
    h.push_str(";gcode_image:\n");
    h.push_str(&format!(";work_speed: {}\n", info.work_speed.round() as i64));
    h.push_str(&format!(";estimated_time(s): {}\n", info.estimated_seconds.round() as i64));
    h.push_str(";is_rotate: false\n");
    h.push_str(&format!(";max_x(mm): {:.3}\n;max_y(mm): {:.3}\n;max_z(mm): {z:.3}\n", b.x1, b.y1));
    h.push_str(&format!(";min_x(mm): {:.3}\n;min_y(mm): {:.3}\n;min_z(mm): {z:.3}\n", b.x0, b.y0));
    h.push_str(&format!(";power(%): {}\n", info.power_percent.round() as i64));
    h.push_str(";Header End\n");
    h
}

/// Lines that put the laser at the focus height for the material.
pub fn focus_prelude(p: &DeviceProfile) -> Vec<String> {
    let s = &p.gcode.snapmaker;
    let z = s.focus_height_mm + s.material_height_mm;
    vec!["G90".to_owned(), format!("G0 Z{} F600", darkburn_grbl::protocol::fmt_num(z))]
}

/// A complete file: header, focus prelude, then the already translated job lines.
pub fn file_text(p: &DeviceProfile, info: &JobInfo, job_lines: &[String]) -> String {
    let mut out = String::new();
    if p.gcode.snapmaker.header {
        out.push_str(&header(p, info));
    }
    for l in focus_prelude(p) {
        out.push_str(&l);
        out.push('\n');
    }
    for l in job_lines {
        out.push_str(l);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::DeviceKind;

    #[test]
    fn header_and_focus() {
        let mut p = DeviceProfile::for_kind(DeviceKind::Snapmaker);
        p.gcode.snapmaker.focus_height_mm = 10.0;
        p.gcode.snapmaker.material_height_mm = 3.5;
        let info = JobInfo { bounds: Rect::new(0.0, 0.0, 50.0, 40.0), work_speed: 1200.0, estimated_seconds: 61.4, power_percent: 80.0 };
        let h = header(&p, &info);
        assert!(h.starts_with(";Header Start\n"));
        assert!(h.contains(";machine: A350\n"));
        assert!(h.contains(";max_x(mm): 50.000\n"));
        assert!(h.contains(";estimated_time(s): 61\n"));
        assert!(h.ends_with(";Header End\n"));
        assert_eq!(focus_prelude(&p), vec!["G90".to_owned(), "G0 Z13.5 F600".to_owned()]);
        let f = file_text(&p, &info, &["G1 X1".to_owned()]);
        assert!(f.ends_with("G0 Z13.5 F600\nG1 X1\n"));
        p.gcode.snapmaker.header = false;
        assert!(file_text(&p, &info, &[]).starts_with("G90\n"));
    }

    #[test]
    fn config_accepts_snapmaker_banner() {
        let p = DeviceProfile::for_kind(DeviceKind::Snapmaker);
        let c = config(&p);
        assert_eq!(c.family, MachineFamily::Snapmaker);
        assert!(c.accept.contains(&FirmwareGuess::Snapmaker));
        assert_eq!(c.commands.fire_on, "M106 P0 S{s}");
    }
}
