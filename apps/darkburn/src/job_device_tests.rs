//! Device settings applied by the job builder, and the DSP-only fields in the Ruida job.

use darkburn_core::kurbo::{Point, Rect};
use darkburn_core::layer::{self, LaserSelect};
use darkburn_core::{Document, Shape};
use darkburn_job::Op;

use crate::job_build::*;
use crate::job_field_tests::{build, count, fill_doc, image_doc, input, line_doc, segs, sq};

#[test]
fn z_reverse_flips_the_z_moves() {
    let mut d = line_doc(20.0);
    d.layer_mut(0).main_mut().z_offset = 2.0;
    let mut i = input(d);
    i.device.z_enabled = true;
    i.device.z_reverse = true;
    let b = build_job_simple(&i).unwrap();
    assert!(b.toolpath.ops.iter().any(|o| matches!(o, Op::ZMove { z } if *z == -2.0)));
    assert!(!b.toolpath.ops.iter().any(|o| matches!(o, Op::ZMove { z } if *z == 2.0)));
}

#[test]
fn red_dot_offset_moves_the_frame_but_not_the_job() {
    let mut i = input(line_doc(20.0));
    i.device.pointer_offset = [3.0, -2.0];
    let plain = build_frame(&input(line_doc(20.0)), FrameKind::Rectangle, 100.0, None).unwrap();
    let dot = build_frame(&i, FrameKind::Rectangle, 100.0, None).unwrap();
    let (a, b) = (plain.ops[0].target().unwrap(), dot.ops[0].target().unwrap());
    assert!((b.x - a.x + 3.0).abs() < 1e-9 && (b.y - a.y - 2.0).abs() < 1e-9, "the head goes so that the dot lands on the frame: {a:?} {b:?}");
    // With the laser firing the frame is not shifted.
    let lit = build_frame(&i, FrameKind::Rectangle, 100.0, Some(1.0)).unwrap();
    assert_eq!(lit.ops[0].target(), plain.ops[0].target());
    // The job itself is not shifted.
    let job_a = build_job_simple(&input(line_doc(20.0))).unwrap();
    let job_b = build_job_simple(&i).unwrap();
    assert_eq!(job_a.toolpath.ops, job_b.toolpath.ops);
}

#[test]
fn frame_bounds_are_checked() {
    let mut d = line_doc(20.0);
    sq(&mut d, 0, 190.0, 190.0, 30.0);
    let i = input(d);
    let tp = build_frame(&i, FrameKind::Rectangle, 100.0, None).unwrap();
    assert!(check_frame_bounds(&i, &tp).is_err());
    let i = input(line_doc(20.0));
    let tp = build_frame(&i, FrameKind::Rectangle, 100.0, None).unwrap();
    assert!(check_frame_bounds(&i, &tp).is_ok());
}

#[test]
fn fast_whitespace_speeds_up_blank_moves_in_scans() {
    let mut d = fill_doc(2.0, 20.0);
    sq(&mut d, 0, 15.0, 15.0, 10.0); // a hole: rows with a blank gap
    d.layer_mut(0).main_mut().speed = 50.0;
    let b = build(d);
    let speeds = |tp: &darkburn_job::Toolpath| -> Vec<f64> {
        tp.ops.iter().filter_map(|o| if let Op::Cut { speed, power, .. } = o { (*power == 0.0).then_some(*speed) } else { None }).collect()
    };
    let dev = DeviceGcodeSettings { fast_whitespace_scan: true, fast_whitespace_speed: 300.0, ..Default::default() };
    let before = speeds(&b.toolpath);
    assert!(!before.is_empty() && before.iter().all(|s| *s == 50.0));
    let after = speeds(&fast_whitespace(&b.toolpath, 300.0));
    assert_eq!(before.len(), after.len());
    assert!(after.iter().all(|s| *s == 300.0));
    // And the G-code carries it: F18000 is 300 mm/s.
    assert!(gcode_text(&b, None, &dev).contains("F18000"));
    assert!(!gcode_text(&b, None, &DeviceGcodeSettings::default()).contains("F18000"));
}

#[test]
fn scan_offset_table_shifts_forward_and_reverse_lines() {
    let black = vec![0u8; 16];
    let mk = |tuning: crate::scan_offsets::ScanTuning| {
        let mut i = input(image_doc(|s| s.image_mode = layer::ImageMode::Threshold, black.clone()));
        i.tuning = Some(tuning);
        let b = build_job_simple(&i).unwrap();
        segs(&b.toolpath).into_iter().filter(|s| s.kind == 'C' && s.power > 0.0).collect::<Vec<_>>()
    };
    let base = mk(crate::scan_offsets::ScanTuning::default());
    let mut t = crate::scan_offsets::ScanTuning::default();
    t.offsets.set(200.0, 0.4);
    let shifted = mk(t);
    assert_eq!(base.len(), shifted.len());
    for (a, b) in base.iter().zip(&shifted) {
        let forward = a.to.x > a.from.x;
        let dx = b.from.x - a.from.x;
        assert!((dx - if forward { -0.2 } else { 0.2 }).abs() < 1e-6, "forward lines move back, reverse lines forward: {dx}");
    }
}

#[test]
fn output_tolerance_changes_how_curves_are_flattened() {
    let mut d = Document::new();
    d.layer_mut(0).main_mut().max_power = 10.0;
    d.add(Shape::ellipse(Rect::new(10.0, 10.0, 110.0, 110.0), 0));
    let n = |tol: f64| {
        let mut i = input(d.clone());
        i.device.tolerance = tol;
        count(&build_job_simple(&i).unwrap().toolpath, |o| matches!(o, Op::Cut { .. }))
    };
    assert!(n(0.001) > n(0.5) * 2, "{} {}", n(0.001), n(0.5));
}

#[test]
fn return_to_finish_goes_back_to_the_start() {
    let s = JobProjectSettings::default().with_device_finish(true);
    assert_eq!(s.finish, darkburn_job::FinishPosition::StartPosition);
    let mut i = input(line_doc(20.0));
    i.settings = s;
    i.settings.start_from = StartMode::UserOrigin;
    i.user_origin = Some(Point::new(50.0, 60.0));
    let b = build_job_simple(&i).unwrap();
    assert_eq!(b.toolpath.ops.last().unwrap().target(), Some(Point::new(50.0, 60.0)));
    let s = JobProjectSettings::default().with_device_finish(false);
    assert_eq!(s.finish, darkburn_job::FinishPosition::Stay);
}

#[test]
fn dsp_fields_reach_the_ruida_job() {
    use darkburn_core::kurbo::Vec2;
    let mut d = line_doc(20.0);
    {
        let s = d.layer_mut(0).main_mut();
        s.pwm_override = true;
        s.pwm_khz = 25.0;
        s.u_offset = 4.0;
        s.laser = LaserSelect::Both;
        s.max_power_2 = 33.0;
        s.min_power_2 = 11.0;
        s.min_power = 5.0;
    }
    let b = build(d.clone());
    let rd = crate::machine_job::toolpath_to_rd(&b.toolpath, Vec2::ZERO).unwrap();
    let l = &rd.layers[0];
    assert_eq!(l.settings.frequency_hz, Some(25_000));
    assert_eq!(l.settings.max_power, [50.0, 33.0]);
    assert_eq!(l.settings.min_power, [5.0, 11.0]);
    assert!(matches!(l.ops[0], darkburn_ruida::RdOp::U(u) if u == 4.0));
    // Laser 1 only: the second tube gets no power; no override without the checkbox.
    {
        let s = d.layer_mut(0).main_mut();
        s.laser = LaserSelect::Laser1;
        s.pwm_override = false;
        s.u_offset = 0.0;
    }
    let rd = crate::machine_job::toolpath_to_rd(&build(d.clone()).toolpath, Vec2::ZERO).unwrap();
    assert_eq!(rd.layers[0].settings.frequency_hz, None);
    assert_eq!(rd.layers[0].settings.max_power, [50.0, 0.0]);
    // Dot mode becomes the layer's dot mode with a zero-length cut per dot.
    {
        let s = d.layer_mut(0).main_mut();
        s.dot_mode = true;
        s.dot_time_ms = 3.0;
        s.dot_spacing = 2.0;
    }
    let rd = crate::machine_job::toolpath_to_rd(&build(d).toolpath, Vec2::ZERO).unwrap();
    assert_eq!(rd.layers[0].settings.dot_mode.map(|m| (m.dot_time_ms, m.spacing_mm)), Some((3.0, 2.0)));
    assert_eq!(rd.layers[0].ops.iter().filter(|o| matches!(o, darkburn_ruida::RdOp::Cut { .. })).count(), 40);
}

#[test]
fn z_relative_only_and_z_optimize_change_the_gcode() {
    let mut d = line_doc(20.0);
    {
        let s = d.layer_mut(0).main_mut();
        s.passes = 2;
        s.z_offset = 2.0;
        s.z_step = 0.0;
    }
    let mut i = input(d);
    i.device.z_enabled = true;
    let b = build_job_simple(&i).unwrap();
    let plain = gcode_text(&b, None, &DeviceGcodeSettings::default());
    assert!(plain.contains("G0 Z2") && !plain.contains("G91"), "{plain}");
    assert_eq!(plain.matches("G0 Z2").count(), 1, "the second pass is already at Z2: {plain}");
    let always = gcode_text(&b, None, &DeviceGcodeSettings { z_optimize: false, ..Default::default() });
    assert_eq!(always.matches("G0 Z").count(), 2, "{always}");
    let rel = gcode_text(&b, None, &DeviceGcodeSettings { z_relative_only: true, ..Default::default() });
    assert!(rel.contains("G91") && rel.contains("G90"), "{rel}");
}

#[test]
fn gcode_device_settings_reach_the_writer() {
    let b = build(line_doc(20.0));
    let g = gcode_text(&b, None, &DeviceGcodeSettings { auto_home: true, start_script: "M999".into(), end_script: "M5 ; done".into(), ..Default::default() });
    assert!(g.starts_with("$H") || g.contains("$H
"), "{g}");
    assert!(g.contains("M999") && g.contains("M5 ; done"));
    assert!(g.find("$H").unwrap() < g.find("M999").unwrap());
    // S with every G1.
    let every = gcode_text(&b, None, &DeviceGcodeSettings { s_every_g1: true, ..Default::default() });
    let lone = gcode_text(&b, None, &DeviceGcodeSettings::default());
    assert!(every.matches('S').count() > lone.matches('S').count());
    // Air command.
    let air = gcode_text(&b, None, &DeviceGcodeSettings { air: darkburn_job::gcode::AirCommand::M7, ..Default::default() });
    assert!(air.contains("M7") && !air.contains("M8"));
    // Overscan as G0.
    let fill = build(fill_doc(1.0, 10.0));
    let mut d = fill_doc(1.0, 10.0);
    d.layer_mut(0).main_mut().overscan = 2.0;
    d.layer_mut(0).main_mut().overscan_percent = false;
    let _ = fill;
    let over = build(d);
    let g0 = gcode_text(&over, None, &DeviceGcodeSettings { overscan_as_g0: true, ..Default::default() });
    let g1 = gcode_text(&over, None, &DeviceGcodeSettings::default());
    assert!(g0.matches("G0").count() > g1.matches("G0").count());
}

#[test]
fn tool_layers_are_framed_only_when_asked_and_never_burn() {
    let mut d = line_doc(20.0);
    sq(&mut d, darkburn_core::T1, 100.0, 100.0, 10.0);
    let i = input(d.clone());
    let plain = build_frame(&i, FrameKind::Rectangle, 100.0, None).unwrap();
    let max_x = |tp: &darkburn_job::Toolpath| tp.ops.iter().filter_map(Op::target).map(|p| p.x).fold(f64::MIN, f64::max);
    assert!((max_x(&plain) - 30.0).abs() < 1e-6, "tool layer ignored by default");
    d.layer_mut(darkburn_core::T1).frame = true;
    let i = input(d.clone());
    let framed = build_frame(&i, FrameKind::Rectangle, 100.0, None).unwrap();
    assert!((max_x(&framed) - 110.0).abs() < 1e-6, "{}", max_x(&framed));
    let rubber = build_frame(&i, FrameKind::RubberBand, 100.0, None).unwrap();
    assert!((max_x(&rubber) - 110.0).abs() < 1e-6);
    // The job itself never includes the tool layer.
    let job = build_job_simple(&i).unwrap();
    assert!((job.bounds.x1 - 30.0).abs() < 1e-6);
}
