//! Pass 8 of the bug hunt, races and stale state: moves of the line machines against the
//! machine's travel when a work origin is set, and the fire button against those moves.
//! In-memory emulators only.

use std::time::Duration;

use darkburn_gcodebots::emu::{EmuConfig, EmuFlavor, LineEmuHandle};
use darkburn_gcodebots::profile::{DeviceKind, DeviceProfile, TransferMode};
use darkburn_gcodebots::{GcodeMachine, MachineError, MachineEvent};
use darkburn_grbl::JogStep;

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

fn moves_sent(emu: &LineEmuHandle) -> usize {
    emu.with(|e| e.stats().arrived.iter().filter(|l| l.contains("G0") || l.contains("G1")).count())
}

fn jog(dx: f64) -> JogStep {
    JogStep { dx, dy: 0.0, dz: 0.0, feed: 6000.0 }
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
