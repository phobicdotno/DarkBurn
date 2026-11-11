//! Bug hunt pass 8: the gate in front of the card. Each test here failed on the code before its fix.

use darkburn_mcc::command::{self, MachineParams, SafeWrite};
use darkburn_mcc::gate::GateRefusal;
use darkburn_mcc::registers::{BLOCKS, alarm1};
use darkburn_mcc::sim::{CardSimulator, LinkError, SimConfig, SimLink};
use std::time::Duration;

fn link() -> SimLink {
    SimLink::new(CardSimulator::new(SimConfig::default()))
}

/// Motion armed and polled once.
fn armed() -> SimLink {
    let mut l = link();
    l.gate.arming.arm_motion().unwrap();
    l.poll_status().unwrap();
    l
}

fn p() -> MachineParams {
    MachineParams::default()
}

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

/// Home both axes on the card and let the gate read the result.
fn homed() -> SimLink {
    let mut l = armed();
    for slot in 0..2 {
        l.write(&command::home_axis(slot).unwrap()).unwrap();
        l.advance(ms(400));
        l.poll_status().unwrap();
    }
    l.gate.homed = true;
    l
}

#[test]
fn nothing_moves_before_the_card_was_asked_how_it_is() {
    // The operator has pressed the emergency stop, but the status was never read, so the gate
    // does not know. A jog, a home, a FIFO clear and the gas valve went straight to the card.
    let mut l = link();
    l.gate.arming.arm_motion().unwrap();
    l.sim.inject_alarm(Some(alarm1::ESTOP), None);
    let gas_on = SafeWrite::try_from_raw(0x65, &[9999, 2, 0b10, 0b10]).unwrap();
    for w in [command::jog_step(0, 5.0, &p()).unwrap(), command::home_axis(0).unwrap(), command::fifo_clear().unwrap(), gas_on] {
        assert_eq!(l.write(&w), Err(LinkError::Refused(GateRefusal::CommLoss)), "{}", w.name());
    }
    assert!(l.sim.commands().is_empty(), "reached the card: {:?}", l.sim.commands());
    assert_eq!(l.sim.outputs(), 0);
    // The stop primitives never wait for a poll.
    l.write(&command::stop_all_no_decel().unwrap()).unwrap();
    l.write(&command::fifo_stop().unwrap()).unwrap();
}

#[test]
fn nothing_moves_on_a_status_that_is_seconds_old() {
    let mut l = homed();
    l.advance(ms(5000));
    // An alarm came up since the last poll; the gate still holds the old, clean status.
    l.sim.inject_alarm(None, Some(1 << 3));
    let before = l.sim.commands().len();
    for w in [command::jog_step(0, 5.0, &p()).unwrap(), command::go_to(10.0, 10.0, &p()).unwrap(), command::move_axis_absolute(0, 5.0, &p()).unwrap()]
    {
        assert_eq!(l.write(&w), Err(LinkError::Refused(GateRefusal::CommLoss)), "{}", w.name());
    }
    assert_eq!(l.sim.commands().len(), before);
    assert_eq!(l.sim.axis_position(0), 0);
}

#[test]
fn a_block_does_not_claim_a_range_that_wraps_past_the_last_register() {
    // `addr + count` wrapped (or stopped the program in a debug build): the range that starts at
    // the last register and is two long "ended" at register 1, inside every block.
    for b in BLOCKS {
        assert!(!b.contains(u32::MAX, 2), "{} claims a wrapped range", b.name);
        assert!(!b.contains(b.start, u32::MAX), "{} claims a huge range", b.name);
        assert!(b.contains(b.start, b.count));
    }
}
