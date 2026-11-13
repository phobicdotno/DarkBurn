//! Bug hunt 10, the machine side: what lies between a finished drawing and the machine. Every
//! test drives the registered commands (or the machine handle under them) against the in-memory
//! emulators; no port, USB device or network connection is opened.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use darkburn_core::kurbo::{Point, Shape as _};
use darkburn_mcp::Backend;
use darkburn_ruida as ruida;
use serde_json::{Value, json};

use crate::api::execute_headless;
use crate::devices::{Device, DeviceType};
use crate::machine_api::*;
use crate::mcp_backend::{McpBackend, SharedApp};
use crate::prefs::Prefs;
use crate::shell::DarkBurn;

const T: Duration = Duration::from_secs(30);

fn app() -> DarkBurn {
    DarkBurn::new(Prefs::default())
}

fn run(app: &mut DarkBurn, id: &str, args: Value) -> Result<Value, String> {
    execute_headless(app, id, &args)
}

#[track_caller]
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

/// Keep polling for `time`, so that anything late still arrives.
fn settle(app: &mut DarkBurn, time: Duration) {
    let end = Instant::now() + time;
    while Instant::now() < end {
        app.machine.tick();
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn temp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("darkburn-hunt10-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn select(app: &mut DarkBurn, kind: DeviceType) -> u64 {
    let id = match app.machine.devices.devices.iter().find(|d| d.kind == kind) {
        Some(d) => d.id,
        None => run(app, "device.add", json!({ "type": format!("{kind:?}") })).unwrap()["id"].as_u64().unwrap(),
    };
    run(app, "device.select", json!({ "id": id })).unwrap();
    id
}

/// The app with the built-in simulator of `kind` connected and ready.
fn connected(kind: DeviceType) -> DarkBurn {
    let mut a = app();
    select(&mut a, kind);
    run(&mut a, "machine.connect", json!({})).unwrap();
    wait(&mut a, "ready", |a| a.machine.handle.snapshot().state == LaserState::Ready);
    a
}

/// A Ruida job of `cuts` cuts: long enough on the simulator to be stopped half way.
fn long_rd_job(cuts: usize) -> JobRequest {
    let mut ops = vec![ruida::RdOp::Move(Point::new(10.0, 10.0))];
    for i in 0..cuts {
        let x = if i % 2 == 0 { 60.0 } else { 10.0 };
        ops.push(ruida::RdOp::Cut { to: Point::new(x, 10.0 + (i / 2) as f64 * 0.01), power: None, speed: None });
    }
    JobRequest::rd(ruida::RdJob { layers: vec![ruida::RdLayer { settings: ruida::LayerSettings::default(), ops }], ..Default::default() })
}

fn log_has(app: &DarkBurn, start: &str) -> bool {
    app.machine.job_log.iter().any(|l| l.starts_with(start))
}

// ----- The device an agent picked is the device the job is written for ---------------------------

/// Commands that come in over MCP run one after the other with no frame of the window between
/// them (and without a window there never is one). The G-code settings of the device that was
/// just picked must still be the ones the job is written with.
#[test]
fn an_agent_that_picks_another_device_gets_that_devices_g_code_settings() {
    let shared: SharedApp = Rc::new(RefCell::new(app()));
    let mut b = McpBackend::new(shared.clone(), egui::Context::default(), true);
    let id = {
        let mut a = shared.borrow_mut();
        let mut d = Device::new(DeviceType::Grbl);
        d.settings.gcode.s_max = 255.0;
        d.settings.gcode.start_script = "G54".into();
        a.machine.devices.add(d)
    };
    b.execute("shape.add_rect", json!({ "x": 10, "y": 10, "width": 20, "height": 20, "layer": 0 })).unwrap();
    b.execute("layer.set_param", json!({ "layer": 0, "field": "max_power", "value": 100 })).unwrap();
    b.execute("device.select", json!({ "id": id })).unwrap();
    let dir = temp("mcp-device");
    let file = dir.join("job.gc");
    b.execute("job.save_gcode", json!({ "path": file.to_str().unwrap() })).unwrap();
    let text = std::fs::read_to_string(&file).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(text.lines().any(|l| l.trim() == "G54"), "the device's start script is missing:\n{text}");
    assert!(text.contains("S255") && !text.contains("S1000"), "full power is S255 on this device:\n{text}");
}

// ----- Ruida: a stopped job is not a finished job ------------------------------------------------

#[test]
fn an_emergency_stop_on_a_ruida_controller_reports_the_job_as_stopped() {
    let mut a = connected(DeviceType::SimulatorRuida);
    a.machine.handle.start_job(long_rd_job(6000)).unwrap();
    wait(&mut a, "the job to run", |a| log_has(a, "Job started") && a.machine.handle.snapshot().state == LaserState::Busy);
    run(&mut a, "machine.emergency_stop", json!({})).unwrap();
    wait(&mut a, "the stop", |a| a.machine.handle.snapshot().state == LaserState::Ready && a.machine.handle.snapshot().job.is_none());
    settle(&mut a, Duration::from_millis(400));
    assert!(!log_has(&a, "Job finished"), "an emergency stop is not a finished job: {:?}", a.machine.job_log);
    assert!(log_has(&a, "Job stopped"), "the log says the job was stopped: {:?}", a.machine.job_log);
}

#[test]
fn a_controller_file_that_is_not_on_the_list_is_not_run() {
    let mut a = connected(DeviceType::SimulatorRuida);
    run(&mut a, "machine.file_list", json!({})).unwrap();
    wait(&mut a, "the file list", |a| a.machine.handle.snapshot().ruida.files.is_some());
    assert_eq!(a.machine.handle.snapshot().ruida.files.as_ref().map(Vec::len), Some(0), "the simulator starts with no files");
    let r = run(&mut a, "machine.file_run", json!({ "index": 7 }));
    assert!(r.is_err(), "there is no file 7 on the controller, yet it was 'started': {r:?}");
    settle(&mut a, Duration::from_millis(200));
    assert!(!log_has(&a, "Job started"), "{:?}", a.machine.job_log);
}

/// The controller refuses the request to run a stored file (here: it answers every datagram
/// with an error). Nothing ran, so nothing finished.
#[test]
fn a_controller_file_that_could_not_be_started_is_not_reported_as_finished() {
    let emu = Arc::new(Mutex::new(ruida::RuidaEmulator::new(ruida::EmulatorConfig { step_mode: true, ..Default::default() })));
    let transport = ruida::MemoryTransport::udp(emu.clone());
    let faults = transport.faults();
    let cfg = ConnectConfig { sim_time_scale: 50.0, ..Default::default() };
    let adapter = RuidaAdapter::new(Box::new(transport), &cfg, true, Some((emu.clone(), 50.0)));
    let mut h = MachineHandle::new();
    h.attach_simulator(Box::new(adapter), Link::SimulatorRuida);
    assert!(h.wait_for(T, |s, _| s.connected() && s.state == LaserState::Ready));
    let job = match simulator_square(Family::Ruida, 20.0).data {
        JobData::Rd(j) => j,
        _ => unreachable!(),
    };
    emu.lock().unwrap().add_file("A", job.to_rd_file(ruida::Magic::DEFAULT).unwrap());
    h.command(MachineCommand::ListFiles).unwrap();
    assert!(h.wait_for(T, |s, _| s.ruida.files.as_ref().is_some_and(|f| f.len() == 1)));

    faults.lock().unwrap().err_replies = 1_000_000;
    h.command(MachineCommand::RunFile(1)).unwrap();
    let mut events: Vec<MachineEvent> = Vec::new();
    let mut collect = |h: &mut MachineHandle, time: Duration| {
        let end = Instant::now() + time;
        while Instant::now() < end {
            events.extend(h.poll());
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    collect(&mut h, Duration::from_millis(400));
    faults.lock().unwrap().err_replies = 0;
    collect(&mut h, Duration::from_millis(3800));
    assert!(events.iter().any(|e| matches!(e, MachineEvent::Error(_))), "the refusal is reported: {events:?}");
    assert!(!events.iter().any(|e| matches!(e, MachineEvent::JobFinished(_))), "a file that never ran is reported as a finished job");
    assert!(events.iter().any(|e| matches!(e, MachineEvent::JobAborted(_))), "the job that was announced is taken back");
    assert_eq!(h.snapshot().state, LaserState::Ready);
    assert!(h.snapshot().job.is_none());
    // And the controller is free again: the file runs on the next try.
    h.command(MachineCommand::RunFile(1)).unwrap();
    assert!(h.wait_for(T, |_, ev| ev.iter().any(|e| matches!(e, MachineEvent::JobFinished(_)))), "the file runs once the link is good");
}

// ----- Fire and overrides ---------------------------------------------------------------------------

/// The fire limit of Device Settings holds from the moment it is set, not from the next connect.
#[test]
fn fire_keeps_to_the_limit_the_device_has_now() {
    let mut a = connected(DeviceType::SimulatorGrbl);
    a.machine.devices.active_mut().unwrap().settings.fire_max_percent = 2.0;
    let r = run(&mut a, "machine.fire", json!({ "percent": 8.0, "secs": 0.2 }));
    assert!(r.is_err(), "8 % was fired with the device's limit at 2 %: {r:?}");
    settle(&mut a, Duration::from_millis(100));
    assert!(!a.machine.handle.snapshot().fire_on);
    // Inside the limit it still fires.
    run(&mut a, "machine.fire", json!({ "percent": 2.0, "secs": 0.2 })).unwrap();
    wait(&mut a, "the laser on", |a| a.machine.handle.snapshot().fire_on);
    run(&mut a, "machine.fire_off", json!({})).unwrap();
    wait(&mut a, "the laser off", |a| !a.machine.handle.snapshot().fire_on);
}

/// The same limit holds for a frame that is walked with the laser on.
#[test]
fn a_frame_with_the_laser_on_keeps_to_the_limit_the_device_has_now() {
    let mut a = connected(DeviceType::SimulatorGrbl);
    a.machine.devices.active_mut().unwrap().settings.fire_max_percent = 2.0;
    let frame = |pct: f64| json!({ "x": 10, "y": 10, "width": 20, "height": 20, "laser_percent": pct });
    let r = run(&mut a, "machine.frame", frame(8.0));
    assert!(r.is_err(), "the frame was walked at 8 % with the device's limit at 2 %: {r:?}");
    settle(&mut a, Duration::from_millis(100));
    assert_eq!(a.machine.handle.snapshot().state, LaserState::Ready, "nothing moved");
    // Inside the limit the frame is walked.
    run(&mut a, "machine.frame", frame(2.0)).unwrap();
    wait(&mut a, "the frame to begin", |a| a.machine.handle.snapshot().state != LaserState::Ready);
    wait(&mut a, "the frame to end", |a| a.machine.handle.snapshot().state == LaserState::Ready);
}

#[test]
fn an_override_that_is_no_percentage_is_refused_not_dropped() {
    let mut a = connected(DeviceType::SimulatorGrbl);
    for bad in [json!({ "feed": -5 }), json!({ "power": 150.5 }), json!({ "rapid": "fast" }), json!({ "feed": 120, "power": 70000 })] {
        let r = run(&mut a, "machine.overrides", bad.clone());
        assert!(r.is_err(), "{bad} was answered with {r:?} and nothing was set");
    }
    run(&mut a, "machine.overrides", json!({ "feed": 120 })).unwrap();
    wait(&mut a, "the feed override", |a| a.machine.handle.snapshot().overrides.0 == 120);
}

// ----- The automation socket ------------------------------------------------------------------------

/// `START` over the automation socket is the Start button: with "Start here" set it begins
/// there, and does not burn the part before it again.
#[test]
fn the_automation_start_begins_where_start_here_says() {
    let mut a = app();
    select(&mut a, DeviceType::SimulatorGrbl);
    let mut spec = a.machine.active().connect_spec(&[]).unwrap();
    spec.config.sim_time_scale = 400.0;
    let (adapter, emu) = crate::connections::grbl_simulator(&spec.config);
    a.machine.handle.attach_simulator(Box::new(adapter), Link::SimulatorGrbl);
    wait(&mut a, "ready", |a| a.machine.handle.snapshot().state == LaserState::Ready);
    run(&mut a, "shape.add_rect", json!({ "x": 10, "y": 10, "width": 20, "height": 20, "layer": 0 })).unwrap();
    run(&mut a, "shape.add_rect", json!({ "x": 60, "y": 10, "width": 20, "height": 20, "layer": 1 })).unwrap();
    let job = crate::job_cmds::build_now(&a).unwrap();
    let second = job
        .toolpath
        .ops
        .iter()
        .enumerate()
        .filter(|(_, op)| matches!(op, darkburn_job::Op::LayerStart { .. }))
        .nth(1)
        .map(|(i, _)| i)
        .expect("the job has two layers");
    run(&mut a, "job.preview_start_here", json!({ "op_index": second })).unwrap();
    assert_eq!(crate::connections::automation_command(&mut a, "START"), "OK");
    wait(&mut a, "the job to finish", |a| log_has(a, "Job finished"));
    wait(&mut a, "ready again", |a| a.machine.handle.snapshot().state == LaserState::Ready);
    let lefts: Vec<f64> = emu.with(|e| e.stats().burns.iter().map(|b| b.from[0].min(b.to[0])).collect());
    assert!(!lefts.is_empty(), "something was burned");
    assert!(lefts.iter().all(|x| *x > 50.0), "the part before 'Start here' was burned again: {lefts:?}");
}

// ----- `.rd` import ----------------------------------------------------------------------------------

/// A layer of an `.rd` file whose first instruction is a cut goes on from where the layer before
/// it left the head.
#[test]
fn an_rd_layer_that_begins_with_a_cut_starts_where_the_layer_before_ended() {
    let cut = |x: f64, y: f64| ruida::RdOp::Cut { to: Point::new(x, y), power: None, speed: None };
    let job = ruida::RdJob {
        reference: Default::default(),
        layers: vec![
            ruida::RdLayer {
                settings: ruida::LayerSettings { index: 0, color: 0xFF0000, ..Default::default() },
                ops: vec![ruida::RdOp::Move(Point::new(10.0, 10.0)), cut(30.0, 10.0)],
            },
            ruida::RdLayer { settings: ruida::LayerSettings { index: 1, color: 0x0000FF, ..Default::default() }, ops: vec![cut(30.0, 25.0)] },
        ],
    };
    let bytes = job.to_rd_file(ruida::Magic::DEFAULT).unwrap();
    let imp = crate::rd_import::import(&bytes).unwrap();
    assert_eq!(imp.items.len(), 2, "one path per layer");
    let darkburn_import::ItemKind::Path(p) = &imp.items[1].kind else { panic!("a path") };
    let b = p.path.bounding_box();
    assert!(b.width() < 0.01 && (b.height() - 15.0).abs() < 0.01, "the second layer is a 15 mm line straight up from (30, 10), got {b:?}");
}

// ----- Galvo: a framing rule switched on after connecting ------------------------------------------

#[test]
fn a_galvo_job_is_held_when_framing_was_made_a_rule_after_connecting() {
    let mut a = app();
    select(&mut a, DeviceType::SimulatorGalvo);
    let settings = a.machine.active().settings.galvo.clone();
    assert!(!settings.require_framing, "the default device does not ask for a frame");
    let (adapter, emu) = crate::galvo_machine::simulator_with_handle(Arc::new(settings));
    a.machine.handle.attach_simulator(Box::new(adapter), Link::SimulatorGalvo);
    wait(&mut a, "galvo ready", |a| a.machine.handle.snapshot().state == LaserState::Ready);
    run(&mut a, "shape.add_rect", json!({ "x": 40, "y": 40, "width": 20, "height": 20, "layer": 0 })).unwrap();
    run(&mut a, "layer.set_param", json!({ "layer": 0, "field": "speed", "value": 800 })).unwrap();
    // The user switches "require framing" on while the board is connected.
    run(&mut a, "galvo.settings_set", json!({ "values": { "require_framing": true } })).unwrap();
    assert!(a.machine.active().settings.galvo.require_framing);
    run(&mut a, "machine.galvo_arm", json!({ "on": true })).unwrap();
    wait(&mut a, "armed", |a| a.machine.handle.snapshot().galvo.as_ref().is_some_and(|g| g.armed));
    let r = run(&mut a, "machine.start", Value::Null);
    assert!(r.as_ref().is_err_and(|e| e.contains("Frame")), "the job started without the frame the device asks for: {r:?}");
    settle(&mut a, Duration::from_millis(200));
    assert!(!emu.lock().unwrap().log().motions.iter().any(|m| m.mark), "nothing was marked");
    // Framed as it lies, it starts.
    run(&mut a, "machine.galvo_frame", json!({ "repeat": false })).unwrap();
    wait(&mut a, "framed", |a| a.machine.handle.snapshot().galvo.as_ref().is_some_and(|g| g.framed_key.is_some()));
    wait(&mut a, "ready", |a| a.machine.handle.snapshot().state == LaserState::Ready);
    if !a.machine.handle.snapshot().galvo.as_ref().is_some_and(|g| g.armed) {
        run(&mut a, "machine.galvo_arm", json!({ "on": true })).unwrap();
        wait(&mut a, "armed again", |a| a.machine.handle.snapshot().galvo.as_ref().is_some_and(|g| g.armed));
    }
    run(&mut a, "machine.start", Value::Null).unwrap();
    wait(&mut a, "marked", |a| a.machine.handle.snapshot().state == LaserState::Ready && emu.lock().unwrap().log().motions.iter().any(|m| m.mark));
}

// ----- Ruida: a job from the current position needs the position ------------------------------------

/// A controller that is connected and ready but has not said where its head is yet (a Ruida
/// controller is ready as soon as it answers; its first status report comes a moment later).
struct NoPositionYet {
    snap: Snapshot,
    jobs: Arc<Mutex<Vec<JobRequest>>>,
}

impl DriverAdapter for NoPositionYet {
    fn poll(&mut self) -> Vec<MachineEvent> {
        Vec::new()
    }
    fn snapshot(&self) -> Snapshot {
        self.snap.clone()
    }
    fn command(&mut self, cmd: MachineCommand) -> Result<(), MachineError> {
        if let MachineCommand::StartJob(job) = cmd {
            self.jobs.lock().unwrap().push(*job);
        }
        Ok(())
    }
    fn close(&mut self) {}
}

#[test]
fn a_ruida_job_from_the_current_position_waits_for_the_position() {
    let mut a = app();
    select(&mut a, DeviceType::SimulatorRuida);
    let jobs = Arc::new(Mutex::new(Vec::new()));
    let snap = Snapshot {
        family: Family::Ruida,
        simulator: true,
        link: LinkState::Connected,
        state: LaserState::Ready,
        has_position: false,
        bed: Some(a.machine.active().bed()),
        ..Default::default()
    };
    a.machine.handle.attach_simulator(Box::new(NoPositionYet { snap, jobs: jobs.clone() }), Link::SimulatorRuida);
    a.machine.tick();
    run(&mut a, "shape.add_rect", json!({ "x": 100, "y": 100, "width": 20, "height": 20, "layer": 0 })).unwrap();
    run(&mut a, "job.set_origin", json!({ "start_from": "current_position" })).unwrap();
    let r = run(&mut a, "job.start", json!({ "allow_out_of_bounds": true }));
    let sent = jobs.lock().unwrap().len();
    assert!(r.is_err() && sent == 0, "a job 'from where the head is' was sent before the controller said where that is: {r:?}, {sent} job(s) sent");
    // With absolute coordinates the position is not needed.
    run(&mut a, "job.set_origin", json!({ "start_from": "absolute" })).unwrap();
    run(&mut a, "job.start", json!({})).unwrap();
    assert_eq!(jobs.lock().unwrap().len(), 1);
}

// ----- Start-up: the device's own G-code settings are kept -----------------------------------------

/// The frames of the window, in the order the shell runs the two pieces that matter here.
fn window_frames(app: &mut DarkBurn, n: usize) {
    let ctx = egui::Context::default();
    for _ in 0..n {
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            crate::machine_ui::frame(ui.ctx(), app);
            crate::job_ui::frame(ui.ctx(), app);
        });
        out.textures_delta.clear();
    }
}

/// `job.json` in the settings folder still holds G-code settings from whenever it was last
/// written (saving a material test preset writes it, for one). At the next start they must not
/// replace the settings of the device that is active now.
#[test]
fn a_restart_keeps_the_g_code_settings_of_the_active_device() {
    let dir = temp("restart");
    // An earlier session left job.json behind, with the settings of the device of that day.
    let mut old = app();
    old.settings_dir = Some(dir.clone());
    run(&mut old, "job.set_gcode_settings", json!({ "s_max": 1000, "start_script": "G55", "accel": 3000 })).unwrap();
    assert!(dir.join("job.json").is_file(), "the earlier session wrote job.json");
    // The device list: the active device has its own settings.
    let mut store = crate::devices::DeviceStore::default();
    let mut d = Device::new(DeviceType::Grbl);
    d.settings.gcode.s_max = 255.0;
    d.settings.gcode.start_script = "G54".into();
    d.settings.additional.accel = 500.0;
    let id = store.add(d);
    assert!(store.select(id));
    store.save(&dir).unwrap();
    // The next start.
    let mut a = app();
    a.settings_dir = Some(dir.clone());
    window_frames(&mut a, 4);
    let dev = a.machine.active().clone();
    let job = (a.job.gcode.s_max, a.job.gcode.start_script.clone(), a.job.additional.accel);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(dev.id, id, "the device list was read");
    assert_eq!(
        (dev.settings.gcode.s_max, dev.settings.gcode.start_script.as_str(), dev.settings.additional.accel),
        (255.0, "G54", 500.0),
        "the device's settings were replaced by what job.json held"
    );
    assert_eq!(job, (255.0, "G54".to_owned(), 500.0), "and the job is written with the device's settings");
}

// ----- Galvo: kerf ------------------------------------------------------------------------------------

/// Holes are often put on a layer of their own, to cut them first. The kerf offset of a galvo job
/// takes such a hole for what it is, as the jobs of the other machines do.
#[test]
fn a_galvo_hole_on_a_layer_of_its_own_shrinks_by_the_kerf() {
    let mut a = app();
    run(&mut a, "shape.add_rect", json!({ "x": 10, "y": 10, "width": 40, "height": 40, "layer": 0 })).unwrap();
    run(&mut a, "shape.add_rect", json!({ "x": 25, "y": 25, "width": 10, "height": 10, "layer": 1 })).unwrap();
    for li in [0, 1] {
        let s = a.editor.doc.layer_mut(li).main_mut();
        s.mode = darkburn_core::layer::Mode::Line;
        s.kerf = 0.5;
    }
    let dev = Device::new(DeviceType::SimulatorGalvo);
    let widths = |doc: &darkburn_core::Document| -> Vec<f64> {
        let job = crate::galvo_job::build_job(doc, &dev).unwrap().job;
        let mut w: Vec<f64> = job.layers.iter().flat_map(|l| l.geometry.iter().map(|p| p.bounding_box().width())).collect();
        w.sort_by(f64::total_cmp);
        w
    };
    let w = widths(&a.editor.doc);
    assert_eq!(w.len(), 2, "{w:?}");
    assert!((w[1] - 41.0).abs() < 0.05, "the plate grows by the kerf: {w:?}");
    assert!((w[0] - 9.0).abs() < 0.05, "the hole shrinks by the kerf: {w:?}");
    // A plate that is only engraved cuts nothing free: the square is a part, and grows.
    a.editor.doc.layer_mut(0).main_mut().mode = darkburn_core::layer::Mode::Fill;
    let w = widths(&a.editor.doc);
    assert!(w.iter().any(|w| (w - 11.0).abs() < 0.05), "a part grows by the kerf: {w:?}");
}

// ----- Disconnecting ---------------------------------------------------------------------------------

/// A link on which every question takes a moment, as on a real network: the worker is then in
/// the middle of a status question most of the time.
struct SlowLink(ruida::MemoryTransport);

impl ruida::Transport for SlowLink {
    fn framing(&self) -> ruida::Framing {
        self.0.framing()
    }
    fn send(&mut self, channel: ruida::Channel, data: &[u8]) -> ruida::Result<()> {
        if channel == ruida::Channel::Data {
            std::thread::sleep(Duration::from_millis(15));
        }
        self.0.send(channel, data)
    }
    fn recv(&mut self, channel: ruida::Channel, timeout: Duration) -> ruida::Result<Option<Vec<u8>>> {
        self.0.recv(channel, timeout)
    }
    fn max_payload(&self) -> usize {
        self.0.max_payload()
    }
    fn describe(&self) -> String {
        self.0.describe()
    }
}

/// Disconnect lets the Pulse key go and stops the controller before the link is closed, also
/// when the worker is busy asking for the status at that moment.
#[test]
fn disconnecting_a_busy_ruida_link_still_lets_the_pulse_key_go() {
    let emu = Arc::new(Mutex::new(ruida::RuidaEmulator::new(Default::default())));
    let cfg = ConnectConfig { sim_time_scale: 50.0, ..Default::default() };
    let adapter = RuidaAdapter::new(Box::new(SlowLink(ruida::MemoryTransport::udp(emu.clone()))), &cfg, true, Some((emu.clone(), 50.0)));
    let mut h = MachineHandle::new();
    h.attach_simulator(Box::new(adapter), Link::SimulatorRuida);
    assert!(h.wait_for(T, |s, _| s.connected() && s.state == LaserState::Ready), "connects: {:?}", h.snapshot().link);
    let keys = |emu: &Arc<Mutex<ruida::RuidaEmulator>>| emu.lock().unwrap().panel_received.clone();
    h.fire(5.0, 9.0).unwrap();
    assert!(h.wait_for(T, |_, _| keys(&emu).contains(&ruida::PanelCommand::KeyDown(ruida::PanelKey::Pulse))), "the Pulse key went down");
    h.disconnect();
    assert_eq!(keys(&emu).last(), Some(&ruida::PanelCommand::KeyUp(ruida::PanelKey::Pulse)), "the controller was left with its Pulse key held down");
}

// ----- Devices ---------------------------------------------------------------------------------------

/// "Use" in the Devices dialog (or a double click) on the device that is already in use changes
/// nothing: the connection, and the job it is running, stay.
#[test]
fn picking_the_device_that_is_already_in_use_keeps_its_connection() {
    let mut a = app();
    let id = select(&mut a, DeviceType::SimulatorGrbl);
    let mut spec = a.machine.active().connect_spec(&[]).unwrap();
    spec.config.sim_time_scale = 20.0;
    let (adapter, emu) = crate::connections::grbl_simulator(&spec.config);
    a.machine.handle.attach_simulator(Box::new(adapter), Link::SimulatorGrbl);
    wait(&mut a, "ready", |a| a.machine.handle.snapshot().state == LaserState::Ready);
    run(&mut a, "shape.add_rect", json!({ "x": 10, "y": 10, "width": 60, "height": 60, "layer": 0 })).unwrap();
    run(&mut a, "job.start", json!({})).unwrap();
    wait(&mut a, "the job to run", |a| a.machine.handle.snapshot().state == LaserState::Busy && log_has(a, "Job started"));
    run(&mut a, "device.select", json!({ "id": id })).unwrap();
    settle(&mut a, Duration::from_millis(100));
    assert!(a.machine.handle.is_connected(), "picking the same device dropped the connection");
    wait(&mut a, "the job to finish", |a| log_has(a, "Job finished") || log_has(a, "Job stopped"));
    assert!(log_has(&a, "Job finished"), "the job was cut off: {:?}", a.machine.job_log);
    assert_eq!(emu.with(|e| e.stats().burns.len()), 4, "the whole square was cut");
}

// ----- Run from here ----------------------------------------------------------------------------------

/// "Run from here" with an operation past the end of the job is refused, as "Save from here" is,
/// instead of turning on the air, running the scripts and moving the head with nothing to cut.
#[test]
fn running_from_an_operation_past_the_end_of_the_job_is_refused() {
    let mut a = app();
    select(&mut a, DeviceType::SimulatorGrbl);
    let mut spec = a.machine.active().connect_spec(&[]).unwrap();
    spec.config.sim_time_scale = 400.0;
    let (adapter, emu) = crate::connections::grbl_simulator(&spec.config);
    a.machine.handle.attach_simulator(Box::new(adapter), Link::SimulatorGrbl);
    wait(&mut a, "ready", |a| a.machine.handle.snapshot().state == LaserState::Ready);
    run(&mut a, "shape.add_rect", json!({ "x": 10, "y": 10, "width": 20, "height": 20, "layer": 0 })).unwrap();
    let ops = crate::job_cmds::build_now(&a).unwrap().toolpath.ops.len();
    let dir = temp("past-the-end");
    let saved = run(&mut a, "job.preview_save_from_here", json!({ "op_index": ops + 50, "path": dir.join("a.gc").to_str().unwrap() }));
    let _ = std::fs::remove_dir_all(&dir);
    assert!(saved.is_err(), "save from here refuses it: {saved:?}");
    let lines = emu.with(|e| e.stats().lines.len());
    let r = run(&mut a, "job.preview_run_from_here", json!({ "op_index": ops + 50 }));
    settle(&mut a, Duration::from_millis(300));
    assert!(r.is_err(), "run from here past the end of the job was answered with {r:?}");
    assert!(!log_has(&a, "Job started"), "{:?}", a.machine.job_log);
    assert_eq!(emu.with(|e| e.stats().lines.len()), lines, "something was sent: {:?}", emu.with(|e| e.stats().lines.clone()));
    // "Start here" set past the end, then Start: the same.
    run(&mut a, "job.preview_start_here", json!({ "op_index": ops + 50 })).unwrap();
    assert!(run(&mut a, "job.start", json!({})).is_err());
    settle(&mut a, Duration::from_millis(300));
    assert!(!log_has(&a, "Job started"), "{:?}", a.machine.job_log);
}
