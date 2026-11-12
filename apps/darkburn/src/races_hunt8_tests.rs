//! Pass 8 of the bug hunt: what the app still shows of a link that has dropped. Simulators only.

use std::time::Duration;

use crate::gcode_machines::SimGcode;
use crate::machine_api::{ConnectConfig, LaserState, Link, MachineHandle};

const T: Duration = Duration::from_secs(20);

/// A handle that fires on a simulator; `unplug` pulls its cable.
fn firing(kind: Option<SimGcode>) -> (MachineHandle, Box<dyn Fn()>) {
    let cfg = ConnectConfig::default();
    let mut h = MachineHandle::new();
    let unplug: Box<dyn Fn()> = match kind {
        None => {
            let (adapter, emu) = crate::connections::grbl_simulator(&cfg);
            h.attach_simulator(Box::new(adapter), Link::SimulatorGrbl);
            Box::new(move || emu.with(|e| e.unplug()))
        }
        Some(k) => {
            let (adapter, emu) = crate::gcode_machines::simulator_adapter(k, &cfg);
            let emu = emu.expect("a line emulator");
            h.attach_simulator(Box::new(adapter), Link::SimulatorGcode(k));
            Box::new(move || emu.with(|e| e.unplug()))
        }
    };
    assert!(h.wait_for(T, |s, _| s.state == LaserState::Ready), "{kind:?}: ready, {:?}", h.snapshot().state);
    h.fire(5.0, 9.0).unwrap();
    assert!(h.wait_for(T, |s, _| s.fire_on), "{kind:?}: fire on");
    (h, unplug)
}

#[test]
fn the_fire_flag_does_not_outlive_a_dropped_link() {
    for kind in [None, Some(SimGcode::Marlin), Some(SimGcode::Smoothie)] {
        let (mut h, unplug) = firing(kind);
        unplug();
        assert!(h.wait_for(T, |s, _| s.state == LaserState::Disconnected), "{kind:?}: the drop was noticed, {:?}", h.snapshot().state);
        for _ in 0..5 {
            h.poll();
            std::thread::sleep(Duration::from_millis(10));
        }
        let s = h.snapshot();
        assert!(!s.connected() && s.job.is_none(), "{kind:?}");
        assert!(!s.fire_on, "{kind:?}: the app still shows the laser firing on a link that is gone");
        assert!(!s.air_on, "{kind:?}: the app still shows the air on");
    }
}
