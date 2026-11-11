//! Bug hunt pass 8: the gate in front of the card. Each test here failed on the code before its fix.

use darkburn_mcc::command::{self, MachineParams, SafeWrite};
use darkburn_mcc::gate::GateRefusal;
use darkburn_mcc::planner::{PathSegment, PlanConfig, dry_run};
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

/// A four second dry stream, started and running.
fn running_job(cfg: SimConfig) -> SimLink {
    let mut l = SimLink::new(CardSimulator::new(cfg));
    l.gate.arming.arm_motion().unwrap();
    l.poll_status().unwrap();
    l.write(&command::fifo_clear().unwrap()).unwrap();
    let plan = dry_run(&[PathSegment::cut(200.0, 0.0, 50.0)], &PlanConfig::default()).unwrap();
    let mut feeder = plan.feeder();
    let st = l.poll_status().unwrap();
    for (id, frame) in feeder.fill(st.fifo_frame_id(), st.fifo_space_margin(), 50).frames {
        l.write(&command::fifo_packed_frame(id, &frame).unwrap()).unwrap();
    }
    l.write(&command::fifo_start().unwrap()).unwrap();
    l.advance(ms(100));
    l.poll_status().unwrap();
    assert!(l.sim.fifo_running(), "the stream runs");
    l
}

#[test]
fn a_card_that_asks_for_a_restart_stops_the_running_stream() {
    let mut l = running_job(SimConfig { fifo_starvation_alarm: false, ..SimConfig::default() });
    l.sim.set_param_status(1);
    l.poll_status().unwrap();
    assert!(!l.sim.fifo_running(), "the stream went on with a card that wants a restart");
    // and it is sent once, not on every poll
    let sent = l.sim.commands().len();
    l.advance(ms(100));
    l.poll_status().unwrap();
    assert_eq!(l.sim.commands().len(), sent);
}

#[test]
fn the_head_is_not_jogged_and_the_fifo_not_cleared_under_a_running_stream() {
    // A card that does not answer "busy" by itself while it streams: the PC side has to refuse.
    let mut l = running_job(SimConfig { fifo_starvation_alarm: false, busy_while_streaming: false, ..SimConfig::default() });
    l.gate.homed = true;
    let sent = l.sim.commands().len();
    let depth = l.sim.fifo_depth();
    for w in [
        command::jog_step(1, 5.0, &p()).unwrap(),
        command::jog_continuous(0, true, &p(), None).unwrap(),
        command::home_axis(0).unwrap(),
        command::go_to(10.0, 10.0, &p()).unwrap(),
        command::move_axis_absolute(0, 5.0, &p()).unwrap(),
        command::fifo_clear().unwrap(),
    ] {
        assert_eq!(l.write(&w), Err(LinkError::Refused(GateRefusal::StreamRunning)), "{}", w.name());
    }
    assert_eq!(l.sim.commands().len(), sent, "reached the card: {:?}", &l.sim.commands()[sent..]);
    assert!(l.sim.fifo_running() && l.sim.fifo_depth() == depth, "the queue was thrown away under the running job");
    // stopping is never refused, and after it the head may be moved again
    l.write(&command::fifo_stop().unwrap()).unwrap();
    l.write(&command::fifo_clear().unwrap()).unwrap();
    l.write(&command::jog_step(1, 5.0, &p()).unwrap()).unwrap();
}
