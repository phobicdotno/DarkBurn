//! Review of the round that added MLaser's controller commands: each test here failed on the code
//! of that round before its fix.

use darkburn_mcc::command::{self, MachineParams, SafeWrite};
use darkburn_mcc::crc::CrcOrder;
use darkburn_mcc::dissect::{Verdict, dissect_request};
use darkburn_mcc::fifo::FifoItem;
use darkburn_mcc::frame::{RawFrame, Reply, Request};
use darkburn_mcc::gate::{GateConfig, GateRefusal, SafetyGate};
use darkburn_mcc::registers::{Status, StatusBlock, alarm2};
use darkburn_mcc::sim::{CardSimulator, LinkError, SimConfig, SimLink};
use std::time::Duration;

fn p() -> MachineParams {
    MachineParams::default()
}

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

fn link(cfg: SimConfig) -> SimLink {
    SimLink::new(CardSimulator::new(cfg))
}

/// Motion armed and polled once.
fn armed() -> SimLink {
    let mut l = link(SimConfig::default());
    l.gate.arming.arm_motion().unwrap();
    l.poll_status().unwrap();
    l
}

fn status(proc: u32) -> StatusBlock {
    let mut w = vec![0u32; 36];
    w[Status::ProcessingStatus as usize] = proc;
    StatusBlock::from_words(&w).unwrap()
}

fn hand_built_write(addr: u32, words: &[u32]) -> Vec<u8> {
    let mut tail = vec![0u8, 0, 0, 0x40];
    tail.extend_from_slice(&addr.to_le_bytes());
    tail.extend_from_slice(&(words.len() as u16).to_le_bytes());
    for w in words {
        tail.extend_from_slice(&w.to_le_bytes());
    }
    let len = (tail.len() - 2) as u16;
    tail[..2].copy_from_slice(&len.to_be_bytes());
    let crc = darkburn_mcc::crc16_modbus(&tail);
    let mut out = vec![0, 3];
    out.extend_from_slice(&darkburn_mcc::crc::crc_bytes(crc, CrcOrder::HiFirst));
    out.extend(tail);
    out
}

// ---- odd input ------------------------------------------------------------------------------

#[test]
fn an_output_write_with_an_empty_mask_is_refused_not_a_panic() {
    // [9999, 2, 0, 0] subtracted one from a zero mask (an overflow) before it was refused.
    let words = [9999u32, 2, 0, 0];
    assert!(SafeWrite::try_from_raw(0x65, &words).is_err());
    let frame = hand_built_write(0x65, &words);
    assert!(Request::decode(&frame).is_err());
    assert!(matches!(dissect_request(&frame).verdict, Verdict::Refused(_)));
    let mut sim = CardSimulator::new(SimConfig::default());
    assert!(RawFrame::parse(&sim.exchange(&frame).unwrap()).is_exception());
    assert!(sim.commands().is_empty());
}

#[test]
fn a_home_slot_beyond_the_word_in_the_gate_configuration_is_refused_not_a_panic() {
    let mut cfg = GateConfig::default();
    cfg.safety.home_slots = vec![32];
    let mut g = SafetyGate::new(cfg);
    g.arming.arm_motion().unwrap();
    g.observe_status(0, &status(0));
    assert!(matches!(g.check_write(&command::home_axis(0).unwrap(), 0), Err(GateRefusal::Limit(_))));
}

#[test]
fn a_held_jog_with_an_unknown_distance_to_the_soft_limit_is_refused() {
    // NaN was ignored by `min`, so the jog went out with the full 4 m.
    assert!(command::jog_continuous(0, true, &p(), Some(f64::NAN)).is_err());
    assert!(command::jog_continuous(1, false, &p(), Some(f64::NAN)).is_err());
    assert_eq!(command::jog_continuous(0, true, &p(), Some(12.5)).unwrap().words()[5], 12_500);
}

#[test]
fn a_jog_is_never_built_as_an_absolute_move() {
    // Bit 31 of the axis word makes the card move to a position; a jog asked for a distance.
    assert!(command::jog_step(0x8000_0000, 5.0, &p()).is_err());
    assert!(command::jog_continuous(0x8000_0001, true, &p(), Some(10.0)).is_err());
    assert!(command::jog_step(1, 5.0, &p()).is_ok());
}

// ---- stops ----------------------------------------------------------------------------------

#[test]
fn a_stop_switches_the_laser_outputs_off_right_after_the_motion_stop() {
    // The vendor's stop switches the laser output off right after the motion stop, before the
    // PWM and analogue steps.
    for fifo in [false, true] {
        let seq = command::stop_sequence(&p(), fifo, 0, 0b1_0001_0000, &[5, 6, 9]);
        assert!(matches!(seq[1].words(), [9999, 2, mask, value] if mask & (1 << 4) != 0 && value & (1 << 4) == 0), "{:?}", seq[1].words());
    }
}

#[test]
fn a_stop_never_writes_an_output_on_from_an_old_output_word() {
    // Port 4 (no known role) was on at the last poll and has gone off since. The stop's bulk
    // write "kept" it, which switched it on again.
    let mut l = armed();
    assert!(l.sim.set_card_word(Status::Do.addr(), 0b1000));
    l.poll_status().unwrap();
    assert!(l.sim.set_card_word(Status::Do.addr(), 0));
    l.stop_sequence(false);
    // The emulated card refuses an output write that switches on a port outside the allow-list
    // and counts it; a real card would not.
    assert!(l.sim.forbidden_attempts().is_empty(), "{:?}", l.sim.forbidden_attempts());
    assert_eq!(l.sim.outputs(), 0, "the stop switched an output on");
    for c in l.sim.commands() {
        if let [9999, 2, _, value] = c.as_slice() {
            assert_eq!(*value, 0, "a stop step wrote a 1: {c:?}");
        }
    }
}

#[test]
fn the_gates_own_port_list_does_not_hold_back_the_switch_off_of_a_stop() {
    // On this machine port 8 is a lamp. With the lamp and the laser gate on, an alarm disarms and
    // the stop goes out; its output step kept the lamp on, so this gate judged it a motion write,
    // refused it while disarmed, and the laser gate stayed on.
    let mut l = link(SimConfig::default());
    l.gate.config.safety.motion_do_ports.push(8);
    l.gate.arming.arm_motion().unwrap();
    assert!(l.sim.set_card_word(Status::Do.addr(), (1 << 7) | (1 << 4)));
    l.poll_status().unwrap();
    l.sim.inject_alarm(None, Some(alarm2::AXIS_COMMAND));
    l.poll_status().unwrap();
    assert_eq!(l.sim.outputs() & (1 << 4), 0, "the laser gate stayed on");
    assert_eq!(l.sim.outputs() & (1 << 7), 1 << 7, "the lamp is left as it was");
}

#[test]
fn a_gate_configured_without_a_default_laser_port_still_switches_it_off() {
    // The stop used the configured laser ports instead of the fixed 5, 6 and 9; a configuration
    // that names other ports left the fibre gate (port 5) on.
    let mut l = armed();
    l.gate.config.safety.laser_do_ports = vec![8];
    assert!(l.sim.set_card_word(Status::Do.addr(), 1 << 4));
    l.poll_status().unwrap();
    l.stop_sequence(false);
    assert_eq!(l.sim.outputs() & (1 << 4), 0, "port 5 stayed on");
    let mut l = armed();
    l.gate.config.safety.laser_do_ports = vec![];
    assert!(l.sim.set_card_word(Status::Do.addr(), (1 << 8) | 0b10));
    l.poll_status().unwrap();
    l.emergency_stop(0);
    assert_eq!(l.sim.outputs(), 0b10, "port 9 stayed on, or the gas was touched");
}

#[test]
fn a_deadman_stop_that_never_reached_the_card_is_sent_again() {
    // The release stop of a held jog was tried three times at once and then given up: a burst of
    // lost datagrams left the jog running to its end.
    let mut l = armed();
    l.write(&command::jog_continuous(0, true, &p(), None).unwrap()).unwrap();
    l.advance(ms(100));
    l.sim.drop_requests(3);
    l.advance(ms(150));
    assert!(!l.sim.commands().iter().any(|c| c[0] == 1), "the three attempts were dropped");
    l.advance(ms(10));
    assert!(l.sim.commands().iter().any(|c| c[0] == 1 && c[1] & 1 == 1), "the release stop was not sent again");
    // and once the card has it, it is not sent again
    let n = l.sim.commands().len();
    l.advance(ms(500));
    assert_eq!(l.sim.commands().len(), n);
}

#[test]
fn a_job_given_up_for_lost_contact_is_not_run_on_by_a_bare_start() {
    let mut g = SafetyGate::default();
    g.arming.arm_motion().unwrap();
    g.observe_status(0, &status(0));
    g.after_send(&command::fifo_frame(1, &vec![FifoItem::tick(1, 0); 10]).unwrap(), 0);
    g.after_send(&command::fifo_start().unwrap(), 0);
    assert!(g.watchdog(5000), "the watchdog fires");
    // contact is back, the card is idle and still holds the frames; the operator arms again
    g.observe_status(6000, &status(0));
    g.arming.arm_motion().unwrap();
    assert_eq!(g.check_write(&command::fifo_start().unwrap(), 6000), Err(GateRefusal::QueueHeld));
    // the same when the gate finds out on a write
    let mut g = SafetyGate::default();
    g.arming.arm_motion().unwrap();
    g.observe_status(0, &status(0));
    g.after_send(&command::fifo_frame(1, &vec![FifoItem::tick(1, 0); 10]).unwrap(), 0);
    g.after_send(&command::fifo_start().unwrap(), 0);
    assert_eq!(g.check_write(&command::fifo_frame(2, &[FifoItem::tick(1, 0)]).unwrap(), 5000), Err(GateRefusal::CommLoss));
    g.observe_status(6000, &status(0));
    g.arming.arm_motion().unwrap();
    assert_eq!(g.check_write(&command::fifo_start().unwrap(), 6000), Err(GateRefusal::QueueHeld));
}

// ---- answers --------------------------------------------------------------------------------

#[test]
fn an_answer_for_another_register_does_not_confirm_a_write() {
    // A stopped job is held; the answer to the clear names register 0x65. The vendor driver
    // checks function and address of an answer; this link took it as "cleared".
    let mut l = armed();
    l.write(&command::fifo_clear().unwrap()).unwrap();
    l.write(&command::fifo_frame(1, &vec![FifoItem::tick(1, 0); 10]).unwrap()).unwrap();
    l.stop_sequence(false);
    assert!(l.gate.queue_held());
    l.poll_status().unwrap();
    let next_seq = l.sim.requests().last().and_then(|r| r.seq).unwrap().wrapping_add(1);
    l.sim.override_next_reply(Reply::Write { func: 0x40, addr: 0x65, count: 1 }.encode(next_seq).unwrap());
    assert_eq!(l.write(&command::fifo_clear().unwrap()), Err(LinkError::Unexpected));
    assert!(l.gate.queue_held(), "the held queue was released on an answer to something else");
    // a read answered with another block is not taken either
    let next_seq = l.sim.requests().last().and_then(|r| r.seq).unwrap().wrapping_add(1);
    l.sim.override_next_reply(Reply::Read { addr: 2000, words: vec![0; 36] }.encode(next_seq).unwrap());
    assert_eq!(l.poll_status(), Err(LinkError::Unexpected));
    // the byte-block echo is not a write echo
    let next_seq = l.sim.requests().last().and_then(|r| r.seq).unwrap().wrapping_add(1);
    l.sim.override_next_reply(Reply::Write { func: 0x26, addr: 0x67, count: 1 }.encode(next_seq).unwrap());
    assert_eq!(l.write(&command::fifo_clear().unwrap()), Err(LinkError::Unexpected));
    assert!(l.gate.queue_held());
}

#[test]
fn damaged_and_odd_frames_are_refused_without_a_panic() {
    // Every request the crate builds, cut off at every byte, with a wrong checksum and with a
    // length field one off: none decodes, none panics, none is served.
    let mut reqs = vec![Request::read(1000, 36).unwrap(), Request::read(60001, 120).unwrap()];
    for w in command::stop_sequence(&p(), false, 0, 0b1_0001_0011, &[5, 6, 9]) {
        reqs.push(Request::write(w));
    }
    reqs.push(Request::write(command::jog_continuous(0, false, &p(), None).unwrap()));
    reqs.push(Request::write(command::fifo_frame(u32::MAX, &vec![FifoItem::tick(i16::MIN, i16::MAX); 99]).unwrap()));
    let mut sim = CardSimulator::new(SimConfig::default());
    for r in &reqs {
        let f = r.encode(0xFFFF).unwrap();
        assert_eq!(Request::decode(&f).unwrap(), (0xFFFF, r.clone()));
        for cut in 0..f.len() {
            assert!(Request::decode(&f[..cut]).is_err(), "cut at {cut}");
            let _ = Reply::decode(&f[..cut], Some(0xFFFF));
            let _ = (dissect_request(&f[..cut]), darkburn_mcc::dissect::dissect_reply(&f[..cut]));
            assert_eq!(sim.exchange(&f[..cut]), Err(darkburn_mcc::sim::Timeout), "cut at {cut} was answered");
        }
        let mut bad = f.clone();
        bad[2] ^= 0x01;
        assert!(Request::decode(&bad).is_err());
        for delta in [1i32, -1] {
            let mut bad = f.clone();
            let len = u16::from_be_bytes([bad[4], bad[5]]) as i32 + delta;
            bad[4..6].copy_from_slice(&(len as u16).to_be_bytes());
            let crc = darkburn_mcc::crc16_modbus(&bad[4..]);
            bad[2..4].copy_from_slice(&darkburn_mcc::crc::crc_bytes(crc, CrcOrder::HiFirst));
            assert!(Request::decode(&bad).is_err(), "length off by {delta}");
        }
    }
    assert!(sim.commands().is_empty() && sim.forbidden_attempts().is_empty());
    // counts of 0 and above the limit
    assert!(Request::read(1000, 0).is_err());
    assert!(Request::read(12002, 251).is_err());
    assert!(SafeWrite::try_from_raw(0x65, &[]).is_err());
    assert!(SafeWrite::try_from_raw(0x66, &[]).is_err());
    assert!(SafeWrite::try_from_raw(0x67, &[]).is_err());
    // a write whose count field says more words than it carries
    let mut f = hand_built_write(0x67, &[3]);
    f[12] = 2;
    let crc = darkburn_mcc::crc16_modbus(&f[4..]);
    f[2..4].copy_from_slice(&darkburn_mcc::crc::crc_bytes(crc, CrcOrder::HiFirst));
    assert!(Request::decode(&f).is_err());
    assert_eq!(RawFrame::parse(&f).request_vector(), None);
}

#[test]
fn a_stream_whose_status_reads_go_unanswered_is_stopped_by_the_watchdog() {
    // The card runs a job and stops answering. The gate's comm-loss watchdog existed, but nothing
    // ran it: the stream went on to its end with nobody watching.
    let mut l = link(SimConfig { tick_us: 1000, fifo_starvation_alarm: false, ..SimConfig::default() });
    l.gate.arming.arm_motion().unwrap();
    l.poll_status().unwrap();
    l.write(&command::fifo_clear().unwrap()).unwrap();
    for id in 1..=40 {
        l.write(&command::fifo_frame(id, &vec![FifoItem::tick(1, 0); 90]).unwrap()).unwrap();
    }
    l.write(&command::fifo_start().unwrap()).unwrap();
    l.poll_status().unwrap();
    l.sim.drop_replies(100_000);
    for _ in 0..15 {
        l.advance(ms(100));
        assert_eq!(l.poll_status(), Err(LinkError::Timeout));
    }
    assert!(!l.sim.fifo_running(), "the stream ran on without contact");
    assert!(l.sim.ticks_consumed() < 1300, "{} ticks ran", l.sim.ticks_consumed());
    assert_eq!(l.gate.arming.state(), darkburn_mcc::gate::ArmState::Disarmed);
    assert!(l.gate.queue_held());
}
