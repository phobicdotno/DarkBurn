//! Golden G-code for the cut-settings fields: perforation, overcut, pauses, manual and partial
//! tabs, dot mode, lead-in and lead-out, Z steps, constant power, flood fill, ramps, overscan
//! as a percentage, angle increase per pass and offset fill.
//! Regenerate with `DARKBURN_BLESS=1 cargo test -p darkburn-job --test golden_fields` and review
//! the diff by eye before committing.

use std::path::PathBuf;

use darkburn_job::gcode::{self, Flavor, GcodeSettings};
use darkburn_job::*;
use kurbo::{Point, Rect, Shape};

fn check(name: &str, actual: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden").join(name);
    if std::env::var_os("DARKBURN_BLESS").is_some() {
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected =
        std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("missing golden file {name}; run with DARKBURN_BLESS=1")).replace("\r\n", "\n");
    if expected != actual {
        let first = expected.lines().zip(actual.lines()).position(|(a, b)| a != b);
        panic!("{name} differs (first differing line {first:?}):\n{actual}");
    }
}

fn absolute() -> JobOptions {
    JobOptions {
        origin: Some(OriginSettings { machine: MachineGeometry { width: 200.0, height: 200.0, ..Default::default() }, ..Default::default() }),
        ..Default::default()
    }
}

fn gcode_of(layer: LayerInput, flavor: Flavor) -> String {
    let tp = generate(&JobInput { layers: vec![layer] }, &absolute(), &mut Progress::none()).unwrap();
    gcode::emit(&tp, &GcodeSettings::new(flavor))
}

fn square(x: f64, y: f64, w: f64) -> kurbo::BezPath {
    Rect::new(x, y, x + w, y + w).to_path(0.01)
}

#[test]
fn perforation_overcut_and_pauses() {
    let mut s = CutSettings::line(10.0, 50.0);
    s.line.perforation = Some(Perforation { cut: 3.0, skip: 1.0 });
    s.line.overcut = 2.0;
    s.line.start_pause_ms = 100.0;
    s.line.end_pause_ms = 50.0;
    s.line.cut_through_start = true;
    s.line.pause_power = 20.0;
    let l = LayerInput::new(0, "Perf", s).with_shape(ShapeInput::new(square(10.0, 10.0, 8.0)));
    check("perforation_overcut_pause_grbl.gcode", &gcode_of(l, Flavor::Grbl));
}

#[test]
fn manual_and_partial_tabs() {
    let mut s = CutSettings::line(10.0, 60.0);
    s.line.tabs = TabSettings { enabled: true, auto: false, size: 2.0, tab_power: Some(10.0), ..Default::default() };
    let mut sh = ShapeInput::new(square(10.0, 10.0, 20.0));
    sh.tabs = vec![Point::new(30.0, 20.0), Point::new(20.0, 10.0)];
    let l = LayerInput::new(0, "Tabs", s).with_shape(sh);
    check("tabs_manual_partial_grbl.gcode", &gcode_of(l, Flavor::Grbl));
}

#[test]
fn dot_mode() {
    let mut s = CutSettings::line(10.0, 40.0);
    s.line.dot_mode = Some(DotMode { time_ms: 5.0, spacing: 4.0 });
    let l = LayerInput::new(0, "Dots", s).with_shape(ShapeInput::new(square(0.0, 0.0, 8.0)));
    check("dot_mode_grbl.gcode", &gcode_of(l, Flavor::Grbl));
}

#[test]
fn lead_in_and_out_as_lines() {
    let mut s = CutSettings::line(10.0, 60.0);
    s.line.lead_in = Some(Lead { length: 3.0, angle_deg: 45.0, style: LeadStyle::Line });
    s.line.lead_out = Some(Lead { length: 2.0, angle_deg: 90.0, style: LeadStyle::Line });
    let l = LayerInput::new(0, "Leads", s).with_shape(ShapeInput::new(square(20.0, 20.0, 10.0)));
    check("lead_line_marlin.gcode", &gcode_of(l, Flavor::Marlin));
}

#[test]
fn z_steps_with_constant_power_and_air() {
    let mut s = CutSettings::line(5.0, 90.0);
    s.passes = 3;
    s.constant_power = true;
    s.air = true;
    s.z = ZSettings { enabled: true, offset: 1.0, step_per_pass: -0.5 };
    let l = LayerInput::new(0, "Z", s).with_shape(ShapeInput::new(square(5.0, 5.0, 5.0)));
    check("z_passes_constant_air_grbl.gcode", &gcode_of(l, Flavor::Grbl));
}

#[test]
fn flood_fill_with_ramps_and_percent_overscan() {
    let mut s = CutSettings::fill(100.0, 40.0, 1.0);
    s.min_power = 10.0;
    s.fill.flood_fill = true;
    s.fill.ramp_length = 1.5;
    s.fill.overscan = Overscan::Percent(2.0);
    s.fill.bidirectional = false;
    let mut p = square(0.0, 0.0, 6.0);
    p.extend(square(10.0, 0.0, 6.0));
    let l = LayerInput::new(0, "Flood", s).with_shape(ShapeInput::new(p));
    check("fill_flood_ramp_grbl.gcode", &gcode_of(l, Flavor::Grbl));
}

#[test]
fn angle_increase_per_pass() {
    let mut s = CutSettings::fill(100.0, 30.0, 1.0);
    s.passes = 2;
    s.fill.angle_deg = 10.0;
    s.fill.angle_step_per_pass = 90.0;
    s.fill.bidirectional = false;
    let l = LayerInput::new(0, "Angle", s).with_shape(ShapeInput::new(square(0.0, 0.0, 4.0)));
    check("fill_angle_step_grbl.gcode", &gcode_of(l, Flavor::Grbl));
}

#[test]
fn offset_fill_rings() {
    let mut s = CutSettings::fill(60.0, 25.0, 1.0);
    s.mode = CutMode::OffsetFill;
    let mut sh = ShapeInput::new(square(0.0, 0.0, 6.0));
    sh.power_scale = 0.5;
    let l = LayerInput::new(0, "Offset", s).with_shape(sh);
    check("offset_fill_grbl.gcode", &gcode_of(l, Flavor::Grbl));
}
