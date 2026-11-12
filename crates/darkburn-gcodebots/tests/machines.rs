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
    assert!(m.as_ref().wait_until(T, |_| emu.with(|e| !e.air_on())), "the air stays on");
    m.disconnect();
}

#[test]
fn framing_draws_the_outline_with_the_laser_off_unless_asked() {
    let emu = LineEmuHandle::with_config(EmuConfig::new(EmuFlavor::Smoothie));
    // A lit frame needs "laser on when framing" in the profile (off by default).
    let mut p = smoothie_profile();
    p.gcode.laser_on_when_framing = true;
    let (m, mut log) = connect(&p, &emu);
    m.frame_rect(Rect::new(10.0, 10.0, 60.0, 40.0), FrameOptions { feed: 6000.0, laser_percent: None, return_to: None }).unwrap();
    assert!(log.wait(m.as_ref(), T, |e| matches!(e, MachineEvent::JobFinished(_))));
    emu.with(|e| {
        assert_eq!(e.stats().burned_mm, 0.0);
        let p = e.machine_position();
        assert!((p[0] - 10.0).abs() < 1e-6 && (p[1] - 10.0).abs() < 1e-6, "{p:?}");
    });
    // With the laser on, power is capped.
    assert!(matches!(
        m.frame_rect(Rect::new(0.0, 0.0, 10.0, 10.0), FrameOptions { feed: 3000.0, laser_percent: Some(60.0), return_to: None }),
        Err(MachineError::InvalidArgument(_))
    ));
    m.frame_rect(Rect::new(0.0, 0.0, 10.0, 10.0), FrameOptions { feed: 6000.0, laser_percent: Some(2.0), return_to: None }).unwrap();
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

/// Outside inline mode a Marlin keeps the laser at the last power until it is told otherwise,
/// moving or not. A pause in the middle of a cut must not leave it burning one spot, and the
/// rest of the cut must burn after the resume.
#[test]
fn marlin_pause_in_a_cut_switches_the_laser_off_and_resume_puts_it_back() {
    for mode in [MarlinPowerMode::M3, MarlinPowerMode::Fan] {
        let mut cfg = EmuConfig::new(EmuFlavor::Marlin).with_time_scale(1.0);
        if mode == MarlinPowerMode::Fan {
            cfg = cfg.with_laser_fan(0);
        }
        let emu = LineEmuHandle::with_config(cfg);
        let mut p = marlin_profile();
        p.gcode.marlin_power = mode;
        p.gcode.tool_index = 0;
        p.gcode.air_assist.on.clear();
        p.gcode.air_assist.off.clear();
        let (m, mut log) = connect(&p, &emu);
        // One cut of sixty pieces, 1 mm each.
        let mut src = vec!["G21".to_owned(), "G90".to_owned(), "M4 S0".to_owned(), "G0 X10 Y10".to_owned()];
        for i in 1..=60 {
            src.push(format!("G1 X{} Y10 S500 F3000", 10 + i));
        }
        src.push("M5".to_owned());
        let out = translate(src.iter().map(String::as_str), &TranslateOptions::for_profile(&p));
        let n = out.len();
        m.start_job(Job::from_lines(out).with_total(n)).unwrap();
        assert!(m.as_ref().wait_until(T, |s| s.job.as_ref().is_some_and(|j| j.lines_acked >= 12)), "{mode:?}");
        m.pause().unwrap();
        assert!(m.as_ref().wait_until(T, |s| s.paused));
        // What the controller already has runs out, then the head stands still.
        assert!(m.as_ref().wait_until(T, |_| emu.with(|e| e.planner_len() == 0)), "{mode:?}: the planner never ran empty");
        worker_passes(m.as_ref(), 25);
        let burned = emu.with(|e| {
            assert_eq!(e.planner_len(), 0, "{mode:?}");
            assert!(
                !e.laser_armed(),
                "{mode:?}: paused in a cut with the laser left on: {:?}",
                e.stats().lines.iter().rev().take(4).collect::<Vec<_>>()
            );
            e.stats().burned_mm
        });
        assert!(burned > 1.0 && burned < 59.0, "{mode:?}: paused inside the cut, {burned} mm burned");
        m.resume().unwrap();
        assert!(log.wait(m.as_ref(), T, |e| matches!(e, MachineEvent::JobFinished(_))), "{mode:?}");
        emu.with(|e| {
            assert!((e.stats().burned_mm - 60.0).abs() < 1e-6, "{mode:?}: the whole cut burns, {} mm", e.stats().burned_mm);
            assert_eq!(e.stats().rapid_burn_mm, 0.0, "{mode:?}");
            assert!(!e.laser_armed(), "{mode:?}");
        });
        m.disconnect();
    }
}

/// A controller answers its lines in the order they came in. A position request sent while the
/// last moves of a job still run is answered when they are done, behind the `M400` that ends
/// the job. Its answer is never taken for the `M400`'s: the job is not finished before the
/// motion is.
#[test]
fn a_position_request_behind_the_last_moves_does_not_end_the_job_early() {
    let emu = LineEmuHandle::with_config(EmuConfig::new(EmuFlavor::Marlin).with_time_scale(1.0));
    let mut p = marlin_profile();
    p.gcode.marlin_power = MarlinPowerMode::M3;
    p.gcode.air_assist.on.clear();
    p.gcode.air_assist.off.clear();
    let (m, mut log) = connect(&p, &emu);
    // Eight pieces of 5 mm at 50 mm/s: 0.8 s of motion, all of it in the planner at once.
    let mut src = vec!["G21".to_owned(), "G90".to_owned(), "M4 S0".to_owned(), "G0 X10 Y10".to_owned()];
    for i in 1..=8 {
        src.push(format!("G1 X{} Y10 S500 F3000", 10 + 5 * i));
    }
    src.push("M5".to_owned());
    let out = translate(src.iter().map(String::as_str), &TranslateOptions::for_profile(&p));
    let n = out.len();
    m.start_job(Job::from_lines(out).with_total(n)).unwrap();
    // The controller has every line and the `M400`; the moves have only begun.
    let arrived = |text: &str| emu.with(|e| e.stats().arrived.iter().filter(|l| *l == text).count());
    assert!(m.as_ref().wait_until(T, |_| arrived("M400") == 1), "the job never came to its end");
    let polls = arrived("M114");
    m.request_status().unwrap();
    assert!(m.as_ref().wait_until(T, |_| arrived("M114") > polls), "the position request was not sent");
    assert!(log.wait(m.as_ref(), T, |e| matches!(e, MachineEvent::JobFinished(_))));
    emu.with(|e| {
        assert_eq!(e.planner_len(), 0, "the job was reported finished with moves still to run");
        assert!((e.stats().burned_mm - 40.0).abs() < 1e-6, "the whole cut burns, {} mm", e.stats().burned_mm);
    });
    m.disconnect();
}

/// A cut of `pieces` pieces of `mm` each along X at 50 mm/s, translated for the profile.
fn one_cut(p: &DeviceProfile, pieces: usize, mm: usize) -> Job {
    let mut src = vec!["G21".to_owned(), "G90".to_owned(), "M4 S0".to_owned(), "G0 X10 Y10".to_owned()];
    for i in 1..=pieces {
        src.push(format!("G1 X{} Y10 S500 F3000", 10 + mm * i));
    }
    src.push("M5".to_owned());
    let out = translate(src.iter().map(String::as_str), &TranslateOptions::for_profile(p));
    let n = out.len();
    Job::from_lines(out).with_total(n)
}

/// Pause and Resume that reach the driver in one breath go out in that order: laser off, then
/// laser on. The other way round the rest of the cut runs with the laser off.
#[test]
fn pause_and_resume_in_one_breath_keep_their_order() {
    for round in 0..3 {
        let emu = LineEmuHandle::with_config(EmuConfig::new(EmuFlavor::Marlin).with_time_scale(4.0));
        let mut p = marlin_profile();
        p.gcode.marlin_power = MarlinPowerMode::M3;
        p.gcode.air_assist.on.clear();
        p.gcode.air_assist.off.clear();
        let (m, mut log) = connect(&p, &emu);
        m.start_job(one_cut(&p, 60, 1)).unwrap();
        assert!(m.as_ref().wait_until(T, |s| s.job.as_ref().is_some_and(|j| j.lines_acked >= 12)));
        m.pause().unwrap();
        m.resume().unwrap();
        assert!(log.wait(m.as_ref(), T, |e| matches!(e, MachineEvent::JobFinished(_))));
        emu.with(|e| {
            let switches: Vec<&String> = e.stats().arrived.iter().filter(|l| l.starts_with("M3") || l.starts_with("M5")).collect();
            assert!((e.stats().burned_mm - 60.0).abs() < 1e-6, "round {round}: the whole cut burns, {} mm: {switches:?}", e.stats().burned_mm);
            assert!(!e.laser_armed());
        });
        m.disconnect();
    }
}

/// A pause that comes when the controller already has every line holds nothing back: the job
/// runs to its end. It must not leave the driver or the controller paused afterwards, with
/// the next job refused.
#[test]
fn a_pause_in_the_last_moments_of_a_job_does_not_outlast_it() {
    for (flavor, p) in [(EmuFlavor::Smoothie, smoothie_profile()), (EmuFlavor::Marlin, marlin_profile())] {
        let emu = LineEmuHandle::with_config(EmuConfig::new(flavor).with_time_scale(1.0));
        let (m, mut log) = connect(&p, &emu);
        // 0.8 s of motion, all of it in the planner at once.
        m.start_job(one_cut(&p, 8, 5)).unwrap();
        assert!(m.as_ref().wait_until(T, |_| emu.with(|e| e.stats().arrived.iter().any(|l| l == "M400"))), "{flavor:?}");
        m.pause().unwrap();
        assert!(log.wait(m.as_ref(), T, |e| matches!(e, MachineEvent::JobFinished(_))), "{flavor:?}");
        emu.with(|e| {
            assert_eq!(e.planner_len(), 0, "{flavor:?}: finished with moves still to run");
            assert!((e.stats().burned_mm - 40.0).abs() < 1e-6, "{flavor:?}: the whole cut burns, {} mm", e.stats().burned_mm);
        });
        assert!(m.as_ref().wait_until(T, |s| !s.paused), "{flavor:?}: still paused after the job ended");
        assert!(
            m.as_ref().wait_until(T, |s| matches!(s.state, MachineState::Idle) && emu.with(|e| !e.is_paused())),
            "{flavor:?}: {:?}",
            m.snapshot().state
        );
        // The next job runs.
        m.start_job(one_cut(&p, 8, 5)).unwrap();
        assert!(log.wait_n(m.as_ref(), T, 2, |e| matches!(e, MachineEvent::JobFinished(_))), "{flavor:?}: the next job never finished");
        emu.with(|e| assert!((e.stats().burned_mm - 80.0).abs() < 1e-6, "{flavor:?}: {} mm", e.stats().burned_mm));
        m.disconnect();
    }
}

/// A Pause that comes while a stop is under way pauses nothing. Its command must not reach
/// the controller: the stop ends the driver's pause, and the controller would be left
/// paused with the next job standing still.
#[test]
fn a_pause_during_a_stop_is_not_sent() {
    let emu = LineEmuHandle::with_config(EmuConfig::new(EmuFlavor::Plain).with_time_scale(1.0));
    let mut p = DeviceProfile::for_kind(DeviceKind::Custom);
    p.gcode.s_max = 255.0;
    (p.gcode.start_script, p.gcode.end_script) = (String::new(), String::new());
    p.gcode.custom.pause = "M107 P3".into();
    p.gcode.custom.resume = "M106 P3".into();
    p.gcode.custom.stop = "M410".into();
    let (m, mut log) = connect(&p, &emu);
    m.start_job(long_cut_job(&p)).unwrap();
    assert!(m.as_ref().wait_until(T, |_| emu.with(|e| e.laser_on() && e.planner_len() >= 1)), "never started cutting");
    m.stop().unwrap();
    m.pause().unwrap();
    assert!(log.wait(m.as_ref(), T, |e| matches!(e, MachineEvent::Stopped)));
    worker_passes(m.as_ref(), 25);
    let tail = emu.with(|e| e.stats().arrived[e.stats().arrived.len().saturating_sub(5)..].to_vec());
    assert!(!tail.iter().any(|l| l == "M107 P3"), "the pause command went out during the stop: {tail:?}");
    assert!(!m.snapshot().paused);
    m.disconnect();
}

/// A stop while the machine homes: the answer that comes back is the stop's, not the
/// homing's. The driver must not say the machine is homed.
#[test]
fn a_homing_that_a_stop_cut_off_is_not_reported_as_done() {
    let emu = LineEmuHandle::with_config(EmuConfig::new(EmuFlavor::Marlin).with_time_scale(1.0));
    let p = marlin_profile();
    let (m, mut log) = connect(&p, &emu);
    m.home().unwrap();
    assert!(m.as_ref().wait_until(T, |_| emu.with(|e| e.stats().arrived.iter().any(|l| l.starts_with("G28")))), "homing was never sent");
    m.stop().unwrap();
    assert!(log.wait(m.as_ref(), T, |e| matches!(e, MachineEvent::Stopped)));
    assert!(!emu.with(|e| e.homed()), "the stop came too late for this test: the homing was done");
    assert_eq!(log.count(|e| matches!(e, MachineEvent::Homed)), 0, "homed was reported for a homing that was cut off");
    m.disconnect();
}

/// A controller that answers out of turn. Every line gets its `ok` at once, a position request
/// too, but `M400` is answered only when the test says the motion is done. No firmware is
/// known to work like this; the driver must not report a job finished early if one does.
#[derive(Clone, Default)]
struct OutOfTurn(std::sync::Arc<std::sync::Mutex<OutOfTurnState>>);

#[derive(Default)]
struct OutOfTurnState {
    rx: Vec<u8>,
    out: Vec<u8>,
    lines: Vec<String>,
    m400_waits: bool,
    motion_done: bool,
}

impl OutOfTurn {
    fn sent(&self, text: &str) -> usize {
        self.0.lock().unwrap().lines.iter().filter(|l| *l == text).count()
    }
}

impl Transport for OutOfTurn {
    fn write_all(&mut self, data: &[u8]) -> std::io::Result<()> {
        let mut s = self.0.lock().unwrap();
        for &b in data {
            if b != b'\n' {
                s.rx.push(b);
                continue;
            }
            let line = String::from_utf8_lossy(&std::mem::take(&mut s.rx)).trim().to_owned();
            match line.as_str() {
                "" => continue,
                "M400" => s.m400_waits = true,
                "M115" => s.out.extend_from_slice(b"FIRMWARE_NAME:Marlin 2.1.2 (Github) PROTOCOL_VERSION:1.0 MACHINE_TYPE:Laser\nok\n"),
                "M114" => s.out.extend_from_slice(b"X:0.00 Y:0.00 Z:0.00 E:0.00 Count X:0 Y:0 Z:0\nok\n"),
                _ => s.out.extend_from_slice(b"ok\n"),
            }
            s.lines.push(line);
        }
        Ok(())
    }
    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> std::io::Result<usize> {
        let end = Instant::now() + timeout;
        loop {
            {
                let mut s = self.0.lock().unwrap();
                if s.m400_waits && s.motion_done {
                    s.m400_waits = false;
                    s.out.extend_from_slice(b"ok\n");
                }
                if !s.out.is_empty() {
                    let n = buf.len().min(s.out.len());
                    buf[..n].copy_from_slice(&s.out[..n]);
                    s.out.drain(..n);
                    return Ok(n);
                }
            }
            if Instant::now() >= end {
                return Ok(0);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    fn name(&self) -> String {
        "out-of-turn".to_owned()
    }
}

/// While the answer to the `M400` that ends a job is owed, the driver sends no position request
/// of its own. Behind the `M400` it could not be answered before the motion ends anyway, and
/// from a controller that answers out of turn its `ok` would be taken for the `M400`'s.
#[test]
fn the_driver_asks_for_no_position_while_it_waits_for_the_end_of_the_motion() {
    let link = OutOfTurn::default();
    let p = marlin_profile();
    let m = p.connect(Box::new(link.clone())).expect("connect");
    let mut log = Log::new();
    assert!(log.wait(m.as_ref(), T, |e| matches!(e, MachineEvent::Connected(_) | MachineEvent::ConnectFailed(_))), "no handshake result");
    assert_eq!(m.snapshot().connection, ConnectionState::Connected);
    m.start_job(one_cut(&p, 4, 5)).unwrap();
    assert!(m.as_ref().wait_until(T, |_| link.sent("M400") == 1), "the job never came to its end");
    let asked = link.sent("M114");
    // Three of the driver's position requests would fall into this wait.
    std::thread::sleep(Duration::from_millis(1600));
    log.drain(m.as_ref());
    assert_eq!(log.count(|e| matches!(e, MachineEvent::JobFinished(_))), 0, "finished before the motion was");
    assert_eq!(link.sent("M114"), asked, "a position request went out while the M400 was unanswered");
    link.0.lock().unwrap().motion_done = true;
    assert!(log.wait(m.as_ref(), T, |e| matches!(e, MachineEvent::JobFinished(_))));
    // The position requests go on afterwards.
    assert!(m.as_ref().wait_until(T, |_| link.sent("M114") > asked));
    m.disconnect();
}

/// A custom G-code controller is not known to switch the laser off by itself when the head
/// stands still: it holds the last `S` of an `M3`/`M4`. A pause in the middle of a cut must
/// switch it off, and the rest of the cut must burn after the resume.
#[test]
fn custom_pause_in_a_cut_switches_the_laser_off_and_resume_puts_it_back() {
    let emu = LineEmuHandle::with_config(EmuConfig::new(EmuFlavor::Plain).with_time_scale(1.0));
    let mut p = DeviceProfile::for_kind(DeviceKind::Custom);
    p.gcode.s_max = 255.0;
    p.gcode.transfer = TransferMode::Buffered;
    (p.gcode.start_script, p.gcode.end_script) = (String::new(), String::new());
    let (m, mut log) = connect(&p, &emu);
    // One cut of sixty pieces, 1 mm each, then a second cut after a travel move.
    let mut src = vec!["G21".to_owned(), "G90".to_owned(), "M3 S0".to_owned(), "G0 X10 Y10".to_owned()];
    for i in 1..=60 {
        src.push(format!("G1 X{} Y10{}", 10 + i, if i == 1 { " S500 F3000" } else { "" }));
    }
    src.push("G0 X10 Y20".to_owned());
    src.push("G1 X20 Y20".to_owned());
    src.push("M5".to_owned());
    let out = translate(src.iter().map(String::as_str), &TranslateOptions::for_profile(&p));
    let n = out.len();
    m.start_job(Job::from_lines(out).with_total(n)).unwrap();
    assert!(m.as_ref().wait_until(T, |s| s.job.as_ref().is_some_and(|j| j.lines_acked >= 12)));
    m.pause().unwrap();
    assert!(m.as_ref().wait_until(T, |s| s.paused));
    // What the controller already has runs out, then the head stands still.
    assert!(m.as_ref().wait_until(T, |_| emu.with(|e| e.planner_len() == 0)), "the planner never ran empty");
    worker_passes(m.as_ref(), 25);
    let burned = emu.with(|e| {
        assert_eq!(e.planner_len(), 0);
        assert!(!e.laser_armed(), "paused in a cut with the laser left on: {:?}", e.stats().lines.iter().rev().take(4).collect::<Vec<_>>());
        e.stats().burned_mm
    });
    assert!(burned > 1.0 && burned < 59.0, "paused inside the cut, {burned} mm burned");
    m.resume().unwrap();
    assert!(log.wait(m.as_ref(), T, |e| matches!(e, MachineEvent::JobFinished(_))));
    emu.with(|e| {
        assert!((e.stats().burned_mm - 70.0).abs() < 1e-6, "both cuts burn whole, {} mm", e.stats().burned_mm);
        assert_eq!(e.stats().rapid_burn_mm, 0.0);
        assert!(e.stats().burns.iter().all(|b| (b.power - 128.0 / 255.0).abs() < 1e-9), "the power after the resume is the cut's own");
        assert!(!e.laser_armed());
    });
    m.disconnect();
}

/// Pause, resume and fire-off templates of more than one line go out in the order they are
/// written, the laser-off of a pause ahead of them and the laser-on of a resume behind them.
#[test]
fn templates_of_several_lines_go_out_in_the_order_written() {
    let emu = LineEmuHandle::with_config(EmuConfig::new(EmuFlavor::Plain).with_time_scale(1.0));
    let mut p = DeviceProfile::for_kind(DeviceKind::Custom);
    p.gcode.s_max = 255.0;
    p.gcode.transfer = TransferMode::Buffered;
    (p.gcode.start_script, p.gcode.end_script) = (String::new(), String::new());
    p.gcode.custom.pause = "M107\nM9".into();
    p.gcode.custom.resume = "M8\nM106 S255".into();
    p.gcode.custom.fire_on = "M3 S{s}".into();
    p.gcode.custom.fire_off = "M5\nM107 P2".into();
    let (m, mut log) = connect(&p, &emu);
    let from = |mark: usize| emu.with(|e| e.stats().arrived[mark..].iter().filter(|l| l.starts_with('M')).cloned().collect::<Vec<_>>());
    let mark = emu.with(|e| e.stats().arrived.len());
    m.fire(5.0, Duration::from_secs(5)).unwrap();
    assert!(m.as_ref().wait_until(T, |_| emu.with(|e| e.laser_armed())));
    m.fire_off().unwrap();
    assert!(m.as_ref().wait_until(T, |_| from(mark).len() >= 3), "{:?}", from(mark));
    assert_eq!(from(mark)[1..], ["M5", "M107 P2"], "fire off");

    m.start_job(one_cut(&p, 60, 1)).unwrap();
    assert!(m.as_ref().wait_until(T, |s| s.job.as_ref().is_some_and(|j| j.lines_acked >= 12)));
    let mark = emu.with(|e| e.stats().arrived.len());
    m.pause().unwrap();
    assert!(m.as_ref().wait_until(T, |_| from(mark).len() >= 3), "{:?}", from(mark));
    assert_eq!(from(mark), ["M5", "M107", "M9"], "pause");
    let mark = emu.with(|e| e.stats().arrived.len());
    m.resume().unwrap();
    assert!(m.as_ref().wait_until(T, |_| from(mark).len() >= 3), "{:?}", from(mark));
    // The job's own word and power: `M4`, half of full power.
    assert_eq!(from(mark)[..3], ["M8", "M106 S255", "M4 S128"], "resume");
    assert!(log.wait(m.as_ref(), T, |e| matches!(e, MachineEvent::JobFinished(_))));
    emu.with(|e| assert!((e.stats().burned_mm - 60.0).abs() < 1e-6, "the whole cut burns, {} mm", e.stats().burned_mm));
    m.disconnect();
}

/// A paused travel move of the same machine: nothing to switch off, and the resume must not
/// switch the laser on over the travel that follows.
#[test]
fn custom_pause_between_cuts_does_not_fire_on_resume() {
    let emu = LineEmuHandle::with_config(EmuConfig::new(EmuFlavor::Plain).with_time_scale(1.0));
    let mut p = DeviceProfile::for_kind(DeviceKind::Custom);
    p.gcode.s_max = 255.0;
    (p.gcode.start_script, p.gcode.end_script) = (String::new(), String::new());
    let (m, mut log) = connect(&p, &emu);
    let mut src = vec!["G21".to_owned(), "G90".to_owned(), "M3 S0".to_owned(), "G0 X10 Y10".to_owned(), "G1 X20 Y10 S500 F3000".to_owned()];
    for i in 1..=40 {
        src.push(format!("G0 X{} Y20", 20 + i));
    }
    src.push("G1 X70 Y20".to_owned());
    src.push("M5".to_owned());
    let out = translate(src.iter().map(String::as_str), &TranslateOptions::for_profile(&p));
    let n = out.len();
    m.start_job(Job::from_lines(out).with_total(n)).unwrap();
    assert!(m.as_ref().wait_until(T, |s| s.job.as_ref().is_some_and(|j| j.lines_acked >= 12)));
    m.pause().unwrap();
    assert!(m.as_ref().wait_until(T, |s| s.paused));
    assert!(m.as_ref().wait_until(T, |_| emu.with(|e| e.planner_len() == 0)));
    worker_passes(m.as_ref(), 25);
    emu.with(|e| assert!(!e.laser_armed()));
    m.resume().unwrap();
    assert!(log.wait(m.as_ref(), T, |e| matches!(e, MachineEvent::JobFinished(_))));
    emu.with(|e| {
        assert!((e.stats().burned_mm - 20.0).abs() < 1e-6, "{} mm", e.stats().burned_mm);
        assert_eq!(e.stats().rapid_burn_mm, 0.0);
        assert!(!e.laser_armed());
    });
    m.disconnect();
}

/// A job that does not end with its own laser off (a G-code file run as it is) on a machine
/// that holds the laser on: the driver's own laser off comes before its move to the finish
/// position, not after it.
#[test]
fn the_finish_move_of_a_job_never_burns() {
    let emu = LineEmuHandle::with_config(EmuConfig::new(EmuFlavor::Marlin).with_time_scale(1.0));
    let mut p = marlin_profile();
    p.gcode.marlin_power = MarlinPowerMode::M3;
    let (m, mut log) = connect(&p, &emu);
    let lines = ["G21", "G90", "G0 X10 Y10", "M3 S128", "G1 X20 Y10 F3000"].map(str::to_owned);
    m.start_job(Job::from_lines(lines).with_total(5).with_finish(darkburn_grbl::FinishPosition::Origin)).unwrap();
    assert!(log.wait(m.as_ref(), T, |e| matches!(e, MachineEvent::JobFinished(_))));
    emu.with(|e| {
        assert_eq!(e.stats().rapid_burn_mm, 0.0, "the move to the finish position burned: {:?}", e.stats().lines);
        assert!((e.stats().burned_mm - 10.0).abs() < 1e-6);
        assert!(!e.laser_armed());
        assert!(e.machine_position()[0].abs() < 1e-6 && e.machine_position()[1].abs() < 1e-6, "the head is at the finish position");
    });
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

#[test]
fn grbl_air_assist_leaves_the_fire_button_on() {
    let emu = EmulatorHandle::new(GrblEmulator::new(EmulatorConfig::atomstack()));
    let p = DeviceProfile::for_kind(DeviceKind::Grbl);
    let m = p.connect(Box::new(emu.transport())).unwrap();
    let mut log = Log::new();
    assert!(log.wait(m.as_ref(), T, |e| matches!(e, MachineEvent::Connected(_))));
    m.fire(1.0, Duration::from_secs(8)).unwrap();
    assert!(m.as_ref().wait_until(T, |s| s.fire_on && emu.with(|e| e.laser_on())));
    m.set_air(true).unwrap();
    assert!(m.as_ref().wait_until(T, |_| emu.with(|e| e.stats().lines.iter().any(|l| l == "M8"))), "the air command arrives");
    worker_passes(m.as_ref(), 3);
    assert!(m.snapshot().fire_on, "the fire button is still on");
    emu.with(|e| {
        assert!(e.laser_on(), "the laser is still on");
        assert!(!e.stats().lines.iter().any(|l| l.starts_with("M5")), "no laser-off line was sent: {:?}", e.stats().lines);
    });
    m.set_air(false).unwrap();
    assert!(m.as_ref().wait_until(T, |_| emu.with(|e| e.stats().lines.iter().any(|l| l == "M9"))));
    assert!(m.snapshot().fire_on);
    m.fire_off().unwrap();
    assert!(m.as_ref().wait_until(T, |s| !s.fire_on && !emu.with(|e| e.laser_on())));
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

// ----- stop, disconnect and the fire button around other commands -----

/// A link whose answers arrive late: every read is held back for `delay` after a write that
/// holds `after`, like a slow USB serial adapter.
struct LateAnswers<T: Transport> {
    inner: T,
    after: u8,
    delay: Duration,
    quiet_until: Option<Instant>,
}

impl<T: Transport> Transport for LateAnswers<T> {
    fn write_all(&mut self, data: &[u8]) -> std::io::Result<()> {
        if data.contains(&self.after) {
            self.quiet_until = Some(Instant::now() + self.delay);
        }
        self.inner.write_all(data)
    }
    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> std::io::Result<usize> {
        if self.quiet_until.is_some_and(|t| Instant::now() < t) {
            std::thread::sleep(timeout.min(Duration::from_millis(2)));
            return Ok(0);
        }
        self.inner.read(buf, timeout)
    }
    fn name(&self) -> String {
        self.inner.name()
    }
}

#[test]
fn smoothie_stop_clears_the_halt_even_when_the_alarm_line_comes_late() {
    let emu = LineEmuHandle::with_config(EmuConfig::new(EmuFlavor::Smoothie).with_time_scale(1.0));
    let p = smoothie_profile();
    let link = LateAnswers { inner: emu.transport(), after: 0x18, delay: Duration::from_millis(120), quiet_until: None };
    let m = p.connect(Box::new(link)).expect("connect");
    let mut log = Log::new();
    assert!(log.wait(m.as_ref(), T, |e| matches!(e, MachineEvent::Connected(_))));
    m.start_job(translated_job(&p, 60)).unwrap();
    assert!(m.as_ref().wait_until(T, |s| s.job.as_ref().is_some_and(|j| j.lines_acked >= 10)));
    m.stop().unwrap();
    assert!(log.wait(m.as_ref(), T, |e| matches!(e, MachineEvent::Stopped)));
    worker_passes(m.as_ref(), 100);
    emu.with(|e| {
        assert!(!e.laser_on());
        assert!(
            e.stats().arrived.iter().any(|l| l == "M999"),
            "the halt was never cleared: {:?}",
            &e.stats().arrived[e.stats().arrived.len().saturating_sub(4)..]
        );
        assert!(!e.is_halted(), "M999 should have cleared the halt");
    });
    let s = m.snapshot();
    assert_eq!(s.activity, Activity::Idle);
    assert!(s.alarm.is_none(), "{:?}", s.alarm);
    m.disconnect();
}

fn long_cut_job(p: &DeviceProfile) -> Job {
    // Six 200 mm cuts at 1200 mm/min: ten seconds each.
    let mut src = vec!["G21".to_owned(), "G90".to_owned(), "M4 S0".to_owned(), "G0 X10 Y10".to_owned()];
    for i in 0..6 {
        let x = if i % 2 == 0 { 210.0 } else { 10.0 };
        src.push(format!("G1 X{x} Y{} S500 F1200", 10 + i));
    }
    src.push("M5".to_owned());
    let out = translate(src.iter().map(String::as_str), &TranslateOptions::for_profile(p));
    let n = out.len();
    Job::from_lines(out).with_total(n)
}

#[test]
fn disconnecting_in_the_middle_of_a_job_stops_the_cut() {
    for (flavor, p) in [(EmuFlavor::Marlin, marlin_profile()), (EmuFlavor::Smoothie, smoothie_profile())] {
        let emu = LineEmuHandle::with_config(EmuConfig::new(flavor).with_time_scale(1.0));
        let (m, _log) = connect(&p, &emu);
        m.start_job(long_cut_job(&p)).unwrap();
        assert!(m.as_ref().wait_until(T, |_| emu.with(|e| e.laser_on() && e.planner_len() >= 1)), "{flavor:?}: never started cutting");
        m.disconnect();
        let burned = emu.with(|e| e.stats().burned_mm);
        // The controller lives on after the program let go of it.
        emu.with(|e| e.advance(Duration::from_secs(30)));
        emu.with(|e| {
            let after = e.stats().burned_mm;
            assert!(after - burned < 1.0, "{flavor:?}: the machine cut another {:.0} mm after Disconnect", after - burned);
        });
    }
}

#[test]
fn a_job_started_while_the_fire_button_is_on_keeps_its_laser() {
    let emu = LineEmuHandle::with_config(EmuConfig::new(EmuFlavor::Marlin).with_time_scale(1.0));
    let p = marlin_profile();
    let (m, mut log) = connect(&p, &emu);
    // The fire button's own timer runs out while the job is cutting.
    m.fire(5.0, Duration::from_millis(600)).unwrap();
    assert!(m.as_ref().wait_until(T, |_| emu.with(|e| e.laser_armed())));
    m.start_job(translated_job(&p, 40)).unwrap();
    assert!(log.wait(m.as_ref(), Duration::from_secs(30), |e| matches!(e, MachineEvent::JobFinished(_))));
    let lines = emu.with(|e| e.stats().arrived.clone());
    let first = lines.iter().position(|l| l.starts_with("G1")).expect("the job cut something");
    let last = lines.iter().rposition(|l| l.starts_with("G1")).expect("the job cut something");
    assert!(last - first > 70, "{lines:?}");
    // The job switches the laser off itself for every travel move (`M5`, then `G0`) and on again
    // before the next cut. An `M5` anywhere else is the fire button's timer cutting into the job.
    let job: Vec<&String> = lines[first..last].iter().filter(|l| *l != "M114").collect();
    for (i, l) in job.iter().enumerate() {
        if *l == "M5" {
            assert!(job.get(i + 1).is_some_and(|n| n.starts_with("G0")), "the laser was switched off in the middle of the job: {lines:?}");
        }
        if l.starts_with("G1") && i > 0 {
            assert!(job[i - 1].starts_with("M3 S") || job[i - 1].starts_with("G1"), "a cut without its power before it: {lines:?}");
        }
    }
    assert!(lines[..first].iter().any(|l| l == "M5"), "the fire button is switched off before the job: {lines:?}");
    assert!(!m.snapshot().fire_on);
    m.disconnect();
}

#[test]
fn homing_switches_the_fire_button_off_first() {
    for (flavor, p) in [(EmuFlavor::Marlin, marlin_profile()), (EmuFlavor::Smoothie, smoothie_profile())] {
        let emu = LineEmuHandle::with_config(EmuConfig::new(flavor).with_time_scale(1.0));
        let (m, mut log) = connect(&p, &emu);
        m.fire(5.0, Duration::from_secs(8)).unwrap();
        assert!(m.as_ref().wait_until(T, |_| emu.with(|e| e.laser_on() || e.laser_armed())), "{flavor:?}");
        m.home().unwrap();
        assert!(log.wait(m.as_ref(), T, |e| matches!(e, MachineEvent::Homed)), "{flavor:?}");
        emu.with(|e| {
            let a = &e.stats().arrived;
            let home = a.iter().rposition(|l| l.starts_with("G28") || l.starts_with("$H")).expect("homing was sent");
            let off = a.iter().position(|l| l == "M5" || l == "fire off" || l.starts_with("M107"));
            assert!(off.is_some_and(|i| i < home), "{flavor:?}: the laser stayed on while homing: {a:?}");
            assert!(!e.laser_on());
        });
        assert!(!m.snapshot().fire_on);
        m.disconnect();
    }
}

#[test]
fn not_a_number_is_refused_by_every_machine() {
    let emu = LineEmuHandle::with_config(EmuConfig::new(EmuFlavor::Marlin));
    let (m, _log) = connect(&marlin_profile(), &emu);
    assert!(m.jog(JogStep { dx: 1.0, dy: 0.0, dz: 0.0, feed: f64::NAN }).is_err());
    assert!(m.jog(JogStep { dx: f64::NAN, dy: 0.0, dz: 0.0, feed: 600.0 }).is_err());
    assert!(m.jog(JogStep { dx: 0.0, dy: 0.0, dz: f64::INFINITY, feed: 600.0 }).is_err());
    assert!(m.frame_rect(Rect::new(1.0, 1.0, 5.0, 5.0), FrameOptions { feed: f64::NAN, laser_percent: None, return_to: None }).is_err());
    assert!(
        m.move_to(darkburn_grbl::MoveTarget { x: Some(1.0), y: None, z: Some(f64::NAN), coords: darkburn_grbl::Coords::Work, feed: 600.0 }).is_err()
    );
    assert!(m.move_to(darkburn_grbl::MoveTarget { x: Some(1.0), y: None, z: None, coords: darkburn_grbl::Coords::Work, feed: f64::NAN }).is_err());
    worker_passes(m.as_ref(), 20);
    emu.with(|e| assert!(!e.stats().arrived.iter().any(|l| l.to_ascii_lowercase().contains("nan") || l.contains("inf")), "{:?}", e.stats().arrived));
    m.disconnect();
}
