//! Driver tests. Everything runs against the in-memory emulator; no port is ever opened.

use std::time::{Duration, Instant};

use darkburn_grbl::protocol::rt;
use darkburn_grbl::*;
use kurbo::{Point, Rect};

const T: Duration = Duration::from_secs(10);

fn emulator(cfg: EmulatorConfig) -> EmulatorHandle {
    EmulatorHandle::new(GrblEmulator::new(cfg))
}

fn connect(emu: &EmulatorHandle, opts: ConnectOptions) -> GrblMachine {
    let m = GrblMachine::connect(Box::new(emu.transport()), opts);
    wait_event(&m, |e| matches!(e, Event::Connected(_)));
    m
}

/// Waits for an event matching `f`, returning it (other events are dropped).
fn wait_event(m: &GrblMachine, f: impl Fn(&Event) -> bool) -> Event {
    let end = Instant::now() + T;
    while Instant::now() < end {
        if let Some(e) = m.next_event(Duration::from_millis(20)) {
            if let Event::ConnectFailed(r) = &e {
                panic!("connect failed: {r}");
            }
            if f(&e) {
                return e;
            }
        }
    }
    panic!("timed out waiting for event");
}

/// Collects events until one matches `f`; returns all of them.
fn collect_until(m: &GrblMachine, f: impl Fn(&Event) -> bool) -> Vec<Event> {
    let end = Instant::now() + T;
    let mut out = Vec::new();
    while Instant::now() < end {
        if let Some(e) = m.next_event(Duration::from_millis(20)) {
            let done = f(&e);
            out.push(e);
            if done {
                return out;
            }
        }
    }
    panic!("timed out; got {out:?}");
}

fn wait_snap(m: &GrblMachine, what: &str, f: impl Fn(&Snapshot) -> bool) {
    assert!(m.wait_until(T, f), "timed out waiting for {what}: {:?}", m.snapshot().status);
}

fn wait_emu(emu: &EmulatorHandle, what: &str, f: impl Fn(&mut GrblEmulator) -> bool) {
    let end = Instant::now() + T;
    while Instant::now() < end {
        if emu.with(&f) {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!("timed out waiting for emulator: {what} (state {})", emu.with(|e| e.state_name()));
}

fn job_lines(n: usize) -> Vec<String> {
    let mut v = vec!["G21 G90".to_owned(), "M4 S0".to_owned()];
    for i in 0..n - 4 {
        let x = 10.0 + (i % 200) as f64 * 0.5;
        let y = 10.0 + (i / 200) as f64 * 0.1;
        // Line lengths vary between ~20 and ~30 bytes.
        if i % 7 == 0 {
            v.push(format!("G1 X{x:.2} Y{y:.2} S{} F6000", 100 + i % 900));
        } else {
            v.push(format!("G1 X{x:.1} Y{y:.1} S{}", i % 1000));
        }
    }
    v.push("M5 S0".to_owned());
    v.push("G0 X0 Y0".to_owned());
    assert_eq!(v.len(), n);
    v
}

#[test]
fn handshake_detects_grbl_and_reads_settings() {
    let emu = emulator(EmulatorConfig::atomstack());
    let m = GrblMachine::connect(Box::new(emu.transport()), ConnectOptions::default());
    let events = collect_until(&m, |e| matches!(e, Event::Connected(_)));
    let Some(Event::Connected(fw)) = events.last() else { unreachable!() };
    assert_eq!(fw.kind, FirmwareKind::Grbl);
    assert_eq!(fw.version, "1.1h");
    assert_eq!(fw.rx_buffer_size, 128);
    assert!(!fw.m3_only());
    let settings = events.iter().find_map(|e| match e {
        Event::Settings(s) => Some(s.clone()),
        _ => None,
    });
    let settings = settings.expect("settings read during handshake");
    let s30 = settings.iter().find(|s| s.number == 30).unwrap();
    assert_eq!(s30.value, "1000");
    assert_eq!(s30.label(), "Maximum S value");
    let snap = m.snapshot();
    assert_eq!(snap.rx_capacity, 127);
    assert_eq!(snap.laser_status(), LaserStatus::Ready);
    wait_snap(&m, "first status", |s| s.status.state == MachineState::Idle);
}

#[test]
fn handshake_flavors() {
    let cases = [
        (EmulatorFlavor::GrblHal, FirmwareKind::GrblHal, 1024, false),
        (EmulatorFlavor::FluidNc, FirmwareKind::FluidNc, 128, false),
        (EmulatorFlavor::Grbl11e, FirmwareKind::Grbl, 128, true),
        (EmulatorFlavor::Grbl09, FirmwareKind::Grbl, 128, true),
        // No reset when the port opens: the driver's soft reset brings the banner.
        (EmulatorFlavor::GrblLpc, FirmwareKind::Grbl, 8192, false),
    ];
    for (flavor, kind, rx, m3) in cases {
        let emu = emulator(EmulatorConfig::flavor(flavor).with_time_scale(50.0));
        let m = GrblMachine::connect(Box::new(emu.transport()), ConnectOptions::default());
        let Event::Connected(fw) = wait_event(&m, |e| matches!(e, Event::Connected(_))) else { unreachable!() };
        assert_eq!(fw.kind, kind, "{flavor:?}");
        assert_eq!(fw.rx_buffer_size, rx, "{flavor:?}");
        assert_eq!(fw.m3_only(), m3, "{flavor:?}");
        // Status parsing works in both grammars.
        m.move_to(MoveTarget { x: Some(5.0), y: Some(6.0), feed: 3000.0, ..Default::default() }).unwrap();
        wait_snap(&m, "move done", |s| s.status.state == MachineState::Idle && (s.status.mpos[0] - 5.0).abs() < 1e-6);
        if flavor == EmulatorFlavor::FluidNc {
            assert_eq!(fw.display_name(), "FluidNC v3.9.7");
        }
        if flavor == EmulatorFlavor::Grbl09 {
            assert!(matches!(m.set_feed_override(120), Err(GrblError::Unsupported(_))));
            assert!(matches!(m.jog_continuous(JogDirection { x: 1, y: 0, z: 0 }, 1000.0), Err(GrblError::Unsupported(_))));
        }
    }
}

#[test]
fn dtr_toggle_resets_the_board() {
    let emu = emulator(EmulatorConfig::atomstack());
    let before = emu.with(|e| e.stats().power_on_resets);
    let opts = ConnectOptions { toggle_dtr: true, ..Default::default() };
    let _m = connect(&emu, opts);
    assert_eq!(emu.with(|e| e.stats().power_on_resets), before + 2, "port open + DTR pulse");
}

#[test]
fn no_banner_fails_cleanly() {
    let emu = emulator(EmulatorConfig::atomstack());
    let t = emu.transport();
    emu.with(|e| e.unplug());
    let m = GrblMachine::connect(Box::new(t), ConnectOptions::default());
    let e = m.next_event(T).unwrap();
    assert!(matches!(e, Event::ConnectFailed(_)), "{e:?}");
}

#[test]
fn streams_10000_lines_with_exact_buffer_accounting() {
    let emu = emulator(EmulatorConfig::atomstack().with_time_scale(2000.0).with_setting(10, 3.0));
    let m = connect(&emu, ConnectOptions::default());
    let lines = job_lines(10_000);
    emu.with(|e| e.stats_mut().lines.clear());
    let job = Job::from_lines(lines.clone()).with_total(lines.len());
    m.start_job(job).unwrap();
    let events = collect_until(&m, |e| matches!(e, Event::JobFinished(_) | Event::JobAborted { .. }));
    let Some(Event::JobFinished(p)) = events.last() else { panic!("{:?}", events.last()) };
    assert_eq!(p.lines_sent, 10_000);
    assert_eq!(p.lines_acked, 10_000);
    assert_eq!(p.errors, 0);
    assert_eq!(p.fraction(), Some(1.0));
    assert!(events.iter().any(|e| matches!(e, Event::JobProgress(_))));

    let snap = m.snapshot();
    assert_eq!(snap.rx_in_flight, 0, "every byte accounted for");
    assert!(snap.rx_in_flight_peak <= 127, "driver never exceeds the RX buffer");
    assert!(snap.rx_in_flight_peak >= 100, "driver keeps the buffer full: {}", snap.rx_in_flight_peak);
    assert_eq!(snap.activity, Activity::Idle);

    emu.with(|e| {
        let s = e.stats();
        assert_eq!(s.rx_overflow, 0, "no byte dropped");
        assert!(s.rx_peak <= 127);
        assert_eq!(s.errors, 0);
        // Every job line arrived once, in order, followed by the driver's tail.
        let got: Vec<&String> = s.lines.iter().filter(|l| !l.starts_with("G4")).collect();
        assert_eq!(&got[..10_000], &lines.iter().collect::<Vec<_>>()[..]);
        assert_eq!(got[10_000], "M5 S0");
        assert!(!e.laser_on());
        assert_eq!(e.state_name(), "Idle");
    });
}

#[test]
fn synchronous_mode_sends_one_line_at_a_time() {
    let emu = emulator(EmulatorConfig::atomstack().with_time_scale(2000.0));
    let opts = ConnectOptions { stream_mode: StreamMode::Synchronous, ..Default::default() };
    let m = connect(&emu, opts);
    let lines = job_lines(300);
    let longest = lines.iter().map(|l| l.len() + 1).max().unwrap();
    emu.with(|e| e.stats_mut().rx_peak = 0);
    m.start_job(Job::from_lines(lines).with_total(300)).unwrap();
    wait_event(&m, |e| matches!(e, Event::JobFinished(_)));
    assert!(m.snapshot().rx_in_flight_peak <= longest);
    emu.with(|e| assert!(e.stats().rx_peak <= longest));
}

#[test]
fn pause_and_resume() {
    let emu = emulator(EmulatorConfig::atomstack().with_time_scale(20.0));
    let m = connect(&emu, ConnectOptions::default());
    // 4 × 300 mm at 3000 mm/min = 24 s simulated = 1.2 s real.
    let job = Job::from_text("G21 G90\nM4 S500\nG1 X300 F3000\nG1 Y300\nG1 X0\nG1 Y0\nM5");
    m.start_job(job).unwrap();
    wait_emu(&emu, "running with laser", |e| e.state_name() == "Run" && e.laser_on());
    m.pause().unwrap();
    wait_snap(&m, "Hold:0", |s| s.status.state == MachineState::Hold(0));
    assert_eq!(m.snapshot().laser_status(), LaserStatus::Paused);
    let held = emu.with(|e| {
        assert!(!e.laser_on(), "laser off in hold");
        e.machine_position()
    });
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(emu.with(|e| e.machine_position()), held, "no motion while paused");
    m.resume().unwrap();
    wait_emu(&emu, "running again", |e| e.state_name() == "Run" && e.laser_on());
    let Event::JobFinished(p) = wait_event(&m, |e| matches!(e, Event::JobFinished(_))) else { unreachable!() };
    assert_eq!(p.lines_acked, 7);
    emu.with(|e| {
        assert_eq!(e.machine_position(), [0.0, 0.0, 0.0]);
        assert!(!e.laser_on());
    });
}

#[test]
fn stop_mid_job_turns_the_laser_off_and_keeps_position() {
    let emu = emulator(EmulatorConfig::atomstack().with_time_scale(20.0));
    let m = connect(&emu, ConnectOptions::default());
    let job = Job::from_text("G21 G90\nM3 S800\nG1 X300 F3000\nG1 Y300\nG1 X0\nM5");
    m.start_job(job).unwrap();
    wait_emu(&emu, "cutting", |e| e.laser_on() && e.machine_position()[0] > 10.0);
    m.stop().unwrap();
    let events = collect_until(&m, |e| matches!(e, Event::Stopped));
    assert!(events.iter().any(|e| matches!(e, Event::JobAborted { reason, .. } if reason == "Stopped")));
    assert!(!events.iter().any(|e| matches!(e, Event::Alarm { .. })), "hold before reset avoids ALARM:3");
    wait_emu(&emu, "M5 after reset", |e| e.stats().lines.last().is_some_and(|l| l == "M5 S0"));
    emu.with(|e| {
        assert!(!e.laser_on());
        assert!(!e.spindle_enabled());
        assert_eq!(e.stats().soft_resets, 1);
        assert!(e.stats().realtime.contains(&rt::FEED_HOLD));
        assert_eq!(e.state_name(), "Idle");
        let x = e.machine_position()[0];
        assert!(x > 10.0 && x < 300.0, "position kept: {x}");
    });
    wait_snap(&m, "idle after stop", |s| s.activity == Activity::Idle && s.status.state == MachineState::Idle);
    // The machine is usable again.
    m.go_to_origin().unwrap();
    wait_emu(&emu, "back at origin", |e| e.machine_position() == [0.0, 0.0, 0.0] && e.state_name() == "Idle");
}

#[test]
fn alarm_at_start_up_blocks_motion_until_unlocked() {
    let emu = emulator(EmulatorConfig::atomstack().with_setting(22, 1.0).with_time_scale(50.0));
    let m = connect(&emu, ConnectOptions::default());
    wait_snap(&m, "alarm state", |s| s.alarm_locked && s.status.state.is_alarm());
    assert_eq!(m.snapshot().laser_status(), LaserStatus::Alarm);
    let step = JogStep { dx: 1.0, feed: 1000.0, ..Default::default() };
    assert_eq!(m.jog(step), Err(GrblError::Alarm));
    assert_eq!(m.start_job(Job::from_text("G0 X10")), Err(GrblError::Alarm));
    assert_eq!(m.fire(1.0, Duration::from_millis(100)), Err(GrblError::Alarm));
    assert_eq!(m.go_to_origin(), Err(GrblError::Alarm));
    assert_eq!(m.send_console("G0 X5"), Err(GrblError::Alarm));
    assert!(m.send_console("$G").is_ok(), "non-motion console lines still pass");
    let before = emu.with(|e| e.stats().lines.len());
    m.home().unwrap();
    wait_event(&m, |e| matches!(e, Event::Homed));
    assert_eq!(emu.with(|e| e.stats().lines[before..].to_vec()), vec!["$G".to_owned(), "$H".to_owned()]);
    wait_snap(&m, "idle after homing", |s| !s.alarm_locked && s.status.state == MachineState::Idle);
    m.jog(step).unwrap();
    wait_emu(&emu, "jogged", |e| e.machine_position()[0] == 1.0);
}

#[test]
fn macro_unlock_then_move_is_allowed() {
    let emu = emulator(EmulatorConfig::atomstack().with_setting(22, 1.0).with_time_scale(50.0));
    let m = connect(&emu, ConnectOptions::default());
    wait_snap(&m, "alarm", |s| s.alarm_locked);
    m.run_macro("$X\nG21 G90 ; units\nG0 X12 Y3\n\n$G").unwrap();
    wait_emu(&emu, "macro ran", |e| e.stats().lines.iter().any(|l| l == "$G"));
    let lines = emu.with(|e| e.stats().lines.clone());
    let tail: Vec<&str> = lines.iter().rev().take(5).rev().map(String::as_str).collect();
    assert_eq!(tail, ["$X", "G21 G90", "G0 X12 Y3", "G4 P0", "$G"], "$ lines one by one, G4 P0 before $G");
    wait_emu(&emu, "moved", |e| e.machine_position()[..2] == [12.0, 3.0]);
}

#[test]
fn hard_limit_during_job_aborts_and_resets() {
    let emu = emulator(EmulatorConfig::atomstack().with_time_scale(20.0));
    let m = connect(&emu, ConnectOptions::default());
    m.start_job(Job::from_text("G21 G90\nM4 S600\nG1 X300 F3000\nG1 Y300\nM5")).unwrap();
    wait_emu(&emu, "moving", |e| e.machine_position()[0] > 5.0);
    emu.with(|e| e.trigger_hard_limit('x'));
    let events = collect_until(&m, |e| matches!(e, Event::Stopped));
    let alarm = events.iter().find_map(|e| match e {
        Event::Alarm { code, message } => Some((*code, message.clone())),
        _ => None,
    });
    let (code, msg) = alarm.expect("alarm event");
    assert_eq!(code, 1);
    assert!(msg.starts_with("Hard limit"));
    assert!(events.iter().any(|e| matches!(e, Event::JobAborted { .. })));
    emu.with(|e| {
        assert!(!e.laser_on());
        assert_eq!(e.stats().soft_resets, 1, "driver resets after a critical alarm");
        assert_eq!(e.state_name(), "Alarm");
    });
    let snap = m.snapshot();
    assert!(snap.alarm_locked);
    assert_eq!(snap.last_alarm.as_ref().map(|a| a.0), Some(1));
    assert_eq!(m.jog(JogStep { dx: 1.0, feed: 100.0, ..Default::default() }), Err(GrblError::Alarm));
    emu.with(|e| e.release_limit_switch());
    m.unlock().unwrap();
    wait_snap(&m, "unlocked", |s| !s.alarm_locked && s.status.state == MachineState::Idle);
}

#[test]
fn soft_limit_in_job_raises_alarm_2() {
    let emu = emulator(EmulatorConfig::atomstack().with_setting(22, 1.0).with_setting(20, 1.0).with_time_scale(200.0));
    let m = connect(&emu, ConnectOptions::default());
    m.home().unwrap();
    wait_event(&m, |e| matches!(e, Event::Homed));
    wait_snap(&m, "idle", |s| s.status.state == MachineState::Idle);
    m.start_job(Job::from_text("G21 G90\nG0 X10\nG0 X500\nG0 X20")).unwrap();
    let Event::Alarm { code, .. } = wait_event(&m, |e| matches!(e, Event::Alarm { .. })) else { unreachable!() };
    assert_eq!(code, 2);
    wait_event(&m, |e| matches!(e, Event::Stopped));
    assert!(m.snapshot().alarm_locked);
    assert!(!emu.with(|e| e.stats().lines.iter().any(|l| l == "G0 X20")), "nothing after the alarm ran");
}

#[test]
fn homing_failure_reports_alarm_9() {
    let mut cfg = EmulatorConfig::atomstack().with_setting(22, 1.0).with_time_scale(50.0);
    cfg.homing_fails = true;
    let emu = emulator(cfg);
    let m = connect(&emu, ConnectOptions::default());
    m.home().unwrap();
    let Event::Alarm { code, message } = wait_event(&m, |e| matches!(e, Event::Alarm { .. })) else { unreachable!() };
    assert_eq!(code, 9);
    assert!(message.contains("switch was not found"));
    std::thread::sleep(Duration::from_millis(300));
    assert!(!m.poll_events().iter().any(|e| matches!(e, Event::Homed)));
    let snap = m.snapshot();
    assert!(snap.alarm_locked);
    assert_eq!(snap.rx_in_flight, 0);
}

#[test]
fn jog_step_diagonal_z_and_continuous_cancel() {
    let emu = emulator(EmulatorConfig::atomstack().with_time_scale(10.0));
    let m = connect(&emu, ConnectOptions::default());
    m.jog(JogStep { dx: 10.0, dy: 10.0, dz: 0.0, feed: 6000.0 }).unwrap();
    m.jog(JogStep { dx: 0.0, dy: 0.0, dz: 2.5, feed: 600.0 }).unwrap();
    wait_emu(&emu, "jogs done", |e| e.machine_position() == [10.0, 10.0, 2.5] && e.state_name() == "Idle");
    let lines = emu.with(|e| e.stats().lines.clone());
    assert!(lines.contains(&"$J=G21 G91 X10 Y10 F6000".to_owned()));
    assert!(lines.contains(&"$J=G21 G91 Z2.5 F600".to_owned()));
    wait_snap(&m, "position known", |s| s.status.mpos[0] == 10.0 && s.status.state == MachineState::Idle);
    assert!(matches!(m.jog(JogStep { dx: -20.0, feed: 100.0, ..Default::default() }), Err(GrblError::OutOfBounds(_))));

    // Continuous jog to the +X+Y corner, released after a moment.
    m.jog_continuous(JogDirection { x: 1, y: 1, z: 0 }, 3000.0).unwrap();
    wait_emu(&emu, "jogging", |e| e.state_name() == "Jog" && e.machine_position()[0] > 12.0);
    m.jog_cancel().unwrap();
    wait_emu(&emu, "cancelled", |e| e.state_name() == "Idle");
    let p = emu.with(|e| e.machine_position());
    assert!(p[0] > 12.0 && p[0] < 400.0, "{p:?}");
    assert!((p[0] - p[1]).abs() < 1e-6, "diagonal stays diagonal");
    assert!(emu.with(|e| e.stats().realtime.contains(&rt::JOG_CANCEL)));
    let cont = emu.with(|e| e.stats().lines.iter().rev().find(|l| l.starts_with("$J=")).cloned().unwrap());
    assert_eq!(cont, "$J=G21 G91 X390 Y390 F3000", "jogs to the travel edge");
}

#[test]
fn settings_read_write_backup_restore() {
    let emu = emulator(EmulatorConfig::atomstack().with_time_scale(50.0));
    let m = connect(&emu, ConnectOptions::default());
    m.write_setting(110, "5000").unwrap();
    let Event::SettingWritten { number, result, .. } = wait_event(&m, |e| matches!(e, Event::SettingWritten { .. })) else { unreachable!() };
    assert_eq!((number, result), (110, Ok(())));
    assert_eq!(emu.with(|e| e.setting(110)), Some(5000.0));

    m.write_setting(999, "1").unwrap();
    let Event::SettingWritten { result, .. } = wait_event(&m, |e| matches!(e, Event::SettingWritten { .. })) else { unreachable!() };
    assert_eq!(result, Err(error_message(3)));
    assert!(matches!(m.write_setting(1, "fast"), Err(GrblError::InvalidArgument(_))));

    m.read_settings().unwrap();
    wait_event(&m, |e| matches!(e, Event::Settings(_)));
    let backup = m.backup_settings();
    assert_eq!(backup.firmware, "GRBL 1.1h");
    assert!(backup.values.contains(&(110, "5000.000".to_owned())));
    let text = backup.to_text();
    assert!(text.contains("; X max rate (mm/min)"));

    // Change things on the controller, then restore the backup.
    emu.with(|e| {
        e.receive(b"$110=1234\n$120=50\n");
        e.take_output();
    });
    let parsed = SettingsBackup::parse(&text).unwrap();
    m.restore_settings(&parsed).unwrap();
    wait_event(&m, |e| matches!(e, Event::Settings(_)));
    emu.with(|e| {
        assert_eq!(e.setting(110), Some(5000.0));
        assert_eq!(e.setting(120), Some(1000.0));
        assert_eq!(e.stats().rx_overflow, 0);
    });
    assert_eq!(m.snapshot().settings.get(&110).map(String::as_str), Some("5000.000"));
}

#[test]
fn errors_are_translated() {
    let emu = emulator(EmulatorConfig::atomstack().with_time_scale(50.0));
    let m = connect(&emu, ConnectOptions::default());
    m.send_console("G1 X1").unwrap();
    let events = collect_until(&m, |e| matches!(e, Event::Error { .. }));
    let Some(Event::Error { code, message, line }) = events.last() else { unreachable!() };
    assert_eq!((*code, line.as_str()), (22, "G1 X1"));
    assert_eq!(message, "No feed rate (F) has been set for this move.");
    assert!(events.iter().any(|e| matches!(e, Event::Console(c) if c.kind == ConsoleKind::Sent && c.text == "G1 X1")));
    assert!(events.iter().any(|e| matches!(e, Event::Console(c) if c.kind == ConsoleKind::Received && c.text.starts_with("error:22 (No feed"))));

    // A job with a bad line: continue past it when asked to.
    let job = Job::from_text("G21\nG1 X5 F1000\nG77\nG1 X6").with_stop_on_error(false);
    m.start_job(job).unwrap();
    let events = collect_until(&m, |e| matches!(e, Event::JobFinished(_)));
    let err = events.iter().find_map(|e| match e {
        Event::JobLineError { line_number, code, message, .. } => Some((*line_number, *code, message.clone())),
        _ => None,
    });
    assert_eq!(err, Some((3, 20, error_message(20))));
    let Some(Event::JobFinished(p)) = events.last() else { unreachable!() };
    assert_eq!(p.errors, 1);

    // ... or stop on it (default).
    m.start_job(Job::from_text("G21\nG77\nG1 X6 F100")).unwrap();
    let events = collect_until(&m, |e| matches!(e, Event::Stopped));
    assert!(events.iter().any(|e| matches!(e, Event::JobLineError { line_number: 2, .. })));
    assert!(events.iter().any(|e| matches!(e, Event::JobAborted { .. })));
}

#[test]
fn overrides_reach_the_controller() {
    let emu = emulator(EmulatorConfig::atomstack().with_time_scale(5.0));
    let m = connect(&emu, ConnectOptions::default());
    m.start_job(Job::from_text("G21 G90\nM4 S500\nG1 X300 F1000\nM5")).unwrap();
    wait_emu(&emu, "running", |e| e.state_name() == "Run");
    m.set_feed_override(150).unwrap();
    m.set_power_override(50).unwrap();
    m.set_rapid_override(RapidOverride::Quarter).unwrap();
    wait_emu(&emu, "overrides applied", |e| e.overrides() == (150, 25, 50));
    wait_snap(&m, "overrides reported", |s| s.status.overrides == Overrides { feed: 150, rapid: 25, power: 50 });
    m.set_feed_override(73).unwrap();
    wait_emu(&emu, "feed 73", |e| e.overrides().0 == 73);
    m.stop().unwrap();
    wait_event(&m, |e| matches!(e, Event::Stopped));
    assert_eq!(emu.with(|e| e.overrides()), (100, 100, 100), "reset restores 100 %");
}

#[test]
fn fire_button_turns_itself_off() {
    let emu = emulator(EmulatorConfig::atomstack());
    let m = connect(&emu, ConnectOptions::default());
    assert!(matches!(m.fire(50.0, Duration::from_secs(1)), Err(GrblError::InvalidArgument(_))));
    assert!(matches!(m.fire(1.0, Duration::from_secs(60)), Err(GrblError::InvalidArgument(_))));
    m.fire(2.0, Duration::from_millis(300)).unwrap();
    wait_emu(&emu, "laser on", |e| e.laser_on());
    assert_eq!(emu.with(|e| e.stats().max_laser_s), 20.0, "2 % of S1000");
    wait_snap(&m, "fire flag", |s| s.fire_on);
    wait_event(&m, |e| matches!(e, Event::FireOff));
    wait_emu(&emu, "laser off", |e| !e.laser_on());
    wait_snap(&m, "fire flag cleared", |s| !s.fire_on);

    // Manual off, and jogging turns the fire button off first.
    m.fire(1.0, Duration::from_secs(5)).unwrap();
    wait_emu(&emu, "laser on", |e| e.laser_on());
    wait_snap(&m, "fire flag", |s| s.fire_on);
    m.jog(JogStep { dx: 1.0, feed: 600.0, ..Default::default() }).unwrap();
    wait_emu(&emu, "off before jog", |e| !e.laser_on() && e.stats().lines.last().is_some_and(|l| l.starts_with("$J=")));
}

#[test]
fn frame_rectangle_with_and_without_laser() {
    // Slow enough that the framing state lasts well over half a second of real time: at 50x
    // the whole frame took 120 ms and a loaded machine could miss it.
    let emu = emulator(EmulatorConfig::atomstack().with_time_scale(10.0));
    let m = connect(&emu, ConnectOptions::default());
    m.frame_rect(Rect::new(10.0, 20.0, 110.0, 70.0), FrameOptions::default()).unwrap();
    wait_snap(&m, "framing", |s| s.activity == Activity::Framing);
    assert_eq!(m.snapshot().laser_status(), LaserStatus::Framing);
    let Event::JobFinished(p) = wait_event(&m, |e| matches!(e, Event::JobFinished(_))) else { unreachable!() };
    assert_eq!(p.kind, JobKind::Frame);
    emu.with(|e| {
        assert_eq!(e.stats().laser_switch_ons, 0, "no laser when framing without it");
        assert_eq!(e.machine_position()[..2], [10.0, 20.0], "back at the first corner");
        assert!(e.stats().lines.contains(&"G1 X110 Y20 F3000".to_owned()));
    });

    let opts = FrameOptions { feed: 6000.0, laser_percent: Some(1.0) };
    m.frame_outline(&[Point::new(0.0, 0.0), Point::new(50.0, 0.0), Point::new(25.0, 40.0)], opts).unwrap();
    wait_event(&m, |e| matches!(e, Event::JobFinished(_)));
    emu.with(|e| {
        assert!(e.stats().laser_switch_ons >= 1);
        assert_eq!(e.stats().max_laser_s, 10.0);
        assert!(!e.laser_on());
    });
    let too_hot = FrameOptions { laser_percent: Some(60.0), ..Default::default() };
    assert!(matches!(m.frame_rect(Rect::new(0.0, 0.0, 1.0, 1.0), too_hot), Err(GrblError::InvalidArgument(_))));
    assert!(matches!(m.frame_rect(Rect::new(0.0, 0.0, 500.0, 10.0), FrameOptions::default()), Err(GrblError::OutOfBounds(_))));
}

#[test]
fn bounds_check_refuses_job() {
    let emu = emulator(EmulatorConfig::atomstack());
    let m = connect(&emu, ConnectOptions::default());
    let job =
        Job::from_text("G0 X450").with_bounds(BoundsCheck { job: Rect::new(0.0, 0.0, 450.0, 10.0), machine: Rect::new(0.0, 0.0, 400.0, 400.0) });
    assert!(matches!(m.start_job(job), Err(GrblError::OutOfBounds(_))));
    std::thread::sleep(Duration::from_millis(50));
    assert!(!emu.with(|e| e.stats().lines.iter().any(|l| l.contains("X450"))));
}

#[test]
fn origin_position_and_finish() {
    let emu = emulator(EmulatorConfig::atomstack().with_time_scale(50.0));
    let m = connect(&emu, ConnectOptions::default());
    m.move_to(MoveTarget { x: Some(100.0), y: Some(50.0), feed: 6000.0, ..Default::default() }).unwrap();
    wait_snap(&m, "at 100,50", |s| s.status.mpos[..2] == [100.0, 50.0] && s.status.state == MachineState::Idle);
    m.set_user_origin().unwrap();
    wait_snap(&m, "wpos zero", |s| s.status.wpos[..2] == [0.0, 0.0] && s.status.wco[..2] == [100.0, 50.0]);
    // A job in work coordinates with a finish position in machine coordinates.
    let job = Job::from_text("G21 G90\nG0 X10 Y10").with_finish(FinishPosition::Machine(Point::new(5.0, 6.0)));
    m.start_job(job).unwrap();
    wait_event(&m, |e| matches!(e, Event::JobFinished(_)));
    assert!(emu.with(|e| e.stats().lines.contains(&"G0 X10 Y10".to_owned())));
    assert_eq!(emu.with(|e| e.machine_position())[..2], [5.0, 6.0]);
    m.move_to(MoveTarget { x: Some(1.0), y: Some(2.0), feed: 6000.0, ..Default::default() }).unwrap();
    wait_snap(&m, "work 1,2", |s| s.status.mpos[..2] == [101.0, 52.0]);
    m.go_to_origin().unwrap();
    wait_snap(&m, "work origin", |s| s.status.mpos[..2] == [100.0, 50.0] && s.status.state == MachineState::Idle);
    m.clear_user_origin().unwrap();
    wait_snap(&m, "origin cleared", |s| s.status.wco[..2] == [0.0, 0.0]);
    m.move_to(MoveTarget { x: Some(3.0), y: None, coords: Coords::Machine, feed: 1000.0, ..Default::default() }).unwrap();
    wait_snap(&m, "machine x3", |s| s.status.mpos[..2] == [3.0, 50.0] && s.status.state == MachineState::Idle);
    assert!(matches!(m.move_to(MoveTarget { x: Some(401.0), feed: 1000.0, ..Default::default() }), Err(GrblError::OutOfBounds(_))));
}

#[test]
fn g92_origin_strategy() {
    let emu = emulator(EmulatorConfig::atomstack().with_time_scale(50.0));
    let opts = ConnectOptions { origin_strategy: OriginStrategy::G92, ..Default::default() };
    let m = connect(&emu, opts);
    m.move_to(MoveTarget { x: Some(30.0), y: Some(40.0), feed: 6000.0, ..Default::default() }).unwrap();
    wait_snap(&m, "moved", |s| s.status.mpos[..2] == [30.0, 40.0] && s.status.state == MachineState::Idle);
    m.set_user_origin().unwrap();
    wait_snap(&m, "wpos zero", |s| s.status.wpos[..2] == [0.0, 0.0]);
    m.clear_user_origin().unwrap();
    wait_snap(&m, "cleared", |s| s.status.wpos[..2] == [30.0, 40.0]);
    let lines = emu.with(|e| e.stats().lines.clone());
    assert!(lines.contains(&"G92 X0 Y0".to_owned()) && lines.contains(&"G92.1".to_owned()));
}

#[test]
fn console_show_all_and_status_rate() {
    let emu = emulator(EmulatorConfig::atomstack().with_time_scale(50.0));
    let m = connect(&emu, ConnectOptions::default());
    m.poll_events();
    m.start_job(Job::from_text("G21\nG0 X1")).unwrap();
    let ev = collect_until(&m, |e| matches!(e, Event::JobFinished(_)));
    assert!(!ev.iter().any(|e| matches!(e, Event::Console(c) if c.text == "G0 X1")), "quiet by default");
    m.set_show_all(true).unwrap();
    m.start_job(Job::from_text("G21\nG0 X2")).unwrap();
    let ev = collect_until(&m, |e| matches!(e, Event::JobFinished(_)));
    assert!(ev.iter().any(|e| matches!(e, Event::Console(c) if c.kind == ConsoleKind::Sent && c.text == "G0 X2")));
    assert!(ev.iter().any(|e| matches!(e, Event::Console(c) if c.kind == ConsoleKind::Received && c.text == "ok")));

    // Status polling stays at or below 5 Hz.
    let q0 = emu.with(|e| e.stats().status_queries);
    std::thread::sleep(Duration::from_millis(1000));
    let q = emu.with(|e| e.stats().status_queries) - q0;
    assert!((3..=6).contains(&q), "{q} polls in 1 s");
}

#[test]
fn disconnect_turns_laser_off_and_reconnect_works() {
    let emu = emulator(EmulatorConfig::atomstack().with_time_scale(20.0));
    let opts = ConnectOptions { max_fire_duration: Duration::from_secs(60), ..Default::default() };
    let m = connect(&emu, opts.clone());
    m.write_setting(111, "4321").unwrap();
    wait_event(&m, |e| matches!(e, Event::SettingWritten { .. }));
    m.fire(1.0, Duration::from_secs(30)).unwrap();
    wait_emu(&emu, "laser on", |e| e.laser_on());
    m.disconnect();
    emu.with(|e| assert!(!e.laser_on(), "disconnect turns the laser off"));

    // Reconnect to the same controller: settings persisted, but the board reset on open.
    let m = connect(&emu, opts.clone());
    assert_eq!(m.snapshot().settings.get(&111).map(String::as_str), Some("4321.000"));

    // A job running when the link drops is reported as aborted.
    m.start_job(Job::from_text("G21 G90\nG1 X300 F600\nG1 Y300")).unwrap();
    wait_emu(&emu, "running", |e| e.state_name() == "Run");
    emu.with(|e| e.unplug());
    let ev = collect_until(&m, |e| matches!(e, Event::Disconnected(Some(_))));
    assert!(ev.iter().any(|e| matches!(e, Event::JobAborted { reason, .. } if reason == "Connection lost")));
    assert_eq!(m.snapshot().laser_status(), LaserStatus::Disconnected);
    assert_eq!(m.jog(JogStep { dx: 1.0, feed: 1.0, ..Default::default() }), Err(GrblError::NotConnected));
    drop(m);

    emu.with(|e| e.replug());
    let m = connect(&emu, opts);
    assert!(m.is_connected());
    // While stopped mid-job before the drop, the board reset on reconnect: nothing moves.
    wait_snap(&m, "idle", |s| s.status.state == MachineState::Idle);
}

#[test]
fn check_mode_via_console() {
    let emu = emulator(EmulatorConfig::atomstack().with_time_scale(50.0));
    let m = connect(&emu, ConnectOptions::default());
    m.send_console("$C").unwrap();
    wait_snap(&m, "check", |s| s.status.state == MachineState::Check);
    m.start_job(Job::from_text("G21 G90\nG1 X100 F1000\nG1 Y100")).unwrap();
    wait_event(&m, |e| matches!(e, Event::JobFinished(_)));
    assert_eq!(emu.with(|e| e.machine_position()), [0.0, 0.0, 0.0], "check mode does not move");
    m.send_console("$C").unwrap();
    wait_snap(&m, "idle again", |s| s.status.state == MachineState::Idle);
    assert_eq!(m.snapshot().rx_in_flight, 0);
}

#[test]
fn eta_hook_is_used() {
    let emu = emulator(EmulatorConfig::atomstack().with_time_scale(200.0));
    let m = connect(&emu, ConnectOptions::default());
    let job = Job::from_lines(job_lines(500))
        .with_total(500)
        .with_eta(Box::new(|p| Some(Duration::from_secs((p.total_lines.unwrap() - p.lines_acked) as u64))));
    m.start_job(job).unwrap();
    let Event::JobFinished(p) = wait_event(&m, |e| matches!(e, Event::JobFinished(_))) else { unreachable!() };
    assert_eq!(p.eta, Some(Duration::ZERO));
}

#[test]
fn homing_turns_the_fire_button_off_first() {
    let emu = emulator(EmulatorConfig::atomstack().with_setting(22, 1.0).with_time_scale(50.0));
    let opts = ConnectOptions { max_fire_duration: Duration::from_secs(60), ..Default::default() };
    let m = connect(&emu, opts);
    m.unlock().unwrap();
    wait_snap(&m, "unlocked", |s| !s.alarm_locked && s.status.state == MachineState::Idle);
    m.fire(1.0, Duration::from_secs(30)).unwrap();
    wait_emu(&emu, "laser on", |e| e.laser_on());
    wait_snap(&m, "fire flag", |s| s.fire_on);
    m.home().unwrap();
    wait_event(&m, |e| matches!(e, Event::Homed));
    emu.with(|e| {
        let lines = &e.stats().lines;
        let home = lines.iter().rposition(|l| l == "$H").expect("homing was sent");
        let fire = lines.iter().rposition(|l| l.contains("M3")).expect("fire was sent");
        assert!(lines[fire..home].iter().any(|l| l.starts_with("M5")), "the laser must be switched off before homing starts: {:?}", &lines[fire..]);
    });
    wait_snap(&m, "fire flag cleared", |s| !s.fire_on);
    emu.with(|e| assert!(!e.laser_on()));
}

#[test]
fn stop_during_homing_acts_at_once() {
    let mut cfg = EmulatorConfig::atomstack().with_setting(22, 1.0);
    cfg.homing_seconds = 30.0;
    let emu = emulator(cfg);
    let m = connect(&emu, ConnectOptions::default());
    m.unlock().unwrap();
    wait_snap(&m, "unlocked", |s| !s.alarm_locked && s.status.state == MachineState::Idle);
    m.home().unwrap();
    wait_emu(&emu, "homing", |e| e.state_name() == "Home");
    let t = Instant::now();
    m.stop().unwrap();
    wait_emu(&emu, "reset", |e| e.stats().soft_resets >= 1);
    assert!(t.elapsed() < Duration::from_secs(1), "Stop took {:?} to reach the controller while homing", t.elapsed());
    wait_event(&m, |e| matches!(e, Event::Stopped));
    wait_snap(&m, "idle driver", |s| s.activity == Activity::Idle);
    // Homing was aborted, so the controller is locked; unlocking makes it usable again.
    m.unlock().unwrap();
    wait_snap(&m, "unlocked", |s| !s.alarm_locked && s.status.state == MachineState::Idle);
}

#[test]
fn stop_on_grbl_09_holds_first_and_waits_for_the_machine_to_stand_still() {
    // Real time, and a machine that needs 0.7 s to slow down: a reset before it stands still
    // raises "ALARM: Abort during cycle" and loses the position.
    let mut cfg = EmulatorConfig::flavor(EmulatorFlavor::Grbl09);
    cfg.decel_seconds = 0.7;
    let emu = emulator(cfg);
    let m = connect(&emu, ConnectOptions::default());
    m.start_job(Job::from_text("G21 G90\nM3 S800\nG1 X300 F3000\nG1 Y300\nG1 X0\nM5")).unwrap();
    wait_emu(&emu, "cutting", |e| e.laser_on() && e.machine_position()[0] > 2.0);
    m.stop().unwrap();
    let events = collect_until(&m, |e| matches!(e, Event::Stopped));
    assert!(!events.iter().any(|e| matches!(e, Event::Alarm { .. })), "Stop left the controller in alarm: {events:?}");
    wait_emu(&emu, "M5 after reset", |e| e.stats().lines.last().is_some_and(|l| l == "M5 S0"));
    emu.with(|e| {
        assert!(!e.laser_on());
        assert_eq!(e.stats().soft_resets, 1);
        assert!(e.stats().realtime.contains(&rt::FEED_HOLD));
        assert_eq!(e.state_name(), "Idle");
    });
    wait_snap(&m, "idle after stop", |s| s.activity == Activity::Idle && s.status.state == MachineState::Idle && !s.alarm_locked);
}

#[test]
fn stop_on_grbl_11_waits_out_a_slow_feed_hold() {
    let mut cfg = EmulatorConfig::atomstack();
    cfg.decel_seconds = 0.7;
    let emu = emulator(cfg);
    let m = connect(&emu, ConnectOptions::default());
    m.start_job(Job::from_text("G21 G90\nM3 S800\nG1 X300 F3000\nG1 Y300\nG1 X0\nM5")).unwrap();
    wait_emu(&emu, "cutting", |e| e.laser_on() && e.machine_position()[0] > 2.0);
    m.stop().unwrap();
    let events = collect_until(&m, |e| matches!(e, Event::Stopped));
    assert!(!events.iter().any(|e| matches!(e, Event::Alarm { .. })), "Stop left the controller in alarm: {events:?}");
    emu.with(|e| assert!(!e.laser_on()));
}

#[test]
fn stop_after_jog_pause_or_start_leaves_no_alarm() {
    let emu = emulator(EmulatorConfig::atomstack().with_time_scale(5.0));
    let m = connect(&emu, ConnectOptions::default());
    m.jog(JogStep { dx: 300.0, feed: 3000.0, ..Default::default() }).unwrap();
    wait_emu(&emu, "jogging", |e| e.state_name() == "Jog");
    m.stop().unwrap();
    let events = collect_until(&m, |e| matches!(e, Event::Stopped));
    assert!(!events.iter().any(|e| matches!(e, Event::Alarm { .. })), "alarm after stop: {events:?}");
    wait_snap(&m, "idle", |s| s.activity == Activity::Idle && s.status.state == MachineState::Idle && !s.alarm_locked);
    // Stop right after a pause, while still decelerating.
    m.start_job(Job::from_text("G21 G90\nM3 S800\nG1 X300 F3000\nG1 Y300\nG1 X0\nM5")).unwrap();
    wait_emu(&emu, "cutting", |e| e.laser_on());
    m.pause().unwrap();
    m.stop().unwrap();
    let events = collect_until(&m, |e| matches!(e, Event::Stopped));
    assert!(!events.iter().any(|e| matches!(e, Event::Alarm { .. })), "alarm after stop: {events:?}");
    wait_snap(&m, "idle", |s| s.activity == Activity::Idle && s.status.state == MachineState::Idle && !s.alarm_locked);
    // Stop right after starting, many times.
    for i in 0..15 {
        m.start_job(Job::from_text("G21 G90\nM3 S800\nG1 X300 F3000\nG1 Y300\nG1 X0\nM5")).unwrap();
        std::thread::sleep(Duration::from_millis(i * 7));
        m.stop().unwrap();
        let events = collect_until(&m, |e| matches!(e, Event::Stopped));
        assert!(!events.iter().any(|e| matches!(e, Event::Alarm { .. })), "alarm after stop {i}: {events:?}");
        wait_snap(&m, "idle", |s| s.activity == Activity::Idle && s.status.state == MachineState::Idle && !s.alarm_locked);
        emu.with(|e| assert!(!e.laser_on()));
    }
}

#[test]
fn not_a_number_is_refused() {
    let emu = emulator(EmulatorConfig::atomstack());
    let m = connect(&emu, ConnectOptions::default());
    wait_snap(&m, "idle", |s| s.status.state == MachineState::Idle);
    assert!(matches!(m.jog(JogStep { dx: 1.0, feed: f64::NAN, ..Default::default() }), Err(GrblError::InvalidArgument(_))));
    assert!(m.jog(JogStep { dx: f64::NAN, feed: 100.0, ..Default::default() }).is_err());
    assert!(m.jog(JogStep { dz: f64::INFINITY, feed: 100.0, ..Default::default() }).is_err());
    assert!(matches!(
        m.frame_rect(Rect::new(1.0, 1.0, 5.0, 5.0), FrameOptions { feed: f64::NAN, ..Default::default() }),
        Err(GrblError::InvalidArgument(_))
    ));
    assert!(m.move_to(MoveTarget { x: Some(1.0), y: None, z: Some(f64::NAN), feed: 100.0, coords: Coords::Work }).is_err());
    assert!(m.move_to(MoveTarget { x: Some(1.0), y: None, z: None, feed: f64::INFINITY, coords: Coords::Work }).is_err());
    std::thread::sleep(Duration::from_millis(200));
    emu.with(|e| assert!(!e.stats().lines.iter().any(|l| l.to_ascii_lowercase().contains("nan") || l.contains("inf")), "{:?}", e.stats().lines));
}
