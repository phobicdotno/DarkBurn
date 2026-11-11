//! Pass 8 of the bug hunt, races and stale state: moves of the line machines against the
//! machine's travel when a work origin is set, and the fire button against those moves.
//! In-memory emulators only.

use std::time::Duration;

use darkburn_gcodebots::emu::{EmuConfig, EmuFlavor, LineEmuHandle};
use darkburn_gcodebots::profile::{DeviceKind, DeviceProfile, TransferMode};
use darkburn_gcodebots::{GcodeMachine, MachineError, MachineEvent};
use darkburn_grbl::{Coords, FrameOptions, JogStep, MoveTarget};
use kurbo::Rect;

const T: Duration = Duration::from_secs(8);

/// A 300 x 200 mm machine of the given kind on its emulator.
fn rig(kind: DeviceKind, emu: EmuConfig) -> (Box<dyn GcodeMachine>, LineEmuHandle) {
    let mut p = DeviceProfile::for_kind(kind);
    p.width_mm = 300.0;
    p.height_mm = 200.0;
    p.gcode.transfer = TransferMode::Buffered;
    let emu = LineEmuHandle::with_config(emu);
    let m = p.connect(Box::new(emu.transport())).expect("connect");
    let start = std::time::Instant::now();
    let mut up = false;
    while !up && start.elapsed() < T {
        up = m.poll_events().iter().any(|e| matches!(e, MachineEvent::Connected(_)));
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(up, "no handshake");
    (m, emu)
}

fn smoothie() -> (Box<dyn GcodeMachine>, LineEmuHandle) {
    rig(DeviceKind::Smoothie, EmuConfig::new(EmuFlavor::Smoothie))
}

fn marlin() -> (Box<dyn GcodeMachine>, LineEmuHandle) {
    rig(DeviceKind::Marlin, EmuConfig::new(EmuFlavor::Marlin))
}

fn moves_sent(emu: &LineEmuHandle) -> usize {
    emu.with(|e| e.stats().arrived.iter().filter(|l| l.contains("G0") || l.contains("G1")).count())
}

fn jog(dx: f64) -> JogStep {
    JogStep { dx, dy: 0.0, dz: 0.0, feed: 6000.0 }
}

/// The head of a Marlin at machine X 100 with the work origin set there. The board reports one
/// position, and after the `G92` that is the work position: X 0.
fn marlin_with_origin_at_100() -> (Box<dyn GcodeMachine>, LineEmuHandle) {
    let (m, emu) = marlin();
    m.jog(jog(100.0)).unwrap();
    assert!(m.as_ref().wait_until(T, |s| (s.mpos[0] - 100.0).abs() < 1e-6), "{:?}", m.snapshot());
    m.set_user_origin().unwrap();
    assert!(m.as_ref().wait_until(T, |s| s.mpos[0].abs() < 1e-6 && s.wpos[0].abs() < 1e-6), "{:?}", m.snapshot());
    assert!((emu.with(|e| e.machine_position()[0]) - 100.0).abs() < 1e-6);
    // one more pass of the worker, so the answer to the G92 itself has been read
    m.request_status().unwrap();
    std::thread::sleep(Duration::from_millis(150));
    (m, emu)
}

// ORIGIN
#[test]
fn go_to_origin_is_refused_when_the_work_origin_lies_outside_the_machine() {
    let (m, emu) = smoothie();
    m.jog(jog(10.0)).unwrap();
    assert!(m.as_ref().wait_until(T, |s| (s.mpos[0] - 10.0).abs() < 1e-6));
    // The head at machine X 10 is called work X 50: work X 0 is machine X -40.
    m.send_console("G92 X50 Y0").unwrap();
    assert!(m.as_ref().wait_until(T, |s| (s.wpos[0] - 50.0).abs() < 1e-6 && (s.mpos[0] - 10.0).abs() < 1e-6), "{:?}", m.snapshot());
    let before = moves_sent(&emu);
    let r = m.go_to_origin();
    assert!(matches!(r, Err(MachineError::OutOfBounds(_))), "{r:?}");
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(moves_sent(&emu), before, "{:?}", emu.with(|e| e.stats().arrived.clone()));
    // An origin on the machine is still gone to.
    m.send_console("G92 X5 Y0").unwrap();
    assert!(m.as_ref().wait_until(T, |s| (s.wpos[0] - 5.0).abs() < 1e-6));
    m.go_to_origin().unwrap();
    assert!(m.as_ref().wait_until(T, |s| s.wpos[0].abs() < 1e-6 && (s.mpos[0] - 5.0).abs() < 1e-6), "{:?}", m.snapshot());
    m.disconnect();
}

// FIREORIGIN
#[test]
fn go_to_origin_switches_the_fire_button_off_first() {
    for kind in [DeviceKind::Marlin, DeviceKind::Smoothie] {
        let flavor = if kind == DeviceKind::Marlin { EmuFlavor::Marlin } else { EmuFlavor::Smoothie };
        let (m, emu) = rig(kind, EmuConfig::new(flavor).with_time_scale(1.0));
        m.jog(jog(20.0)).unwrap();
        assert!(m.as_ref().wait_until(T, |s| (s.mpos[0] - 20.0).abs() < 1e-6), "{flavor:?}");
        m.fire(5.0, Duration::from_secs(8)).unwrap();
        assert!(m.as_ref().wait_until(T, |_| emu.with(|e| e.laser_on() || e.laser_armed())), "{flavor:?}");
        m.go_to_origin().unwrap();
        assert!(m.as_ref().wait_until(T, |s| s.mpos[0].abs() < 1e-6), "{flavor:?}");
        emu.with(|e| {
            let a = &e.stats().arrived;
            let go = a.iter().rposition(|l| l.starts_with("G0 X0 Y0")).expect("the move was sent");
            let off = a.iter().position(|l| l == "M5" || l == "fire off" || l.starts_with("M107"));
            assert!(off.is_some_and(|i| i < go), "{flavor:?}: the laser stayed on for the move: {a:?}");
            assert!(!e.laser_on());
        });
        assert!(!m.snapshot().fire_on);
        m.disconnect();
    }
}

// MARLIN
#[test]
fn a_marlin_jog_after_set_origin_does_not_leave_the_machine() {
    let (m, emu) = marlin_with_origin_at_100();
    let before = moves_sent(&emu);
    // Reported X 0 + 250 looks fine; the head is at machine X 100 and would go to 350 of 300.
    let r = m.jog(jog(250.0));
    assert!(matches!(r, Err(MachineError::OutOfBounds(_))), "{r:?}");
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(moves_sent(&emu), before);
    // A jog that stays on the machine still runs.
    m.jog(jog(150.0)).unwrap();
    assert!(m.as_ref().wait_until(T, |_| (emu.with(|e| e.machine_position()[0]) - 250.0).abs() < 1e-6));
    m.disconnect();
}

#[test]
fn a_marlin_move_frame_and_go_to_origin_after_set_origin_are_checked_on_the_machine() {
    let (m, emu) = marlin_with_origin_at_100();
    let before = moves_sent(&emu);
    let r = m.move_to(MoveTarget { x: Some(250.0), y: None, z: None, coords: Coords::Work, feed: 6000.0 });
    assert!(matches!(r, Err(MachineError::OutOfBounds(_))), "{r:?}");
    let r = m.frame_rect(Rect::new(0.0, 0.0, 250.0, 10.0), FrameOptions::default());
    assert!(matches!(r, Err(MachineError::OutOfBounds(_))), "{r:?}");
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(moves_sent(&emu), before);
    // Inside the machine: work X 150 is machine X 250.
    m.move_to(MoveTarget { x: Some(150.0), y: None, z: None, coords: Coords::Work, feed: 6000.0 }).unwrap();
    assert!(m.as_ref().wait_until(T, |_| (emu.with(|e| e.machine_position()[0]) - 250.0).abs() < 1e-6));
    m.go_to_origin().unwrap();
    assert!(m.as_ref().wait_until(T, |_| (emu.with(|e| e.machine_position()[0]) - 100.0).abs() < 1e-6));
    m.disconnect();
}

#[test]
fn clearing_the_origin_on_a_marlin_ends_the_shift() {
    let (m, emu) = marlin_with_origin_at_100();
    m.clear_user_origin().unwrap();
    assert!(m.as_ref().wait_until(T, |s| (s.mpos[0] - 100.0).abs() < 1e-6), "{:?}", m.snapshot());
    m.request_status().unwrap();
    std::thread::sleep(Duration::from_millis(150));
    m.jog(jog(150.0)).unwrap();
    assert!(m.as_ref().wait_until(T, |s| (s.mpos[0] - 250.0).abs() < 1e-6 && (emu.with(|e| e.machine_position()[0]) - 250.0).abs() < 1e-6));
    let r = m.jog(jog(100.0));
    assert!(matches!(r, Err(MachineError::OutOfBounds(_))), "{r:?}");
    m.disconnect();
}
