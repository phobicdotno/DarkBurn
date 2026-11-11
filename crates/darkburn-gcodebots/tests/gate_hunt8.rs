//! Pass 8 of the bug hunt, safety gates: moves of the line machines against the machine's travel.
//! In-memory emulators only.

use std::time::Duration;

use darkburn_gcodebots::emu::{EmuConfig, EmuFlavor, LineEmuHandle};
use darkburn_gcodebots::profile::{DeviceKind, DeviceProfile, TransferMode};
use darkburn_gcodebots::{GcodeMachine, MachineError, MachineEvent};
use darkburn_grbl::{Coords, JogStep, MoveTarget};

const T: Duration = Duration::from_secs(8);

fn smoothie() -> (Box<dyn GcodeMachine>, LineEmuHandle) {
    let mut p = DeviceProfile::for_kind(DeviceKind::Smoothie);
    p.width_mm = 300.0;
    p.height_mm = 200.0;
    p.gcode.transfer = TransferMode::Buffered;
    let emu = LineEmuHandle::with_config(EmuConfig::new(EmuFlavor::Smoothie));
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

fn moves_sent(emu: &LineEmuHandle) -> usize {
    emu.with(|e| e.stats().arrived.iter().filter(|l| l.contains("G0")).count())
}

#[test]
fn a_move_in_machine_coordinates_does_not_leave_the_machine() {
    let (m, emu) = smoothie();
    let r = m.move_to(MoveTarget { x: Some(5000.0), y: None, z: None, coords: Coords::Machine, feed: 6000.0 });
    assert!(matches!(r, Err(MachineError::OutOfBounds(_))), "{r:?}");
    assert_eq!(moves_sent(&emu), 0);
    // A place inside the machine is still reached.
    m.move_to(MoveTarget { x: Some(50.0), y: Some(20.0), z: None, coords: Coords::Machine, feed: 6000.0 }).unwrap();
    assert!(m.as_ref().wait_until(T, |s| (s.mpos[0] - 50.0).abs() < 1e-6));
    m.disconnect();
}

#[test]
fn a_move_in_work_coordinates_is_checked_with_the_work_offset() {
    let (m, emu) = smoothie();
    m.jog(JogStep { dx: 100.0, dy: 0.0, dz: 0.0, feed: 6000.0 }).unwrap();
    assert!(m.as_ref().wait_until(T, |s| (s.mpos[0] - 100.0).abs() < 1e-6));
    m.send_console("G92 X0 Y0").unwrap();
    assert!(m.as_ref().wait_until(T, |s| (s.mpos[0] - 100.0).abs() < 1e-6 && s.wpos[0].abs() < 1e-6), "{:?}", m.snapshot());
    let before = moves_sent(&emu);
    // Work X 250 is machine X 350 on a 300 mm machine.
    let r = m.move_to(MoveTarget { x: Some(250.0), y: None, z: None, coords: Coords::Work, feed: 6000.0 });
    assert!(matches!(r, Err(MachineError::OutOfBounds(_))), "{r:?}");
    assert_eq!(moves_sent(&emu), before);
    m.disconnect();
}
