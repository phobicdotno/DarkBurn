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

/// The burning moves of the image job without and with a 0.4 mm scanning offset at its speed.
fn with_and_without_offset(setup: impl Fn(&mut BuildInput, &mut Document)) -> Vec<(crate::job_field_tests::Seg, crate::job_field_tests::Seg)> {
    let mk = |tuning: crate::scan_offsets::ScanTuning| {
        let mut d = image_doc(|s| s.image_mode = layer::ImageMode::Threshold, vec![0u8; 16]);
        let mut i = input(Document::new());
        setup(&mut i, &mut d);
        i.doc = d;
        i.tuning = Some(tuning);
        let b = build_job_simple(&i).unwrap();
        segs(&b.toolpath).into_iter().filter(|s| s.kind == 'C' && s.power > 0.0).collect::<Vec<_>>()
    };
    let base = mk(crate::scan_offsets::ScanTuning::default());
    let mut t = crate::scan_offsets::ScanTuning::default();
    t.offsets.set(200.0, 0.4);
    let shifted = mk(t);
    assert_eq!(base.len(), shifted.len());
    assert!(base.len() > 4);
    base.into_iter().zip(shifted).collect()
}

#[test]
fn scan_offsets_follow_the_direction_each_line_is_really_scanned_in() {
    // The machine's 0,0 is top left: the head starts above the image and scans it top down,
    // so every line runs the other way than in a bottom-up scan.
    let pairs = with_and_without_offset(|i, _| i.device.origin = darkburn_job::Corner::TopLeft);
    assert!(pairs[0].0.from.y < pairs[pairs.len() - 1].0.from.y, "scanned from the top (machine Y grows downwards)");
    for (a, b) in &pairs {
        let forward = a.to.x > a.from.x;
        let dx = b.from.x - a.from.x;
        assert!((dx - if forward { -0.2 } else { 0.2 }).abs() < 1e-6, "forward lines move back, reverse lines forward: {dx} (forward: {forward})");
    }
}

#[test]
fn scan_offsets_leave_a_one_way_scan_alone() {
    let pairs = with_and_without_offset(|_, d| d.layer_mut(3).main_mut().bidirectional = false);
    for (a, b) in &pairs {
        assert!(a.to.x > a.from.x && b.to.x > b.from.x, "every line runs forward");
        assert!((b.from.x - a.from.x).abs() < 1e-9, "no line moves: {}", b.from.x - a.from.x);
    }
}

#[test]
fn a_job_on_a_machine_with_an_origin_set_runs_where_its_frame_runs() {
    use crate::machine_api::{JobData, LaserState};
    use serde_json::json;
    let mut a = crate::shell::DarkBurn::new(crate::prefs::Prefs::default());
    let run = |a: &mut crate::shell::DarkBurn, id: &str, args: serde_json::Value| crate::api::execute_headless(a, id, &args).unwrap();
    let wait = |a: &mut crate::shell::DarkBurn, what: &str, f: &dyn Fn(&crate::shell::DarkBurn) -> bool| {
        let end = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            a.machine.tick();
            if f(a) {
                return;
            }
            assert!(std::time::Instant::now() < end, "timed out waiting for {what}");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    };
    let ready = |a: &crate::shell::DarkBurn| a.machine.handle.snapshot().state == LaserState::Ready;
    let id = a.machine.devices.devices.iter().find(|d| d.kind == crate::devices::DeviceType::SimulatorGrbl).unwrap().id;
    run(&mut a, "device.select", json!({ "id": id }));
    run(&mut a, "machine.connect", json!({}));
    wait(&mut a, "ready", &ready);
    run(&mut a, "shape.add_rect", json!({ "x": 10, "y": 10, "width": 20, "height": 20, "layer": 0 }));
    // Put the head at 50, 60 and set the origin there (GRBL keeps it as a work offset).
    run(&mut a, "machine.move_to", json!({ "x": 50, "y": 60 }));
    wait(&mut a, "the move", &|a| {
        let s = a.machine.handle.snapshot();
        ready(a) && (s.machine_pos[0] - 50.0).abs() < 1e-6 && (s.machine_pos[1] - 60.0).abs() < 1e-6
    });
    run(&mut a, "machine.set_origin", json!({}));
    wait(&mut a, "the work offset", &|a| {
        let s = a.machine.handle.snapshot();
        ready(a) && s.work_pos[0].abs() < 1e-6 && s.work_pos[1].abs() < 1e-6 && (s.machine_pos[0] - 50.0).abs() < 1e-6
    });
    // The X range of the moves in a job's G-code, as the controller reads them.
    let x_range = |a: &crate::shell::DarkBurn| {
        let job = crate::job_cmds::build_now(a).unwrap();
        let JobData::Gcode(lines) = crate::machine_job::request_for(a, &job, None, false).unwrap().data else { panic!("a G-code job") };
        let xs: Vec<f64> = lines
            .iter()
            .filter(|l| l.starts_with("G0") || l.starts_with("G1"))
            .flat_map(|l| l.split_whitespace())
            .filter_map(|w| w.strip_prefix('X'))
            .filter_map(|v| v.parse().ok())
            .collect();
        (xs.iter().copied().fold(f64::MAX, f64::min), xs.iter().copied().fold(f64::MIN, f64::max))
    };
    let frame_range = |a: &crate::shell::DarkBurn| {
        let tp = build_frame(&crate::job_cmds::build_input(a), FrameKind::Rectangle, 100.0, None).unwrap();
        let xs: Vec<f64> = crate::machine_job::work_points(a, &tp).iter().map(|p| p.x).collect();
        (xs.iter().copied().fold(f64::MAX, f64::min), xs.iter().copied().fold(f64::MIN, f64::max))
    };
    // Absolute coordinates: the square is at bed 10..30, which is 40 mm left of the origin.
    assert_eq!(frame_range(&a), (-40.0, -20.0));
    assert_eq!(x_range(&a), frame_range(&a), "absolute coordinates");
    // From the user origin: the square's corner sits on the origin.
    run(&mut a, "job.set_origin", json!({ "start_from": "user_origin" }));
    assert_eq!(frame_range(&a), (0.0, 20.0));
    assert_eq!(x_range(&a), frame_range(&a), "user origin");
}

#[test]
fn a_saved_absolute_job_is_not_run_shifted_on_a_machine_with_an_origin_set() {
    use crate::machine_api::LaserState;
    use serde_json::json;
    let mut a = crate::shell::DarkBurn::new(crate::prefs::Prefs::default());
    let run = |a: &mut crate::shell::DarkBurn, id: &str, args: serde_json::Value| crate::api::execute_headless(a, id, &args);
    let wait = |a: &mut crate::shell::DarkBurn, what: &str, f: &dyn Fn(&crate::shell::DarkBurn) -> bool| {
        let end = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            a.machine.tick();
            if f(a) {
                return;
            }
            assert!(std::time::Instant::now() < end, "timed out waiting for {what}");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    };
    let ready = |a: &crate::shell::DarkBurn| a.machine.handle.snapshot().state == LaserState::Ready;
    let id = a.machine.devices.devices.iter().find(|d| d.kind == crate::devices::DeviceType::SimulatorGrbl).unwrap().id;
    run(&mut a, "device.select", json!({ "id": id })).unwrap();
    run(&mut a, "machine.connect", json!({})).unwrap();
    wait(&mut a, "ready", &ready);
    run(&mut a, "shape.add_rect", json!({ "x": 10, "y": 10, "width": 20, "height": 20, "layer": 0 })).unwrap();
    let dir = std::env::temp_dir().join(format!("darkburn-run-gcode-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (abs, rel, other) = (dir.join("abs.gc"), dir.join("rel.gc"), dir.join("other.gc"));
    run(&mut a, "job.save_gcode", json!({ "path": abs.to_str().unwrap() })).unwrap();
    run(&mut a, "job.set_origin", json!({ "start_from": "current_position" })).unwrap();
    run(&mut a, "job.save_gcode", json!({ "path": rel.to_str().unwrap() })).unwrap();
    std::fs::write(&other, "G21\nG90\nG0 X1 Y1\n").unwrap();
    let text = |p: &std::path::Path| std::fs::read_to_string(p).unwrap();
    assert!(crate::machine_job::gcode_in_machine_coordinates(&text(&abs)));
    assert!(!crate::machine_job::gcode_in_machine_coordinates(&text(&rel)), "a job from the current position moves in steps");
    assert!(!crate::machine_job::gcode_in_machine_coordinates(&text(&other)), "a file from elsewhere is in work coordinates");
    // Put the head at 50, 60 and set the origin there (GRBL keeps it as a work offset).
    run(&mut a, "machine.move_to", json!({ "x": 50, "y": 60 })).unwrap();
    wait(&mut a, "the move", &|a| ready(a) && (a.machine.handle.snapshot().machine_pos[0] - 50.0).abs() < 1e-6);
    run(&mut a, "machine.set_origin", json!({})).unwrap();
    wait(&mut a, "the work offset", &|a| {
        let s = a.machine.handle.snapshot();
        ready(a) && s.work_pos[0].abs() < 1e-6 && (s.machine_pos[0] - 50.0).abs() < 1e-6
    });
    // The file's X10 would be read as 10 mm from the origin: bed X60, not bed X10.
    let err = run(&mut a, "job.run_gcode", json!({ "path": abs.to_str().unwrap() })).unwrap_err();
    assert!(err.contains("origin"), "{err}");
    a.machine.tick();
    assert!(ready(&a), "nothing was started");
    // A file from elsewhere is run as it is.
    run(&mut a, "job.run_gcode", json!({ "path": other.to_str().unwrap() })).unwrap();
    wait(&mut a, "the other file's move", &|a| ready(a) && (a.machine.handle.snapshot().work_pos[0] - 1.0).abs() < 1e-6);
    // With the origin cleared the saved job runs where it was drawn.
    run(&mut a, "machine.clear_origin", json!({})).unwrap();
    wait(&mut a, "no work offset", &|a| {
        let s = a.machine.handle.snapshot();
        ready(a) && (s.machine_pos[0] - s.work_pos[0]).abs() < 1e-6 && (s.machine_pos[1] - s.work_pos[1]).abs() < 1e-6
    });
    run(&mut a, "job.run_gcode", json!({ "path": abs.to_str().unwrap() })).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Every G-code file DarkBurn saves with absolute coordinates is known as one, whatever the
/// device writes before the job: a header, a prelude, a start script.
#[test]
fn a_saved_absolute_job_is_recognised_for_every_g_code_device() {
    use crate::devices::DeviceType as T;
    use serde_json::json;
    let run = |a: &mut crate::shell::DarkBurn, id: &str, args: serde_json::Value| crate::api::execute_headless(a, id, &args);
    let dir = std::env::temp_dir().join(format!("darkburn-known-gcode-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("job.gc");
    let mut wrong = Vec::new();
    for ty in [T::Grbl, T::GrblHal, T::FluidNc, T::Marlin, T::Smoothie, T::Snapmaker, T::LinuxCnc, T::CustomGcode] {
        for script in ["", "G91\nG0 Z5\nG90"] {
            let mut a = crate::shell::DarkBurn::new(crate::prefs::Prefs::default());
            let id = run(&mut a, "device.add", json!({ "type": format!("{ty:?}") })).unwrap()["id"].as_u64().unwrap();
            run(&mut a, "device.select", json!({ "id": id })).unwrap();
            a.machine.devices.active_mut().unwrap().settings.gcode.start_script = script.to_owned();
            a.job.gcode.start_script = script.to_owned();
            run(&mut a, "shape.add_rect", json!({ "x": 10, "y": 10, "width": 20, "height": 20, "layer": 0 })).unwrap();
            run(&mut a, "job.save_gcode", json!({ "path": file.to_str().unwrap() })).unwrap();
            let abs = std::fs::read_to_string(&file).unwrap();
            if !crate::machine_job::gcode_in_machine_coordinates(&abs) {
                wrong.push(format!(
                    "{ty:?} with start script {script:?}: not seen as bed coordinates:\n{}",
                    abs.lines().take(24).collect::<Vec<_>>().join("\n")
                ));
            }
            run(&mut a, "job.set_origin", json!({ "start_from": "current_position" })).unwrap();
            run(&mut a, "job.save_gcode", json!({ "path": file.to_str().unwrap(), "allow_out_of_bounds": true })).unwrap();
            let rel = std::fs::read_to_string(&file).unwrap();
            if crate::machine_job::gcode_in_machine_coordinates(&rel) {
                wrong.push(format!(
                    "{ty:?} with start script {script:?}: a job from the current position seen as bed coordinates:\n{}",
                    rel.lines().take(24).collect::<Vec<_>>().join("\n")
                ));
            }
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(wrong.is_empty(), "{}", wrong.join("\n\n"));
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
fn pauses_a_ruida_job_cannot_carry_are_named_in_a_warning() {
    let mut d = line_doc(20.0);
    {
        let s = d.layer_mut(0).main_mut();
        s.start_pause_ms = 200.0;
        s.cut_through_start = true;
        s.cut_through_power = 80.0;
        s.end_pause_ms = 50.0;
    }
    // A G-code machine gets the pauses: no warning.
    let gcode = build_job_simple(&input(d.clone())).unwrap();
    assert!(gcode.toolpath.ops.iter().any(|o| matches!(o, Op::Dwell { .. })));
    assert!(!gcode.warnings.iter().any(|w| w.contains("pause")), "{:?}", gcode.warnings);
    // A Ruida job has no place for them, and says so.
    let ruida_dev = crate::devices::Device::new(crate::devices::DeviceType::SimulatorRuida);
    assert!(ruida_dev.job_device().no_pauses);
    assert!(!crate::devices::Device::default().job_device().no_pauses);
    let mut i = input(d.clone());
    i.device.no_pauses = true;
    let b = build_job_simple(&i).unwrap();
    let said: Vec<&String> = b.warnings.iter().filter(|w| w.contains("pause")).collect();
    assert_eq!(said.len(), 1, "one warning for the layer: {:?}", b.warnings);
    assert!(said[0].contains(&layer::layer_label(0)), "{said:?}");
    // Without pauses there is nothing to warn about, and dot mode is sent (no warning either).
    {
        let s = d.layer_mut(0).main_mut();
        s.start_pause_ms = 0.0;
        s.cut_through_start = false;
        s.end_pause_ms = 0.0;
        s.dot_mode = true;
        s.dot_time_ms = 3.0;
        s.dot_spacing = 2.0;
    }
    let mut i = input(d);
    i.device.no_pauses = true;
    let b = build_job_simple(&i).unwrap();
    assert!(!b.warnings.iter().any(|w| w.contains("pause")), "{:?}", b.warnings);
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
    let g = gcode_text(
        &b,
        None,
        &DeviceGcodeSettings { auto_home: true, start_script: "M999".into(), end_script: "M5 ; done".into(), ..Default::default() },
    );
    assert!(
        g.starts_with("$H")
            || g.contains(
                "$H
"
            ),
        "{g}"
    );
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

#[test]
fn scan_offsets_shift_the_lines_of_a_fill_too() {
    let mk = |tuning: crate::scan_offsets::ScanTuning, bidirectional: bool| {
        let mut d = fill_doc(2.0, 20.0);
        d.layer_mut(0).main_mut().bidirectional = bidirectional;
        let mut i = input(d);
        i.tuning = Some(tuning);
        let b = build_job_simple(&i).unwrap();
        segs(&b.toolpath).into_iter().filter(|s| s.kind == 'C' && s.power > 0.0).collect::<Vec<_>>()
    };
    let mut t = crate::scan_offsets::ScanTuning::default();
    t.offsets.set(100.0, 0.4);
    let (base, shifted) = (mk(crate::scan_offsets::ScanTuning::default(), true), mk(t.clone(), true));
    assert_eq!(base.len(), 10);
    assert_eq!(base.len(), shifted.len());
    for (a, b) in base.iter().zip(&shifted) {
        let forward = a.to.x > a.from.x;
        let dx = b.from.x - a.from.x;
        assert!((dx - if forward { -0.2 } else { 0.2 }).abs() < 1e-6, "forward lines move back, reverse lines forward: {dx}");
    }
    let (base, shifted) = (mk(crate::scan_offsets::ScanTuning::default(), false), mk(t, false));
    for (a, b) in base.iter().zip(&shifted) {
        assert!((b.from.x - a.from.x).abs() < 1e-9, "a one-way fill is not shifted");
    }
}

#[test]
fn the_time_estimate_knows_the_fast_whitespace_scan() {
    let mut a = crate::shape_ops::tests::app();
    let mut d = fill_doc(1.0, 40.0);
    sq(&mut d, 0, 15.0, 15.0, 9.0); // a hole: nine rows cross 9 mm of blank
    d.layer_mut(0).main_mut().speed = 5.0;
    a.editor.doc = d;
    let seconds = |a: &crate::shell::DarkBurn| {
        let job = build_job_simple(&crate::job_cmds::build_input(a)).unwrap();
        crate::job_cmds::summary(a, &job)["seconds"].as_f64().unwrap()
    };
    let plain = seconds(&a);
    a.job.gcode.fast_whitespace_scan = true;
    a.job.gcode.fast_whitespace_speed = 400.0;
    let fast = seconds(&a);
    // 81 mm of blank: 16 s at 5 mm/s, well under a second at 400 mm/s.
    assert!(fast < plain - 10.0, "the estimate is shorter with fast whitespace on: {plain} s, then {fast} s");
}
