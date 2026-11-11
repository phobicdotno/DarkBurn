//! Ruida controllers end to end, without hardware: the Ethernet transport over UDP sockets the
//! test opens itself on 127.0.0.1, the USB byte stream against the in-memory emulator, both
//! through the machine API; the `.rd` machine file and its way back in; hardware overscan; the
//! board list; and the DSP controllers DarkBurn does not support yet.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use darkburn_core::kurbo::Vec2;
use darkburn_grbl::PortInfo;
use darkburn_ruida as ruida;
use ruida::emulator::MotionKind;
use serde_json::{Value, json};

use crate::api::execute_headless;
use crate::devices::{ConnectionKind, Device, DeviceType, pick_ftdi_port};
use crate::job_field_tests::{build, fill_doc, line_doc};
use crate::machine_api::*;
use crate::machine_job::{RdOptions, hardware_overscan_modes, machine_file_kind, toolpath_to_rd, toolpath_to_rd_with};
use crate::prefs::Prefs;
use crate::shell::DarkBurn;

const T: Duration = Duration::from_secs(20);

type Emu = Arc<Mutex<ruida::RuidaEmulator>>;

fn emulator(cfg: ruida::EmulatorConfig) -> Emu {
    Arc::new(Mutex::new(ruida::RuidaEmulator::new(ruida::EmulatorConfig { step_mode: true, ..cfg })))
}

/// Put a Ruida driver on `transport` into the handle. The emulator behind it is stepped by the
/// adapter, as for the built-in simulator; every byte still goes through `transport`.
fn attach(h: &mut MachineHandle, transport: Box<dyn ruida::Transport>, emu: &Emu) {
    let cfg = ConnectConfig { sim_time_scale: 50.0, ..Default::default() };
    let adapter = RuidaAdapter::new(transport, &cfg, true, Some((emu.clone(), 50.0)));
    h.attach_simulator(Box::new(adapter), Link::SimulatorRuida);
    assert!(h.wait_for(T, |s, _| s.connected()), "connects: {:?}", h.snapshot().link);
    assert!(h.wait_for(T, |s, _| s.state == LaserState::Ready));
}

fn wait_event(h: &mut MachineHandle, f: impl Fn(&MachineEvent) -> bool) -> bool {
    h.wait_for(T, |_, ev| ev.iter().any(&f))
}

fn cuts(emu: &Emu) -> usize {
    emu.lock().unwrap().motion.iter().filter(|m| m.kind == MotionKind::Cut).count()
}

fn app() -> DarkBurn {
    DarkBurn::new(Prefs::default())
}

fn run(app: &mut DarkBurn, id: &str, args: Value) -> Result<Value, String> {
    execute_headless(app, id, &args)
}

fn wait(app: &mut DarkBurn, what: &str, mut f: impl FnMut(&DarkBurn) -> bool) {
    let end = Instant::now() + T;
    loop {
        app.machine.tick();
        if f(app) {
            return;
        }
        assert!(Instant::now() < end, "timed out waiting for {what}; state {:?}", app.machine.handle.snapshot().state);
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn temp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("darkburn-ruida-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn select(app: &mut DarkBurn, kind: DeviceType) -> u64 {
    let id = match app.machine.devices.devices.iter().find(|d| d.kind == kind) {
        Some(d) => d.id,
        None => app.machine.devices.add(Device::new(kind)),
    };
    assert!(app.machine.devices.select(id));
    id
}

// ----- RUIDA-01, CONN-04: Ethernet (UDP) ---------------------------------------------------------

#[test]
fn ruida_over_ethernet_runs_through_the_machine_api_on_loopback_udp() {
    let emu = emulator(Default::default());
    let server = ruida::UdpEmulatorServer::start(emu.clone()).expect("two loopback sockets");
    let mut h = MachineHandle::new();
    attach(&mut h, Box::new(server.transport().unwrap()), &emu);
    assert!(h.snapshot().firmware.contains("RDC6442S"), "{}", h.snapshot().firmware);
    assert!(h.wait_for(T, |s, _| s.bed == Some((600.0, 400.0))), "bed read from the controller: {:?}", h.snapshot().bed);

    // A job, streamed datagram by datagram and run.
    h.start_job(simulator_square(Family::Ruida, 30.0)).unwrap();
    assert!(wait_event(&mut h, |e| matches!(e, MachineEvent::JobStarted)));
    assert!(wait_event(&mut h, |e| matches!(e, MachineEvent::JobFinished(_))), "job finished");
    assert!(h.wait_for(T, |s, _| s.state == LaserState::Ready && s.job.is_none()));
    assert_eq!(cuts(&emu), 4);

    // Moves and the position report.
    h.move_to(MoveSpec { x: Some(50.0), y: Some(40.0), feed: 6000.0, ..Default::default() }).unwrap();
    assert!(h.wait_for(T, |s, _| (s.machine_pos[0] - 50.0).abs() < 0.01 && (s.machine_pos[1] - 40.0).abs() < 0.01));
    h.home().unwrap();
    assert!(wait_event(&mut h, |e| matches!(e, MachineEvent::Homed)));

    // The jog port (UDP 50207 on a real controller): Fire presses and releases the Pulse key.
    h.fire(5.0, 0.2).unwrap();
    assert!(h.wait_for(T, |s, _| s.fire_on));
    assert!(h.wait_for(T, |s, _| !s.fire_on));
    let released = |e: &Emu| e.lock().unwrap().panel_received.contains(&ruida::PanelCommand::KeyUp(ruida::PanelKey::Pulse));
    assert!(h.wait_for(T, |_, _| released(&emu)), "the Pulse key went down and up on the jog port");
    assert!(emu.lock().unwrap().panel_received.contains(&ruida::PanelCommand::KeyDown(ruida::PanelKey::Pulse)));

    // Files on the controller.
    let job = match simulator_square(Family::Ruida, 20.0).data {
        JobData::Rd(j) => j,
        _ => unreachable!(),
    };
    let mut req = JobRequest::new(JobData::RdFile { name: "NET".into(), data: job.to_rd_file(ruida::Magic::DEFAULT).unwrap() });
    req.name = "NET".into();
    req.start_immediately = false;
    req.auto_increment_name = false;
    h.start_job(req).unwrap();
    assert!(h.wait_for(T, |s, _| s.ruida.files.as_ref().is_some_and(|f| f.len() == 1 && f[0].name == "NET")));

    // Settings.
    h.read_settings().unwrap();
    assert!(wait_event(&mut h, |e| matches!(e, MachineEvent::SettingsLoaded)));
    assert_eq!(h.backup_settings().map(|b| b.family()), Some(Family::Ruida));

    let e = emu.lock().unwrap();
    assert_eq!((e.checksum_failures, e.file_sum_mismatches), (0, 0));
    drop(e);
    assert!(server.datagrams() > 50, "everything went over the sockets: {}", server.datagrams());
    h.disconnect();
}

#[test]
fn a_drawing_runs_on_a_ruida_over_loopback_udp_from_the_app() {
    let mut a = app();
    select(&mut a, DeviceType::SimulatorRuida);
    let emu = emulator(Default::default());
    let server = ruida::UdpEmulatorServer::start(emu.clone()).unwrap();
    attach(&mut a.machine.handle, Box::new(server.transport().unwrap()), &emu);
    run(&mut a, "shape.add_rect", json!({ "x": 10, "y": 10, "width": 20, "height": 20, "layer": 0 })).unwrap();
    let before = server.datagrams();
    run(&mut a, "job.start", Value::Null).unwrap();
    wait(&mut a, "finish", |a| a.machine.job_log.iter().any(|l| l.starts_with("Job finished")));
    wait(&mut a, "ready", |a| a.machine.handle.snapshot().state == LaserState::Ready);
    assert_eq!(cuts(&emu), 4);
    assert!(server.datagrams() > before);
    // Send stores it on the controller.
    run(&mut a, "job.send", Value::Null).unwrap();
    wait(&mut a, "stored", |a| a.machine.handle.snapshot().ruida.files.as_ref().is_some_and(|f| !f.is_empty()));
    assert_eq!(emu.lock().unwrap().files().len(), 1);
    a.machine.handle.disconnect();
}

#[test]
fn a_real_ruida_ethernet_link_is_offered_but_never_opened_in_tests() {
    let mut d = Device::new(DeviceType::Ruida);
    assert_eq!(d.kind.connections(), &[ConnectionKind::Udp, ConnectionKind::Usb, ConnectionKind::Bridge]);
    d.connection = ConnectionKind::Udp;
    d.host = "not an address".into();
    assert!(d.validate().iter().any(|m| m.contains("IP address")));
    d.host = "10.0.0.7".into();
    let spec = d.connect_spec(&[]).unwrap();
    assert_eq!(spec.link, Link::Udp { ip: "10.0.0.7".into() });
    assert_eq!(spec.link.family(), Family::Ruida);
    assert_eq!(spec.config.net_timeout_ms, Some(3000));
    let mut h = MachineHandle::new();
    assert!(matches!(h.connect(&spec, None), Err(MachineError::Refused(_))));
    assert!(matches!(h.connect(&spec, Some(RealLinkConsent::from_user_click())), Err(MachineError::Refused(_))));
    assert!(!h.is_connected());
}

// ----- RUIDA-02, CONN-06: USB ---------------------------------------------------------------------

#[test]
fn ruida_over_usb_runs_through_the_machine_api_on_the_stream_emulator() {
    let emu = emulator(Default::default());
    let mut h = MachineHandle::new();
    attach(&mut h, Box::new(ruida::MemoryTransport::stream(emu.clone())), &emu);
    assert!(h.snapshot().firmware.contains("RDC6442S"));

    h.start_job(simulator_square(Family::Ruida, 30.0)).unwrap();
    assert!(wait_event(&mut h, |e| matches!(e, MachineEvent::JobStarted)));
    assert!(wait_event(&mut h, |e| matches!(e, MachineEvent::JobFinished(_))), "job finished");
    assert!(h.wait_for(T, |s, _| s.state == LaserState::Ready && s.job.is_none()));
    assert_eq!(cuts(&emu), 4);

    // Jog (steps and held keys go on the data channel), home, position.
    h.move_to(MoveSpec { x: Some(50.0), y: Some(40.0), feed: 6000.0, ..Default::default() }).unwrap();
    h.jog_step(JogSpec { dx: 5.0, feed: 3000.0, ..Default::default() }).unwrap();
    assert!(h.wait_for(T, |s, _| (s.machine_pos[0] - 55.0).abs() < 0.01));
    h.jog_start([0, 1, 0, 0], 3000.0).unwrap();
    h.jog_stop().unwrap();
    assert!(h.wait_for(T, |s, _| (s.machine_pos[1] - 50.0).abs() < 0.01), "one key press moved Y: {:?}", h.snapshot().machine_pos);
    h.home().unwrap();
    assert!(wait_event(&mut h, |e| matches!(e, MachineEvent::Homed)));

    // Files.
    let job = match simulator_square(Family::Ruida, 20.0).data {
        JobData::Rd(j) => j,
        _ => unreachable!(),
    };
    let mut req = JobRequest::new(JobData::RdFile { name: "USB".into(), data: job.to_rd_file(ruida::Magic::DEFAULT).unwrap() });
    req.name = "USB".into();
    req.auto_increment_name = false;
    req.start_immediately = false;
    h.start_job(req).unwrap();
    assert!(h.wait_for(T, |s, _| s.ruida.files.as_ref().is_some_and(|f| f.len() == 1 && f[0].name == "USB")));

    // USB has no jog port: Fire and the controller's own Frame say so at once.
    let e = h.fire(5.0, 0.2).unwrap_err();
    assert!(matches!(&e, MachineError::Unsupported(m) if m.contains("Ethernet")), "{e}");
    assert!(!h.snapshot().fire_on);
    assert!(matches!(h.command(MachineCommand::FrameOnController), Err(MachineError::Unsupported(_))));
    assert!(emu.lock().unwrap().panel_received.is_empty());
    h.disconnect();
}

fn port(name: &str, vid: u16, pid: u16, serial: Option<&str>) -> PortInfo {
    PortInfo { name: name.into(), vid: Some(vid), pid: Some(pid), serial_number: serial.map(str::to_owned), ..Default::default() }
}

#[test]
fn the_usb_link_is_the_serial_port_of_the_controllers_ftdi_chip() {
    let ports = [port("COM3", 0x1A86, 0x7523, None), port("COM7", 0x0403, 0x6001, Some("A50285BI")), port("COM9", 0x0403, 0x6001, Some("B2"))];
    assert_eq!(pick_ftdi_port(&ports, "", &[]), Some("COM7".into()), "the first FTDI port");
    assert_eq!(pick_ftdi_port(&ports, "", &["com7".into()]), Some("COM9".into()), "excluded ports are skipped");
    assert_eq!(pick_ftdi_port(&ports, "b2", &[]), Some("COM9".into()), "by adapter serial number");
    assert_eq!(pick_ftdi_port(&ports, "com3", &[]), Some("COM3".into()), "a typed port is taken as it is");
    assert_eq!(pick_ftdi_port(&ports, "COM44", &[]), None);
    assert_eq!(pick_ftdi_port(&ports[..1], "", &[]), None, "a CH340 is not a Ruida controller");

    let mut d = Device::new(DeviceType::Ruida);
    d.connection = ConnectionKind::Usb;
    assert_eq!(d.link(&ports).unwrap(), Link::Usb { serial: None, port: Some("COM7".into()) });
    d.usb_serial = " B2 ".into();
    let spec = d.connect_spec(&ports).unwrap();
    assert_eq!(spec.link, Link::Usb { serial: Some("B2".into()), port: Some("COM9".into()) });
    assert_eq!(spec.link.label(), "USB COM9");
    assert_eq!(spec.link.family(), Family::Ruida);
    if !cfg!(feature = "usb") {
        d.usb_serial.clear();
        let e = d.link(&ports[..1]).unwrap_err();
        assert!(e.contains("No USB serial port") && e.contains("Rescan"), "{e}");
    }
    // It is a real link: nothing opens it in a test, with or without the user's click.
    let mut h = MachineHandle::new();
    assert!(matches!(h.connect(&spec, None), Err(MachineError::Refused(_))));
    assert!(matches!(h.connect(&spec, Some(RealLinkConsent::from_user_click())), Err(MachineError::Refused(_))));
    // An older saved link without the port still loads.
    let old: Link = serde_json::from_str(r#"{"Usb":{"serial":null}}"#).unwrap();
    assert_eq!(old, Link::Usb { serial: None, port: None });
}

// ----- RUIDA-13, EXP-07, FILE-09, LASER-06: the RD machine file -----------------------------------

fn two_layer_drawing(a: &mut DarkBurn) {
    run(a, "shape.add_rect", json!({ "x": 20, "y": 20, "width": 30, "height": 20, "layer": 0 })).unwrap();
    run(a, "layer.set_param", json!({ "layer": 0, "field": "speed", "value": 30 })).unwrap();
    run(a, "layer.set_param", json!({ "layer": 0, "field": "max_power", "value": 40 })).unwrap();
    run(a, "layer.set_param", json!({ "layer": 0, "field": "min_power", "value": 12.5 })).unwrap();
    run(a, "shape.add_rect", json!({ "x": 70, "y": 30, "width": 10, "height": 10, "layer": 1 })).unwrap();
    run(a, "layer.set_param", json!({ "layer": 1, "field": "speed", "value": 80 })).unwrap();
    run(a, "layer.set_param", json!({ "layer": 1, "field": "max_power", "value": 100 })).unwrap();
}

#[test]
fn save_machine_file_writes_the_rd_job_the_driver_would_send() {
    let mut a = app();
    select(&mut a, DeviceType::Ruida);
    assert_eq!(machine_file_kind(a.machine.active()).label, "Save RD file");
    assert_eq!(machine_file_kind(a.machine.active()).extensions, &["rd"]);
    two_layer_drawing(&mut a);
    let dir = temp("save");
    let file = dir.join("part.rd");
    let r = run(&mut a, "file.save_gcode", json!({ "path": file.to_string_lossy() })).unwrap();
    assert_eq!(r["format"], "rd");
    let bytes = std::fs::read(&file).unwrap();
    assert_eq!(r["bytes"].as_u64().unwrap() as usize, bytes.len());

    // The file is scrambled: plain it starts with D8 10 (bed coordinates) and ends with D7.
    assert_ne!(&bytes[..2], &[0xD8, 0x10]);
    let plain = ruida::swizzle::unswizzle(&bytes, ruida::Magic::DEFAULT);
    assert_eq!(&plain[..2], &[0xD8, 0x10]);
    assert_eq!(plain.last(), Some(&0xD7));

    // It is exactly the stream the driver sends for this job.
    let built = a.job.last_job.clone().expect("saving records the job");
    let direct = toolpath_to_rd_with(&built.toolpath, Vec2::ZERO, RdOptions::of(a.machine.active())).unwrap();
    assert_eq!(bytes, direct.to_rd_file(ruida::Magic::DEFAULT).unwrap());

    let job = ruida::RdJob::from_rd_file(&bytes, None).unwrap();
    assert_eq!(job.reference, ruida::RefPoint::MachineZero);
    assert_eq!(job.layers.len(), 2);
    let s = &job.layers[0].settings;
    assert_eq!(s.speed_mm_s, 30.0);
    assert!((s.max_power[0] - 40.0).abs() < 0.01 && (s.min_power[0] - 12.5).abs() < 0.01, "{s:?}");
    assert_eq!(job.layers.iter().flat_map(|l| &l.ops).filter(|o| matches!(o, ruida::RdOp::Cut { .. })).count(), 8);
    assert_ne!(job.layers[0].settings.color, job.layers[1].settings.color, "layers keep their colours");

    // A controller takes the file: upload it, run it, and the emulator cuts both squares.
    let emu = emulator(Default::default());
    let mut h = MachineHandle::new();
    attach(&mut h, Box::new(ruida::MemoryTransport::udp(emu.clone())), &emu);
    let mut req = JobRequest::new(JobData::RdFile { name: "PART".into(), data: bytes.clone() });
    req.name = "PART".into();
    req.start_immediately = true;
    req.auto_increment_name = false;
    h.start_job(req).unwrap();
    assert!(wait_event(&mut h, |e| matches!(e, MachineEvent::JobFinished(_))), "the stored file ran");
    assert_eq!(cuts(&emu), 8);
    assert_eq!(emu.lock().unwrap().file_sum_mismatches, 0, "the file sum is right");
    h.disconnect();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_saved_rd_file_imports_back_as_the_same_shapes_and_layer_settings() {
    let mut a = app();
    select(&mut a, DeviceType::Ruida);
    two_layer_drawing(&mut a);
    let dir = temp("roundtrip");
    let file = dir.join("part.rd");
    run(&mut a, "job.save_gcode", json!({ "path": file.to_string_lossy() })).unwrap();

    let mut b = app();
    // Imports come in as one group by default; here each shape is looked at on its own.
    b.prefs.import_group = false;
    crate::file_io::open_sync(&mut b, &file).unwrap();
    let shapes = b.editor.doc.shapes.clone();
    assert_eq!(shapes.len(), 2);
    let big = shapes.iter().find(|s| (s.bbox().width() - 30.0).abs() < 0.01).expect("the 30 x 20 rectangle");
    let small = shapes.iter().find(|s| (s.bbox().width() - 10.0).abs() < 0.01).expect("the 10 x 10 square");
    let (bb, sb) = (big.bbox(), small.bbox());
    assert!((bb.height() - 20.0).abs() < 0.01 && (sb.height() - 10.0).abs() < 0.01, "{bb:?} {sb:?}");
    // They keep their places relative to each other: 50 mm apart, tops level.
    assert!((sb.x0 - bb.x0 - 50.0).abs() < 0.01 && (sb.y1 - bb.y1).abs() < 0.01, "{bb:?} {sb:?}");
    assert_ne!(big.layer, small.layer, "one layer each");
    let (l0, l1) = (b.editor.doc.layer(big.layer), b.editor.doc.layer(small.layer));
    assert_eq!((l0.main().speed, l0.main().max_power, l0.main().min_power), (30.0, 40.0, 12.5));
    assert_eq!((l1.main().speed, l1.main().max_power), (80.0, 100.0));

    // And the imported drawing saves to the same cuts again.
    select(&mut b, DeviceType::Ruida);
    let again = dir.join("again.rd");
    run(&mut b, "job.save_gcode", json!({ "path": again.to_string_lossy(), "allow_out_of_bounds": true })).unwrap();
    let first = ruida::RdJob::from_rd_file(&std::fs::read(&file).unwrap(), None).unwrap();
    let second = ruida::RdJob::from_rd_file(&std::fs::read(&again).unwrap(), None).unwrap();
    let lengths = |j: &ruida::RdJob| -> Vec<(u32, u32)> {
        j.layers
            .iter()
            .map(|l| {
                let mut at = darkburn_core::kurbo::Point::ZERO;
                let mut len = 0.0;
                for op in &l.ops {
                    match *op {
                        ruida::RdOp::Move(p) => at = p,
                        ruida::RdOp::Cut { to, .. } => {
                            len += at.distance(to);
                            at = to;
                        }
                        _ => {}
                    }
                }
                ((len * 100.0_f64).round() as u32, (l.settings.speed_mm_s * 1000.0) as u32)
            })
            .collect()
    };
    let (mut x, mut y) = (lengths(&first), lengths(&second));
    x.sort_unstable();
    y.sort_unstable();
    assert_eq!(x, y, "cut length and speed of each layer");
    assert_eq!(x, vec![(4000, 80_000), (10_000, 30_000)]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_job_from_the_current_position_is_saved_as_measured_from_the_head() {
    let mut a = app();
    select(&mut a, DeviceType::Ruida);
    two_layer_drawing(&mut a);
    run(&mut a, "job.set_origin", json!({ "start_from": "current_position" })).unwrap();
    let dir = temp("current");
    let file = dir.join("here.rd");
    run(&mut a, "job.save_gcode", json!({ "path": file.to_string_lossy(), "allow_out_of_bounds": true })).unwrap();
    let bytes = std::fs::read(&file).unwrap();
    assert_eq!(&ruida::swizzle::unswizzle(&bytes, ruida::Magic::DEFAULT)[..2], &[0xD8, 0x12]);
    let job = ruida::RdJob::from_rd_file(&bytes, None).unwrap();
    assert_eq!(job.reference, ruida::RefPoint::CurrentPosition);

    // The controller runs it from where the head stands.
    let emu = emulator(ruida::EmulatorConfig::default());
    emu.lock().unwrap().set_step_mode(false);
    let mut m = ruida::RuidaMachine::new(ruida::MemoryTransport::udp(emu.clone()), ruida::MachineConfig::default());
    m.connect().unwrap();
    m.move_to(darkburn_core::kurbo::Point::new(100.0, 100.0), false).unwrap();
    m.run_job_now(&job).unwrap();
    let e = emu.lock().unwrap();
    let cut: Vec<_> = e.motion.iter().filter(|r| r.kind == MotionKind::Cut).collect();
    assert_eq!(cut.len(), 8);
    let b = job.bounds_mm();
    let min_x = cut.iter().map(|c| c.from[0].min(c.to[0])).min().unwrap();
    assert_eq!(min_x, 100_000 + (b.x0 * 1000.0).round() as i32, "offset by the head position");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_save_button_follows_the_device_and_g_code_devices_still_get_g_code() {
    let mut a = app();
    select(&mut a, DeviceType::SimulatorGrbl);
    assert_eq!(machine_file_kind(a.machine.active()).label, "Save G-code");
    two_layer_drawing(&mut a);
    let dir = temp("gcode");
    let file = dir.join("part.gc");
    let r = run(&mut a, "file.save_gcode", json!({ "path": file.to_string_lossy() })).unwrap();
    assert_eq!(r["format"], "gcode");
    assert!(std::fs::read_to_string(&file).unwrap().contains("G1"));
    select(&mut a, DeviceType::SimulatorRuida);
    assert_eq!(machine_file_kind(a.machine.active()).label, "Save RD file");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_laser_panel_button_saves_an_rd_file_for_a_ruida_device() {
    use crate::ui_harness::Gui;
    let mut g = Gui::standard();
    {
        let mut a = g.app();
        select(&mut a, DeviceType::SimulatorRuida);
    }
    g.exec("shape.add_rect", json!({ "x": 20, "y": 20, "width": 30, "height": 20, "layer": 0 }));
    let file = g.temp("panel.rd");
    g.answer_dialog(&file);
    let area = g.app().prefs.panel("laser").area;
    if g.rect_of("panel:tab:laser").is_some() {
        g.click_id("panel:tab:laser");
    } else {
        g.click_id(&format!("panel:overflow:{area:?}"));
        g.click_label("Laser");
    }
    g.frames(2);
    assert!(g.rect_of("dialog:button::Save G-code").is_none(), "no G-code button for a Ruida device");
    for _ in 0..10 {
        let r = g.rect_of("dialog:button::Save RD file").expect("the Save RD file button");
        if r.bottom() < g.screen().bottom() - 40.0 {
            break;
        }
        let at = g.rect_of("dialog:button::Connect").unwrap().center();
        g.scroll(at, egui::vec2(0.0, -120.0));
    }
    g.click_id("dialog:button::Save RD file");
    let end = Instant::now() + T;
    while !file.exists() {
        g.frames(2);
        assert!(Instant::now() < end, "timed out waiting for the RD file; texts: {:?}", g.visible_texts());
        std::thread::sleep(Duration::from_millis(15));
    }
    let job = ruida::RdJob::from_rd_file(&std::fs::read(&file).unwrap(), None).unwrap();
    assert_eq!(job.layers.len(), 1);
    assert_eq!(job.layers[0].ops.iter().filter(|o| matches!(o, ruida::RdOp::Cut { .. })).count(), 4);
    g.frames(2);
    assert!(g.has_text("RD file saved"), "{:?}", g.visible_texts());
    g.close_dialogs();
    // The File menu entry is named after the device's file too.
    g.open_menu_only("File");
    assert!(g.has_text("Save RD file") && !g.has_text("Save G-code"), "{:?}", g.visible_texts());
    g.escape();
    {
        let mut a = g.app();
        select(&mut a, DeviceType::SimulatorGrbl);
    }
    g.open_menu_only("File");
    assert!(g.has_text("Save G-code"), "{:?}", g.visible_texts());
    g.escape();
}

// ----- RUIDA-12: hardware overscan, corner power, dot mode, PWM override --------------------------

fn x_range(job: &ruida::RdJob) -> (f64, f64) {
    let xs: Vec<f64> = job
        .layers
        .iter()
        .flat_map(|l| &l.ops)
        .filter_map(|o| match o {
            ruida::RdOp::Move(p) | ruida::RdOp::Cut { to: p, .. } => Some(p.x),
            _ => None,
        })
        .collect();
    (xs.iter().copied().fold(f64::MAX, f64::min), xs.iter().copied().fold(f64::MIN, f64::max))
}

fn y_range(job: &ruida::RdJob) -> (f64, f64) {
    let ys: Vec<f64> = job
        .layers
        .iter()
        .flat_map(|l| &l.ops)
        .filter_map(|o| match o {
            ruida::RdOp::Move(p) | ruida::RdOp::Cut { to: p, .. } => Some(p.y),
            _ => None,
        })
        .collect();
    (ys.iter().copied().fold(f64::MAX, f64::min), ys.iter().copied().fold(f64::MIN, f64::max))
}

fn cut_ops(job: &ruida::RdJob) -> Vec<ruida::RdOp> {
    job.layers.iter().flat_map(|l| &l.ops).filter(|o| matches!(o, ruida::RdOp::Cut { .. })).copied().collect()
}

const HARD: RdOptions = RdOptions { hardware_overscan: true, reference: ruida::RefPoint::MachineZero };

#[test]
fn hardware_overscan_leaves_the_overscan_to_the_controller() {
    let mut d = fill_doc(0.5, 20.0);
    d.layer_mut(0).main_mut().overscan = 5.0;
    d.layer_mut(0).main_mut().overscan_percent = false;
    let b = build(d.clone());
    let soft = toolpath_to_rd(&b.toolpath, Vec2::ZERO).unwrap();
    let hard = toolpath_to_rd_with(&b.toolpath, Vec2::ZERO, HARD).unwrap();
    assert_eq!(soft.layers[0].settings.overscan, ruida::OverscanMode::Off);
    assert_eq!(hard.layers[0].settings.overscan, ruida::OverscanMode::HorizontalBidirectional);
    let (s, h) = (x_range(&soft), x_range(&hard));
    assert!((s.0 - 5.0).abs() < 0.01 && (s.1 - 35.0).abs() < 0.01, "DarkBurn's own overscan runs 5 mm past the square: {s:?}");
    assert!((h.0 - 10.0).abs() < 0.01 && (h.1 - 30.0).abs() < 0.01, "with hardware overscan the job stays on the square: {h:?}");
    assert_eq!(cut_ops(&soft), cut_ops(&hard), "the burning moves are the same");
    assert!(hard.layers[0].ops.len() < soft.layers[0].ops.len());

    // One direction only.
    let mut uni = d.clone();
    uni.layer_mut(0).main_mut().bidirectional = false;
    let t = build(uni).toolpath;
    assert_eq!(hardware_overscan_modes(&t), [ruida::OverscanMode::HorizontalUnidirectional]);

    // Lines along Y.
    let mut vertical = d.clone();
    vertical.layer_mut(0).main_mut().scan_angle = 90.0;
    let t = build(vertical).toolpath;
    let v = toolpath_to_rd_with(&t, Vec2::ZERO, HARD).unwrap();
    assert_eq!(v.layers[0].settings.overscan, ruida::OverscanMode::VerticalBidirectional);
    let (y, x) = (y_range(&v), x_range(&v));
    assert!((y.0 - 10.0).abs() < 0.01 && (y.1 - 30.0).abs() < 0.01 && x.0 > 9.9 && x.1 < 30.1, "{x:?} {y:?}");

    // A scan at another angle keeps DarkBurn's overscan: the controller has no mode for it.
    let mut angled = d.clone();
    angled.layer_mut(0).main_mut().scan_angle = 30.0;
    let t = build(angled).toolpath;
    assert_eq!(hardware_overscan_modes(&t), [ruida::OverscanMode::Off]);
    assert_eq!(toolpath_to_rd_with(&t, Vec2::ZERO, HARD).unwrap(), toolpath_to_rd(&t, Vec2::ZERO).unwrap());

    // Line layers and fills without overscan are not touched.
    let t = build(line_doc(20.0)).toolpath;
    assert_eq!(toolpath_to_rd_with(&t, Vec2::ZERO, HARD).unwrap(), toolpath_to_rd(&t, Vec2::ZERO).unwrap());
    let t = build(fill_doc(0.5, 20.0)).toolpath;
    assert_eq!(toolpath_to_rd_with(&t, Vec2::ZERO, HARD).unwrap(), toolpath_to_rd(&t, Vec2::ZERO).unwrap());
}

#[test]
fn hardware_overscan_corner_power_dot_mode_and_pwm_reach_the_controller() {
    let mut a = app();
    let id = select(&mut a, DeviceType::SimulatorRuida);
    a.machine.devices.get_mut(id).unwrap().settings.dsp.hardware_overscan = true;
    assert!(RdOptions::of(a.machine.active()).hardware_overscan);
    // Layer 0: a fill with overscan and a PWM override. Layer 1: a line with corner power.
    // Layer 2: dot mode.
    run(&mut a, "shape.add_rect", json!({ "x": 20, "y": 20, "width": 20, "height": 10, "layer": 0 })).unwrap();
    run(&mut a, "shape.add_rect", json!({ "x": 60, "y": 20, "width": 20, "height": 10, "layer": 1 })).unwrap();
    run(&mut a, "shape.add_rect", json!({ "x": 100, "y": 20, "width": 10, "height": 10, "layer": 2 })).unwrap();
    {
        let s = a.editor.doc.layer_mut(0).main_mut();
        s.mode = darkburn_core::Mode::Fill;
        s.interval = 1.0;
        s.overscan = 5.0;
        s.overscan_percent = false;
        s.pwm_override = true;
        s.pwm_khz = 5.0;
        let s = a.editor.doc.layer_mut(1).main_mut();
        s.mode = darkburn_core::Mode::Line;
        s.max_power = 60.0;
        s.min_power = 15.0;
        let s = a.editor.doc.layer_mut(2).main_mut();
        s.mode = darkburn_core::Mode::Line;
        s.dot_mode = true;
        s.dot_time_ms = 3.0;
        s.dot_spacing = 2.0;
    }
    let emu = emulator(Default::default());
    attach(&mut a.machine.handle, Box::new(ruida::MemoryTransport::udp(emu.clone())), &emu);
    run(&mut a, "job.start", Value::Null).unwrap();
    wait(&mut a, "finish", |a| a.machine.job_log.iter().any(|l| l.starts_with("Job finished")));

    let e = emu.lock().unwrap();
    assert_eq!(e.overscan_modes.len(), 1, "only the fill layer asks for hardware overscan: {:?}", e.overscan_modes);
    assert_eq!(e.overscan_modes[0].1, ruida::OverscanMode::HorizontalBidirectional);
    assert_eq!(e.overscan(), ruida::OverscanMode::Off);
    let has = |c: &ruida::Command| e.received.contains(c);
    // PWM override: C6 60 with the layer's frequency.
    assert!(e.received.iter().any(|c| matches!(c, ruida::Command::FrequencyPart { hz: 5000, .. })), "PWM frequency of the fill layer");
    // Corner power is the layer's minimum power, maximum power its maximum (14-bit, raw / 163.84).
    assert!(has(&ruida::Command::MinPower { laser: ruida::Laser::L1, power: ruida::Power::from_percent(15.0) }));
    assert!(has(&ruida::Command::MaxPower { laser: ruida::Laser::L1, power: ruida::Power::from_percent(60.0) }));
    assert_eq!(ruida::Power::from_percent(15.0).0, 2458);
    // Dot mode: the dot time and spacing of the layer, and one zero-length cut per dot.
    use ruida::command::DelayKind;
    assert!(has(&ruida::Command::Delay { kind: DelayKind::AddDelay, us: 3000 }), "dot time in microseconds");
    assert!(has(&ruida::Command::Delay { kind: DelayKind::LaserInterval, us: 2000 }));
    let dots = e.motion.iter().filter(|m| m.kind == MotionKind::Cut && m.from == m.to).count();
    assert_eq!(dots, 20, "40 mm of outline at 2 mm");
    // The fill never left the rectangle in X: the run-up and run-out are the controller's.
    let fill_x: Vec<i32> =
        e.motion.iter().filter(|m| m.kind == MotionKind::Cut && m.to[0] <= 40_000 + 1).flat_map(|m| [m.from[0], m.to[0]]).collect();
    assert!(fill_x.iter().all(|x| (20_000..=40_000).contains(x)), "{:?}", fill_x.iter().min());
    drop(e);
    a.machine.handle.disconnect();
}

// ----- RUIDA-14: boards ---------------------------------------------------------------------------

#[test]
fn known_boards_are_named_and_unknown_ones_work_as_generic_ruida() {
    let note = crate::devices::known_boards_note();
    for name in ["RDC6442S", "RDC6442G", "RDC8445S", "644", "654", "658", "5121", "320", "420B", "6332G", "KT332N"] {
        assert!(note.contains(name), "{name}: {note}");
    }
    for (card_id, want) in [(0x6510_6510, Some("RDC6442S")), (0x9010_9010, Some("RDC8445S")), (0x1234_5678, None)] {
        let emu = emulator(ruida::EmulatorConfig { card_id, firmware: "RDLC-V8.01.70".into(), ..Default::default() });
        let mut h = MachineHandle::new();
        attach(&mut h, Box::new(ruida::MemoryTransport::udp(emu.clone())), &emu);
        let info = h.snapshot().ruida.info.clone().expect("controller info read on connect");
        assert_eq!(info.card_id, card_id);
        assert_eq!(info.board.as_deref(), want);
        assert_eq!(info.firmware, "RDLC-V8.01.70");
        match want {
            Some(name) => {
                assert!(h.snapshot().firmware.starts_with(name));
                assert_eq!(ruida::settings::board_label(card_id), name);
            }
            None => assert!(ruida::settings::board_label(card_id).contains("0x12345678")),
        }
        // Known or not, it runs a job.
        h.start_job(simulator_square(Family::Ruida, 10.0)).unwrap();
        assert!(wait_event(&mut h, |e| matches!(e, MachineEvent::JobFinished(_))));
        h.disconnect();
    }
}

// ----- TROCEN-01/02, TOPW-01/02: not supported yet, and the app says so ---------------------------

#[test]
fn trocen_and_topwisdom_are_refused_with_a_clear_reason() {
    for (kind, ext) in [(DeviceType::Trocen, ".ud5"), (DeviceType::TopWisdom, ".out")] {
        assert!(!kind.selectable(), "the wizard does not offer {kind:?}");
        assert!(kind.needs_capture().unwrap().starts_with("Not supported yet"));
        let mut a = app();
        select(&mut a, kind);
        two_layer_drawing(&mut a);
        assert!(a.machine.active().validate().iter().any(|m| m.starts_with("Not supported yet")));
        let e = a.machine.active().link(&[]).unwrap_err();
        assert!(e.starts_with("Not supported yet"), "{e}");
        assert!(run(&mut a, "machine.connect", json!({})).is_err());
        assert!(!a.machine.handle.is_connected());
        // No invented machine file either.
        let dir = temp(&format!("{kind:?}"));
        let file = dir.join("job.out");
        let e = run(&mut a, "file.save_gcode", json!({ "path": file.to_string_lossy() })).unwrap_err();
        assert!(e.contains("not supported yet") && e.contains(ext), "{e}");
        assert!(!file.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

// ----- Bug hunt, pass 8: the Fire button holds the controller's Pulse key down ------------------

/// The last thing the controller heard about its Pulse key: down?
fn pulse_is_down(emu: &Emu) -> bool {
    let e = emu.lock().unwrap();
    let last = e
        .panel_received
        .iter()
        .rev()
        .find(|c| matches!(c, ruida::PanelCommand::KeyDown(ruida::PanelKey::Pulse) | ruida::PanelCommand::KeyUp(ruida::PanelKey::Pulse)));
    matches!(last, Some(ruida::PanelCommand::KeyDown(_)))
}

fn firing() -> (MachineHandle, Emu, ruida::UdpEmulatorServer) {
    let emu = emulator(Default::default());
    let server = ruida::UdpEmulatorServer::start(emu.clone()).expect("two loopback sockets");
    let mut h = MachineHandle::new();
    attach(&mut h, Box::new(server.transport().unwrap()), &emu);
    h.fire(5.0, 9.0).unwrap();
    assert!(h.wait_for(T, |_, _| pulse_is_down(&emu)), "the Pulse key went down");
    (h, emu, server)
}

#[test]
fn disconnecting_while_firing_lets_the_pulse_key_go() {
    let (mut h, emu, _server) = firing();
    h.disconnect();
    assert!(!pulse_is_down(&emu), "the controller was left with its Pulse key held down");
}

#[test]
fn stop_while_firing_lets_the_pulse_key_go() {
    let (mut h, emu, _server) = firing();
    h.stop().unwrap();
    let end = Instant::now() + Duration::from_secs(2);
    while pulse_is_down(&emu) && Instant::now() < end {
        h.wait_for(Duration::from_millis(20), |_, _| false);
    }
    assert!(!pulse_is_down(&emu), "Stop left the Pulse key held down");
    assert!(!h.snapshot().fire_on);
}

#[test]
fn a_job_does_not_start_under_a_held_pulse_key() {
    let (mut h, emu, _server) = firing();
    let r = h.start_job(simulator_square(Family::Ruida, 30.0));
    let end = Instant::now() + Duration::from_secs(2);
    while pulse_is_down(&emu) && Instant::now() < end {
        h.wait_for(Duration::from_millis(20), |_, _| false);
    }
    assert!(r.is_err() || !pulse_is_down(&emu), "a job was started with the Pulse key still held down");
}

/// Pass 8, races: every command that moves the head lets the Pulse key go first, as the G-code
/// drivers switch their fire button off before a jog, a move or homing.
#[test]
fn no_move_runs_under_a_held_pulse_key() {
    type Move = (&'static str, fn(&mut MachineHandle) -> Result<(), MachineError>);
    let moves: [Move; 7] = [
        ("jog step", |h| h.jog_step(JogSpec { dx: 5.0, feed: 3000.0, ..Default::default() })),
        ("continuous jog", |h| h.jog_start([1, 0, 0, 0], 3000.0)),
        ("home", |h| h.home()),
        ("go to origin", |h| h.go_to_origin()),
        ("move", |h| h.move_to(MoveSpec { x: Some(20.0), y: Some(20.0), feed: 6000.0, ..Default::default() })),
        ("frame", |h| h.frame_rect(darkburn_core::kurbo::Rect::new(10.0, 10.0, 40.0, 40.0), FrameSpec::default())),
        ("frame outline", |h| {
            let p = darkburn_core::kurbo::Point::new;
            h.frame_outline(vec![p(10.0, 10.0), p(40.0, 10.0), p(40.0, 40.0)], FrameSpec::default())
        }),
    ];
    let mut held = Vec::new();
    for (name, go) in moves {
        let (mut h, emu, _server) = firing();
        let r = go(&mut h);
        let end = Instant::now() + Duration::from_secs(1);
        while pulse_is_down(&emu) && Instant::now() < end {
            h.wait_for(Duration::from_millis(20), |_, _| false);
        }
        if r.is_ok() && (pulse_is_down(&emu) || h.snapshot().fire_on) {
            held.push(name);
        }
        let _ = h.jog_stop();
        h.disconnect();
    }
    assert!(held.is_empty(), "the Pulse key stayed down for: {held:?}");
}

/// Pass 8, races: from the moment a job is asked for until the controller is done with it,
/// nothing else is accepted, also not in the short time before its first status shows the job.
#[test]
fn nothing_else_is_accepted_while_a_job_is_on_its_way_to_the_controller() {
    type Ask = (&'static str, fn(&mut MachineHandle) -> Result<(), MachineError>);
    let asks: [Ask; 8] = [
        ("fire", |h| h.fire(5.0, 5.0)),
        ("jog step", |h| h.jog_step(JogSpec { dx: 5.0, feed: 3000.0, ..Default::default() })),
        ("continuous jog", |h| h.jog_start([1, 0, 0, 0], 3000.0)),
        ("home", |h| h.home()),
        ("go to origin", |h| h.go_to_origin()),
        ("move", |h| h.move_to(MoveSpec { x: Some(20.0), y: Some(20.0), feed: 6000.0, ..Default::default() })),
        ("frame", |h| h.frame_rect(darkburn_core::kurbo::Rect::new(10.0, 10.0, 40.0, 40.0), FrameSpec::default())),
        ("a second start", |h| h.start_job(simulator_square(Family::Ruida, 30.0))),
    ];
    let mut accepted = Vec::new();
    for (name, ask) in asks {
        let emu = emulator(Default::default());
        let server = ruida::UdpEmulatorServer::start(emu.clone()).expect("two loopback sockets");
        let mut h = MachineHandle::new();
        attach(&mut h, Box::new(server.transport().unwrap()), &emu);
        h.start_job(simulator_square(Family::Ruida, 30.0)).unwrap();
        // at once: no status has been read since the start
        if ask(&mut h).is_ok() {
            accepted.push(name);
        }
        assert!(wait_event(&mut h, |e| matches!(e, MachineEvent::JobFinished(_))), "{name}: the job still runs to its end");
        assert!(!pulse_is_down(&emu), "{name}: the Pulse key is down");
        assert_eq!(cuts(&emu), 4, "{name}: one square was cut, once");
        h.disconnect();
    }
    assert!(accepted.is_empty(), "accepted while a job was on its way: {accepted:?}");
}

/// Pass 8, races: "laser on while framing". A frame that was just asked for counts as running
/// before the first status report shows the head moving.
#[test]
fn the_pulse_key_is_not_pressed_on_top_of_a_frame_that_was_just_asked_for() {
    type Ask = (&'static str, fn(&mut MachineHandle) -> Result<(), MachineError>);
    let frames: [Ask; 2] = [
        ("frame", |h| h.frame_rect(darkburn_core::kurbo::Rect::new(10.0, 10.0, 200.0, 200.0), FrameSpec::default())),
        ("frame outline", |h| {
            let p = darkburn_core::kurbo::Point::new;
            h.frame_outline(vec![p(10.0, 10.0), p(200.0, 10.0), p(200.0, 200.0)], FrameSpec::default())
        }),
    ];
    for (name, frame) in frames {
        let emu = emulator(Default::default());
        let server = ruida::UdpEmulatorServer::start(emu.clone()).expect("two loopback sockets");
        let mut h = MachineHandle::new();
        attach(&mut h, Box::new(server.transport().unwrap()), &emu);
        frame(&mut h).unwrap();
        let fire = h.fire(5.0, 5.0);
        let start = h.start_job(simulator_square(Family::Ruida, 30.0));
        h.wait_for(Duration::from_millis(300), |_, _| pulse_is_down(&emu));
        assert!(fire.is_err() && !pulse_is_down(&emu), "{name}: fired on top of the frame ({fire:?})");
        assert!(start.is_err(), "{name}: a job was started on top of the frame");
        h.disconnect();
    }
}
