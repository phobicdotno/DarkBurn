//! Bug hunt pass 8, races: what the gate does when the card's state changes under a held key.
//! Simulator only.

use darkburn_mcc::command::{self, MachineParams};
use darkburn_mcc::gate::ArmState;
use darkburn_mcc::registers::Status;
use darkburn_mcc::sim::{CardSimulator, SimConfig, SimLink};
use std::time::Duration;

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

/// The key is down: the lease is renewed and the status polled, ten times a second.
fn hold(l: &mut SimLink, tenths: u32) {
    for _ in 0..tenths {
        l.advance(ms(100));
        l.gate.jog_keepalive(0, l.sim.now_ms());
        l.poll_status().unwrap();
    }
}

#[test]
fn a_restart_required_status_stops_a_continuous_jog_that_is_held() {
    // A move on this simulator takes two seconds and lands on its target at the end.
    let mut l = SimLink::new(CardSimulator::new(SimConfig { motion_time_ms: 2000, ..SimConfig::default() }));
    l.gate.arming.arm_motion().unwrap();
    l.poll_status().unwrap();
    l.write(&command::jog_continuous(0, true, &MachineParams::default(), None).unwrap()).unwrap();
    hold(&mut l, 3);
    assert_eq!(l.sim.status_block().word(Status::RunStatus), 1, "the jog is not running");
    // The card now asks for a restart; the key stays down.
    l.sim.set_param_status(1);
    hold(&mut l, 2);
    assert_eq!(l.sim.status_block().word(Status::RunStatus), 0, "the head still moves on a card that asked for a restart");
    hold(&mut l, 30);
    assert_eq!(l.sim.axis_position(0), 0, "the jog ran to its end");
    assert_eq!(l.gate.arming.state(), ArmState::Disarmed);
    assert!(!l.gate.jog_keepalive(0, l.sim.now_ms()), "the jog is still leased");
}
