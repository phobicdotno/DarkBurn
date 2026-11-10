//! Drives every machine type through the common `GcodeMachine` interface against the
//! in-memory emulators. No port or network connection is ever opened.

use std::time::{Duration, Instant};

use darkburn_gcodebots::emu::{EmuConfig, EmuFlavor, LineEmuHandle, SmoothieStatusStyle};
use darkburn_gcodebots::profile::{DeviceKind, DeviceProfile, MarlinPowerMode, TransferMode};
use darkburn_gcodebots::translate::{TranslateOptions, translate};
use darkburn_gcodebots::{GcodeMachine, MachineError, MachineEvent, MachineFamily, MachineSnapshot};
use darkburn_grbl::{
    Activity, ConnectionState, EmulatorConfig, EmulatorHandle, FrameOptions, GrblEmulator, Job, JogStep, LaserStatus, MachineState, Transport,
};
use kurbo::{Point, Rect};

const T: Duration = Duration::from_secs(8);

/// Collects events as they arrive so a test can look back at them.
struct Log {
    events: Vec<MachineEvent>,
}

impl Log {
    fn new() -> Self {
        Self { events: Vec::new() }
    }
    fn drain(&mut self, m: &dyn GcodeMachine) {
        self.events.extend(m.poll_events());
    }
    /// Waits until an event matching `pred` has been seen (new or earlier).
    fn wait(&mut self, m: &dyn GcodeMachine, timeout: Duration, pred: impl Fn(&MachineEvent) -> bool) -> bool {
        let end = Instant::now() + timeout;
        loop {
            self.drain(m);
            if self.events.iter().any(&pred) {
                return true;
            }
            if Instant::now() >= end {
                return false;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    fn wait_n(&mut self, m: &dyn GcodeMachine, timeout: Duration, n: usize, pred: impl Fn(&MachineEvent) -> bool) -> bool {
        let end = Instant::now() + timeout;
        loop {
            self.drain(m);
            if self.count(&pred) >= n {
                return true;
            }
            if Instant::now() >= end {
                return false;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    fn count(&self, pred: impl Fn(&MachineEvent) -> bool) -> usize {
        self.events.iter().filter(|e| pred(e)).count()
    }
}

/// Waits until the driver's worker has gone round its loop `n` more times (it stamps the
/// snapshot on every pass). Tests use it instead of a pause of some length: how long the
/// worker takes depends on how busy the computer is, how many passes it has made does not.
fn worker_passes(m: &dyn GcodeMachine, n: usize) {
    let end = Instant::now() + T;
    let mut last = m.snapshot().updated;
    let mut seen = 0;
    while seen < n {
        assert!(Instant::now() < end, "the driver's worker stopped running");
        std::thread::sleep(Duration::from_millis(1));
        let now = m.snapshot().updated;
        if now != last {
            last = now;
            seen += 1;
        }
    }
}

/// Asserts that the driver sends no motion while its worker makes many passes. What counts
/// is what has arrived at the controller, not what the controller has taken in: lines sent
/// before the pause may still be waiting for room in its planner.
fn assert_no_more_moves_are_sent(m: &dyn GcodeMachine, emu: &LineEmuHandle) {
    let moves = || emu.with(|e| e.stats().arrived.iter().filter(|l| l.starts_with('G')).count());
    let before = moves();
    worker_passes(m, 25);
    assert_eq!(moves(), before, "moves kept flowing while paused");
}

fn smoothie_profile() -> DeviceProfile {
    let mut p = DeviceProfile::for_kind(DeviceKind::Smoothie);
    p.width_mm = 300.0;
    p.height_mm = 200.0;
    p.gcode.transfer = TransferMode::Buffered;
    p
}

fn marlin_profile() -> DeviceProfile {
    let mut p = DeviceProfile::for_kind(DeviceKind::Marlin);
    p.width_mm = 300.0;
    p.height_mm = 200.0;
    p.gcode.transfer = TransferMode::Buffered;
    p
}

fn connect(p: &DeviceProfile, emu: &LineEmuHandle) -> (Box<dyn GcodeMachine>, Log) {
    let m = p.connect(Box::new(emu.transport())).expect("connect");
    let mut log = Log::new();
    assert!(log.wait(m.as_ref(), T, |e| matches!(e, MachineEvent::Connected(_) | MachineEvent::ConnectFailed(_))), "no handshake result");
    (m, log)
}

fn burn_job(lines: usize) -> Job {
    // GRBL-style source: 5 mm powered strokes along X, stepping Y.
    let mut src = vec!["G21".to_owned(), "G90".to_owned(), "M4 S0".to_owned()];
    for i in 0..lines {
        let y = (i / 2) as f64;
        let (x0, x1) = if i % 2 == 0 { (10.0, 15.0) } else { (15.0, 10.0) };
        src.push(format!("G0 X{x0} Y{y}"));
        src.push(format!("G1 X{x1} Y{y} S500 F6000"));
    }
    src.push("M5".to_owned());
    Job::from_text(&src.join("\n"))
}

fn translated_job(p: &DeviceProfile, lines: usize) -> Job {
    let mut src = vec!["G21".to_owned(), "G90".to_owned(), "M4 S0".to_owned()];
    for i in 0..lines {
        let y = (i / 2) as f64;
        let (x0, x1) = if i % 2 == 0 { (10.0, 15.0) } else { (15.0, 10.0) };
        src.push(format!("G0 X{x0} Y{y}"));
        src.push(format!("G1 X{x1} Y{y} S500 F6000"));
    }
    src.push("M5".to_owned());
    let out = translate(src.iter().map(String::as_str), &TranslateOptions::for_profile(p));
    let n = out.len();
    Job::from_lines(out).with_total(n)
}

// ----- Smoothieware -----

#[test]
fn smoothie_handshake_names_the_firmware() {
    let emu = LineEmuHandle::with_config(EmuConfig::new(EmuFlavor::Smoothie));
    let (m, _log) = connect(&smoothie_profile(), &emu);
    let s = m.snapshot();
    assert_eq!(s.connection, ConnectionState::Connected);
    assert_eq!(s.family, Some(MachineFamily::Smoothie));
    assert!(s.firmware.contains("edge-94de3d9"), "{}", s.firmware);
    assert_eq!(s.laser_status(), LaserStatus::Ready);
    m.disconnect();
}

#[test]
fn a_marlin_board_is_refused_by_the_smoothie_driver() {
    let emu = LineEmuHandle::with_config(EmuConfig::new(EmuFlavor::Marlin));
    let (m, log) = connect(&smoothie_profile(), &emu);
    assert!(log.events.iter().any(|e| matches!(e, MachineEvent::ConnectFailed(msg) if msg.contains("Marlin"))), "{:?}", log.events);
    assert!(matches!(m.snapshot().connection, ConnectionState::Failed(_)));
}

#[test]
fn smoothie_job_streams_with_a_small_window_and_finishes_with_the_laser_off() {
    let emu = LineEmuHandle::with_config(EmuConfig::new(EmuFlavor::Smoothie).with_planner(4));
    let p = smoothie_profile();
    let (m, mut log) = connect(&p, &emu);
    m.start_job(translated_job(&p, 40)).unwrap();
    assert!(log.wait(m.as_ref(), T, |e| matches!(e, MachineEvent::JobFinished(_))));
    let fin = log.events.iter().find_map(|e| if let MachineEvent::JobFinished(p) = e { Some(p.clone()) } else { None }).unwrap();
    assert_eq!(fin.errors, 0);
    assert_eq!(fin.lines_acked, fin.total_lines.unwrap());
    emu.with(|e| {
        // No character counting: the planner never held more than its capacity and the laser
        // was never on during a rapid.
        assert!(e.stats().max_planner_len <= 4);
        assert_eq!(e.stats().rapid_burn_mm, 0.0);
        assert!((e.stats().burned_mm - 40.0 * 5.0).abs() < 1.0, "{}", e.stats().burned_mm);
        assert!(!e.laser_on());
        assert!(!e.laser_armed());
        // S was scaled to the 0..1 range Smoothieware expects.
        assert!(e.stats().lines.iter().any(|l| l.contains("S0.5")), "{:?}", &e.stats().lines[..8]);
    });
    assert_eq!(m.snapshot().activity, Activity::Idle);
    m.disconnect();
}

#[test]
fn smoothie_status_reports_reach_the_snapshot() {
    for style in [SmoothieStatusStyle::Comma, SmoothieStatusStyle::Pipe] {
        let emu = LineEmuHandle::with_config(EmuConfig::new(EmuFlavor::Smoothie).with_status_style(style));
        let (m, _log) = connect(&smoothie_profile(), &emu);
        m.jog(JogStep { dx: 12.0, dy: 7.0, dz: 0.0, feed: 6000.0 }).unwrap();
        let ok = m.as_ref().wait_until(T, |s| (s.mpos[0] - 12.0).abs() < 1e-6 && (s.mpos[1] - 7.0).abs() < 1e-6);
        assert!(ok, "{style:?}: {:?}", m.snapshot().mpos);
        assert_eq!(m.snapshot().state, MachineState::Idle);
        m.disconnect();
    }
}

#[test]
fn smoothie_pause_uses_suspend_and_resume_continues() {
    let emu = LineEmuHandle::with_config(EmuConfig::new(EmuFlavor::Smoothie).with_time_scale(2.0));
    let p = smoothie_profile();
    let (m, mut log) = connect(&p, &emu);
    m.start_job(translated_job(&p, 30)).unwrap();
    assert!(m.as_ref().wait_until(T, |s| s.job.as_ref().is_some_and(|j| j.lines_acked >= 6)));
    m.pause().unwrap();
    assert!(m.as_ref().wait_until(T, |s| s.paused));
    // Smoothieware reads its serial lines in order, so `suspend` waits behind the lines the
    // planner has not taken yet. This is why the profile has a "reduce pause delay" option.
    assert!(m.as_ref().wait_until(T, |_| emu.with(|e| e.is_paused())));
    assert!(emu.with(|e| e.stats().lines.iter().any(|l| l == "suspend")));
    assert_eq!(m.snapshot().laser_status(), LaserStatus::Paused);
    // No new job lines go out while paused.
    assert_no_more_moves_are_sent(m.as_ref(), &emu);
    m.resume().unwrap();
    // Speed the emulator up so the rest finishes quickly.
    assert!(log.wait(m.as_ref(), Duration::from_secs(60), |e| matches!(e, MachineEvent::JobFinished(_))));
    assert!(emu.with(|e| e.stats().lines.iter().any(|l| l == "resume")));
    m.disconnect();
}

#[test]
fn smoothie_stop_empties_the_planner_and_switches_the_laser_off() {
    let emu = LineEmuHandle::with_config(EmuConfig::new(EmuFlavor::Smoothie).with_time_scale(1.0));
    let p = smoothie_profile();
    let (m, mut log) = connect(&p, &emu);
    m.start_job(translated_job(&p, 60)).unwrap();
    assert!(m.as_ref().wait_until(T, |s| s.job.as_ref().is_some_and(|j| j.lines_acked >= 10)));
    m.stop().unwrap();
    assert!(log.wait(m.as_ref(), T, |e| matches!(e, MachineEvent::Stopped)));
    assert_eq!(log.count(|e| matches!(e, MachineEvent::JobAborted { .. })), 1);
    emu.with(|e| {
        assert_eq!(e.planner_len(), 0);
        assert!(!e.laser_on());
        assert!(!e.laser_armed());
        assert!(!e.is_halted(), "M999 should have cleared the halt");
    });
    let s = m.snapshot();
    assert_eq!(s.activity, Activity::Idle);
    assert!(s.alarm.is_none(), "{:?}", s.alarm);
    // The machine is usable again.
    m.jog(JogStep { dx: 1.0, dy: 0.0, dz: 0.0, feed: 3000.0 }).unwrap();
    m.disconnect();
}

#[test]
fn smoothie_fire_button_is_capped_and_switches_itself_off() {
    let emu = LineEmuHandle::with_config(EmuConfig::new(EmuFlavor::Smoothie));
    let (m, mut log) = connect(&smoothie_profile(), &emu);
    assert!(matches!(m.fire(50.0, Duration::from_millis(100)), Err(MachineError::InvalidArgument(_))));
    assert!(matches!(m.fire(5.0, Duration::from_secs(600)), Err(MachineError::InvalidArgument(_))));
    // The pulse is timed from the button press and is dropped if the driver gets to it too
    // late, so it is long enough here for a busy test machine to send it.
    m.fire(5.0, Duration::from_secs(1)).unwrap();
    assert!(m.as_ref().wait_until(T, |_| emu.with(|e| e.stats().lines.iter().any(|l| l == "fire 5"))));
    assert!(log.wait(m.as_ref(), T, |e| matches!(e, MachineEvent::FireOff)), "it switches itself off");
    assert!(m.as_ref().wait_until(T, |_| !emu.with(|e| e.laser_on())));
    // Jogging while firing turns it off first.
    m.fire(5.0, Duration::from_secs(5)).unwrap();
    assert!(m.as_ref().wait_until(T, |s| s.fire_on));
    m.jog(JogStep { dx: 1.0, dy: 0.0, dz: 0.0, feed: 3000.0 }).unwrap();
    assert!(m.as_ref().wait_until(T, |_| !emu.with(|e| e.laser_on())));
    m.disconnect();
}

#[test]
fn smoothie_config_is_read_and_written() {
    let emu = LineEmuHandle::with_config(EmuConfig::new(EmuFlavor::Smoothie));
    let (m, mut log) = connect(&smoothie_profile(), &emu);
    m.read_settings().unwrap();
    assert!(log.wait(m.as_ref(), T, |e| matches!(e, MachineEvent::Settings(_))));
    let s = m.snapshot();
    assert_eq!(s.settings.get("laser_module_max_power").map(String::as_str), Some("1.0"));
    assert_eq!(s.settings.get("alpha_max_travel").map(String::as_str), Some("300"));
    m.write_setting("laser_module_max_power", "0.8").unwrap();
    assert!(log.wait(m.as_ref(), T, |e| matches!(e, MachineEvent::SettingWritten { result: Ok(()), .. })));
    assert_eq!(emu.with(|e| e.config_value("laser_module_max_power").map(str::to_owned)), Some("0.8".to_owned()));
    // Keys and values cannot smuggle a second command.
    assert!(m.write_setting("a b", "1").is_err());
    assert!(m.write_setting("key", "1\nM112").is_err());
    assert!(m.write_setting("key", "1 # x").is_err());
    m.disconnect();
}

#[test]
fn smoothie_emergency_stop_halts_and_unlock_clears() {
    let emu = LineEmuHandle::with_config(EmuConfig::new(EmuFlavor::Smoothie));
    let (m, mut log) = connect(&smoothie_profile(), &emu);
    m.emergency_stop().unwrap();
    assert!(log.wait(m.as_ref(), T, |e| matches!(e, MachineEvent::Alarm(_))));
    assert_eq!(m.snapshot().laser_status(), LaserStatus::Alarm);
    assert!(matches!(m.start_job(burn_job(2)), Err(MachineError::Alarm)));
    assert!(matches!(m.jog(JogStep { dx: 1.0, dy: 0.0, dz: 0.0, feed: 1000.0 }), Err(MachineError::Alarm)));
    m.unlock().unwrap();
    assert!(m.as_ref().wait_until(T, |s| s.alarm.is_none()));
    assert!(!emu.with(|e| e.is_halted()));
    m.disconnect();
}

#[test]
fn smoothie_home_and_air_assist() {
    let emu = LineEmuHandle::with_config(EmuConfig::new(EmuFlavor::Smoothie));
    let (m, mut log) = connect(&smoothie_profile(), &emu);
    m.jog(JogStep { dx: 20.0, dy: 20.0, dz: 0.0, feed: 6000.0 }).unwrap();
    m.home().unwrap();
    assert!(log.wait(m.as_ref(), T, |e| matches!(e, MachineEvent::Homed)));
    assert!(emu.with(|e| e.homed()));
    m.set_air(true).unwrap();
    assert!(m.as_ref().wait_until(T, |_| emu.with(|e| e.air_on())));
    m.set_air(false).unwrap();
    assert!(m.as_ref().wait_until(T, |_| !emu.with(|e| !e.air_on())));
    m.disconnect();
}

#[test]
fn framing_draws_the_outline_with_the_laser_off_unless_asked() {
    let emu = LineEmuHandle::with_config(EmuConfig::new(EmuFlavor::Smoothie));
    let (m, mut log) = connect(&smoothie_profile(), &emu);
    m.frame_rect(Rect::new(10.0, 10.0, 60.0, 40.0), FrameOptions { feed: 6000.0, laser_percent: None }).unwrap();
    assert!(log.wait(m.as_ref(), T, |e| matches!(e, MachineEvent::JobFinished(_))));
    emu.with(|e| {
        assert_eq!(e.stats().burned_mm, 0.0);
        let p = e.machine_position();
        assert!((p[0] - 10.0).abs() < 1e-6 && (p[1] - 10.0).abs() < 1e-6, "{p:?}");
    });
    // With the laser on, power is capped.
    assert!(matches!(
        m.frame_rect(Rect::new(0.0, 0.0, 10.0, 10.0), FrameOptions { feed: 3000.0, laser_percent: Some(60.0) }),
        Err(MachineError::InvalidArgument(_))
    ));
    m.frame_rect(Rect::new(0.0, 0.0, 10.0, 10.0), FrameOptions { feed: 6000.0, laser_percent: Some(2.0) }).unwrap();
    assert!(log.wait_n(m.as_ref(), T, 2, |e| matches!(e, MachineEvent::JobFinished(_))));
    emu.with(|e| {
        assert!(e.stats().burned_mm > 30.0);
        assert!(!e.laser_armed());
    });
    // Out of travel.
    assert!(matches!(m.frame_rect(Rect::new(0.0, 0.0, 500.0, 10.0), FrameOptions::default()), Err(MachineError::OutOfBounds(_))));
    m.disconnect();
}

// ----- Marlin -----

#[test]
fn marlin_handshake_and_position_polling() {
    let emu = LineEmuHandle::with_config(EmuConfig::new(EmuFlavor::Marlin));
    let (m, _log) = connect(&marlin_profile(), &emu);
    let s = m.snapshot();
    assert_eq!(s.family, Some(MachineFamily::Marlin));
    assert!(s.firmware.contains("2.1.2"), "{}", s.firmware);
    assert!(!m.capabilities().hardware_pause);
    m.jog(JogStep { dx: 25.0, dy: 5.0, dz: 0.0, feed: 6000.0 }).unwrap();
    assert!(m.as_ref().wait_until(T, |s| (s.mpos[0] - 25.0).abs() < 0.01 && (s.mpos[1] - 5.0).abs() < 0.01), "{:?}", m.snapshot().mpos);
    // Jog was G91 G0 ... G90, so the emulator is back in absolute mode.
    assert!(emu.with(|e| {
        let l = &e.stats().lines;
        l.iter().any(|x| x == "G91") && l.iter().any(|x| x == "G90")
    }));
    m.disconnect();
}

#[test]
fn marlin_job_in_m3_mode_burns_only_on_g1() {
    let emu = LineEmuHandle::with_config(EmuConfig::new(EmuFlavor::Marlin));
    let mut p = marlin_profile();
    p.gcode.marlin_power = MarlinPowerMode::Inline;
    let (m, mut log) = connect(&p, &emu);
    m.start_job(translated_job(&p, 20)).unwrap();
    assert!(log.wait(m.as_ref(), T, |e| matches!(e, MachineEvent::JobFinished(_))));
    emu.with(|e| {
        assert!(e.stats().lines.iter().any(|l| l == "M4 I"), "{:?}", &e.stats().lines[..6]);
        assert_eq!(e.stats().rapid_burn_mm, 0.0);
        assert!((e.stats().burned_mm - 100.0).abs() < 1.0, "{}", e.stats().burned_mm);
        assert!(!e.laser_armed());
    });
    m.disconnect();
}

#[test]
fn marlin_fan_mode_never_leaves_the_fan_laser_on_during_rapids() {
    let emu = LineEmuHandle::with_config(EmuConfig::new(EmuFlavor::Marlin).with_laser_fan(0));
    let mut p = marlin_profile();
    p.gcode.marlin_power = MarlinPowerMode::Fan;
    p.gcode.tool_index = 0;
    p.gcode.air_assist.on.clear();
    p.gcode.air_assist.off.clear();
    let (m, mut log) = connect(&p, &emu);
    m.start_job(translated_job(&p, 20)).unwrap();
    assert!(log.wait(m.as_ref(), T, |e| matches!(e, MachineEvent::JobFinished(_))));
    emu.with(|e| {
        assert_eq!(e.stats().rapid_burn_mm, 0.0, "the translator must switch the fan off before a G0");
        assert!(e.stats().burned_mm > 90.0);
        assert_eq!(e.fan_value(0), 0.0);
    });
    m.disconnect();
}

#[test]
fn marlin_pause_only_stops_sending_and_stop_uses_quick_stop() {
    let emu = LineEmuHandle::with_config(EmuConfig::new(EmuFlavor::Marlin).with_time_scale(1.0));
    let p = marlin_profile();
    let (m, mut log) = connect(&p, &emu);
    m.start_job(translated_job(&p, 60)).unwrap();
    assert!(m.as_ref().wait_until(T, |s| s.job.as_ref().is_some_and(|j| j.lines_acked >= 8)));
    m.pause().unwrap();
    assert!(m.as_ref().wait_until(T, |s| s.paused));
    assert_eq!(m.snapshot().state, MachineState::Hold(0));
    assert_no_more_moves_are_sent(m.as_ref(), &emu);
    m.stop().unwrap();
    assert!(log.wait(m.as_ref(), T, |e| matches!(e, MachineEvent::Stopped)));
    emu.with(|e| {
        assert!(e.stats().lines.iter().any(|l| l == "M410"));
        assert!(!e.laser_armed());
        assert_eq!(e.planner_len(), 0);
    });
    assert_eq!(m.snapshot().activity, Activity::Idle);
    m.disconnect();
}

#[test]
fn marlin_settings_m503_m92_m500() {
    let emu = LineEmuHandle::with_config(EmuConfig::new(EmuFlavor::Marlin));
    let (m, mut log) = connect(&marlin_profile(), &emu);
    m.read_settings().unwrap();
    assert!(log.wait(m.as_ref(), T, |e| matches!(e, MachineEvent::Settings(_))));
    let s = m.snapshot();
    assert_eq!(s.settings.get("M92.X").map(String::as_str), Some("80.00"));
    assert_eq!(s.settings.get("M211.S").map(String::as_str), Some("1"));
    m.write_setting("M92.X", "100").unwrap();
    assert!(log.wait(m.as_ref(), T, |e| matches!(e, MachineEvent::SettingWritten { key, result: Ok(()), .. } if key == "M92.X")));
    assert_eq!(emu.with(|e| e.marlin_setting("M92").map(|a| a[0])), Some(100.0));
    m.persist_settings().unwrap();
    assert!(m.as_ref().wait_until(T, |_| emu.with(|e| e.stats().lines.iter().any(|l| l == "M500"))));
    assert!(m.write_setting("M92", "1").is_err());
    assert!(m.write_setting("M92.X", "abc").is_err());
    assert!(m.write_setting("M92.X", "1\nM112").is_err());
    m.disconnect();
}

#[test]
fn marlin_emergency_stop_is_final_until_reconnect() {
    let emu = LineEmuHandle::with_config(EmuConfig::new(EmuFlavor::Marlin));
    let (m, mut log) = connect(&marlin_profile(), &emu);
    m.emergency_stop().unwrap();
    assert!(log.wait(m.as_ref(), T, |e| matches!(e, MachineEvent::Alarm(_))));
    assert!(emu.with(|e| e.is_halted()));
    assert_eq!(m.snapshot().laser_status(), LaserStatus::Alarm);
    assert!(matches!(m.unlock(), Err(MachineError::Unsupported(_))));
    assert!(matches!(m.home(), Err(MachineError::Alarm)));
    // After a power cycle and a new connection it works again.
    m.disconnect();
    emu.with(|e| e.power_cycle());
    let (m2, _l) = connect(&marlin_profile(), &emu);
    assert_eq!(m2.snapshot().connection, ConnectionState::Connected);
    assert!(m2.snapshot().alarm.is_none());
    m2.disconnect();
}

#[test]
fn marlin_fire_uses_the_selected_power_mode() {
    for (mode, tool, expect_on, expect_off) in [(MarlinPowerMode::M3, 0, "M3 S", "M5"), (MarlinPowerMode::Fan, 2, "M106 P2 S", "M107 P2")] {
        let emu = LineEmuHandle::with_config(EmuConfig::new(EmuFlavor::Marlin));
        let mut p = marlin_profile();
        p.gcode.marlin_power = mode;
        p.gcode.tool_index = tool;
        let (m, mut log) = connect(&p, &emu);
        // Switched off by hand once the controller has the fire command: a short timed pulse
        // may rightly be dropped when the driver is late, and then there is no command to see.
        m.fire(5.0, Duration::from_secs(5)).unwrap();
        assert!(m.as_ref().wait_until(T, |_| emu.with(|e| e.stats().lines.iter().any(|l| l.starts_with(expect_on)))), "{mode:?}");
        m.fire_off().unwrap();
        assert!(log.wait(m.as_ref(), T, |e| matches!(e, MachineEvent::FireOff)));
        assert!(m.as_ref().wait_until(T, |_| emu.with(|e| e.stats().lines.iter().any(|l| l == expect_off))), "{mode:?}");
        m.disconnect();
    }
}

#[test]
fn marlin_soft_endstops_clamp_and_travel_check_refuses_first() {
    let emu = LineEmuHandle::with_config(EmuConfig::new(EmuFlavor::Marlin));
    let (m, _log) = connect(&marlin_profile(), &emu);
    assert!(matches!(m.jog(JogStep { dx: 400.0, dy: 0.0, dz: 0.0, feed: 3000.0 }), Err(MachineError::OutOfBounds(_))));
    m.disconnect();
}

// ----- Custom device -----

#[test]
fn custom_device_uses_its_templates_and_scripts() {
    let emu = LineEmuHandle::with_config(EmuConfig::new(EmuFlavor::Plain));
    let mut p = DeviceProfile::for_kind(DeviceKind::Custom);
    p.gcode.custom.home = "$H\nG92 X0 Y0".into();
    p.gcode.custom.jog = "G91 G0 X{dx} Y{dy} F{feed}\nG90".into();
    p.gcode.custom.pause = "M0".into();
    p.gcode.custom.resume = "M108".into();
    p.gcode.air_assist.on = "M42 P5 S255".into();
    p.gcode.air_assist.off = "M42 P5 S0".into();
    p.gcode.start_script = "G21\nG90\nM42 P1 S255".into();
    p.gcode.end_script = "M5\nM42 P1 S0".into();
    p.gcode.s_max = 255.0;
    let (m, mut log) = connect(&p, &emu);
    assert_eq!(m.family(), MachineFamily::Custom);
    m.jog(JogStep { dx: 3.0, dy: 2.0, dz: 0.0, feed: 1200.0 }).unwrap();
    m.home().unwrap();
    assert!(log.wait(m.as_ref(), T, |e| matches!(e, MachineEvent::Homed)));
    m.set_air(true).unwrap();
    m.start_job(translated_job(&p, 6)).unwrap();
    assert!(log.wait(m.as_ref(), T, |e| matches!(e, MachineEvent::JobFinished(_))));
    let lines = emu.with(|e| e.stats().lines.clone());
    assert!(lines.contains(&"G91 G0 X3 Y2 F1200".to_owned()), "{lines:?}");
    assert!(lines.contains(&"$H".to_owned()) && lines.contains(&"G92 X0 Y0".to_owned()));
    assert!(lines.contains(&"M42 P5 S255".to_owned()));
    // Start and end scripts bracket the job; S was scaled to 0..255.
    let first = lines.iter().position(|l| l == "M42 P1 S255").unwrap();
    let last = lines.iter().position(|l| l == "M42 P1 S0").unwrap();
    assert!(first < last);
    assert!(lines.iter().any(|l| l.contains("S128")), "{lines:?}");
    // Pause with a template, line-at-a-time streaming.
    assert!(!m.capabilities().hardware_pause);
    m.disconnect();
}

#[test]
fn custom_device_without_templates_reports_unsupported() {
    let emu = LineEmuHandle::with_config(EmuConfig::new(EmuFlavor::Plain));
    let mut p = DeviceProfile::for_kind(DeviceKind::Custom);
    p.gcode.custom.home.clear();
    p.gcode.air_assist.on.clear();
    p.gcode.air_assist.off.clear();
    let (m, _l) = connect(&p, &emu);
    assert!(matches!(m.home(), Err(MachineError::Unsupported(_))));
    assert!(matches!(m.set_air(true), Err(MachineError::Unsupported(_))));
    assert!(matches!(m.read_settings(), Err(MachineError::Unsupported(_))));
    assert!(matches!(m.set_power_override(100), Err(MachineError::Unsupported(_))));
    m.disconnect();
}

// ----- GRBL adapter and the common interface -----

#[test]
fn grbl_adapter_looks_like_any_other_machine() {
    let emu = EmulatorHandle::new(GrblEmulator::new(EmulatorConfig::atomstack().with_time_scale(50.0)));
    let p = DeviceProfile::for_kind(DeviceKind::Grbl);
    let m = p.connect(Box::new(emu.transport())).unwrap();
    let mut log = Log::new();
    assert!(log.wait(m.as_ref(), T, |e| matches!(e, MachineEvent::Connected(_))));
    assert_eq!(m.family(), MachineFamily::Grbl);
    assert!(m.snapshot().firmware.starts_with("GRBL 1.1"));
    assert!(m.capabilities().hardware_pause && m.capabilities().jog_cancel);
    m.jog(JogStep { dx: 10.0, dy: 5.0, dz: 0.0, feed: 6000.0 }).unwrap();
    assert!(m.as_ref().wait_until(T, |s| (s.mpos[0] - 10.0).abs() < 0.01 && s.state == MachineState::Idle));
    m.start_job(burn_job(10)).unwrap();
    assert!(log.wait(m.as_ref(), T, |e| matches!(e, MachineEvent::JobFinished(_))));
    m.read_settings().unwrap();
    assert!(log.wait(m.as_ref(), T, |e| matches!(e, MachineEvent::Settings(_))));
    assert!(m.snapshot().settings.contains_key("$30"));
    m.write_setting("$30", "1000").unwrap();
    assert!(log.wait(m.as_ref(), T, |e| matches!(e, MachineEvent::SettingWritten { result: Ok(()), .. })));
    assert!(m.write_setting("S30", "1").is_err());
    m.disconnect();
}

/// One routine that works on any machine: connect state, a jog, a short job, stop.
fn exercise(m: &dyn GcodeMachine, mut log: Log, make_job: impl Fn() -> Job) {
    assert!(m.snapshot().is_connected());
    m.jog(JogStep { dx: 5.0, dy: 5.0, dz: 0.0, feed: 6000.0 }).unwrap();
    assert!(m.wait_until(T, |s| (s.mpos[0] - 5.0).abs() < 0.05 && s.activity == Activity::Idle));
    m.start_job(make_job()).unwrap();
    assert!(m.wait_until(T, |s| s.activity != Activity::Idle), "{:?}", m.snapshot().family);
    assert!(matches!(m.start_job(make_job()), Err(MachineError::Busy)), "a second job must be refused while one runs");
    assert!(matches!(m.jog(JogStep { dx: 1.0, dy: 0.0, dz: 0.0, feed: 100.0 }), Err(MachineError::Busy)));
    assert!(log.wait(m, Duration::from_secs(30), |e| matches!(e, MachineEvent::JobFinished(_))));
    assert!(m.wait_until(T, |s| s.activity == Activity::Idle && s.state == MachineState::Idle));
    m.start_job(make_job()).unwrap();
    m.stop().unwrap();
    assert!(log.wait(m, T, |e| matches!(e, MachineEvent::Stopped | MachineEvent::JobAborted { .. })));
    assert!(m.wait_until(T, |s| s.activity == Activity::Idle));
}

#[test]
fn every_machine_type_passes_the_same_routine() {
    // GRBL
    {
        let emu = EmulatorHandle::new(GrblEmulator::new(EmulatorConfig::atomstack().with_time_scale(50.0)));
        let m = DeviceProfile::for_kind(DeviceKind::Grbl).connect(Box::new(emu.transport())).unwrap();
        let mut log = Log::new();
        assert!(log.wait(m.as_ref(), T, |e| matches!(e, MachineEvent::Connected(_))));
        exercise(m.as_ref(), log, || burn_job(200));
        m.disconnect();
    }
    // Smoothieware, Marlin, Custom
    for (kind, flavor) in
        [(DeviceKind::Smoothie, EmuFlavor::Smoothie), (DeviceKind::Marlin, EmuFlavor::Marlin), (DeviceKind::Custom, EmuFlavor::Plain)]
    {
        let emu = LineEmuHandle::with_config(EmuConfig::new(flavor));
        let mut p = DeviceProfile::for_kind(kind);
        p.width_mm = 300.0;
        p.height_mm = 200.0;
        let (m, log) = connect(&p, &emu);
        let pc = p.clone();
        exercise(m.as_ref(), log, move || translated_job(&pc, 200));
        emu.with(|e| assert!(!e.laser_armed(), "{kind:?}: laser left armed"));
        m.disconnect();
    }
}

#[test]
fn profile_connect_refuses_file_only_and_design_only_devices() {
    let emu = LineEmuHandle::with_config(EmuConfig::new(EmuFlavor::Plain));
    for k in [DeviceKind::LinuxCnc, DeviceKind::DesignOnly] {
        let r = DeviceProfile::for_kind(k).connect(Box::new(emu.transport()));
        assert!(matches!(r, Err(MachineError::Unsupported(_))), "{k:?}");
    }
}

#[test]
fn losing_the_link_aborts_the_job_and_reports_it() {
    let emu = LineEmuHandle::with_config(EmuConfig::new(EmuFlavor::Marlin).with_time_scale(1.0));
    let p = marlin_profile();
    let (m, mut log) = connect(&p, &emu);
    m.start_job(translated_job(&p, 60)).unwrap();
    assert!(m.as_ref().wait_until(T, |s| s.job.as_ref().is_some_and(|j| j.lines_acked >= 4)));
    emu.with(|e| e.unplug());
    assert!(log.wait(m.as_ref(), T, |e| matches!(e, MachineEvent::Disconnected(Some(_)))));
    assert!(log.events.iter().any(|e| matches!(e, MachineEvent::JobAborted { .. })));
    assert_eq!(m.snapshot().laser_status(), LaserStatus::Disconnected);
    assert!(matches!(m.jog(JogStep { dx: 1.0, dy: 0.0, dz: 0.0, feed: 100.0 }), Err(MachineError::NotConnected)));
}

#[test]
fn console_lines_are_checked_and_echoed() {
    let emu = LineEmuHandle::with_config(EmuConfig::new(EmuFlavor::Smoothie));
    let (m, mut log) = connect(&smoothie_profile(), &emu);
    assert!(m.send_console("version\nM112").is_err(), "multi-line console input is refused");
    m.send_console("version").unwrap();
    assert!(log.wait(m.as_ref(), T, |e| matches!(e, MachineEvent::Console(c) if c.kind == darkburn_grbl::ConsoleKind::Sent && c.text == "version")));
    // The handshake printed the build line too; the console command prints it a second time.
    assert!(log.wait_n(m.as_ref(), T, 2, |e| matches!(e, MachineEvent::Console(c) if c.text.contains("MCU: LPC1769"))));
    m.run_macro("G91\nG0 X2\nG90 ; back").unwrap();
    assert!(m.as_ref().wait_until(T, |s| (s.mpos[0] - 2.0).abs() < 0.05));
    m.disconnect();
}

#[test]
fn bounds_check_and_busy_rules_apply() {
    let emu = LineEmuHandle::with_config(EmuConfig::new(EmuFlavor::Smoothie));
    let (m, _l) = connect(&smoothie_profile(), &emu);
    let job =
        burn_job(2).with_bounds(darkburn_grbl::BoundsCheck { job: Rect::new(0.0, 0.0, 400.0, 10.0), machine: Rect::new(0.0, 0.0, 300.0, 200.0) });
    assert!(matches!(m.start_job(job), Err(MachineError::OutOfBounds(_))));
    assert!(matches!(m.jog(JogStep { dx: 1.0, dy: 0.0, dz: 0.0, feed: 0.0 }), Err(MachineError::InvalidArgument(_))));
    assert!(m.frame_outline(&[], FrameOptions::default()).is_err());
    let _ = Point::new(0.0, 0.0);
    m.disconnect();
}

// ----- events and the snapshot -----

/// A link that is slow to come back from a read, like a serial port waiting out its timeout.
/// It stretches the time the worker spends between handling a command and its next pass.
struct SlowLink<T: Transport> {
    inner: T,
    delay: Duration,
}

impl<T: Transport> Transport for SlowLink<T> {
    fn write_all(&mut self, data: &[u8]) -> std::io::Result<()> {
        self.inner.write_all(data)
    }
    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> std::io::Result<usize> {
        std::thread::sleep(self.delay);
        self.inner.read(buf, timeout)
    }
    fn name(&self) -> String {
        self.inner.name()
    }
}

/// Reads events one by one until `pred` matches and returns the snapshot taken at that moment.
fn snapshot_at(m: &dyn GcodeMachine, pred: impl Fn(&MachineEvent) -> bool) -> MachineSnapshot {
    let end = Instant::now() + T;
    loop {
        let left = end.checked_duration_since(Instant::now()).expect("the event never came");
        if let Some(e) = m.next_event(left)
            && pred(&e)
        {
            return m.snapshot();
        }
    }
}

#[test]
fn a_fire_pulse_the_driver_reaches_too_late_is_dropped_not_fired_late() {
    // The link holds the driver up for longer than the pulse lasts.
    let emu = LineEmuHandle::with_config(EmuConfig::new(EmuFlavor::Marlin));
    let link = SlowLink { inner: emu.transport(), delay: Duration::from_millis(40) };
    let m = marlin_profile().connect(Box::new(link)).expect("connect");
    let m = m.as_ref();
    snapshot_at(m, |e| matches!(e, MachineEvent::Connected(_)));
    m.fire(5.0, Duration::from_millis(5)).unwrap();
    let s = snapshot_at(m, |e| matches!(e, MachineEvent::FireOff));
    assert!(!s.fire_on);
    emu.with(|e| {
        assert!(!e.stats().lines.iter().any(|l| l.starts_with("M3")), "{:?}", e.stats().lines);
        assert!(!e.laser_on());
    });
}

#[test]
fn the_snapshot_already_shows_what_an_event_reports() {
    for kind in [DeviceKind::Marlin, DeviceKind::Smoothie] {
        let flavor = if kind == DeviceKind::Marlin { EmuFlavor::Marlin } else { EmuFlavor::Smoothie };
        let emu = LineEmuHandle::with_config(EmuConfig::new(flavor));
        let p = if kind == DeviceKind::Marlin { marlin_profile() } else { smoothie_profile() };
        let link = SlowLink { inner: emu.transport(), delay: Duration::from_millis(30) };
        let m = p.connect(Box::new(link)).expect("connect");
        let m = m.as_ref();

        let s = snapshot_at(m, |e| matches!(e, MachineEvent::Connected(_)));
        assert_eq!(s.connection, ConnectionState::Connected, "{kind:?}");

        m.fire(5.0, Duration::from_millis(60)).unwrap();
        let s = snapshot_at(m, |e| matches!(e, MachineEvent::FireOff));
        assert!(!s.fire_on, "{kind:?}");

        m.start_job(translated_job(&p, 4)).unwrap();
        let s = snapshot_at(m, |e| matches!(e, MachineEvent::JobStarted));
        assert_eq!(s.activity, Activity::Job, "{kind:?}");
        let s = snapshot_at(m, |e| matches!(e, MachineEvent::JobFinished(_)));
        assert_eq!(s.activity, Activity::Idle, "{kind:?}");

        m.emergency_stop().unwrap();
        let s = snapshot_at(m, |e| matches!(e, MachineEvent::Alarm(_)));
        assert!(s.alarm.is_some(), "{kind:?}");
        assert_eq!(s.laser_status(), LaserStatus::Alarm, "{kind:?}");
    }
}
