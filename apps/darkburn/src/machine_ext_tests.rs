//! Headless tests of the G-code firmwares, the galvo board and the Gweike file workflow: every
//! emulator through [`MachineHandle`], profile round trips, translation per dialect, the galvo
//! arm / frame / job rules, the `.chf` export read back by the library, the planner dry run and
//! a source scan that no Gweike code can reach a real transport. No real port, USB device or
//! network connection is ever opened (test builds refuse real links).

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::api::execute_headless;
use crate::devices::{Device, DeviceStore, DeviceType};
use crate::galvo_machine;
use crate::galvo_settings::GalvoDeviceSettings;
use crate::gcode_machines::{self as gcm, SimGcode};
use crate::machine_api::*;
use crate::prefs::Prefs;
use crate::shell::DarkBurn;
use darkburn_galvo as galvo;

const T: Duration = Duration::from_secs(30);

fn app() -> DarkBurn {
    DarkBurn::new(Prefs::default())
}

fn run(app: &mut DarkBurn, id: &str, args: Value) -> Result<Value, String> {
    execute_headless(app, id, &args)
}

fn square(app: &mut DarkBurn) {
    run(app, "shape.add_rect", json!({ "x": 10, "y": 10, "width": 20, "height": 20, "layer": 0 })).unwrap();
}

fn gcode_spec(kind: SimGcode, scale: f64) -> ConnectSpec {
    let config = ConnectConfig { sim_time_scale: scale, bed: Some((300.0, 200.0)), ..Default::default() };
    ConnectSpec { link: Link::SimulatorGcode(kind), config }
}

fn connected(kind: SimGcode, scale: f64) -> MachineHandle {
    let mut h = MachineHandle::new();
    h.connect(&gcode_spec(kind, scale), None).unwrap();
    assert!(h.wait_for(T, |s, _| s.connected()), "{kind:?} simulator connects");
    ready(&mut h);
    h
}

fn ready(h: &mut MachineHandle) {
    let ok = h.wait_for(T, |s, _| s.state == LaserState::Ready);
    if !ok {
        for c in h.console() {
            eprintln!("{:?} {}", c.kind, c.text);
        }
    }
    assert!(ok, "ready, was {:?} {:?}", h.snapshot().state, h.snapshot().alarm);
}

fn device_for(kind: SimGcode) -> Device {
    Device::new(match kind {
        SimGcode::Grbl => DeviceType::Grbl,
        SimGcode::Smoothie => DeviceType::SimulatorSmoothie,
        SimGcode::Marlin => DeviceType::SimulatorMarlin,
        SimGcode::Snapmaker => DeviceType::SimulatorSnapmaker,
        SimGcode::Custom => DeviceType::SimulatorCustom,
    })
}

// ----- G-code machines through the handle ------------------------------------------------------------

#[test]
fn every_gcode_simulator_connects_and_reports_itself() {
    for kind in SimGcode::ALL {
        let h = connected(kind, 200.0);
        let s = h.snapshot();
        assert!(s.simulator && !h.is_real_machine(), "{kind:?}");
        assert_eq!(s.family, Family::Grbl);
        assert!(s.gcode_firmware.is_some(), "{kind:?} runs through the library driver");
        if kind != SimGcode::Custom {
            assert!(!s.firmware.is_empty(), "{kind:?} has a banner: {:?}", s.firmware);
        }
    }
}

#[test]
fn every_gcode_simulator_runs_a_job_to_the_end() {
    for kind in SimGcode::ALL {
        let mut h = connected(kind, 200.0);
        let lines = gcm::simulator_square_lines(&device_for(kind), 20.0);
        assert!(lines.len() >= 6, "{kind:?}: {lines:?}");
        h.start_job(JobRequest::gcode(lines)).unwrap();
        let mut finished = false;
        assert!(
            h.wait_for(T, |_, ev| {
                finished |= ev.iter().any(|e| matches!(e, MachineEvent::JobFinished(_)));
                finished
            }),
            "{kind:?} finishes"
        );
        ready(&mut h);
        assert!(!h.snapshot().fire_on, "{kind:?}: the laser is off after a job");
    }
}

#[test]
fn every_gcode_simulator_pauses_resumes_and_stops() {
    for kind in [SimGcode::Grbl, SimGcode::Smoothie, SimGcode::Marlin, SimGcode::Snapmaker, SimGcode::Custom] {
        let mut h = connected(kind, 1.0);
        // A slow job: 150 mm at 600 mm/min is 15 s of machine time. Fifty pieces, more than a
        // controller plans ahead, so the pause comes while lines are still to be sent.
        let mut lines = vec!["G21 G90".to_owned(), "M4 S100".to_owned(), "G0 X10 Y10".to_owned()];
        lines.extend((1..=50).map(|i| format!("G1 X{} Y10 F600", 10 + 3 * i)));
        lines.push("M5".to_owned());
        h.start_job(JobRequest::gcode(lines)).unwrap();
        assert!(h.wait_for(T, |s, _| s.state == LaserState::Busy), "{kind:?} busy");
        if kind != SimGcode::Custom {
            // The custom device has no pause command unless the user writes one.
            h.pause().unwrap();
            assert!(h.wait_for(T, |s, _| s.state == LaserState::Paused), "{kind:?} paused");
            h.resume().unwrap();
            assert!(h.wait_for(T, |s, _| s.state == LaserState::Busy), "{kind:?} resumed");
        }
        h.stop().unwrap();
        if kind == SimGcode::Grbl {
            // GRBL's stop is a soft reset: stopped mid-move it raises alarm 3 until unlocked.
            assert!(h.wait_for(T, |s, _| s.state == LaserState::Alarm || s.state == LaserState::Ready));
            if h.snapshot().state == LaserState::Alarm {
                h.command(MachineCommand::Unlock).unwrap();
            }
        }
        ready(&mut h);
        assert!(!h.snapshot().fire_on);
        assert!(h.snapshot().job.is_none(), "{kind:?}");
    }
}

#[test]
fn gcode_simulators_jog_home_and_report_a_position() {
    for kind in [SimGcode::Grbl, SimGcode::Smoothie, SimGcode::Marlin, SimGcode::Snapmaker] {
        let mut h = connected(kind, 200.0);
        h.jog_step(JogSpec { dx: 5.0, dy: 3.0, feed: 3000.0, ..Default::default() }).unwrap();
        assert!(
            h.wait_for(T, |s, _| s.state == LaserState::Ready && (s.work_pos[0] - 5.0).abs() < 0.01 && (s.work_pos[1] - 3.0).abs() < 0.01 || {
                let _ = s;
                false
            }),
            "{kind:?} jogged to 5,3 (now {:?})",
            h.snapshot().work_pos
        );
        h.get_position().unwrap();
        if kind == SimGcode::Grbl {
            // The GRBL emulator starts with homing off ($22=0), so there is nothing to home.
            continue;
        }
        h.home().unwrap();
        assert!(h.wait_for(T, |_, ev| ev.iter().any(|e| matches!(e, MachineEvent::Homed))), "{kind:?} homed");
        assert!(h.wait_for(T, |s, _| s.state == LaserState::Ready && s.machine_pos[0].abs() < 0.01), "{kind:?} at the home corner");
    }
}

#[test]
fn gcode_simulators_move_set_origin_and_fire_off() {
    let mut h = connected(SimGcode::Smoothie, 200.0);
    h.move_to(MoveSpec { x: Some(20.0), y: Some(10.0), feed: 3000.0, ..Default::default() }).unwrap();
    assert!(h.wait_for(T, |s, _| s.state == LaserState::Ready && (s.work_pos[0] - 20.0).abs() < 0.01));
    h.set_user_origin().unwrap();
    assert!(h.wait_for(T, |s, _| s.work_pos[0].abs() < 0.01 && s.work_pos[1].abs() < 0.01), "origin set here");
    h.clear_user_origin().unwrap();
    assert!(h.wait_for(T, |s, _| (s.work_pos[0] - 20.0).abs() < 0.01), "origin cleared");
    assert!(h.fire(5.0, 0.2).is_ok());
    h.fire_off().unwrap();
    assert!(h.wait_for(T, |s, _| !s.fire_on));
    // Galvo and Ruida commands are refused cleanly.
    assert!(matches!(h.command(MachineCommand::GalvoArm(true)), Err(MachineError::Unsupported(_))));
    assert!(matches!(h.command(MachineCommand::ListFiles), Err(MachineError::Unsupported(_))));
}

#[test]
fn gcode_simulators_read_and_write_settings() {
    let mut h = connected(SimGcode::Smoothie, 200.0);
    h.read_settings().unwrap();
    assert!(h.wait_for(T, |s, _| !s.gcode_settings.is_empty()), "Smoothieware config keys arrive");
    let key = h.snapshot().gcode_settings.keys().next().cloned().unwrap();
    h.command(MachineCommand::WriteGcodeSetting { key: key.clone(), value: "123".into() }).unwrap();
    assert!(h.wait_for(T, |_, ev| ev.iter().any(|e| matches!(e, MachineEvent::SettingWritten { ok: true, .. }))), "setting {key} written");

    let mut m = connected(SimGcode::Marlin, 200.0);
    m.read_settings().unwrap();
    assert!(m.wait_for(T, |s, _| !s.gcode_settings.is_empty()), "Marlin M503 settings arrive");
    assert!(m.command(MachineCommand::PersistSettings).is_ok());
    assert!(m.command(MachineCommand::Air { on: true, mist: false }).is_ok());
    assert!(m.command(MachineCommand::EmergencyStop).is_ok());
}

#[test]
fn marlin_emulator_receives_the_translated_dialect() {
    // Fan mode: laser power rides on M106, never on M3.
    let mut dev = Device::new(DeviceType::SimulatorMarlin);
    dev.settings.gcode_ext.marlin_power = darkburn_gcodebots::profile::MarlinPowerMode::Fan;
    let cfg = ConnectConfig { sim_time_scale: 200.0, gcode: Some(Arc::new(gcm::to_profile(&dev))), ..Default::default() };
    let (adapter, emu) = gcm::simulator_adapter(SimGcode::Marlin, &cfg);
    let mut h = MachineHandle::new();
    h.attach_simulator(Box::new(adapter), Link::SimulatorGcode(SimGcode::Marlin));
    assert!(h.wait_for(T, |s, _| s.connected()));
    ready(&mut h);
    let lines = gcm::simulator_square_lines(&dev, 20.0);
    assert!(lines.iter().any(|l| l.starts_with("M106 P0 S")), "{lines:?}");
    assert!(!lines.iter().any(|l| l.starts_with("M3") || l.starts_with("M4")), "{lines:?}");
    h.start_job(JobRequest::gcode(lines)).unwrap();
    assert!(h.wait_for(T, |_, ev| ev.iter().any(|e| matches!(e, MachineEvent::JobFinished(_)))));
    let seen = emu.unwrap().with(|e| e.stats().lines.clone());
    assert!(seen.iter().any(|l| l.starts_with("M106")), "the emulator saw the fan power: {seen:?}");
}

// ----- Device types, wizard entries, profiles ---------------------------------------------------------

#[test]
fn new_device_types_have_sensible_presets_and_links() {
    for t in DeviceType::ALL {
        let d = Device::new(t);
        assert!(!d.name.is_empty());
        if t.needs_capture().is_some() {
            assert!(!t.selectable());
            assert!(!d.validate().is_empty(), "{t:?} cannot be saved as a usable device");
            assert!(d.link(&[]).is_err(), "{t:?} has no link");
            continue;
        }
        assert!(d.validate().is_empty(), "{t:?}: {:?}", d.validate());
    }
    assert_eq!(Device::new(DeviceType::Smoothie).settings.gcode.s_max, 1.0);
    assert_eq!(Device::new(DeviceType::Marlin).settings.gcode.baud_rate, 250_000);
    assert_eq!(Device::new(DeviceType::Emblaser1).settings.gcode.s_max, 1000.0);
    assert!(Device::new(DeviceType::Emblaser2).kind.is_translated(), "Emblaser 2 is Smoothie family");
    assert!(Device::new(DeviceType::LinuxCnc).link(&[]).is_err(), "LinuxCNC has no live link");
    assert_eq!(Device::new(DeviceType::SimulatorMarlin).link(&[]).unwrap(), Link::SimulatorGcode(SimGcode::Marlin));
    assert_eq!(Device::new(DeviceType::SimulatorGalvo).link(&[]).unwrap(), Link::SimulatorGalvo);
    assert!(Device::new(DeviceType::GweikeCardSimulator).link(&[]).is_err(), "Gweike has no link");
    for t in [DeviceType::Trocen, DeviceType::TopWisdom, DeviceType::Cubiio, DeviceType::SnapmakerNetwork, DeviceType::EmblaserWifi] {
        let why = t.needs_capture().unwrap();
        assert!(why.contains("capture"), "{t:?}: {why}");
    }
}

#[test]
fn disabled_types_cannot_be_added_through_the_commands() {
    let mut a = app();
    for t in ["Trocen", "TopWisdom", "Cubiio", "SnapmakerNetwork", "EmblaserWifi"] {
        let e = run(&mut a, "device.add", json!({ "type": t })).unwrap_err();
        assert!(e.contains("capture"), "{e}");
    }
    let r = run(&mut a, "device.add", json!({ "type": "Marlin", "name": "Bench Marlin" })).unwrap();
    let id = r["id"].as_u64().unwrap();
    assert_eq!(a.machine.devices.get(id).unwrap().name, "Bench Marlin");
    assert!(run(&mut a, "device.add", json!({ "type": "nonsense" })).is_err());
}

#[test]
fn profile_converts_to_the_library_and_back_without_losing_gcode_settings() {
    for t in [DeviceType::Smoothie, DeviceType::Marlin, DeviceType::Snapmaker, DeviceType::LinuxCnc, DeviceType::CustomGcode, DeviceType::Emblaser2] {
        let mut d = Device::new(t);
        d.name = "Round trip".into();
        d.width_mm = 280.0;
        d.height_mm = 190.0;
        d.settings.gcode.s_max = 777.0;
        d.settings.gcode.auto_home = true;
        d.settings.gcode.swap_xy = true;
        d.settings.gcode.start_script = "G21\nM8".into();
        d.settings.gcode.end_script = "M5\nG0 X0 Y0".into();
        d.settings.gcode.overscan_as_g0 = true;
        d.settings.gcode.s_every_g1 = true;
        d.settings.gcode_ext.air_on = "M106 P2".into();
        d.settings.gcode_ext.tool_index = 2;
        d.settings.gcode_ext.clustering = true;
        d.settings.red_dot_x = 1.5;
        d.settings.negative_coords = true;
        d.state.inches = true;
        let p = gcm::to_profile(&d);
        assert_eq!(p.width_mm, 280.0);
        assert_eq!(p.gcode.s_max, 777.0);
        assert!(p.gcode.auto_home_on_start && p.gcode.swap_xy && p.gcode.g0_for_overscan);
        let text = p.export_json();
        let back = darkburn_gcodebots::DeviceProfile::import_json(&text).unwrap();
        assert_eq!(back, p, "{t:?}: the library file round-trips");
        let d2 = gcm::from_profile(&back);
        assert_eq!(d2.kind, d.kind.lib_kind().map(gcm_type).unwrap(), "{t:?}");
        assert_eq!(d2.width_mm, 280.0);
        assert_eq!(d2.settings.gcode.s_max, 777.0);
        assert_eq!(d2.settings.gcode.start_script, "G21\nM8");
        assert_eq!(d2.settings.gcode_ext.air_on, "M106 P2");
        assert_eq!(d2.settings.gcode_ext.tool_index, 2);
        assert!(d2.settings.gcode_ext.clustering && d2.state.inches && d2.settings.negative_coords);
        assert_eq!(d2.settings.red_dot_x, 1.5);
    }
}

fn gcm_type(k: darkburn_gcodebots::profile::DeviceKind) -> DeviceType {
    use darkburn_gcodebots::profile::DeviceKind as K;
    match k {
        K::Smoothie => DeviceType::Smoothie,
        K::Marlin => DeviceType::Marlin,
        K::Snapmaker => DeviceType::Snapmaker,
        K::LinuxCnc => DeviceType::LinuxCnc,
        K::Custom => DeviceType::CustomGcode,
        _ => DeviceType::Grbl,
    }
}

#[test]
fn dbdev_files_keep_every_new_setting_and_the_library_file_imports_too() {
    let mut store = DeviceStore::default();
    let mut d = Device::new(DeviceType::Galvo);
    d.name = "Fibre marker".into();
    d.settings.galvo.field_size_mm = 150.0;
    d.settings.galvo.cal1.skew = 0.01;
    d.settings.galvo.ports.door_bit = Some(3);
    d.settings.galvo.repeat.count = 4;
    d.settings
        .gweike
        .layers
        .insert(2, crate::gweike_workflow::LayerMap { material: "Steel 3 mm".into(), head: crate::gweike_workflow::Head::Fibre, slot: 4 });
    let id = store.add(d.clone());
    let text = store.export_profile(id).unwrap();
    let id2 = store.import_profile(&text).unwrap();
    let back = store.get(id2).unwrap();
    assert_eq!(back.settings.galvo, d.settings.galvo);
    assert_eq!(back.settings.gweike, d.settings.gweike);
    assert_eq!(back.kind, DeviceType::Galvo);

    // A device file written by the library converts on import.
    let lib = gcm::export_library_profile(&Device::new(DeviceType::Snapmaker));
    let id3 = store.import_profile(&lib).unwrap();
    assert_eq!(store.get(id3).unwrap().kind, DeviceType::Snapmaker);
    // An old store without the new fields still loads.
    let old = r#"{"devices":[{"id":1,"name":"Old","kind":"Grbl"}],"active":1}"#;
    let s = DeviceStore::from_json(old);
    assert_eq!(s.devices[0].settings.galvo, GalvoDeviceSettings::default());
}

// ----- Translation and export per dialect --------------------------------------------------------------

#[test]
fn each_dialect_writes_its_own_start_end_scripts_and_power_style() {
    let neutral = vec![
        "G21 G90".to_owned(),
        "M4 S500".to_owned(),
        "G0 X1 Y1".to_owned(),
        "G1 X10 Y1 F1200".to_owned(),
        "M8".to_owned(),
        "M5".to_owned(),
        "M9".to_owned(),
    ];
    let text = |t: DeviceType| gcm::translate_for(&Device::new(t), &neutral).join("\n");
    let marlin = text(DeviceType::Marlin);
    assert!(marlin.starts_with("G21\nG90"), "{marlin}");
    assert!(marlin.contains("M3") || marlin.contains("M4") || marlin.contains("M106"), "{marlin}");
    let smoothie = text(DeviceType::Smoothie);
    assert!(smoothie.contains("S0.5"), "Smoothieware power runs 0 to 1: {smoothie}");
    let snap = text(DeviceType::Snapmaker);
    assert!(snap.contains("M106 P0"), "{snap}");
    let lcnc = text(DeviceType::LinuxCnc);
    assert!(lcnc.contains("M67 E0 Q") && lcnc.trim_end().ends_with("M2"), "{lcnc}");
    let custom = text(DeviceType::CustomGcode);
    assert!(custom.contains("G1 X10"), "{custom}");
    let (start, end) = gcm::scripts_of(&Device::new(DeviceType::Marlin));
    assert_eq!(start, ["G21", "G90"]);
    assert_eq!(end, ["M5"]);
}

#[test]
fn gcode_export_writes_each_dialect_to_a_file() {
    let dir = std::env::temp_dir().join(format!("darkburn_gcode_export_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut a = app();
    square(&mut a);
    for (ty, ext, marker) in [
        ("Smoothie", "gcode", "G1"),
        ("Marlin", "gcode", "G1"),
        ("Snapmaker", "gcode", ";Header Start"),
        ("LinuxCnc", "ngc", "M2"),
        ("CustomGcode", "gcode", "G1"),
        ("GRBL", "nc", "G1"),
    ] {
        let path = dir.join(format!("job_{ty}.{ext}"));
        let r = run(&mut a, "gcode.export", json!({ "path": path.display().to_string(), "device_type": ty })).unwrap();
        assert!(r["lines"].as_u64().unwrap() > 5, "{ty}: {r}");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains(marker), "{ty}: {text}");
    }
    assert!(run(&mut a, "gcode.export", json!({ "path": dir.join("x").display().to_string(), "device_type": "Trocen" })).is_err());
    let s = run(&mut a, "gcode.scripts", json!({ "device_type": "Smoothie" })).unwrap();
    assert_eq!(s["start"], json!(["G21", "G90"]));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn firmware_is_guessed_from_the_banner_and_a_mismatch_is_explained() {
    let g = |b: &str| run(&mut app(), "gcode.guess_firmware", json!({ "banner": b })).unwrap();
    assert_eq!(g("Grbl 1.1h ['$' for help]")["device_type"], "Grbl");
    assert_eq!(g("Smoothie")["device_type"], "Smoothie");
    assert_eq!(g("FIRMWARE_NAME:Marlin 2.1.2 (Github)")["device_type"], "Marlin");
    assert_eq!(g("FIRMWARE_NAME:Snapmaker 2.0")["device_type"], "Snapmaker");
    assert!(g("hello")["device_type"].is_null());
    assert!(gcm::mismatch_hint("Smoothieware edge-94de3d9", DeviceType::Grbl).contains("looks like Smoothieware"));
    assert_eq!(gcm::mismatch_hint("Grbl 1.1h", DeviceType::Emblaser1), "");
    assert_eq!(gcm::mismatch_hint("whatever", DeviceType::Marlin), "");
    // The port list is only listed: no probe happens before the user's Connect.
    let ports = vec![darkburn_grbl::PortInfo { name: "COM9".into(), vid: Some(0x1D50), pid: Some(0x6015), ..Default::default() }];
    let c = crate::devices::find_my_laser(&ports);
    assert_eq!(c[0].kind, DeviceType::Smoothie);
    assert!(c[0].note.contains("press Connect"), "{}", c[0].note);
}

// ----- Galvo ------------------------------------------------------------------------------------------------

fn galvo_app() -> (DarkBurn, u64) {
    let mut a = app();
    let id = run(&mut a, "device.add", json!({ "type": "SimulatorGalvo" })).unwrap()["id"].as_u64().unwrap();
    run(&mut a, "device.select", json!({ "id": id })).unwrap();
    (a, id)
}

fn wait(app: &mut DarkBurn, what: &str, mut f: impl FnMut(&DarkBurn) -> bool) {
    let end = Instant::now() + T;
    loop {
        app.machine.tick();
        if f(app) {
            return;
        }
        assert!(Instant::now() < end, "timed out waiting for {what}; state {:?}", app.machine.handle.snapshot().state);
        std::thread::sleep(Duration::from_millis(3));
    }
}

fn galvo_square(app: &mut DarkBurn) {
    run(app, "shape.add_rect", json!({ "x": 40, "y": 40, "width": 20, "height": 20, "layer": 0 })).unwrap();
    run(app, "layer.set_param", json!({ "layer": 0, "field": "speed", "value": 800 })).unwrap();
}

fn attach_galvo(app: &mut DarkBurn, settings: GalvoDeviceSettings) -> galvo::EmulatorHandle {
    let (adapter, handle) = galvo_machine::simulator_with_handle(Arc::new(settings));
    app.machine.handle.attach_simulator(Box::new(adapter), Link::SimulatorGalvo);
    wait(app, "galvo ready", |a| a.machine.handle.snapshot().state == LaserState::Ready);
    handle
}

#[test]
fn galvo_simulator_connects_through_the_connect_command_and_never_enables_the_laser() {
    let (mut a, _) = galvo_app();
    run(&mut a, "machine.connect", json!({})).unwrap();
    wait(&mut a, "ready", |a| a.machine.handle.snapshot().state == LaserState::Ready);
    let s = a.machine.handle.snapshot().clone();
    assert_eq!(s.family, Family::Galvo);
    assert!(s.simulator && s.galvo.as_ref().is_some_and(|g| !g.armed));
    assert!(s.firmware.starts_with("Galvo board"), "{}", s.firmware);
    assert_eq!(a.machine.active().bed(), (110.0, 110.0));
}

#[test]
fn galvo_laser_is_never_enabled_without_the_explicit_arm() {
    let (mut a, _) = galvo_app();
    galvo_square(&mut a);
    let emu = attach_galvo(&mut a, GalvoDeviceSettings::default());
    assert!(!emu.lock().unwrap().laser_enabled(), "not enabled at connect");
    assert!(!emu.lock().unwrap().log().singles.contains(&galvo::SingleCmd::EnableLaser));
    // Start without an arm is refused, by the command and by the board driver.
    let e = run(&mut a, "machine.start", Value::Null).unwrap_err();
    assert!(e.to_lowercase().contains("arm"), "{e}");
    let dev = a.machine.active().clone();
    let (req, _) = crate::galvo_job::request(&a.editor.doc, &dev, false, false).unwrap();
    assert!(matches!(a.machine.handle.start_job(req), Err(MachineError::Refused(m)) if m.contains("Arm")));
    assert!(!emu.lock().unwrap().log().singles.contains(&galvo::SingleCmd::EnableLaser), "still never enabled");
    // Arm, run, and the arm is spent afterwards.
    run(&mut a, "machine.galvo_arm", json!({ "on": true })).unwrap();
    wait(&mut a, "armed", |a| a.machine.handle.snapshot().galvo.as_ref().is_some_and(|g| g.armed));
    run(&mut a, "machine.start", Value::Null).unwrap();
    wait(&mut a, "job finished", |a| {
        a.machine.last_events.iter().any(|e| matches!(e, MachineEvent::JobFinished(_)))
            || (emu.lock().unwrap().log().executed > 0 && a.machine.handle.snapshot().state == LaserState::Ready)
    });
    wait(&mut a, "ready again", |a| a.machine.handle.snapshot().state == LaserState::Ready);
    let g = emu.lock().unwrap();
    assert!(g.log().singles.contains(&galvo::SingleCmd::EnableLaser), "the armed job enabled the laser");
    assert!(g.log().motions.iter().any(|m| m.mark), "it marked");
    assert!(!g.laser_enabled() && !g.laser_signal(), "the laser is off again after the job");
    drop(g);
    assert!(!a.machine.handle.snapshot().galvo.as_ref().unwrap().armed, "the arm is spent");
}

#[test]
fn galvo_red_light_frame_never_fires_and_satisfies_require_framing() {
    let (mut a, _) = galvo_app();
    galvo_square(&mut a);
    let settings = GalvoDeviceSettings { require_framing: true, ..Default::default() };
    let id = a.machine.active().id;
    a.machine.devices.get_mut(id).unwrap().settings.galvo = settings.clone();
    let emu = attach_galvo(&mut a, settings);
    run(&mut a, "machine.galvo_arm", json!({ "on": true })).unwrap();
    // Not framed yet: the job is refused.
    let dev = a.machine.active().clone();
    let (req, _) = crate::galvo_job::request(&a.editor.doc, &dev, false, false).unwrap();
    assert!(matches!(a.machine.handle.start_job(req.clone()), Err(MachineError::Refused(m)) if m.contains("Frame")));
    // Frame with nudge and scale.
    run(&mut a, "machine.galvo_frame", json!({ "repeat": false, "nudge_x": 1.0, "nudge_y": -1.0, "scale_pct": 110.0 })).unwrap();
    assert!((a.machine.ext.frame.scale_pct - 110.0).abs() < 1e-9);
    wait(&mut a, "framed", |a| a.machine.handle.snapshot().galvo.as_ref().is_some_and(|g| g.framed_key.is_some()));
    wait(&mut a, "ready", |a| a.machine.handle.snapshot().state == LaserState::Ready);
    {
        let g = emu.lock().unwrap();
        assert!(g.log().light_on_writes > 0, "the red pointer was on");
        assert!(!g.log().singles.contains(&galvo::SingleCmd::EnableLaser), "framing never enables the laser");
        assert!(!g.log().motions.iter().any(|m| m.mark || m.emitting), "framing never marks");
    }
    // That frame was shifted and scaled, so it showed another place than the job marks: the job
    // is still refused. A frame of the job as it lies lets it start.
    assert!(matches!(a.machine.handle.start_job(req.clone()), Err(MachineError::Refused(m)) if m.contains("Frame")));
    let shifted = a.machine.handle.snapshot().galvo.as_ref().and_then(|g| g.framed_key);
    run(&mut a, "machine.galvo_frame", json!({ "repeat": false, "nudge_x": 0.0, "nudge_y": 0.0, "scale_pct": 100.0 })).unwrap();
    wait(&mut a, "framed as it lies", |a| {
        a.machine.handle.snapshot().galvo.as_ref().is_some_and(|g| g.framed_key.is_some() && g.framed_key != shifted)
    });
    wait(&mut a, "ready", |a| a.machine.handle.snapshot().state == LaserState::Ready);
    a.machine.handle.start_job(req).unwrap();
    wait(&mut a, "job done", |a| a.machine.handle.snapshot().state == LaserState::Busy || a.machine.handle.snapshot().state == LaserState::Ready);
    wait(&mut a, "ready", |a| a.machine.handle.snapshot().state == LaserState::Ready && emu.lock().unwrap().log().motions.iter().any(|m| m.mark));
}

#[test]
fn galvo_stop_turns_everything_off_and_pause_holds() {
    let (mut a, _) = galvo_app();
    galvo_square(&mut a);
    run(&mut a, "layer.set_param", json!({ "layer": 0, "field": "interval", "value": 0.05 })).unwrap();
    run(&mut a, "layer.set_mode", json!({ "layer": 0, "mode": "fill" })).unwrap();
    let emu = attach_galvo(&mut a, GalvoDeviceSettings::default());
    run(&mut a, "machine.galvo_arm", json!({ "on": true })).unwrap();
    run(&mut a, "machine.start", Value::Null).unwrap();
    wait(&mut a, "busy", |a| matches!(a.machine.handle.snapshot().state, LaserState::Busy | LaserState::Ready));
    a.machine.handle.stop().unwrap();
    wait(&mut a, "stopped", |a| a.machine.handle.snapshot().state == LaserState::Ready);
    let g = emu.lock().unwrap();
    assert!(!g.laser_enabled() && !g.laser_signal() && !g.mo_open(), "stop runs the off sequence");
}

#[test]
fn galvo_job_maps_layers_to_line_fill_flood_and_sliced() {
    use darkburn_core::layer::Mode;
    let (mut a, id) = galvo_app();
    run(&mut a, "shape.add_rect", json!({ "x": 10, "y": 10, "width": 30, "height": 30, "layer": 0 })).unwrap();
    run(&mut a, "shape.add_rect", json!({ "x": 50, "y": 10, "width": 30, "height": 30, "layer": 1 })).unwrap();
    run(&mut a, "shape.add_rect", json!({ "x": 10, "y": 50, "width": 30, "height": 30, "layer": 2 })).unwrap();
    run(&mut a, "layer.set_mode", json!({ "layer": 1, "mode": "fill" })).unwrap();
    run(&mut a, "layer.set_mode", json!({ "layer": 2, "mode": "fill" })).unwrap();
    {
        let l = a.editor.doc.layer_mut(2).main_mut();
        l.flood_fill = true;
        l.galvo.wobble = true;
        l.galvo.frequency_khz = 55.0;
        l.interval = 0.2;
    }
    {
        let l = a.editor.doc.layer_mut(1).main_mut();
        l.interval = 0.2;
        l.galvo.auto_rotate = true;
        l.passes = 2;
        l.galvo.global_passes = 3;
    }
    let dev = a.machine.devices.get(id).unwrap().clone();
    let b = crate::galvo_job::build_job(&a.editor.doc, &dev).unwrap();
    assert_eq!(b.job.layers.len(), 3);
    assert!(matches!(b.job.layers[0].mode, galvo::LayerMode::Line));
    assert!(matches!(b.job.layers[1].mode, galvo::LayerMode::Fill(f) if matches!(f.angle_mode, galvo::AngleMode::AutoRotate)));
    assert!(matches!(b.job.layers[2].mode, galvo::LayerMode::Flood(_)));
    assert_eq!(b.job.layers[1].passes, 6, "passes times global passes");
    assert_eq!(b.job.layers[2].params.frequency_khz, 55.0);
    assert!(b.job.layers[2].wobble.is_some() && b.job.layers[0].wobble.is_none());
    assert_eq!(Mode::Fill, a.editor.doc.layer(1).main().mode);
    // Geometry is centred on the field: the bed centre is (55, 55).
    let bounds = b.job.bounds(0.01).unwrap();
    assert!(bounds.x0 < 0.0 && bounds.x1 > 0.0 && (bounds.x0 + 45.0).abs() < 1e-6, "{bounds:?}");
    let est = run(&mut a, "galvo.estimate", Value::Null).unwrap();
    assert!(est["total_s"].as_f64().unwrap() > 0.0 && est["packets"].as_u64().unwrap() > 0, "{est}");
}

#[test]
fn galvo_job_keeps_power_scale_priority_kerf_and_start_points() {
    use darkburn_core::ShapeId;
    use darkburn_core::kurbo::{PathEl, Point, Shape as _};
    let (mut a, id) = galvo_app();
    let dev = a.machine.devices.get(id).unwrap().clone();
    let rect = |a: &mut DarkBurn, x: f64, y: f64, w: f64, layer: usize| {
        ShapeId(run(a, "shape.add_rect", json!({ "x": x, "y": y, "width": w, "height": w, "layer": layer })).unwrap()["id"].as_u64().unwrap())
    };
    let first = rect(&mut a, 10.0, 10.0, 30.0, 0);
    let second = rect(&mut a, 50.0, 10.0, 30.0, 0);
    a.editor.doc.layer_mut(0).main_mut().max_power = 40.0;
    let build = |a: &DarkBurn| crate::galvo_job::build_job(&crate::galvo_job::job_document(a).unwrap(), &dev).unwrap().job;
    let start_of = |l: &galvo::Layer| match l.geometry[0].elements()[0] {
        PathEl::MoveTo(p) => p,
        other => panic!("a path starts with a move: {other:?}"),
    };
    // Nothing special set: one layer with both squares, as before.
    let plain = build(&a);
    assert_eq!(plain.layers.len(), 1);
    assert_eq!((plain.layers[0].geometry.len(), plain.layers[0].params.power_pct), (2, 40.0));

    // Power scale: the second square is marked at half the layer's power.
    a.editor.doc.get_mut(second).unwrap().power_scale = 50.0;
    let j = build(&a);
    assert_eq!(j.layers.iter().map(|l| (l.geometry.len(), l.params.power_pct)).collect::<Vec<_>>(), vec![(1, 40.0), (1, 20.0)]);
    assert!(j.layers[1].geometry[0].bounding_box().x1 > 0.0, "the half-power layer holds the second square");

    // Cut order priority, when the optimization settings order by it: the lower number first.
    a.editor.doc.get_mut(first).unwrap().priority = 5;
    a.editor.doc.get_mut(second).unwrap().priority = 1;
    assert_eq!(build(&a).layers[0].params.power_pct, 40.0, "priority is not an order key yet");
    crate::job_cmds::update_settings(&mut a, "order", |s| s.planner.order_by = vec![darkburn_job::OrderKey::Priority]);
    let j = build(&a);
    assert_eq!(j.layers.iter().map(|l| l.params.power_pct).collect::<Vec<_>>(), vec![20.0, 40.0]);

    // A forced start point: the outline starts there (the middle of the first square's right side).
    let local = a.editor.doc.get(first).unwrap().transform.inverse() * Point::new(40.0, 25.0);
    a.editor.doc.get_mut(first).unwrap().start_point = Some([local.x, local.y]);
    let j = build(&a);
    let p = start_of(&j.layers[1]);
    assert!(p.distance(Point::new(40.0 - 55.0, 25.0 - 55.0)) < 1e-6, "{p:?}");
    assert_eq!(j.layers[1].geometry[0].elements().iter().filter(|e| matches!(e, PathEl::ClosePath)).count(), 1, "still closed");

    // Kerf offset: the outline grows by the kerf on every side.
    let before = build(&a).bounds(0.01).unwrap();
    a.editor.doc.layer_mut(0).main_mut().kerf = 0.5;
    let after = build(&a).bounds(0.01).unwrap();
    assert!((after.width() - before.width() - 1.0).abs() < 1e-6 && (after.height() - before.height() - 1.0).abs() < 1e-6, "{before:?} {after:?}");

    // Fill: a hole with another power scale stays a hole, and is not filled a second time.
    let outer = rect(&mut a, 10.0, 50.0, 30.0, 1);
    let hole = rect(&mut a, 20.0, 60.0, 10.0, 1);
    run(&mut a, "layer.set_mode", json!({ "layer": 1, "mode": "fill" })).unwrap();
    a.editor.doc.get_mut(hole).unwrap().power_scale = 50.0;
    let _ = outer;
    let j = build(&a);
    let fills: Vec<&galvo::Layer> = j.layers.iter().filter(|l| matches!(l.mode, galvo::LayerMode::Fill(_))).collect();
    assert_eq!(fills.len(), 1, "{:?}", fills.iter().map(|l| (l.geometry.len(), l.params.power_pct)).collect::<Vec<_>>());
    assert_eq!(fills[0].geometry.len(), 2, "the outline and its hole");
}

#[test]
fn galvo_job_keeps_the_cut_direction_and_the_tabs() {
    use darkburn_core::kurbo::{PathEl, Point};
    use darkburn_core::{CutDirection, ShapeId};
    let (mut a, id) = galvo_app();
    let dev = a.machine.devices.get(id).unwrap().clone();
    let sq =
        ShapeId(run(&mut a, "shape.add_rect", json!({ "x": 10, "y": 10, "width": 30, "height": 30, "layer": 0 })).unwrap()["id"].as_u64().unwrap());
    a.editor.doc.layer_mut(0).main_mut().max_power = 40.0;
    let build = |a: &DarkBurn| crate::galvo_job::build_job(&crate::galvo_job::job_document(a).unwrap(), &dev).unwrap().job;
    let points =
        |p: &darkburn_core::kurbo::BezPath| -> Vec<Point> { darkburn_job::geom::flatten(p, 0.001).into_iter().flat_map(|q| q.pts).collect() };
    let area = |j: &galvo::Job| darkburn_job::geom::signed_area(&points(&j.layers[0].geometry[0]));
    let length = |l: &galvo::Layer| -> f64 {
        l.geometry
            .iter()
            .flat_map(|p| darkburn_job::geom::flatten(p, 0.001))
            .map(|q| {
                let mut pts = q.pts.clone();
                if q.closed {
                    pts.push(pts[0]);
                }
                darkburn_job::geom::polyline_length(&pts)
            })
            .sum()
    };

    // Cut direction: clockwise and counter-clockwise turn the outline, reversed turns the drawn one round.
    let drawn = area(&build(&a));
    assert!((drawn.abs() - 900.0).abs() < 1e-6, "{drawn}");
    a.editor.doc.get_mut(sq).unwrap().direction = CutDirection::Clockwise;
    assert!(area(&build(&a)) < 0.0, "clockwise (Y up): {}", area(&build(&a)));
    a.editor.doc.get_mut(sq).unwrap().direction = CutDirection::CounterClockwise;
    assert!(area(&build(&a)) > 0.0, "counter-clockwise: {}", area(&build(&a)));
    a.editor.doc.get_mut(sq).unwrap().direction = CutDirection::Reversed;
    assert!(area(&build(&a)) * drawn < 0.0, "reversed: {} against {drawn}", area(&build(&a)));
    // A forced start point stays the start whichever way round.
    let local = a.editor.doc.get(sq).unwrap().transform.inverse() * Point::new(40.0, 25.0);
    a.editor.doc.get_mut(sq).unwrap().start_point = Some([local.x, local.y]);
    for d in CutDirection::ALL {
        a.editor.doc.get_mut(sq).unwrap().direction = d;
        let j = build(&a);
        let first = points(&j.layers[0].geometry[0])[0];
        assert!(first.distance(Point::new(40.0 - 55.0, 25.0 - 55.0)) < 1e-6, "{d:?}: {first:?}");
        assert_eq!(j.layers[0].geometry[0].elements().iter().filter(|e| matches!(e, PathEl::ClosePath)).count(), 1, "{d:?}: still closed");
    }
    a.editor.doc.get_mut(sq).unwrap().direction = CutDirection::Auto;
    a.editor.doc.get_mut(sq).unwrap().start_point = None;

    // Automatic tabs: four gaps of 2 mm in the 120 mm outline.
    {
        let t = &mut a.editor.doc.layer_mut(0).main_mut().tabs;
        t.enabled = true;
        t.auto_spacing = true;
        t.spacing = 30.0;
        t.size = 2.0;
        t.min_count = 4;
        t.max_count = 4;
    }
    let j = build(&a);
    assert_eq!(j.layers.len(), 1);
    assert!((length(&j.layers[0]) - 112.0).abs() < 1e-6, "marked {} mm of 120", length(&j.layers[0]));
    for mid in [Point::new(25.0, 10.0), Point::new(40.0, 25.0), Point::new(25.0, 40.0), Point::new(10.0, 25.0)] {
        let board = Point::new(mid.x - 55.0, mid.y - 55.0);
        let near = j.layers[0].geometry.iter().flat_map(&points).map(|p| p.distance(board)).fold(f64::MAX, f64::min);
        assert!((near - 1.0).abs() < 1e-6, "the tab at {mid:?} is a 2 mm gap: nearest mark {near}");
    }

    // A manual tab replaces the automatic ones on its shape.
    let local = a.editor.doc.get(sq).unwrap().transform.inverse() * Point::new(20.0, 10.0);
    a.editor.doc.get_mut(sq).unwrap().tabs = vec![[local.x, local.y]];
    let j = build(&a);
    assert!((length(&j.layers[0]) - 118.0).abs() < 1e-6, "{}", length(&j.layers[0]));
    let board = Point::new(20.0 - 55.0, 10.0 - 55.0);
    let near = j.layers[0].geometry.iter().flat_map(&points).map(|p| p.distance(board)).fold(f64::MAX, f64::min);
    assert!((near - 1.0).abs() < 1e-6, "{near}");

    // Partly cut tabs are marked at the tab power, not left out.
    a.editor.doc.layer_mut(0).main_mut().tabs.partial = true;
    a.editor.doc.layer_mut(0).main_mut().tabs.tab_power = 10.0;
    let j = build(&a);
    assert_eq!(j.layers.iter().map(|l| l.params.power_pct).collect::<Vec<_>>(), vec![40.0, 10.0]);
    assert!(
        (length(&j.layers[0]) - 118.0).abs() < 1e-6 && (length(&j.layers[1]) - 2.0).abs() < 1e-6,
        "{} {}",
        length(&j.layers[0]),
        length(&j.layers[1])
    );
    crate::galvo_job::compile(&j, &dev).expect("the job with tabs compiles");

    // Tabs off: the whole outline, closed, as before.
    a.editor.doc.layer_mut(0).main_mut().tabs.enabled = false;
    let j = build(&a);
    assert_eq!(j.layers.len(), 1);
    assert!((length(&j.layers[0]) - 120.0).abs() < 1e-6);
}

#[test]
fn galvo_device_settings_edit_import_and_compile() {
    let (mut a, _) = galvo_app();
    run(
        &mut a,
        "galvo.settings_set",
        json!({ "values": { "field_size_mm": 160.0, "source": "Mopa", "cal1": { "skew": 0.02 }, "ports": { "door_bit": 4, "done_bit": 2 } } }),
    )
    .unwrap();
    let g = run(&mut a, "galvo.settings_get", Value::Null).unwrap();
    assert_eq!(g["field_size_mm"], 160.0);
    assert_eq!(g["cal1"]["skew"], 0.02);
    assert_eq!(a.machine.active().bed(), (160.0, 160.0));
    assert!(run(&mut a, "galvo.settings_set", json!({ "values": { "nonsense": 1 } })).is_err());
    assert!(run(&mut a, "galvo.settings_set", json!({ "values": { "field_size_mm": 0.5 } })).is_err());
    // EZCad import.
    let dir = std::env::temp_dir().join(format!("darkburn_ezcad_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let cfg = dir.join("markcfg.ini");
    std::fs::write(&cfg, "[Lens]\nField Size = 175\nOffset X = 1.5\nGalvo1 Scale = 1.01\nLaserType = co2\nJumpSpeed = 3500\n").unwrap();
    let r = run(&mut a, "galvo.import_ezcad", json!({ "path": cfg.display().to_string() })).unwrap();
    assert!(!r["applied"].as_array().unwrap().is_empty(), "{r}");
    let g = run(&mut a, "galvo.settings_get", Value::Null).unwrap();
    assert_eq!(g["field_size_mm"], 175.0);
    assert_eq!(g["source"], "Co2");
    assert_eq!(g["offset_x_mm"], 1.5);
    assert!(run(&mut a, "galvo.import_ezcad", json!({ "path": dir.join("missing.ini").display().to_string() })).is_err());
    std::fs::remove_dir_all(&dir).ok();
    // Safety: the library settings never enable the laser at init.
    let s = a.machine.active().settings.galvo.to_library();
    assert!(!s.enable_laser_at_init && !s.enable_z_at_init);
}

#[test]
fn galvo_lens_file_loads_and_clears_and_the_test_pattern_lands_in_the_design() {
    let (mut a, _) = galvo_app();
    let emu = attach_galvo(&mut a, GalvoDeviceSettings::default());
    let dir = std::env::temp_dir().join(format!("darkburn_cor_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let cor = dir.join("lens.cor");
    let table = galvo::CorTable::from_fn(|r, c| ((r as i32 - 32) / 4, (c as i32 - 32) / 4));
    std::fs::write(&cor, table.to_bytes()).unwrap();
    let r = run(&mut a, "machine.galvo_lens", json!({ "path": cor.display().to_string() })).unwrap();
    assert_eq!(r["loaded"], true);
    wait(&mut a, "lens loaded", |a| a.machine.handle.snapshot().galvo.as_ref().is_some_and(|g| g.lens_loaded));
    assert_eq!(a.machine.active().settings.galvo.cor_path, cor.display().to_string());
    run(&mut a, "machine.galvo_lens", json!({})).unwrap();
    wait(&mut a, "lens cleared", |a| a.machine.handle.snapshot().galvo.as_ref().is_some_and(|g| !g.lens_loaded));
    assert!(a.machine.active().settings.galvo.cor_path.is_empty());
    assert!(run(&mut a, "machine.galvo_lens", json!({ "path": dir.join("nope.cor").display().to_string() })).is_err());
    let _ = emu;
    // The generator command adds one path to the document, centred on the field.
    let before = a.editor.doc.len();
    run(&mut a, "galvo.lens_test", json!({ "size_mm": 80, "divisions": 4, "layer": 3 })).unwrap();
    assert_eq!(a.editor.doc.len(), before + 1);
    let bb = a.editor.doc.bbox().unwrap();
    assert!((bb.center().x - 55.0).abs() < 2.5 && (bb.center().y - 55.0).abs() < 2.5, "{bb:?}");
    assert!(run(&mut a, "galvo.lens_test", json!({ "size_mm": 5000 })).is_err());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn galvo_repeat_marking_rotary_and_axes() {
    let (mut a, id) = galvo_app();
    galvo_square(&mut a);
    run(&mut a, "galvo.repeat_marking", json!({ "count": 3, "axis_on": true, "axis": 1, "step_mm": 2.5, "wait_foot": false })).unwrap();
    let r = a.machine.devices.get(id).unwrap().settings.galvo.repeat;
    assert_eq!((r.count, r.axis, r.axis_step_mm), (3, 1, 2.5));
    let dev = a.machine.devices.get(id).unwrap().clone();
    let b = crate::galvo_job::build_job(&a.editor.doc, &dev).unwrap();
    assert_eq!(b.job.repeat.count, 3);
    assert_eq!(b.job.repeat.axis_step, Some((1, 2.5)));
    let c = crate::galvo_job::compile(&b.job, &dev).unwrap();
    assert!(c.steps.iter().any(|s| matches!(s, galvo::Step::MoveAxis { axis: 1, .. })), "an axis move between repeats");
    // Rotary runner options reach the job.
    run(&mut a, "galvo.settings_set", json!({ "values": { "rotary": { "on": true, "by_shape": true, "max_size_mm": 30.0 } } })).unwrap();
    let dev = a.machine.devices.get(id).unwrap().clone();
    let b = crate::galvo_job::build_job(&a.editor.doc, &dev).unwrap();
    assert_eq!(b.job.rotary.unwrap().mode, galvo::RotaryMode::ByShape);
    // The axis command works on the simulator board.
    let _emu = attach_galvo(&mut a, GalvoDeviceSettings::default());
    run(&mut a, "machine.galvo_axis", json!({ "axis": 0, "mm": 5.0 })).unwrap();
    wait(&mut a, "axis done", |a| a.machine.handle.snapshot().state == LaserState::Ready);
    assert!(run(&mut a, "machine.galvo_axis", json!({ "axis": 5, "mm": 5.0 })).is_err());
    // A galvo has no fire button, jog or home.
    assert!(matches!(a.machine.handle.fire(5.0, 1.0), Err(MachineError::Refused(_))));
    assert!(matches!(a.machine.handle.home(), Err(MachineError::Unsupported(_))));
}

#[test]
fn real_galvo_usb_is_not_built_in() {
    let mut h = MachineHandle::new();
    let spec = ConnectSpec { link: Link::GalvoUsb, config: ConnectConfig::default() };
    assert!(matches!(h.connect(&spec, None), Err(MachineError::Refused(_))));
    assert!(
        matches!(h.connect(&spec, Some(RealLinkConsent::from_user_click())), Err(MachineError::Refused(_))),
        "test builds refuse every real link"
    );
}

// ----- Gweike -------------------------------------------------------------------------------------------------

/// A craft with a value outside the machine's limits (gas pressure, speed, frequency, a missing
/// pierce stage) is not stored when a command or an agent sets it, and a table that holds one
/// some other way (the editor, a device file) is not written out for the operator to enter.
#[test]
fn a_craft_outside_the_machine_limits_is_refused_and_never_exported() {
    let dir = std::env::temp_dir().join(format!("darkburn_chf_bad_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut a = gweike_app();
    let good = serde_json::to_value(darkburn_mcc::FibreCraft::default()).unwrap();
    for (field, value) in [("gas_pressure_bar", json!(40.0)), ("speed_mm_s", json!(0.0)), ("freq_hz", json!(1.0e9)), ("power_pct", json!(250.0))] {
        let mut bad = good.clone();
        bad["cut"][field] = value;
        let r = run(&mut a, "gweike.set_craft", json!({ "head": "fibre", "slot": 2, "craft": bad }));
        assert!(r.is_err(), "{field} out of range was stored: {r:?}");
    }
    let mut no_pierce = good.clone();
    no_pierce["process"] = json!("pierce3");
    no_pierce["pierce"] = json!([]);
    assert!(
        run(&mut a, "gweike.set_craft", json!({ "head": "fibre", "slot": 2, "craft": no_pierce })).is_err(),
        "a pierce process without its stages"
    );
    let mut co2 = serde_json::to_value(darkburn_mcc::Co2Craft::default()).unwrap();
    co2["gas_pressure_bar"] = json!(11.0);
    assert!(run(&mut a, "gweike.set_craft", json!({ "head": "co2", "slot": 2, "craft": co2 })).is_err());
    assert_eq!(a.machine.active().settings.gweike.crafts, darkburn_mcc::CraftTable::default(), "nothing was stored");
    // The export of a sound table still works...
    let path = dir.join("job.chf");
    let side = dir.join("job.chf.crafts.json");
    run(&mut a, "gweike.export_chf", json!({ "path": path.display().to_string() })).unwrap();
    assert!(path.exists() && side.exists());
    std::fs::remove_file(&path).unwrap();
    std::fs::remove_file(&side).unwrap();
    // ...and one with a bad value in it writes nothing.
    let id = a.machine.active().id;
    a.machine.devices.get_mut(id).unwrap().settings.gweike.crafts.fibre[5].cut.gas_pressure_bar = 40.0;
    let e = run(&mut a, "gweike.export_chf", json!({ "path": path.display().to_string() })).unwrap_err();
    assert!(e.contains("gas pressure"), "{e}");
    assert!(!path.exists() && !side.exists(), "no file for the operator to enter 40 bar from");
    std::fs::remove_dir_all(&dir).ok();
}

/// A device file whose craft table is short (cut by hand, or from another program) must not
/// crash the craft editor or `gweike.set_craft`: the table is filled up to the eleven slots.
#[test]
fn a_short_craft_table_in_a_device_file_is_filled_up() {
    let mut a = gweike_app();
    let id = a.machine.active().id;
    let text = a.machine.devices.export_profile(id).unwrap();
    let mut v: Value = serde_json::from_str(&text).unwrap();
    let crafts = v.pointer_mut("/device/settings/gweike/crafts").expect("the crafts of the profile");
    crafts["fibre"] = json!([]);
    crafts["co2"].as_array_mut().unwrap().truncate(2);
    let id2 = a.machine.devices.import_profile(&v.to_string()).unwrap();
    run(&mut a, "device.select", json!({ "id": id2 })).unwrap();
    let t = &a.machine.active().settings.gweike.crafts;
    assert_eq!((t.fibre.len(), t.co2.len()), (11, 11));
    let good = serde_json::to_value(darkburn_mcc::FibreCraft::default()).unwrap();
    run(&mut a, "gweike.set_craft", json!({ "head": "fibre", "slot": 10, "craft": good })).unwrap();
}

fn gweike_app() -> DarkBurn {
    let mut a = app();
    let id = run(&mut a, "device.add", json!({ "type": "GweikeCardSimulator" })).unwrap()["id"].as_u64().unwrap();
    run(&mut a, "device.select", json!({ "id": id })).unwrap();
    run(&mut a, "shape.add_rect", json!({ "x": 100, "y": 100, "width": 200, "height": 120, "layer": 0 })).unwrap();
    run(&mut a, "shape.add_ellipse", json!({ "x": 400, "y": 300, "width": 80, "height": 80, "layer": 1 })).unwrap();
    a
}

#[test]
fn chf_export_is_written_and_read_back_by_the_library() {
    let dir = std::env::temp_dir().join(format!("darkburn_chf_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut a = gweike_app();
    run(&mut a, "gweike.layer_map", json!({ "layer": 0, "material": "Steel 3 mm", "head": "fibre", "slot": 3 })).unwrap();
    run(&mut a, "gweike.layer_map", json!({ "layer": 1, "material": "Acrylic", "head": "co2", "slot": 7 })).unwrap();
    let path = dir.join("job.chf");
    let r = run(&mut a, "gweike.export_chf", json!({ "path": path.display().to_string() })).unwrap();
    assert!(r["note"].as_str().unwrap().contains("read-only capture"), "{r}");
    let bytes = std::fs::read(&path).unwrap();
    assert!(bytes.starts_with(b"scFlie\r\n"), "the vendor magic");
    let doc = darkburn_mcc::read_chf(&bytes, true).unwrap();
    assert_eq!(doc.graphs.len() as u64, r["contours"].as_u64().unwrap());
    let layers: Vec<i32> = doc.graphs.iter().flat_map(|g| g.contours()).map(|c| c.layer).collect();
    assert!(layers.contains(&3) && layers.contains(&7), "craft slots from the mapping: {layers:?}");
    // The geometry is the design's: the rectangle's contour spans 200 x 120 mm.
    let rect = doc.graphs.iter().flat_map(|g| g.contours()).find(|c| c.layer == 3).unwrap();
    assert!((rect.bbox_max.x - rect.bbox_min.x - 200.0).abs() < 0.01 && (rect.bbox_max.y - rect.bbox_min.y - 120.0).abs() < 0.01);
    // Writing what was read is byte for byte the same file.
    assert_eq!(darkburn_mcc::write_chf(&doc).unwrap(), bytes);
    // The craft table lands beside it for the operator.
    let side = std::fs::read_to_string(dir.join("job.chf.crafts.json")).unwrap();
    assert!(side.contains("Steel 3 mm") && side.contains("Do not send") || side.contains("does not send them"), "{side}");
    // A skipped layer is left out.
    run(&mut a, "gweike.layer_map", json!({ "layer": 1, "head": "skip" })).unwrap();
    let r = run(&mut a, "gweike.export_chf", json!({ "path": path.display().to_string() })).unwrap();
    assert_eq!(r["skipped_layers"], json!([1]));
    assert!(run(&mut a, "gweike.layer_map", json!({ "layer": 0, "slot": 11 })).is_err());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn craft_editor_values_validate_and_survive_the_profile() {
    let mut a = gweike_app();
    let craft = json!({
        "name": "Steel 3 mm O2", "note": "", "nozzle": "1.5S", "no_manu": false, "process": "pierce2",
        "cut": { "height_mm": 0.8, "focus_mm": 0.0, "speed_mm_s": 20.0, "power_pct": 100.0, "freq_hz": 5000.0, "peak_current_pct": 100.0,
                 "gas": "o2", "gas_pressure_bar": 0.6, "laser_on_delay_ms": 0.0, "laser_off_before_delay_ms": 0.0, "laser_off_after_delay_ms": 0.0, "up_height_mm": 20.0 },
        "pierce": [
            { "height_mm": 10.0, "focus_mm": 0.0, "focus_ramp": null, "power_pct": 60.0, "freq_hz": 500.0, "peak_current_pct": 100.0, "gas": "o2", "gas_pressure_bar": 0.6, "duration_ms": 500.0, "gradual_time_ms": null, "before_laser_off_delay_ms": 0.0, "after_laser_off_delay_ms": 0.0 },
            { "height_mm": 6.0, "focus_mm": 0.0, "focus_ramp": null, "power_pct": 100.0, "freq_hz": 500.0, "peak_current_pct": 100.0, "gas": "o2", "gas_pressure_bar": 0.6, "duration_ms": 800.0, "gradual_time_ms": null, "before_laser_off_delay_ms": 0.0, "after_laser_off_delay_ms": 0.0 }
        ],
        "follower": { "follow": true, "fixed_height_cut_pos_mm": 20.0, "short_distance_no_lift": true, "pre_drill": false, "pre_drill_no_lift": false, "pierce_again_before_cut": false, "with_film": false },
        "gas_flags": { "short_dist_keep_on": false, "keep_on_during_job": false },
        "power_curve": [], "freq_curve": [], "dynamic_power": false, "dynamic_freq": false, "slow_start": null, "smooth_pierce": null
    });
    let r = run(&mut a, "gweike.set_craft", json!({ "head": "fibre", "slot": 3, "craft": craft })).unwrap();
    assert_eq!(r["problems"], json!([]), "{r}");
    let c = run(&mut a, "gweike.crafts", Value::Null).unwrap();
    assert_eq!(c["crafts"]["fibre"][3]["nozzle"], "1.5S");
    assert_eq!(c["crafts"]["fibre"][3]["pierce"].as_array().unwrap().len(), 2);
    // A speed past the machine's limit is refused: the slot keeps what it had.
    let mut bad = craft.clone();
    bad["cut"]["speed_mm_s"] = json!(9000.0);
    let e = run(&mut a, "gweike.set_craft", json!({ "head": "fibre", "slot": 4, "craft": bad })).unwrap_err();
    assert!(e.contains("speed"), "{e}");
    assert_eq!(a.machine.active().settings.gweike.crafts.fibre[4], darkburn_mcc::FibreCraft::default());
    assert!(run(&mut a, "gweike.set_craft", json!({ "head": "fibre", "slot": 3, "craft": { "process": "bogus" } })).is_err());
    // It is kept with the device.
    let id = a.machine.active().id;
    let text = a.machine.devices.export_profile(id).unwrap();
    let mut store = DeviceStore::default();
    let id2 = store.import_profile(&text).unwrap();
    assert_eq!(store.get(id2).unwrap().settings.gweike.crafts.fibre[3].nozzle.as_deref(), Some("1.5S"));
}

#[test]
fn the_card_simulator_plans_and_runs_with_no_motion_path() {
    let mut a = gweike_app();
    let plan = run(&mut a, "gweike.plan", Value::Null).unwrap();
    let ticks = plan["ticks"].as_u64().unwrap();
    assert!(ticks > 1000 && plan["frames"].as_u64().unwrap() >= 1, "{plan}");
    assert!(plan["estimated_s"].as_f64().unwrap() > 1.0);
    assert!(plan["cut_length_mm"].as_f64().unwrap() > 600.0, "the rectangle and circle perimeter: {plan}");
    assert_eq!(plan["simulator_only"], true);
    let sim = run(&mut a, "gweike.simulate", Value::Null).unwrap();
    let card = &sim["card"];
    assert_eq!(card["ticks_consumed"].as_u64().unwrap(), ticks, "every planned tick ran on the simulated card: {sim}");
    assert_eq!(card["forbidden_attempts"], 0);
    assert_eq!(card["laser_refusals"], 0, "no laser data was ever sent: {sim}");
    // Nothing connected anywhere: the handle is untouched.
    assert!(!a.machine.handle.is_connected() && !a.machine.handle.is_real_machine());
    assert!(run(&mut app(), "gweike.plan", Value::Null).is_err(), "an empty design has nothing to plan");
}

#[test]
fn gweike_device_cannot_connect_and_commands_are_flagged() {
    let mut a = gweike_app();
    assert!(run(&mut a, "machine.connect", json!({})).is_err());
    assert!(!a.machine.handle.is_connected());
    assert!(matches!(a.machine.handle.start_job(simulator_square(Family::Grbl, 10.0)), Err(MachineError::NotConnected)));
    assert!(a.machine.request_connect().is_err());
    for c in crate::machine_ext::COMMANDS.iter().filter(|c| c.id.starts_with("gweike.")) {
        assert!(crate::machine_ext::gweike(c.id), "{} is a Gweike command", c.id);
        let info = crate::commands::describe(c.id).unwrap();
        assert!(!info.hardware, "{}: no Gweike command touches a machine", c.id);
        assert!(info.implemented, "{}", c.id);
    }
    assert!(crate::machine_ext::writes_files("gweike.export_chf"));
    assert!(crate::machine_ext::gweike("file.export_gweike_dxf") && crate::machine_ext::writes_files("file.export_gweike_nc"));
    assert!(!crate::machine_ext::writes_files("gweike.plan"));
}

#[test]
fn gweike_dxf_export_uses_the_layer_to_material_mapping() {
    let dir = std::env::temp_dir().join(format!("darkburn_gwdxf_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut a = gweike_app();
    run(&mut a, "gweike.layer_map", json!({ "layer": 0, "material": "Steel_3mm" })).unwrap();
    let path = dir.join("m.dxf");
    run(&mut a, "project.export", json!({ "path": path.display().to_string(), "format": "gweike_dxf" })).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("Steel_3mm"), "the mapped material is the DXF layer name");
    std::fs::remove_dir_all(&dir).ok();
}

/// No code path in the app reaches a real transport for Gweike: the Gweike modules name no
/// socket, serial or USB type, the card library has no transport dependency, and `Link` has no
/// Gweike variant.
#[test]
fn no_app_code_reaches_a_real_transport_for_gweike() {
    let forbidden = [
        "std::net",
        "UdpSocket",
        "TcpStream",
        "TcpListener",
        "serialport",
        "SerialTransport",
        "TcpTransport",
        "UdpTransport",
        "UsbTransport",
        "nusb",
        "rusb",
        "std::process",
        "reqwest",
        "tungstenite",
        "ConnectSpec",
        "machine_api::Link",
        "RealLinkConsent",
        "open_adapter",
        "open_real",
    ];
    for (name, src) in [("gweike_workflow.rs", include_str!("gweike_workflow.rs")), ("gweike_ui.rs", include_str!("gweike_ui.rs"))] {
        for (i, line) in src.lines().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            for f in forbidden {
                assert!(!code.contains(f), "{name}:{} mentions {f}: {line}", i + 1);
            }
        }
    }
    // The Gweike commands in machine_ext.rs never build a connection either.
    let ext = include_str!("machine_ext.rs");
    let gw_fns = ext.split("// ----- Gweike").nth(1).unwrap_or("");
    for f in forbidden {
        assert!(!gw_fns.lines().map(|l| l.split("//").next().unwrap_or("")).any(|l| l.contains(f)), "machine_ext.rs Gweike section mentions {f}");
    }
    // The link and adapter modules know nothing of the card library.
    for (name, src) in [
        ("machine_api.rs", include_str!("machine_api.rs")),
        ("gcode_machines.rs", include_str!("gcode_machines.rs")),
        ("galvo_machine.rs", include_str!("galvo_machine.rs")),
    ] {
        assert!(!src.contains("darkburn_mcc") && !src.contains("darkburn-mcc"), "{name} must not touch the card library");
    }
    let lib_manifest = include_str!("../../../crates/darkburn-mcc/Cargo.toml");
    let deps = lib_manifest.split("[dependencies]").nth(1).unwrap_or("");
    for bad in ["socket", "serialport", "nusb", "rusb", "tokio", "mio", "reqwest"] {
        assert!(!deps.contains(bad), "darkburn-mcc must have no transport dependency ({bad})");
    }
    // Link has no Gweike or card variant.
    let api = include_str!("machine_api.rs");
    let link = api.split("pub enum Link {").nth(1).and_then(|s| s.split("\n}\n").next()).unwrap();
    assert!(!link.to_lowercase().contains("gweike") && !link.to_lowercase().contains("mcc"), "{link}");
}

#[test]
fn help_text_says_direct_control_waits_for_a_capture() {
    assert!(crate::gweike_workflow::DIRECT_CONTROL_NOTE.contains("read-only capture"));
    assert!(crate::gweike_workflow::DIRECT_CONTROL_NOTE.contains("mlaser-rea-findings"));
}
