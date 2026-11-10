//! Tests of the connection kinds, the GRBL flavours and the pieces around them.
//!
//! Hardware safety: nothing here reaches a machine. The GRBL and Ruida controllers are the
//! in-memory emulators. Where a real socket transport is exercised, the test starts its own
//! listener on 127.0.0.1 with a port the operating system picks, and puts an emulator behind it.
//! No serial port and no USB device is ever opened.

use std::io::{Read as _, Write as _};
use std::net::{TcpListener, TcpStream, UdpSocket};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use darkburn_grbl as grbl;
use darkburn_grbl::Transport as _;
use darkburn_ruida as ruida;
use darkburn_ruida::Transport as _;
use serde_json::{Value, json};

use crate::api::execute_headless;
use crate::connections::{self, BridgeWizard, SimFirmware};
use crate::devices::{ConnectionKind, Device, DeviceType};
use crate::machine_api::*;
use crate::machine_ui::{self, Source};
use crate::prefs::Prefs;
use crate::shell::DarkBurn;

const T: Duration = Duration::from_secs(30);

const GRBL_FLAVOURS: [(SimFirmware, &str); 6] = [
    (SimFirmware::Grbl11h, "GRBL 1.1h"),
    (SimFirmware::Grbl11e, "GRBL 1.1e"),
    (SimFirmware::Grbl09, "GRBL 0.9j"),
    (SimFirmware::GrblLpc, "GRBL 1.1f"),
    (SimFirmware::GrblHal, "grblHAL"),
    (SimFirmware::FluidNc, "FluidNC"),
];

// ----- helpers ------------------------------------------------------------------------------------

fn sim_spec(fw: SimFirmware, scale: f64) -> ConnectSpec {
    let mut s = ConnectSpec::simulator(Family::Grbl);
    s.config.sim_firmware = fw;
    s.config.sim_time_scale = scale;
    s
}

fn ready(h: &mut MachineHandle, what: &str) {
    let ok = h.wait_for(T, |s, _| s.state == LaserState::Ready);
    if !ok {
        for c in h.console() {
            eprintln!("{:?} {}", c.kind, c.text);
        }
    }
    assert!(ok, "{what}: ready, was {:?} {:?} {:?}", h.snapshot().state, h.snapshot().link, h.snapshot().alarm);
}

/// After Stop the controller is reset. A reset while the head still moves leaves GRBL locked in
/// an alarm (position may be lost; GRBL 0.9 always does this), which Unlock clears.
fn ready_after_stop(h: &mut MachineHandle, what: &str) {
    assert!(
        h.wait_for(T, |s, _| matches!(s.state, LaserState::Ready | LaserState::Alarm)),
        "{what}: settles after Stop, was {:?}",
        h.snapshot().state
    );
    if h.snapshot().state == LaserState::Alarm {
        h.command(MachineCommand::Unlock).unwrap();
    }
    ready(h, what);
}

fn wait_event(h: &mut MachineHandle, f: impl Fn(&MachineEvent) -> bool) -> bool {
    h.wait_for(T, |_, ev| ev.iter().any(&f))
}

/// A zigzag of `moves` short cuts, constant power so every flavour takes it.
fn zigzag(moves: usize, step: f64, feed: u32) -> JobRequest {
    let mut lines = vec!["G21 G90".to_owned(), "M3 S100".to_owned()];
    for i in 0..moves {
        lines.push(format!("G1 X{} Y{} F{feed}", 10.0 + (i % 2) as f64 * step, 10.0 + i as f64 * 0.1));
    }
    lines.push("M5 S0".into());
    JobRequest::gcode(lines)
}

/// A handle on the GRBL simulator that keeps the in-memory board reachable.
fn attached(fw: SimFirmware, scale: f64, change: impl FnOnce(&mut ConnectConfig)) -> (MachineHandle, grbl::EmulatorHandle) {
    let mut spec = sim_spec(fw, scale);
    change(&mut spec.config);
    let (adapter, emu) = connections::grbl_simulator(&spec.config);
    let mut h = MachineHandle::new();
    h.attach_simulator(Box::new(adapter), Link::SimulatorGrbl);
    (h, emu)
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
        connections::frame_tick(app);
        if f(app) {
            return;
        }
        assert!(Instant::now() < end, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(3));
    }
}

fn state(app: &DarkBurn) -> LaserState {
    app.machine.handle.snapshot().state
}

fn add(app: &mut DarkBurn, ty: DeviceType) -> u64 {
    let id = run(app, "device.add", json!({ "type": format!("{ty:?}") })).unwrap()["id"].as_u64().unwrap();
    run(app, "device.select", json!({ "id": id })).unwrap();
    id
}

fn select_sim(app: &mut DarkBurn) -> u64 {
    let id = app.machine.devices.devices.iter().find(|d| d.kind == DeviceType::SimulatorGrbl).unwrap().id;
    run(app, "device.select", json!({ "id": id })).unwrap();
    id
}

fn bt_port(name: &str) -> grbl::PortInfo {
    grbl::PortInfo { name: name.into(), bluetooth: true, ..Default::default() }
}

fn usb_port(name: &str) -> grbl::PortInfo {
    grbl::PortInfo { name: name.into(), vid: Some(0x1A86), pid: Some(0x7523), ..Default::default() }
}

// ----- loopback listeners (each test starts its own, on 127.0.0.1, with a port the OS picks) -------

fn timed_out(e: &std::io::Error) -> bool {
    matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut)
}

/// A GRBL emulator behind a TCP listener on this computer. Returns the port.
fn grbl_behind_tcp(emu: grbl::EmulatorHandle) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let Ok((mut s, _)) = listener.accept() else { return };
        s.set_read_timeout(Some(Duration::from_millis(2))).unwrap();
        let mut t = emu.transport();
        let mut buf = [0u8; 512];
        loop {
            match s.read(&mut buf) {
                Ok(0) => return,
                Ok(n) => t.write_all(&buf[..n]).unwrap(),
                Err(e) if timed_out(&e) => {}
                Err(_) => return,
            }
            match t.read(&mut buf, Duration::from_millis(1)) {
                Ok(0) => {}
                Ok(n) => {
                    if s.write_all(&buf[..n]).is_err() {
                        return;
                    }
                }
                Err(_) => return,
            }
        }
    });
    port
}

fn percent_decode(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

/// A GRBL emulator behind a small web server on this computer: `GET <prefix><text>` passes the
/// text to the controller and answers with what the controller printed. Returns the port and
/// the request paths seen.
fn grbl_behind_http(emu: grbl::EmulatorHandle, prefix: &'static str) -> (u16, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen2 = seen.clone();
    let link = Arc::new(Mutex::new(emu.transport()));
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(s) = conn else { return };
            let (link, seen) = (link.clone(), seen2.clone());
            std::thread::spawn(move || serve_http(s, &link, &seen, prefix));
        }
    });
    (port, seen)
}

fn serve_http(mut s: TcpStream, link: &Mutex<grbl::MemoryTransport>, seen: &Mutex<Vec<String>>, prefix: &str) {
    let mut pending = Vec::new();
    let mut buf = [0u8; 1024];
    loop {
        // One request: everything up to the blank line.
        let head = loop {
            if let Some(end) = pending.windows(4).position(|w| w == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&pending[..end]).into_owned();
                pending.drain(..end + 4);
                break head;
            }
            match s.read(&mut buf) {
                Ok(0) | Err(_) => return,
                Ok(n) => pending.extend_from_slice(&buf[..n]),
            }
        };
        let path = head.lines().next().and_then(|l| l.split_whitespace().nth(1)).unwrap_or_default().to_owned();
        seen.lock().unwrap().push(path.clone());
        let (status, body) = match path.strip_prefix(prefix) {
            None => ("404 Not Found", Vec::new()),
            Some(text) => {
                let bytes = percent_decode(text);
                let mut t = link.lock().unwrap();
                if bytes.len() == 1 && grbl::protocol::rt::is_realtime(bytes[0]) {
                    t.write_all(&bytes).unwrap();
                } else {
                    t.write_all(&bytes).unwrap();
                    t.write_all(b"\n").unwrap();
                }
                // What the controller printed in the next few milliseconds is the answer.
                let mut body = Vec::new();
                let mut quiet = 0;
                while quiet < 3 {
                    match t.read(&mut buf, Duration::from_millis(2)) {
                        Ok(0) | Err(_) => quiet += 1,
                        Ok(n) => {
                            quiet = 0;
                            body.extend_from_slice(&buf[..n]);
                        }
                    }
                }
                ("200 OK", body)
            }
        };
        let head = format!("HTTP/1.1 {status}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n", body.len());
        if s.write_all(head.as_bytes()).and_then(|_| s.write_all(&body)).is_err() {
            return;
        }
    }
}

/// A Ruida emulator behind a TCP relay on this computer, framed like a network bridge. Returns
/// the port.
fn ruida_behind_bridge() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let Ok((mut s, _)) = listener.accept() else { return };
        s.set_read_timeout(Some(Duration::from_millis(2))).unwrap();
        let emu = Arc::new(Mutex::new(ruida::RuidaEmulator::new(ruida::EmulatorConfig::default())));
        let mut udp = ruida::MemoryTransport::udp(emu);
        let mut rx = Vec::new();
        let mut buf = [0u8; 2048];
        loop {
            match s.read(&mut buf) {
                Ok(0) => return,
                Ok(n) => rx.extend_from_slice(&buf[..n]),
                Err(e) if timed_out(&e) => {}
                Err(_) => return,
            }
            while let Some(packet) = connections::bridge_unframe(&mut rx) {
                udp.send(ruida::transport::Channel::Data, &packet).unwrap();
            }
            while let Ok(Some(reply)) = udp.recv(ruida::transport::Channel::Data, Duration::from_millis(1)) {
                if s.write_all(&connections::bridge_frame(&reply)).is_err() {
                    return;
                }
            }
        }
    });
    port
}

fn loopback(link: Link) -> MachineHandle {
    let mut h = MachineHandle::new();
    let spec = ConnectSpec { link, config: ConnectConfig::default() };
    assert!(matches!(h.connect(&spec, None), Err(MachineError::Refused(_))), "a real link always needs the user's click");
    h.connect(&spec, Some(RealLinkConsent::from_user_click())).unwrap();
    h
}

// ----- GRBL flavours through the machine API [GRBL-01..GRBL-05] -----------------------------------

#[test]
fn every_grbl_flavour_connects_and_runs_a_job_with_progress() {
    for (fw, name) in GRBL_FLAVOURS {
        let mut h = MachineHandle::new();
        h.connect(&sim_spec(fw, 20.0), None).unwrap();
        assert!(h.wait_for(T, |s, _| s.connected()), "{fw:?} connects");
        assert!(h.is_simulator() && !h.is_real_machine());
        assert!(h.snapshot().firmware.contains(name), "{fw:?} says {}", h.snapshot().firmware);
        ready(&mut h, name);
        let moves = 40;
        h.start_job(zigzag(moves, 5.0, 3000)).unwrap();
        let (mut started, mut mid, mut finished) = (false, false, None);
        let done = h.wait_for(T, |s, ev| {
            started |= ev.iter().any(|e| matches!(e, MachineEvent::JobStarted));
            if let Some(j) = &s.job {
                assert_eq!(j.total_lines, Some(moves + 3), "{fw:?}");
                mid |= j.lines_done > 0 && j.lines_done < moves + 3 && j.fraction.is_some_and(|f| f > 0.0 && f < 1.0);
            }
            if let Some(MachineEvent::JobFinished(p)) = ev.iter().find(|e| matches!(e, MachineEvent::JobFinished(_))) {
                finished = Some(p.clone());
            }
            finished.is_some()
        });
        assert!(done && started, "{fw:?}: the job runs to the end");
        assert!(mid, "{fw:?}: progress was reported while the job ran");
        let p = finished.unwrap();
        assert_eq!((p.lines_done, p.errors), (moves + 3, 0), "{fw:?}");
        ready(&mut h, name);
        assert!(!h.snapshot().fire_on);
    }
}

#[test]
fn every_grbl_flavour_pauses_resumes_stops_and_reports_an_alarm() {
    for (fw, name) in GRBL_FLAVOURS {
        let (mut h, emu) = attached(fw, 3.0, |_| {});
        ready(&mut h, name);
        h.start_job(zigzag(60, 20.0, 600)).unwrap();
        assert!(h.wait_for(T, |s, _| s.state == LaserState::Busy), "{fw:?} busy");
        h.pause().unwrap();
        assert!(h.wait_for(T, |s, _| s.state == LaserState::Paused), "{fw:?} paused, was {:?}", h.snapshot().state);
        // "Paused" is shown from the moment the hold is asked for; the machine still slows down.
        assert!(h.wait_for(T, |_, _| emu.with(|e| e.state_name() == "Hold:0")), "{fw:?} comes to a stop");
        let held = emu.with(|e| e.machine_position());
        h.wait_for(Duration::from_millis(150), |_, _| false);
        assert_eq!(emu.with(|e| e.machine_position()), held, "{fw:?}: nothing moves while paused");
        h.resume().unwrap();
        assert!(h.wait_for(T, |s, _| s.state == LaserState::Busy), "{fw:?} runs again");
        h.wait_for(Duration::from_millis(300), |_, _| false);
        h.stop().unwrap();
        assert!(wait_event(&mut h, |e| matches!(e, MachineEvent::Stopped)), "{fw:?} stops");
        ready_after_stop(&mut h, name);
        assert!(!emu.with(|e| e.laser_on()), "{fw:?}: the laser is off after Stop");

        // A limit switch trips during the next job: the job ends and the alarm is shown.
        h.start_job(zigzag(60, 20.0, 600)).unwrap();
        assert!(h.wait_for(T, |s, _| s.state == LaserState::Busy), "{fw:?} busy again");
        emu.with(|e| e.trigger_hard_limit('x'));
        let mut alarm = None;
        assert!(
            h.wait_for(T, |s, ev| {
                if let Some(MachineEvent::Alarm(m)) = ev.iter().find(|e| matches!(e, MachineEvent::Alarm(_))) {
                    alarm = Some(m.clone());
                }
                alarm.is_some() && s.state == LaserState::Alarm
            }),
            "{fw:?}: alarm, was {:?}",
            h.snapshot().state
        );
        assert!(alarm.unwrap().to_lowercase().contains("limit"), "{fw:?}: the alarm is in plain words");
        assert!(h.snapshot().alarm.is_some());
        let refused = h.start_job(zigzag(2, 1.0, 600));
        assert!(matches!(refused, Err(MachineError::Alarm | MachineError::Busy)), "{fw:?}: no job while in alarm, got {refused:?}");
        assert!(!emu.with(|e| e.laser_on()));
        emu.with(|e| e.release_limit_switch());
        // The alarm is reported at once; the driver then resets the controller, and until that
        // is done it answers "busy". Unlock goes out when the driver takes it, not after a pause.
        let end = Instant::now() + T;
        loop {
            match h.command(MachineCommand::Unlock) {
                Ok(()) => break,
                Err(MachineError::Busy) if Instant::now() < end => {
                    h.wait_for(Duration::from_millis(5), |_, _| false);
                }
                Err(e) => panic!("{fw:?}: Unlock after the limit alarm: {e:?}"),
            }
        }
        ready(&mut h, name);
    }
}

#[test]
fn streaming_tracks_the_controller_buffer_of_each_flavour() {
    // (firmware, bytes the controller's receive buffer can hold)
    let cases = [
        (SimFirmware::Grbl11h, 127),
        (SimFirmware::Grbl09, 127),
        (SimFirmware::FluidNc, 127),
        (SimFirmware::GrblHal, 1023),
        (SimFirmware::GrblLpc, 8191),
    ];
    for (fw, capacity) in cases {
        let (mut h, emu) = attached(fw, 4.0, |_| {});
        ready(&mut h, fw.label());
        let moves = 400;
        h.start_job(zigzag(moves, 2.0, 6000)).unwrap();
        assert!(wait_event(&mut h, |e| matches!(e, MachineEvent::JobFinished(_))), "{fw:?} finishes");
        let (overflow, peak, lines) = emu.with(|e| (e.stats().rx_overflow, e.stats().rx_peak, e.stats().lines_processed));
        assert_eq!(overflow, 0, "{fw:?}: the controller's buffer never overflows");
        assert!(lines >= moves + 3, "{fw:?}: every line arrived ({lines})");
        assert!(peak <= capacity, "{fw:?}: at most {capacity} bytes waiting, saw {peak}");
        assert!(peak > 60, "{fw:?}: several lines are sent ahead, saw {peak}");
        if capacity > 127 {
            assert!(peak > 127, "{fw:?}: the bigger buffer is used, saw {peak}");
        }
    }
    // Synchronous transfer sends one line and waits.
    let (mut h, emu) = attached(SimFirmware::Grbl11h, 4.0, |c| c.synchronous = true);
    ready(&mut h, "synchronous");
    h.start_job(zigzag(100, 2.0, 6000)).unwrap();
    assert!(wait_event(&mut h, |e| matches!(e, MachineEvent::JobFinished(_))));
    let peak = emu.with(|e| e.stats().rx_peak);
    assert!(peak < 60, "one line at a time, saw {peak}");
}

#[test]
fn older_grbl_gets_constant_power_jobs_from_the_app() {
    for (fw, m3_only) in [(SimFirmware::Grbl11e, true), (SimFirmware::Grbl09, true), (SimFirmware::Grbl11h, false)] {
        let mut a = app();
        select_sim(&mut a);
        run(&mut a, "device.set_connection", json!({ "sim_firmware": format!("{fw:?}") })).unwrap();
        machine_ui::sync_job_state(&mut a);
        assert_eq!(a.job.device.m3_only, m3_only, "{fw:?}");
        run(&mut a, "machine.connect", json!({})).unwrap();
        wait(&mut a, "ready", |a| state(a) == LaserState::Ready);
        run(&mut a, "machine.show_all", json!({ "on": true })).unwrap();
        run(&mut a, "shape.add_rect", json!({ "x": 10, "y": 10, "width": 20, "height": 20, "layer": 0 })).unwrap();
        run(&mut a, "machine.start", Value::Null).unwrap();
        wait(&mut a, "job finish", |a| a.machine.job_log.iter().any(|l| l.starts_with("Job finished")));
        let sent: Vec<String> = a.machine.handle.console().iter().filter(|c| c.kind == ConsoleKind::Sent).map(|c| c.text.clone()).collect();
        let has = |word: &str| sent.iter().any(|l| l.split_whitespace().any(|w| w == word));
        assert!(has("M3") || has("M4"), "{fw:?}: the job was echoed: {sent:?}");
        assert_eq!(has("M4"), !m3_only, "{fw:?}: dynamic power only where the firmware has it: {sent:?}");
        assert_eq!(has("M3"), m3_only, "{fw:?}: {sent:?}");
    }
    // The real device types: GRBL-M3 always writes M3, and plain GRBL says so when an old board answers.
    assert!(Device::new(DeviceType::GrblM3).job_device().m3_only);
    assert!(!Device::new(DeviceType::Grbl).job_device().m3_only);
    assert!(crate::gcode_machines::mismatch_hint("GRBL 1.1e", DeviceType::Grbl).contains("GRBL-M3"));
    assert!(crate::gcode_machines::mismatch_hint("GRBL 0.9j", DeviceType::Grbl).contains("GRBL-M3"));
    assert_eq!(crate::gcode_machines::mismatch_hint("GRBL 1.1h", DeviceType::Grbl), "");
    assert_eq!(crate::gcode_machines::mismatch_hint("GRBL 1.1e", DeviceType::GrblM3), "");
    assert_eq!(crate::gcode_machines::mismatch_hint("grblHAL 1.1f", DeviceType::GrblHal), "");
}

#[test]
fn the_grbl_types_and_their_connections_can_be_picked() {
    let labels: Vec<&str> = DeviceType::ALL.iter().map(|t| t.label()).collect();
    for want in ["GRBL", "GRBL-M3 (1.1e or older)", "GRBL-LPC (Smoothieboard)", "grblHAL", "FluidNC"] {
        assert!(labels.contains(&want), "{want}");
    }
    for t in [DeviceType::Grbl, DeviceType::GrblM3, DeviceType::GrblLpc, DeviceType::GrblHal, DeviceType::FluidNc] {
        assert!(t.selectable() && t.family() == Family::Grbl && !t.is_simulator());
        assert!(t.connections().contains(&ConnectionKind::Serial), "{t:?}");
        let mut a = app();
        let id = add(&mut a, t);
        assert_eq!(a.machine.devices.get(id).unwrap().kind, t);
        // Nothing connects without the user's confirmation, and never in a test build.
        a.machine.ports = vec![usb_port("COM7")];
        a.machine.ports_scanned = true;
        assert!(run(&mut a, "machine.connect", json!({})).unwrap_err().contains("only connect to a simulator"), "{t:?}: never from an agent");
        a.machine.request_connect().unwrap();
        assert!(a.machine.dlg.confirm_connect.is_some(), "{t:?}: Connect asks first");
        assert!(!a.machine.handle.is_connected());
        a.machine.confirm_connect();
        assert!(!a.machine.handle.is_connected() && !a.machine.handle.is_real_machine(), "{t:?}: refused in a test build");
    }
    assert!(DeviceType::FluidNc.connections().contains(&ConnectionKind::Http));
    assert!(DeviceType::GrblLpc.connections().contains(&ConnectionKind::RawUsb));
    assert!(DeviceType::Ruida.connections().contains(&ConnectionKind::Bridge));
}

// ----- connection kinds [CONN-01, CONN-05, CONN-08] ------------------------------------------------

#[test]
fn serial_baud_bluetooth_and_raw_usb_are_device_settings() {
    let mut a = app();
    let id = add(&mut a, DeviceType::Grbl);
    a.machine.ports = vec![grbl::PortInfo { name: "COM1".into(), ..Default::default() }, usb_port("COM7"), bt_port("COM12")];
    a.machine.ports_scanned = true;

    // USB serial with a baud rate [CONN-01].
    let r = run(&mut a, "device.set_connection", json!({ "connection": "Serial", "baud": 57_600 })).unwrap();
    assert_eq!(r["link"], "COM7 @ 57600");
    let d = a.machine.devices.get(id).unwrap().clone();
    assert_eq!(d.link(&a.machine.ports), Ok(Link::Serial { port: "COM7".into(), baud: 57_600 }));
    assert!(run(&mut a, "device.set_connection", json!({ "baud": 0 })).unwrap_err().contains("baud"));

    // Bluetooth is a serial port to the operating system [CONN-08].
    let r = run(&mut a, "device.set_connection", json!({ "connection": "Bluetooth", "baud": 115_200 })).unwrap();
    assert_eq!(r["link"], "COM12 @ 115200");
    assert_eq!(bt_port("COM12").label(), "COM12 (Bluetooth)");
    let d = a.machine.devices.get(id).unwrap().clone();
    assert!(d.link(&[usb_port("COM7")]).unwrap_err().contains("Bluetooth"), "no Bluetooth port: say so");
    run(&mut a, "device.set_connection", json!({ "serial_port": "COM1" })).unwrap();
    assert_eq!(a.machine.devices.get(id).unwrap().link(&a.machine.ports), Ok(Link::Serial { port: "COM1".into(), baud: 115_200 }));

    // Raw USB [CONN-05].
    assert!(run(&mut a, "device.set_connection", json!({ "connection": "RawUsb" })).unwrap_err().contains("USB vendor"));
    let r = run(
        &mut a,
        "device.set_connection",
        json!({ "connection": "RawUsb", "usb_vid": "1a86", "usb_pid": "0x7523", "usb_interface": 1, "usb_ep_out": 2, "usb_ep_in": 3 }),
    )
    .unwrap();
    assert_eq!(r["link"], "USB 1A86:7523 interface 1");
    let d = a.machine.devices.get(id).unwrap().clone();
    assert_eq!(d.link(&[]), Ok(Link::RawUsb { vid: 0x1A86, pid: 0x7523, serial: None, interface: 1, ep_out: 0x02, ep_in: 0x83 }));
    // Opening it needs the user's click, and a test build never opens it at all.
    let spec = d.connect_spec(&[]).unwrap();
    let mut h = MachineHandle::new();
    assert!(matches!(h.connect(&spec, None), Err(MachineError::Refused(_))));
    assert!(matches!(h.connect(&spec, Some(RealLinkConsent::from_user_click())), Err(MachineError::Refused(_))));

    // A type only takes the connections it offers.
    let marlin = add(&mut a, DeviceType::Marlin);
    assert!(run(&mut a, "device.set_connection", json!({ "id": marlin, "connection": "Http" })).unwrap_err().contains("USB serial"));
    assert!(run(&mut a, "device.set_connection", json!({ "connection": "Nonsense" })).is_err());

    // The settings survive a save and load.
    let text = serde_json::to_string(&a.machine.devices).unwrap();
    let back: crate::devices::DeviceStore = serde_json::from_str(&text).unwrap();
    assert_eq!(back.get(id).unwrap().settings.conn, d.settings.conn);
    assert_eq!(back.get(id).unwrap().connection, ConnectionKind::RawUsb);
    // A device list written before these settings existed still loads.
    let old: crate::connections::ConnSettings = serde_json::from_str("{}").unwrap();
    assert_eq!(old, crate::connections::ConnSettings::default());
}

#[test]
fn find_my_laser_lists_ports_and_usb_devices_without_opening_any() {
    let mut a = app();
    let r = run(&mut a, "device.find", Value::Null).unwrap();
    assert!(r["ports"].is_array() && r["usb"].is_array());
    assert!(r["note"].as_str().unwrap().contains("nothing was opened"));
    assert!(!a.machine.handle.is_connected());
    let usb =
        grbl::UsbDeviceInfo { vid: 0x1A86, pid: 0x7523, product: Some("USB Serial".into()), serial_number: Some("A1".into()), ..Default::default() };
    let w = connections::wizard_from_usb(&usb);
    assert_eq!(w.draft.connection, ConnectionKind::RawUsb);
    assert_eq!((w.draft.settings.conn.usb_vid.as_str(), w.draft.settings.conn.usb_pid.as_str(), w.draft.usb_serial.as_str()), ("1A86", "7523", "A1"));
    assert!(w.problems().is_empty(), "{:?}", w.problems());
    assert!(!crate::commands::describe("device.find").unwrap().hardware);
    assert!(!crate::commands::describe("device.set_connection").unwrap().hardware);
    assert!(crate::commands::describe("machine.bridge_rescan").unwrap().hardware, "the rescan opens a connection");
}

// ----- DTR [GRBL-11] ----------------------------------------------------------------------------

#[test]
fn dtr_pulse_and_level_come_from_the_device_settings() {
    // Device settings reach the connection.
    let mut a = app();
    let id = add(&mut a, DeviceType::Grbl);
    let spec = a.machine.devices.get(id).unwrap().connect_spec(&[usb_port("COM7")]).unwrap();
    assert!(!spec.config.toggle_dtr && spec.config.dtr, "by default DTR is on and not pulsed");
    run(&mut a, "device.set_connection", json!({ "pulse_dtr": true, "dtr_enabled": false })).unwrap();
    let spec = a.machine.devices.get(id).unwrap().connect_spec(&[usb_port("COM7")]).unwrap();
    assert!(spec.config.toggle_dtr && !spec.config.dtr);

    // And the driver does it: counted on the in-memory board.
    let resets = |change: fn(&mut ConnectConfig)| {
        let (mut h, emu) = attached(SimFirmware::Grbl11h, 100.0, change);
        ready(&mut h, "dtr");
        emu.with(|e| (e.stats().power_on_resets, e.dtr()))
    };
    let (plain, level) = resets(|_| {});
    assert!(level, "DTR is held on");
    let (pulsed, level) = resets(|c| c.toggle_dtr = true);
    assert_eq!(pulsed, plain + 1, "the pulse resets the board once more");
    assert!(level);
    let (low, level) = resets(|c| c.dtr = false);
    assert_eq!(low, plain, "holding DTR off does not reset the board");
    assert!(!level, "DTR is held off");
    let (both, level) = resets(|c| {
        c.toggle_dtr = true;
        c.dtr = false;
    });
    assert_eq!(both, plain + 1);
    assert!(!level, "pulsed, then left off");
}

// ----- TCP/IP [CONN-02] -----------------------------------------------------------------------------

#[test]
fn tcp_runs_a_job_against_a_loopback_listener() {
    let emu = grbl::EmulatorHandle::new(grbl::GrblEmulator::new(grbl::EmulatorConfig::atomstack().with_time_scale(4.0)));
    let port = grbl_behind_tcp(emu.clone());
    let mut h = loopback(Link::Tcp { host: "127.0.0.1".into(), port });
    assert!(h.wait_for(T, |s, _| s.connected()), "connects over TCP: {:?}", h.snapshot().link);
    assert!(h.is_real_machine() && !h.is_simulator(), "a TCP link is treated as a real machine");
    assert!(h.snapshot().port.starts_with("tcp://127.0.0.1:"), "{}", h.snapshot().port);
    assert!(h.snapshot().firmware.contains("GRBL 1.1h"));
    ready(&mut h, "tcp");
    h.start_job(zigzag(60, 20.0, 600)).unwrap();
    assert!(h.wait_for(T, |s, _| s.state == LaserState::Busy && s.job.as_ref().is_some_and(|j| j.lines_done > 0)));
    h.pause().unwrap();
    assert!(h.wait_for(T, |s, _| s.state == LaserState::Paused));
    h.resume().unwrap();
    assert!(h.wait_for(T, |s, _| s.state == LaserState::Busy));
    h.stop().unwrap();
    assert!(wait_event(&mut h, |e| matches!(e, MachineEvent::Stopped)));
    ready_after_stop(&mut h, "tcp after stop");
    h.start_job(zigzag(30, 2.0, 6000)).unwrap();
    assert!(wait_event(&mut h, |e| matches!(e, MachineEvent::JobFinished(_))));
    assert_eq!(emu.with(|e| e.stats().rx_overflow), 0);
    h.disconnect();
    assert!(!h.is_connected());
}

#[test]
fn only_loopback_addresses_open_in_a_test_build() {
    let mut h = MachineHandle::new();
    for link in [
        Link::Tcp { host: "192.168.1.50".into(), port: 23 },
        Link::Tcp { host: "localhost".into(), port: 23 },
        Link::Tcp { host: "fluidnc.local".into(), port: 23 },
        Link::Http { host: "10.0.0.7".into(), port: 80, path: "/command?plain=".into() },
        Link::Bridge { host: "10.0.3.3".into(), port: 5005 },
        Link::Serial { port: "COM3".into(), baud: 115_200 },
        Link::Udp { ip: "127.0.0.1".into() },
        Link::Usb { serial: None, port: None },
        Link::RawUsb { vid: 0x1A86, pid: 0x7523, serial: None, interface: 0, ep_out: 2, ep_in: 0x82 },
    ] {
        let spec = ConnectSpec { link: link.clone(), config: ConnectConfig::default() };
        assert!(matches!(h.connect(&spec, Some(RealLinkConsent::from_user_click())), Err(MachineError::Refused(_))), "{link:?}");
        assert!(!connections::loopback_only(&link), "{link:?}");
    }
    assert!(connections::is_loopback_host("127.0.0.1") && connections::is_loopback_host(" ::1 "));
    assert!(!connections::is_loopback_host("localhost") && !connections::is_loopback_host("127.0.0.1.example.com"));
    assert!(connections::probe_bridge("192.168.1.50", 5005, Source::Ui).unwrap_err().contains("disabled in test builds"));
}

// ----- HTTP [CONN-07] ---------------------------------------------------------------------------------

#[test]
fn http_runs_a_job_against_a_loopback_web_server() {
    let emu = grbl::EmulatorHandle::new(grbl::GrblEmulator::new(grbl::EmulatorConfig::atomstack().with_time_scale(20.0)));
    let (port, seen) = grbl_behind_http(emu.clone(), "/command?plain=");
    let mut h = loopback(Link::Http { host: "127.0.0.1".into(), port, path: "/command?plain=".into() });
    assert!(h.wait_for(T, |s, _| s.connected()), "connects over HTTP: {:?}", h.snapshot().link);
    assert!(h.snapshot().firmware.contains("GRBL 1.1h"), "{}", h.snapshot().firmware);
    assert!(h.snapshot().port.starts_with("http://127.0.0.1:"));
    ready(&mut h, "http");
    h.start_job(zigzag(12, 5.0, 6000)).unwrap();
    let finished = wait_event(&mut h, |e| matches!(e, MachineEvent::JobFinished(_)));
    if !finished {
        for c in h.console() {
            eprintln!("{:?} {}", c.kind, c.text);
        }
        eprintln!("requests seen: {}", seen.lock().unwrap().len());
    }
    assert!(finished, "the job finishes over HTTP: {:?} {:?} {:?}", h.snapshot().state, h.snapshot().link, h.snapshot().job);
    ready(&mut h, "http after the job");
    let seen = seen.lock().unwrap().clone();
    assert!(seen.iter().all(|p| p.starts_with("/command?plain=")), "{seen:?}");
    assert!(seen.iter().any(|p| p == "/command?plain=%3F"), "status requests are sent as their own request");
    assert!(seen.iter().any(|p| p.contains("G1%20X15")), "lines are URL-encoded: {seen:?}");
    let processed = emu.with(|e| e.stats().lines.clone());
    assert!(processed.iter().any(|l| l.starts_with("G1 X15")), "the controller got the line as typed: {processed:?}");
    assert_eq!(emu.with(|e| e.stats().rx_overflow), 0);
}

#[test]
fn http_reports_a_server_that_refuses() {
    // A listener that answers 404 to everything.
    let emu = grbl::EmulatorHandle::new(grbl::GrblEmulator::new(grbl::EmulatorConfig::atomstack()));
    let (port, _) = grbl_behind_http(emu, "/somewhere-else?x=");
    let mut h = loopback(Link::Http { host: "127.0.0.1".into(), port, path: "/command?plain=".into() });
    let mut why = None;
    assert!(h.wait_for(T, |_, ev| {
        for e in ev {
            if let MachineEvent::ConnectFailed(m) | MachineEvent::Disconnected(Some(m)) = e {
                why = Some(m.clone());
            }
        }
        why.is_some()
    }));
    assert!(why.unwrap().contains("404"));
    assert!(!h.is_connected());
}

// ----- network bridge [CONN-03, DEV-06, LASER-20] ------------------------------------------------------

#[test]
fn bridge_frames_round_trip() {
    let f = connections::bridge_frame(&[1, 2, 3]);
    assert_eq!(f, vec![0x4C, 0, 3, 1, 2, 3]);
    let mut buf = vec![9, 9];
    buf.extend(&f);
    buf.extend(connections::bridge_frame(&[0x4C; 300]));
    buf.extend(&[0x4C, 0]);
    assert_eq!(connections::bridge_unframe(&mut buf), Some(vec![1, 2, 3]));
    assert_eq!(connections::bridge_unframe(&mut buf), Some(vec![0x4C; 300]));
    assert_eq!(connections::bridge_unframe(&mut buf), None, "half a header waits for more");
    assert_eq!(buf, vec![0x4C, 0]);
    assert_eq!(connections::bridge_unframe(&mut Vec::new()), None);
}

#[test]
fn a_ruida_controller_answers_through_a_loopback_bridge() {
    let port = ruida_behind_bridge();
    let mut h = loopback(Link::Bridge { host: "127.0.0.1".into(), port });
    assert!(h.wait_for(T, |s, _| s.connected()), "connects through the bridge: {:?}", h.snapshot().link);
    assert_eq!(h.snapshot().family, Family::Ruida);
    assert!(h.is_real_machine());
    ready(&mut h, "bridge");
    h.command(MachineCommand::ReadControllerInfo).unwrap();
    assert!(h.wait_for(T, |s, _| s.ruida.info.is_some()), "controller info comes back through the relay");
    h.disconnect();
}

#[test]
fn bridge_rescan_and_setup() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let closed = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let mut a = app();
    // The wizard's logic.
    let mut w = BridgeWizard::default();
    assert!(!w.problems().is_empty(), "an address is needed");
    w.host = "127.0.0.1".into();
    w.port = port;
    assert!(w.problems().is_empty());
    w.step = 1;
    w.name = " ".into();
    w.width = 0.0;
    assert_eq!(w.problems().len(), 2);
    assert!(w.finish(&mut a.machine.devices).is_err());
    // Set up through the command.
    assert!(run(&mut a, "device.bridge_setup", json!({ "host": "" })).is_err());
    let id = run(&mut a, "device.bridge_setup", json!({ "host": "127.0.0.1", "port": port, "name": "Big laser", "width": 900, "height": 600 }))
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    assert_eq!(a.machine.devices.active, Some(id));
    let d = a.machine.devices.get(id).unwrap().clone();
    assert_eq!(
        (d.kind, d.connection, d.name.as_str(), d.width_mm, d.height_mm),
        (DeviceType::Ruida, ConnectionKind::Bridge, "Big laser", 900.0, 600.0)
    );
    assert_eq!(d.link(&[]), Ok(Link::Bridge { host: "127.0.0.1".into(), port }));
    assert!(!a.machine.handle.is_connected(), "setting up never connects");

    // Rescan: a bridge that answers, then one that does not.
    let r = run(&mut a, "machine.bridge_rescan", Value::Null).unwrap();
    assert!(r["message"].as_str().unwrap().starts_with("A bridge answered"));
    assert_eq!(a.machine.conn.bridge_status.as_ref().map(|s| s.1), Some(true));
    let e = run(&mut a, "machine.bridge_rescan", json!({ "port": closed })).unwrap_err();
    assert!(e.starts_with("Nothing answered"), "{e}");
    assert_eq!(a.machine.conn.bridge_status.as_ref().map(|s| s.1), Some(false));
    assert!(run(&mut a, "machine.bridge_rescan", json!({ "host": "192.168.1.9" })).is_err(), "never past this computer in a test");
    assert!(!a.machine.handle.is_connected(), "a rescan is not a connection to the laser");
    drop(listener);
}

// ----- automation socket [CONN-09] ------------------------------------------------------------------------

fn ask(a: &mut DarkBurn, client: &UdpSocket, port: u16, text: &str) -> String {
    client.send_to(text.as_bytes(), ("127.0.0.1", port)).unwrap();
    let end = Instant::now() + T;
    let mut buf = [0u8; 256];
    loop {
        a.machine.tick();
        connections::frame_tick(a);
        match client.recv_from(&mut buf) {
            Ok((n, from)) => {
                assert_eq!(from.port(), port, "the answer comes from the automation socket");
                return String::from_utf8_lossy(&buf[..n]).into_owned();
            }
            Err(e) if timed_out(&e) => {}
            Err(e) => panic!("{e}"),
        }
        assert!(Instant::now() < end, "no answer to {text}");
    }
}

#[test]
fn automation_socket_answers_status_and_starts_a_job() {
    let mut a = app();
    connections::frame_tick(&mut a);
    assert_eq!(a.machine.conn.automation.local_port, None, "off by default: nothing listens");
    assert!(!a.machine.devices.automation.enabled && !a.machine.devices.automation.allow_remote);

    // The user turns it on. Port 0 lets the system pick one; the answer goes back to the sender.
    a.machine.devices.automation = connections::AutomationSettings { enabled: true, port: 0, reply_port: 0, allow_remote: false };
    connections::frame_tick(&mut a);
    let port = a.machine.conn.automation.local_port.expect("listening");
    let client = UdpSocket::bind("127.0.0.1:0").unwrap();
    client.set_read_timeout(Some(Duration::from_millis(5))).unwrap();

    assert_eq!(ask(&mut a, &client, port, "PING"), "OK");
    assert_eq!(ask(&mut a, &client, port, "STATUS"), "DISCONNECTED");
    assert_eq!(ask(&mut a, &client, port, "START"), "ERROR: no laser is connected");
    assert!(ask(&mut a, &client, port, "FORMAT C:").starts_with("ERROR: unknown command"));

    select_sim(&mut a);
    run(&mut a, "machine.connect", json!({})).unwrap();
    wait(&mut a, "ready", |a| state(a) == LaserState::Ready);
    assert_eq!(ask(&mut a, &client, port, "status\n"), "READY");
    assert!(ask(&mut a, &client, port, "START").starts_with("ERROR"), "nothing to burn yet");
    for i in 0..6 {
        run(&mut a, "shape.add_rect", json!({ "x": 10 + i * 30, "y": 10, "width": 20, "height": 300, "layer": 0 })).unwrap();
    }
    assert_eq!(ask(&mut a, &client, port, "START"), "OK");
    wait(&mut a, "busy", |a| state(a) == LaserState::Busy);
    assert!(ask(&mut a, &client, port, "STATUS").starts_with("BUSY"));
    assert_eq!(ask(&mut a, &client, port, "PAUSE"), "OK");
    wait(&mut a, "paused", |a| state(a) == LaserState::Paused);
    assert_eq!(ask(&mut a, &client, port, "STATUS"), "PAUSED");
    assert_eq!(ask(&mut a, &client, port, "RESUME"), "OK");
    wait(&mut a, "busy again", |a| state(a) == LaserState::Busy);
    assert_eq!(ask(&mut a, &client, port, "STOP"), "OK");
    wait(&mut a, "settled after stop", |a| matches!(state(a), LaserState::Ready | LaserState::Alarm));
    if state(&a) == LaserState::Alarm {
        run(&mut a, "machine.unlock", Value::Null).unwrap();
    }
    wait(&mut a, "ready after stop", |a| state(a) == LaserState::Ready);
    let s = run(&mut a, "device.automation_status", Value::Null).unwrap();
    assert_eq!(s["listening_on"], port);
    assert!(s["log"].as_array().unwrap().iter().any(|l| l == "START -> OK"), "{s}");

    // With the checklist on, START waits for the person at the machine.
    let id = a.machine.devices.active.unwrap();
    a.machine.devices.get_mut(id).unwrap().settings.checklist_on = true;
    assert!(ask(&mut a, &client, port, "START").starts_with("WAITING"));
    assert_eq!(state(&a), LaserState::Ready, "nothing runs until the checklist is ticked");

    // Turning it off closes the socket.
    a.machine.devices.automation.enabled = false;
    connections::frame_tick(&mut a);
    assert_eq!(a.machine.conn.automation.local_port, None);
    client.send_to(b"PING", ("127.0.0.1", port)).unwrap();
    let mut buf = [0u8; 16];
    std::thread::sleep(Duration::from_millis(30));
    connections::frame_tick(&mut a);
    assert!(client.recv_from(&mut buf).is_err(), "nobody answers once it is off");
}

// ----- detected lasers [GRBL-12] --------------------------------------------------------------------------------

#[test]
fn common_diode_lasers_are_recognised_on_connect() {
    for (fw, brand, model) in [
        (SimFirmware::Ortur, "Ortur", "Ortur Laser Master 2 Pro S1"),
        (SimFirmware::XTool, "xTool", "xTool D1Pro"),
        (SimFirmware::Atomstack, "Atomstack", "ATOMSTACK A20"),
        (SimFirmware::Sculpfun, "Sculpfun", "SCULPFUN S30"),
    ] {
        let mut a = app();
        let id = select_sim(&mut a);
        run(&mut a, "device.set_connection", json!({ "sim_firmware": format!("{fw:?}") })).unwrap();
        {
            let d = a.machine.devices.get_mut(id).unwrap();
            d.width_mm = 410.0;
            d.height_mm = 400.0;
        }
        assert_eq!(run(&mut a, "device.detected", Value::Null).unwrap()["detected"], Value::Null, "nothing before connecting");
        assert!(run(&mut a, "device.adopt_detected", Value::Null).is_err());
        run(&mut a, "machine.connect", json!({})).unwrap();
        wait(&mut a, "detected", |a| a.machine.handle.snapshot().detected.is_some() && state(a) == LaserState::Ready);
        let d = a.machine.handle.snapshot().detected.clone().unwrap();
        assert_eq!((d.brand, d.model.as_str()), (brand, model), "{fw:?}");
        assert!(a.machine.message.as_ref().is_some_and(|m| m.text.contains("This looks like") && m.text.contains(model)), "{fw:?}: the user is told");
        let j = run(&mut a, "device.detected", Value::Null).unwrap();
        assert_eq!(j["detected"]["brand"], brand);
        assert_eq!((j["width_mm"].as_f64(), j["height_mm"].as_f64(), j["s_max"].as_f64()), (Some(410.0), Some(400.0), Some(1000.0)), "{j}");
        // Taking it over sets the size and S value the controller reports.
        {
            let d = a.machine.devices.get_mut(id).unwrap();
            d.width_mm = 100.0;
            d.settings.gcode.s_max = 255.0;
        }
        let r = run(&mut a, "device.adopt_detected", Value::Null).unwrap();
        assert_eq!(r["width_mm"], 410.0);
        let d = a.machine.devices.get(id).unwrap();
        assert_eq!((d.width_mm, d.height_mm, d.settings.gcode.s_max), (410.0, 400.0, 1000.0));
        assert!(d.name.starts_with("Simulator"), "a simulator keeps its name");
    }
    // Plain GRBL names nobody.
    let mut a = app();
    select_sim(&mut a);
    run(&mut a, "machine.connect", json!({})).unwrap();
    wait(&mut a, "ready", |a| state(a) == LaserState::Ready);
    run(&mut a, "machine.console", json!({ "line": "$I" })).unwrap();
    wait(&mut a, "idle", |a| state(a) == LaserState::Ready);
    assert_eq!(a.machine.handle.snapshot().detected, None);
    assert_eq!(run(&mut a, "device.detected", Value::Null).unwrap()["detected"], Value::Null);
}

// ----- the window ---------------------------------------------------------------------------------------------------

fn walk(s: &egui::epaint::Shape, found: &mut Vec<String>) {
    match s {
        egui::epaint::Shape::Text(t) => found.push(t.galley.text().to_string()),
        egui::epaint::Shape::Vec(v) => v.iter().for_each(|x| walk(x, found)),
        _ => {}
    }
}

/// Draw the machine panels and dialogs; the text on screen.
fn draw(ctx: &egui::Context, app: &mut DarkBurn) -> Vec<String> {
    let mut found = Vec::new();
    for _ in 0..4 {
        let input =
            egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 1400.0))), ..Default::default() };
        let mut out = ctx.run_ui(input, |ui| {
            machine_ui::frame(ui.ctx(), app);
            ui.push_id("laser_panel", |ui| {
                egui::ScrollArea::vertical().id_salt("laser").show(ui, |ui| {
                    crate::panels::contents(ui, "laser", app);
                });
            });
            crate::devices_ui::dialogs(ui.ctx(), app);
        });
        out.textures_delta.clear();
        found.clear();
        for s in &out.shapes {
            walk(&s.shape, &mut found);
        }
    }
    found
}

fn shows(t: &[String], want: &str) -> bool {
    t.iter().any(|x| x == want || x.contains(want))
}

#[test]
fn device_settings_show_the_fields_of_each_connection() {
    let ctx = egui::Context::default();
    let mut a = app();
    let id = add(&mut a, DeviceType::FluidNc);
    for (conn, wants) in [
        ("Serial", vec!["USB serial", "Baud rate"]),
        ("Bluetooth", vec!["Bluetooth serial", "Baud rate", "Pair the laser"]),
        ("Tcp", vec!["TCP/IP", "Host", "Port"]),
        ("Http", vec!["HTTP", "Host", "Request"]),
        ("RawUsb", vec!["Raw USB", "Vendor ID (hex)", "Product ID (hex)", "OUT endpoint", "IN endpoint"]),
    ] {
        let mut d = a.machine.devices.get(id).unwrap().clone();
        d.connection = [ConnectionKind::Serial, ConnectionKind::Bluetooth, ConnectionKind::Tcp, ConnectionKind::Http, ConnectionKind::RawUsb]
            .into_iter()
            .find(|k| format!("{k:?}") == conn)
            .unwrap();
        a.machine.dlg.settings = Some((d, 0));
        let t = draw(&ctx, &mut a);
        for w in wants {
            assert!(shows(&t, w), "{conn}: {w} is on the Basic page: {t:?}");
        }
    }
    // DTR on the GCode page.
    a.machine.dlg.settings = Some((a.machine.devices.get(id).unwrap().clone(), 1));
    let t = draw(&ctx, &mut a);
    assert!(shows(&t, "Pulse DTR when connecting") && shows(&t, "Keep DTR on while connected"), "{t:?}");
    // A Ruida device offers the bridge.
    let ruida_id = add(&mut a, DeviceType::Ruida);
    let mut d = a.machine.devices.get(ruida_id).unwrap().clone();
    d.connection = ConnectionKind::Bridge;
    a.machine.dlg.settings = Some((d, 0));
    let t = draw(&ctx, &mut a);
    assert!(shows(&t, "Network bridge (TCP relay)") && shows(&t, "Bridge address"), "{t:?}");
    // The simulator offers its firmware.
    let sim = select_sim(&mut a);
    a.machine.dlg.settings = Some((a.machine.devices.get(sim).unwrap().clone(), 0));
    let t = draw(&ctx, &mut a);
    assert!(shows(&t, "Simulated firmware") && shows(&t, "GRBL 1.1h"), "{t:?}");
    a.machine.dlg.settings = None;
}

#[test]
fn devices_dialog_wizard_and_laser_panel_show_the_new_pieces() {
    let ctx = egui::Context::default();
    let mut a = app();
    a.machine.dlg.devices = true;
    let t = draw(&ctx, &mut a);
    assert!(shows(&t, "Network bridge") && shows(&t, "Automation") && shows(&t, "Find my laser"), "{t:?}");
    a.machine.dlg.devices = false;

    // The new GRBL types are in the wizard.
    a.machine.dlg.wizard = Some(crate::devices::Wizard::new());
    let t = draw(&ctx, &mut a);
    assert!(shows(&t, "GRBL-M3 (1.1e or older)") && shows(&t, "GRBL-LPC (Smoothieboard)"), "{t:?}");
    a.machine.dlg.wizard = None;

    // Find My Laser lists USB devices too.
    a.machine.conn.usb_devices = vec![grbl::UsbDeviceInfo { vid: 0x1A86, pid: 0x7523, product: Some("USB Serial".into()), ..Default::default() }];
    a.machine.dlg.find = Some(Vec::new());
    let t = draw(&ctx, &mut a);
    assert!(shows(&t, "USB Serial (1A86:7523)") && shows(&t, "Add as raw USB"), "{t:?}");
    a.machine.dlg.find = None;

    // The bridge wizard, page by page.
    run(&mut a, "device.bridge_wizard", Value::Null).unwrap();
    let t = draw(&ctx, &mut a);
    assert!(shows(&t, "Network bridge setup") && shows(&t, "Bridge address") && shows(&t, "Test the bridge"), "{t:?}");
    assert!(shows(&t, "Enter the bridge's host name or IP address."), "{t:?}");
    {
        let w = a.machine.conn.bridge.as_mut().unwrap();
        w.host = "127.0.0.1".into();
        w.step = 1;
    }
    let t = draw(&ctx, &mut a);
    assert!(shows(&t, "Name") && shows(&t, "Width"), "{t:?}");
    a.machine.conn.bridge.as_mut().unwrap().step = 2;
    let t = draw(&ctx, &mut a);
    assert!(shows(&t, "Network bridge at 127.0.0.1:5005") && shows(&t, "Add"), "{t:?}");
    a.machine.conn.bridge = None;

    // The automation dialog.
    run(&mut a, "device.automation_open", Value::Null).unwrap();
    let t = draw(&ctx, &mut a);
    assert!(shows(&t, "Automation socket") && shows(&t, "Listen for automation commands") && shows(&t, "Not listening."), "{t:?}");
    a.machine.conn.automation_open = false;

    // Laser panel: the bridge rescan for a bridge device, the detected laser once connected.
    run(&mut a, "device.bridge_setup", json!({ "host": "127.0.0.1", "name": "Bridge laser" })).unwrap();
    let t = draw(&ctx, &mut a);
    assert!(shows(&t, "Rescan bridge"), "{t:?}");
    select_sim(&mut a);
    run(&mut a, "device.set_connection", json!({ "sim_firmware": "Ortur" })).unwrap();
    run(&mut a, "machine.connect", json!({})).unwrap();
    wait(&mut a, "detected", |a| a.machine.handle.snapshot().detected.is_some());
    let t = draw(&ctx, &mut a);
    assert!(shows(&t, "Detected laser: Ortur Laser Master 2 Pro S1 (OLF 172)") && shows(&t, "Use it"), "{t:?}");
    assert!(!shows(&t, "Rescan bridge"), "only for a bridge device");
}
