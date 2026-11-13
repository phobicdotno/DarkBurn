//! Bug hunt 10, the app's machine side (Laser panel, device reads, galvo settings). Every test
//! drives the app against the in-memory emulators; no port, USB device or network connection is
//! opened.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::api::execute_headless;
use crate::devices::DeviceType;
use crate::machine_api::LaserState;
use crate::prefs::Prefs;
use crate::shell::DarkBurn;

const T: Duration = Duration::from_secs(30);

fn run(app: &mut DarkBurn, id: &str, args: Value) -> Result<Value, String> {
    execute_headless(app, id, &args)
}

#[track_caller]
fn wait_ready(app: &mut DarkBurn) {
    let end = Instant::now() + T;
    while app.machine.handle.snapshot().state != LaserState::Ready {
        app.machine.tick();
        assert!(Instant::now() < end, "never ready: {:?}", app.machine.handle.snapshot().state);
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Draw one of the Laser panel's panels for a frame.
fn draw_panel(app: &mut DarkBurn, panel: &str) {
    let ctx = egui::Context::default();
    for _ in 0..2 {
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            assert!(crate::laser_panel::show_panel(ui, panel, app));
        });
        out.textures_delta.clear();
    }
}

/// Files ticked in the File List of one controller stay ticked after the user switches to
/// another controller, whose list has not even been read: its Start and Delete buttons then run
/// or delete files of the new controller by the old one's numbers.
#[test]
fn the_file_list_forgets_ticks_from_another_controller() {
    let mut a = DarkBurn::new(Prefs::default());
    let first = a.machine.devices.devices.iter().find(|d| d.kind == DeviceType::SimulatorRuida).unwrap().id;
    run(&mut a, "device.select", json!({ "id": first })).unwrap();
    run(&mut a, "machine.connect", json!({})).unwrap();
    wait_ready(&mut a);
    // Ticked on the first controller.
    a.machine.file_sel.insert(3);
    let second = run(&mut a, "device.duplicate", json!({ "id": first })).unwrap()["id"].as_u64().unwrap();
    run(&mut a, "device.select", json!({ "id": second })).unwrap();
    run(&mut a, "machine.connect", json!({})).unwrap();
    wait_ready(&mut a);
    assert!(a.machine.handle.snapshot().ruida.files.is_none(), "the new controller's list is not read yet");
    draw_panel(&mut a, "file_list");
    assert!(
        a.machine.file_sel.is_empty(),
        "Start and Delete would act on file(s) {:?} of a controller whose list was never shown",
        a.machine.file_sel
    );
}

/// A Laser 2 power draft read from one dual-tube controller stays in the Laser panel after the
/// user switches to another dual-tube controller: Write is enabled there and sends the first
/// controller's minimum and maximum power to the second.
#[test]
fn the_laser2_draft_does_not_follow_to_another_controller() {
    let mut a = DarkBurn::new(Prefs::default());
    let first = a.machine.devices.devices.iter().find(|d| d.kind == DeviceType::SimulatorRuida).unwrap().id;
    a.machine.devices.get_mut(first).unwrap().settings.dsp.dual_tube = true;
    run(&mut a, "device.select", json!({ "id": first })).unwrap();
    run(&mut a, "machine.connect", json!({})).unwrap();
    wait_ready(&mut a);
    run(&mut a, "machine.dsp_read", json!({ "what": "laser2" })).unwrap();
    let end = Instant::now() + T;
    while a.machine.handle.snapshot().ruida.laser2.is_none() {
        a.machine.tick();
        assert!(Instant::now() < end, "laser 2 never read");
        std::thread::sleep(Duration::from_millis(5));
    }
    draw_panel(&mut a, "move");
    assert!(a.machine.dlg.laser2.is_some(), "the panel shows the first controller's laser 2");
    // The user types other limits for the first controller.
    if let Some(l) = a.machine.dlg.laser2.as_mut() {
        l.min_power = 123;
        l.max_power = 456;
    }
    let second = run(&mut a, "device.duplicate", json!({ "id": first })).unwrap()["id"].as_u64().unwrap();
    run(&mut a, "device.select", json!({ "id": second })).unwrap();
    assert!(a.machine.dlg.laser2.is_none(), "switching controllers keeps the old controller's laser 2 draft");
    run(&mut a, "machine.connect", json!({})).unwrap();
    wait_ready(&mut a);
    assert!(a.machine.handle.snapshot().ruida.laser2.is_none(), "the new controller's laser 2 is not read yet");
    // A draft left behind by anything else must not survive a frame either.
    a.machine.dlg.laser2 = Some(ruida_laser2(123, 456));
    draw_panel(&mut a, "move");
    assert!(a.machine.dlg.laser2.is_none(), "Write would send laser 2 values the new controller never reported");
    // Nothing connected: no draft.
    a.machine.handle.disconnect();
    a.machine.dlg.laser2 = Some(ruida_laser2(123, 456));
    draw_panel(&mut a, "move");
    assert!(a.machine.dlg.laser2.is_none(), "a laser 2 draft with nothing connected");
}

fn ruida_laser2(min: u32, max: u32) -> darkburn_ruida::settings::Laser2Settings {
    darkburn_ruida::settings::Laser2Settings { min_power: min, max_power: max, ..Default::default() }
}

/// After the link to a GRBL controller drops, its settings stay in the snapshot; "Read speeds
/// and acceleration from the controller" then writes the lost controller's numbers into
/// whichever device is selected next.
#[test]
fn motion_is_not_read_from_a_lost_link() {
    use crate::machine_api::{ConnectConfig, Link};
    let mut a = DarkBurn::new(Prefs::default());
    let grbl = a.machine.devices.devices.iter().find(|d| d.kind == DeviceType::SimulatorGrbl).unwrap().id;
    run(&mut a, "device.select", json!({ "id": grbl })).unwrap();
    let (adapter, emu) = crate::connections::grbl_simulator(&ConnectConfig::default());
    a.machine.handle.attach_simulator(Box::new(adapter), Link::SimulatorGrbl);
    wait_ready(&mut a);
    run(&mut a, "machine.read_settings", Value::Null).unwrap();
    let end = Instant::now() + T;
    while a.machine.handle.snapshot().grbl_settings.is_empty() {
        a.machine.tick();
        assert!(Instant::now() < end, "settings never read");
        std::thread::sleep(Duration::from_millis(5));
    }
    emu.with(|e| e.unplug());
    let end = Instant::now() + T;
    while a.machine.handle.snapshot().connected() {
        a.machine.tick();
        assert!(Instant::now() < end, "the drop was never noticed");
        std::thread::sleep(Duration::from_millis(5));
    }
    for _ in 0..5 {
        a.machine.tick();
    }
    let snap = a.machine.handle.snapshot().clone();
    assert!(crate::motion_read::from_snapshot(&snap, Default::default()).is_none(), "settings of a link that is gone");
    let other = run(&mut a, "device.duplicate", json!({ "id": grbl })).unwrap()["id"].as_u64().unwrap();
    run(&mut a, "device.select", json!({ "id": other })).unwrap();
    let before = a.machine.active().settings.additional;
    assert!(run(&mut a, "device.read_motion", Value::Null).is_err(), "read the lost controller's speeds into another device");
    let after = a.machine.active().settings.additional;
    assert_eq!((before.max_speed, before.accel, before.rapid_speed), (after.max_speed, after.accel, after.rapid_speed));
}

/// Galvo settings that are not numbers pass the checks, and the EZCad import is not checked.
#[test]
fn galvo_settings_refuse_values_that_are_not_numbers() {
    use crate::galvo_settings::GalvoDeviceSettings;
    let good = GalvoDeviceSettings::default();
    assert!(good.validate().is_empty(), "{:?}", good.validate());
    let bad: Vec<(&str, Box<dyn Fn(&mut GalvoDeviceSettings)>)> = vec![
        ("jump speed", Box::new(|s| s.timings.jump_speed_mm_s = f64::NAN)),
        ("jump distance limit", Box::new(|s| s.timings.jump_distance_limit_mm = f64::NAN)),
        ("frequency", Box::new(|s| s.default_frequency_khz = f64::NAN)),
        ("frequency inf", Box::new(|s| s.default_frequency_khz = f64::INFINITY)),
        ("light speed", Box::new(|s| s.light_speed_mm_s = f64::NAN)),
        ("light speed zero", Box::new(|s| s.light_speed_mm_s = 0.0)),
        ("offset x", Box::new(|s| s.offset_x_mm = f64::NAN)),
        ("offset y", Box::new(|s| s.offset_y_mm = f64::INFINITY)),
        ("skew", Box::new(|s| s.cal1.skew = f64::NAN)),
        ("bulge", Box::new(|s| s.cal2.bulge = f64::NAN)),
        ("trapezoid", Box::new(|s| s.cal1.trapezoid = f64::NAN)),
        ("q pulse", Box::new(|s| s.default_q_pulse_ns = f64::NAN)),
    ];
    for (what, f) in bad {
        let mut s = GalvoDeviceSettings::default();
        f(&mut s);
        assert!(!s.validate().is_empty(), "{what}: accepted");
    }
}

#[test]
fn the_ezcad_import_is_checked() {
    let mut a = DarkBurn::new(Prefs::default());
    let id = run(&mut a, "device.add", json!({ "type": "SimulatorGalvo" })).unwrap()["id"].as_u64().unwrap();
    run(&mut a, "device.select", json!({ "id": id })).unwrap();
    let before = a.machine.active().settings.galvo.clone();
    let dir = std::env::temp_dir().join(format!("darkburn_hunt10b_ezcad_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let cfg = dir.join("markcfg.ini");
    std::fs::write(&cfg, "[Lens]\nField Size = 2\nJumpSpeed = 3500\n").unwrap();
    let r = run(&mut a, "galvo.import_ezcad", json!({ "path": cfg.display().to_string() }));
    std::fs::remove_dir_all(&dir).ok();
    assert!(r.is_err(), "a 2 mm field was imported: {r:?}");
    assert_eq!(a.machine.active().settings.galvo, before, "a refused import changed the settings");
}

/// A NaN speed, or rows not in order (the Scanning Offset window open) or at the same speed,
/// gave a NaN offset, and the image job shifted every scan line by NaN.
#[test]
fn scan_offsets_never_give_nan() {
    use crate::scan_offsets::{OffsetEntry, ScanOffsetTable};
    let mut t = ScanOffsetTable::default();
    t.set(50.0, 0.1);
    t.set(100.0, 0.2);
    assert_eq!(t.offset_at(f64::NAN), 0.0, "a NaN speed");
    // Rows as the window holds them while being edited: unsorted, duplicated, not numbers.
    let raw = ScanOffsetTable {
        entries: vec![
            OffsetEntry { speed: 100.0, offset: 0.2 },
            OffsetEntry { speed: 50.0, offset: 0.1 },
            OffsetEntry { speed: 50.0, offset: 0.1 },
            OffsetEntry { speed: 75.0, offset: f64::NAN },
            OffsetEntry { speed: f64::NAN, offset: 0.4 },
        ],
    };
    for speed in [10.0, 50.0, 60.0, 75.0, 99.0, 100.0, 500.0, f64::NAN, f64::INFINITY] {
        assert!(raw.offset_at(speed).is_finite(), "speed {speed}: {}", raw.offset_at(speed));
    }
    let dup = ScanOffsetTable {
        entries: vec![OffsetEntry { speed: 50.0, offset: 0.1 }, OffsetEntry { speed: 50.0, offset: 0.3 }, OffsetEntry { speed: 50.0, offset: 0.3 }],
    };
    for speed in [10.0, 50.0, 60.0] {
        assert!(dup.offset_at(speed).is_finite(), "speed {speed}: {}", dup.offset_at(speed));
    }
    let mid = raw.offset_at(75.0);
    assert!((mid - 0.15).abs() < 1e-9, "75 mm/s is between 0.1 and 0.2: {mid}");
}

/// A machine command given an argument of the wrong type is refused, not run with the default in
/// its place: `machine.fire {"percent":"x"}` fired at the default power, `machine.move_to
/// {"y":"20"}` moved in X only, `machine.connect {"device":"3"}` connected the active device.
#[test]
fn machine_commands_refuse_arguments_of_the_wrong_type() {
    use crate::machine_api::{ConnectConfig, Link};
    let mut a = DarkBurn::new(Prefs::default());
    let grbl = a.machine.devices.devices.iter().find(|d| d.kind == DeviceType::SimulatorGrbl).unwrap().id;
    run(&mut a, "device.select", json!({ "id": grbl })).unwrap();
    a.machine.devices.devices.iter_mut().find(|d| d.id == grbl).unwrap().settings.gcode.fire_button = true;
    let (adapter, _emu) = crate::connections::grbl_simulator(&ConnectConfig::default());
    a.machine.handle.attach_simulator(Box::new(adapter), Link::SimulatorGrbl);
    wait_ready(&mut a);
    for (id, args) in [
        ("machine.fire", json!({ "percent": "x" })),
        ("machine.fire", json!({ "secs": [1] })),
        ("machine.move_to", json!({ "x": 10, "y": "20" })),
        ("machine.jog", json!({ "dx": "x" })),
        ("machine.jog", json!({ "dx": 1, "feed": "fast" })),
        ("machine.jog_start", json!({ "x": "right" })),
        ("machine.frame", json!({ "x": 0, "y": 0, "width": 10, "height": 10, "laser_percent": "50" })),
    ] {
        let r = run(&mut a, id, args.clone());
        assert!(r.as_ref().is_err_and(|e| e.contains("must be")), "{id} {args} was taken: {r:?}");
    }
    // The same commands with arguments of the right type still run.
    run(&mut a, "machine.jog", json!({ "dx": 1 })).unwrap();
    run(&mut a, "machine.disconnect", Value::Null).unwrap();
    let r = run(&mut a, "machine.connect", json!({ "device": "3" }));
    assert!(r.as_ref().is_err_and(|e| e.contains("must be")), "machine.connect with a device id that is text was taken: {r:?}");
}

/// The air assist button sends the device's own air command: a GRBL device set to "M7 on, M9 off"
/// got M8 from the button while its jobs used M7, so the button switched another output.
#[test]
fn the_air_button_sends_the_devices_own_air_command() {
    use crate::machine_api::{ConnectConfig, Link};
    let mut a = DarkBurn::new(Prefs::default());
    let grbl = a.machine.devices.devices.iter().find(|d| d.kind == DeviceType::SimulatorGrbl).unwrap().id;
    run(&mut a, "device.select", json!({ "id": grbl })).unwrap();
    a.machine.devices.devices.iter_mut().find(|d| d.id == grbl).unwrap().settings.gcode.air = darkburn_job::gcode::AirCommand::M7;
    let (adapter, emu) = crate::connections::grbl_simulator(&ConnectConfig::default());
    a.machine.handle.attach_simulator(Box::new(adapter), Link::SimulatorGrbl);
    wait_ready(&mut a);
    let before = emu.with(|e| e.stats().lines.len());
    run(&mut a, "machine.air", json!({ "on": true })).unwrap();
    run(&mut a, "machine.air", json!({ "on": false })).unwrap();
    let end = Instant::now() + T;
    let sent = loop {
        a.machine.tick();
        let l = emu.with(|e| e.stats().lines[before..].to_vec());
        if l.iter().any(|l| l.trim() == "M9") {
            break l;
        }
        assert!(Instant::now() < end, "the air commands never arrived: {l:?}");
        std::thread::sleep(Duration::from_millis(5));
    };
    assert!(sent.iter().any(|l| l.trim() == "M7"), "air on is the device's M7: {sent:?}");
    assert!(!sent.iter().any(|l| l.trim() == "M8"), "M8 is another output on this device: {sent:?}");
}

/// A GRBL controller counts as ready as soon as it has said hello; where its head is, and so its
/// work offset, comes with the first status report. A job or frame started in between was placed
/// with an offset of zero, so a job with an origin set ran somewhere else on the bed.
#[test]
fn a_grbl_job_waits_for_the_first_status_report() {
    use crate::machine_api::{ConnectConfig, Link};
    let mut a = DarkBurn::new(Prefs::default());
    let grbl = a.machine.devices.devices.iter().find(|d| d.kind == DeviceType::SimulatorGrbl).unwrap().id;
    run(&mut a, "device.select", json!({ "id": grbl })).unwrap();
    let (adapter, _emu) = crate::connections::grbl_simulator(&ConnectConfig::default());
    a.machine.handle.attach_simulator(Box::new(adapter), Link::SimulatorGrbl);
    wait_ready(&mut a);
    run(&mut a, "shape.add_rect", json!({ "x": 10, "y": 10, "width": 20, "height": 20, "layer": 0 })).unwrap();
    let job = crate::job_cmds::build_now(&a).unwrap();
    let end = Instant::now() + T;
    while !a.machine.handle.snapshot().has_position {
        a.machine.tick();
        assert!(Instant::now() < end, "no status report ever came");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(crate::machine_job::request_for(&a, &job, None, false).is_ok(), "a job once the position is known");
    // Between hello and the first status report.
    {
        let s = a.machine.handle.snapshot_mut();
        s.has_position = false;
        s.machine_pos = [0.0; 4];
        s.work_pos = [0.0; 4];
    }
    let r = crate::machine_job::request_for(&a, &job, None, false);
    assert!(r.is_err(), "a GRBL job was placed before the controller said where the head is");
    let r = crate::machine_job::frame(&mut a, crate::job_build::FrameKind::Rectangle);
    assert!(r.is_err(), "a GRBL frame was placed before the controller said where the head is");
}

/// Running a stored Ruida file before the file list was ever read announced a job for a number
/// nobody had seen on this controller (the number may be from another one). It waits for the list.
#[test]
fn a_controller_file_is_not_run_before_the_list_was_read() {
    let mut a = DarkBurn::new(Prefs::default());
    let ruida = a.machine.devices.devices.iter().find(|d| d.kind == DeviceType::SimulatorRuida).unwrap().id;
    run(&mut a, "device.select", json!({ "id": ruida })).unwrap();
    run(&mut a, "machine.connect", json!({})).unwrap();
    wait_ready(&mut a);
    assert!(a.machine.handle.snapshot().ruida.files.is_none(), "the list is not read yet");
    let r = run(&mut a, "machine.file_run", json!({ "index": 1 }));
    assert!(r.is_err(), "a file was run before the controller's list was read: {r:?}");
    assert!(!a.machine.job_log.iter().any(|l| format!("{l:?}").contains("Job started")), "a job was announced");
}
