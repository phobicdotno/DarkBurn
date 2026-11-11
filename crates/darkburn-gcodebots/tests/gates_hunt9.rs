//! Pass 9 of the bug hunt, safety gates as a matrix: the controller halts (kill button, limit
//! switch) in every phase, against the line machines. In-memory emulators only.

use std::time::{Duration, Instant};

use darkburn_gcodebots::emu::{EmuConfig, EmuFlavor, LineEmuHandle};
use darkburn_gcodebots::profile::{DeviceKind, DeviceProfile, TransferMode};
use darkburn_gcodebots::translate::{TranslateOptions, translate};
use darkburn_gcodebots::{GcodeMachine, MachineError, MachineEvent};
use darkburn_grbl::{Activity, FrameOptions, Job, JogStep, LaserStatus};
use kurbo::Rect;

const T: Duration = Duration::from_secs(8);

fn profile(kind: DeviceKind) -> DeviceProfile {
    let mut p = DeviceProfile::for_kind(kind);
    p.width_mm = 300.0;
    p.height_mm = 200.0;
    p.gcode.transfer = TransferMode::Buffered;
    p
}

fn rig(kind: DeviceKind, flavor: EmuFlavor) -> (Box<dyn GcodeMachine>, LineEmuHandle, DeviceProfile) {
    let p = profile(kind);
    let emu = LineEmuHandle::with_config(EmuConfig::new(flavor).with_time_scale(1.0));
    let m = p.connect(Box::new(emu.transport())).expect("connect");
    let end = Instant::now() + T;
    while !m.poll_events().iter().any(|e| matches!(e, MachineEvent::Connected(_))) {
        assert!(Instant::now() < end, "no handshake");
        std::thread::sleep(Duration::from_millis(2));
    }
    (m, emu, p)
}

fn smoothie() -> (Box<dyn GcodeMachine>, LineEmuHandle, DeviceProfile) {
    rig(DeviceKind::Smoothie, EmuFlavor::Smoothie)
}

/// Six 200 mm cuts at 1200 mm/min: ten seconds each.
fn long_cut(p: &DeviceProfile) -> Job {
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

/// A job of many short cuts: most of it is still in the program when the controller halts.
fn many_cuts(p: &DeviceProfile) -> Job {
    let mut src = vec!["G21".to_owned(), "G90".to_owned(), "M4 S0".to_owned()];
    for i in 0..400 {
        let y = (i / 2) as f64 * 0.2;
        let (x0, x1) = if i % 2 == 0 { (10.0, 60.0) } else { (60.0, 10.0) };
        src.push(format!("G0 X{x0} Y{y}"));
        src.push(format!("G1 X{x1} Y{y} S500 F1200"));
    }
    src.push("M5".to_owned());
    let out = translate(src.iter().map(String::as_str), &TranslateOptions::for_profile(p));
    let n = out.len();
    Job::from_lines(out).with_total(n)
}

fn wait_emu(emu: &LineEmuHandle, what: &str, f: impl Fn(&mut darkburn_gcodebots::emu::LineEmulator) -> bool) {
    let end = Instant::now() + T;
    while Instant::now() < end {
        if emu.with(&f) {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!("timed out waiting for the emulator: {what}");
}

fn moves_arrived(emu: &LineEmuHandle) -> usize {
    emu.with(|e| e.stats().arrived.iter().filter(|l| l.starts_with("G0") || l.starts_with("G1")).count())
}

/// After a halt: the driver shows no job, no pause and no fire, the laser is off, and clearing
/// the halt brings nothing back.
fn ended_for_good(m: &dyn GcodeMachine, emu: &LineEmuHandle, what: &str) {
    assert!(m.wait_until(T, |s| s.alarm.is_some() && s.activity == Activity::Idle), "{what}: {:?}", m.snapshot());
    let s = m.snapshot();
    assert!(!s.paused && !s.fire_on, "{what}: paused {} fire {}", s.paused, s.fire_on);
    assert_eq!(s.laser_status(), LaserStatus::Alarm, "{what}");
    emu.with(|e| assert!(!e.laser_on(), "{what}: the laser is on"));
    assert!(matches!(m.start_job(Job::from_text("G0 X1")), Err(MachineError::Alarm)), "{what}: a job starts on a halted machine");
    let before = moves_arrived(emu);
    m.unlock().unwrap();
    assert!(m.wait_until(T, |s| s.alarm.is_none()), "{what}: {:?}", m.snapshot().alarm);
    assert!(!emu.with(|e| e.is_halted()), "{what}: the halt was not cleared");
    let pos = emu.with(|e| e.machine_position());
    let _ = m.resume();
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(moves_arrived(emu), before, "{what}: moves were sent after the halt was cleared");
    emu.with(|e| {
        assert!(!e.laser_on(), "{what}: the laser came back on");
        assert_eq!(e.machine_position(), pos, "{what}: the machine moved again");
    });
    let s = m.snapshot();
    assert_eq!(s.activity, Activity::Idle, "{what}");
    assert!(!s.paused && !s.fire_on, "{what}");
}

#[test]
fn a_smoothie_halt_while_idle_blocks_everything_that_moves_or_fires() {
    let (m, emu, _) = smoothie();
    emu.with(|e| e.press_kill_button());
    assert!(m.wait_until(T, |s| s.alarm.is_some()));
    assert!(matches!(m.frame_rect(Rect::new(0.0, 0.0, 10.0, 10.0), FrameOptions::default()), Err(MachineError::Alarm)));
    assert!(matches!(m.fire(1.0, Duration::from_millis(200)), Err(MachineError::Alarm)));
    assert!(matches!(m.jog(JogStep { dx: 1.0, dy: 0.0, dz: 0.0, feed: 1000.0 }), Err(MachineError::Alarm)));
    assert!(matches!(m.go_to_origin(), Err(MachineError::Alarm)));
    ended_for_good(m.as_ref(), &emu, "idle");
    m.disconnect();
}

#[test]
fn a_smoothie_halt_while_streaming_paused_framing_or_firing_ends_it_for_good() {
    for phase in ["job", "paused job", "frame", "fire"] {
        let (m, emu, p) = smoothie();
        match phase {
            "frame" => {
                m.frame_rect(Rect::new(0.0, 0.0, 200.0, 150.0), FrameOptions { feed: 1200.0, ..FrameOptions::default() }).unwrap();
                wait_emu(&emu, "framing", |e| e.machine_position()[0] > 2.0);
            }
            "fire" => {
                m.fire(1.0, Duration::from_secs(8)).unwrap();
                wait_emu(&emu, "firing", |e| e.laser_on());
            }
            _ => {
                m.start_job(many_cuts(&p)).unwrap();
                wait_emu(&emu, "cutting", |e| e.laser_on() && e.machine_position()[0] > 12.0);
                if phase == "paused job" {
                    m.pause().unwrap();
                    assert!(m.wait_until(T, |s| s.paused), "{phase}");
                }
            }
        }
        emu.with(|e| e.press_kill_button());
        ended_for_good(m.as_ref(), &emu, phase);
        m.disconnect();
    }
}

/// The controller's `ALARM:` line is lost on the wire; only its status reports say Alarm.
#[test]
fn a_smoothie_halt_seen_only_in_the_status_report_ends_it_for_good() {
    for phase in ["job", "paused job", "fire"] {
        let (m, emu, p) = smoothie();
        if phase == "fire" {
            m.fire(1.0, Duration::from_secs(8)).unwrap();
            wait_emu(&emu, "firing", |e| e.laser_on());
        } else {
            m.start_job(many_cuts(&p)).unwrap();
            wait_emu(&emu, "cutting", |e| e.laser_on() && e.machine_position()[0] > 12.0);
            if phase == "paused job" {
                m.pause().unwrap();
                assert!(m.wait_until(T, |s| s.paused), "{phase}");
            }
        }
        emu.with(|e| {
            e.press_kill_button();
            let _ = e.take_output();
        });
        ended_for_good(m.as_ref(), &emu, phase);
        m.disconnect();
    }
}

#[test]
fn a_marlin_kill_in_the_middle_of_a_job_ends_it_and_nothing_goes_on() {
    let (m, emu, p) = rig(DeviceKind::Marlin, EmuFlavor::Marlin);
    m.start_job(long_cut(&p)).unwrap();
    wait_emu(&emu, "cutting", |e| e.laser_on());
    emu.with(|e| e.press_kill_button());
    assert!(m.wait_until(T, |s| s.alarm.is_some() && s.activity == Activity::Idle), "{:?}", m.snapshot());
    let s = m.snapshot();
    assert!(!s.paused && !s.fire_on);
    emu.with(|e| assert!(!e.laser_on()));
    // A halted Marlin needs a power cycle: nothing may start, and unlock does not pretend.
    assert!(matches!(m.start_job(Job::from_text("G0 X1")), Err(MachineError::Alarm)));
    let before = moves_arrived(&emu);
    let _ = m.unlock();
    let _ = m.resume();
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(moves_arrived(&emu), before, "moves were sent to a halted Marlin");
    assert!(m.snapshot().alarm.is_some(), "the halt of a Marlin was cleared without a restart");
    m.disconnect();
}
