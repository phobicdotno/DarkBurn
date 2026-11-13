//! The controller commands added from the command inventory: for each one the bytes it encodes
//! to, the decode of those bytes, a round trip through the in-memory card, the refusal where a
//! project rule applies, and the gate refusing it while nothing is armed.

use darkburn_mcc::command::{self, MachineParams, Policy, SafeWrite, StopVariant};
use darkburn_mcc::fifo::FifoItem;
use darkburn_mcc::frame::{RawFrame, Request, RequestError};
use darkburn_mcc::gate::{ArmState, GateRefusal};
use darkburn_mcc::registers::{Forbidden, MIN_PROGRAM_VERSION_SP1, Status, ZfState, axis};
use darkburn_mcc::sim::{CardSimulator, ConnectOptions, LinkError, SimConfig, SimLink};
use darkburn_mcc::{CrcOrder, crc16_modbus};
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

/// The datagram of a write after the sequence number and the checksum: length, unit, function,
/// register, count, words.
fn body(w: &SafeWrite) -> Vec<u8> {
    let bytes = Request::write(w.clone()).encode(0x1234).unwrap();
    let raw = RawFrame::parse(&bytes);
    assert_eq!(raw.seq, Some(0x1234));
    assert_eq!(raw.crc_order, Some(CrcOrder::HiFirst), "PC byte order of the checksum");
    assert!(raw.length_ok());
    bytes[4..].to_vec()
}

/// Encode, decode, and compare: the write comes back as itself, and its vector as well.
fn decodes_to_itself(w: &SafeWrite) {
    let bytes = Request::write(w.clone()).encode(9).unwrap();
    let (seq, req) = Request::decode(&bytes).unwrap_or_else(|e| panic!("{}: {e}", w.name()));
    assert_eq!(seq, 9);
    assert_eq!(req, Request::write(w.clone()), "{}", w.name());
    assert_eq!(RawFrame::parse(&bytes).request_vector(), Some(w.request_vector()), "{}", w.name());
}

/// A write datagram built by hand, for vectors the crate refuses to build.
fn hand_built(addr: u32, words: &[u32]) -> Vec<u8> {
    let mut tail = vec![0u8, 0, 0, 0x40];
    tail.extend_from_slice(&addr.to_le_bytes());
    tail.extend_from_slice(&(words.len() as u16).to_le_bytes());
    for w in words {
        tail.extend_from_slice(&w.to_le_bytes());
    }
    let len = (tail.len() - 2) as u16;
    tail[..2].copy_from_slice(&len.to_be_bytes());
    let crc = crc16_modbus(&tail);
    let mut out = vec![0, 7, (crc >> 8) as u8, crc as u8];
    out.extend(tail);
    out
}

// ---- digital outputs -----------------------------------------------------------------------

#[test]
fn an_output_switch_encodes_decodes_and_reaches_the_card() {
    let on = command::do_set(2, true).unwrap();
    assert_eq!(on.words(), &[9999, 2, 0b10, 0b10]);
    assert_eq!(on.policy(), Policy::Motion);
    #[rustfmt::skip]
    assert_eq!(body(&on), [
        0x00, 0x18, 0x00, 0x40, // length 24, unit 0, WRITE
        0x65, 0x00, 0x00, 0x00, 0x04, 0x00, // register 0x65, 4 words
        0x0F, 0x27, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00,
    ]);
    decodes_to_itself(&on);
    let off = command::do_set(2, false).unwrap();
    assert_eq!(off.words(), &[9999, 2, 0b10, 0]);
    assert_eq!(off.policy(), Policy::Always);
    assert_eq!(off, command::do_off(2).unwrap());
    decodes_to_itself(&off);

    let mut l = armed();
    l.write(&on).unwrap();
    assert!(l.poll_status().unwrap().do_is_on(2));
    assert_eq!(l.sim.outputs(), 0b10);
    l.write(&off).unwrap();
    assert!(!l.poll_status().unwrap().do_is_on(2));
    assert!(l.sim.forbidden_attempts().is_empty());
    assert_eq!(l.sim.laser_refusals(), 0);
}

#[test]
fn laser_ports_and_unknown_ports_cannot_be_switched_on() {
    for port in [5, 6, 9] {
        assert!(matches!(command::do_set(port, true), Err(Forbidden::LaserData(_))), "port {port}");
        assert_eq!(command::do_set(port, false).unwrap().policy(), Policy::Always, "off is always there");
    }
    // ports the machine description does not name, and the extended output word
    for port in [4, 8, 10] {
        assert!(matches!(command::do_set(port, true), Err(Forbidden::Command(_))), "port {port}");
    }
    for port in [0, 11, 18, 26, 27, u32::MAX] {
        assert!(command::do_set(port, true).is_err(), "port {port}");
        assert!(command::do_set(port, false).is_err(), "port {port}");
    }
    // the lamp and gas ports are the ones that can be switched on, and only with motion armed
    for port in [1, 2, 3, 7] {
        assert_eq!(command::do_set(port, true).unwrap().policy(), Policy::Motion);
    }
}

// ---- analogue output and PWM ---------------------------------------------------------------

#[test]
fn analogue_output_is_zero_or_nothing() {
    let z = command::da_set(2, 0.0).unwrap();
    assert_eq!(z.words(), &[9999, 4, 1, 0]);
    assert_eq!(z.policy(), Policy::Always);
    assert_eq!(z, command::da_zero(2).unwrap());
    #[rustfmt::skip]
    assert_eq!(body(&z), [
        0x00, 0x18, 0x00, 0x40, 0x65, 0x00, 0x00, 0x00, 0x04, 0x00,
        0x0F, 0x27, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    ]);
    decodes_to_itself(&z);
    // the millivolt word follows the vendor's rounding
    assert_eq!(command::da_millivolts(0.0).unwrap(), 0);
    assert_eq!(command::da_millivolts(0.0009).unwrap(), 0);
    assert_eq!(command::da_millivolts(0.001).unwrap(), 50);
    assert_eq!(command::da_millivolts(0.0499).unwrap(), 50);
    assert_eq!(command::da_millivolts(2.5).unwrap(), 2500);
    assert_eq!(command::da_millivolts(12.0).unwrap(), 10_000);
    assert!(command::da_millivolts(-0.5).is_err());
    assert!(command::da_millivolts(f64::NAN).is_err());
    // a pressure or a power is never written
    for volts in [0.001, 0.03, 2.5, 10.0, 99.0] {
        assert!(matches!(command::da_set(1, volts), Err(Forbidden::Command(_))), "{volts} V");
    }
    assert!(command::da_set(1, -1.0).is_err());
    assert!(command::da_set(0, 0.0).is_err());
    assert!(command::da_set(3, 0.0).is_err());

    let mut l = link();
    l.write(&z).unwrap();
    assert_eq!(l.poll_status().unwrap().word(Status::Da2), 0);
    assert_eq!(l.sim.commands(), &[vec![9999, 4, 1, 0]]);
}

#[test]
fn pwm_without_motion_is_zero_duty_or_nothing() {
    let w = command::pwm_set(1234, 0, 3).unwrap();
    assert_eq!(w.words(), &[9999, 3, 1234, 0]);
    assert_eq!(w.policy(), Policy::Always);
    #[rustfmt::skip]
    assert_eq!(body(&w), [
        0x00, 0x18, 0x00, 0x40, 0x65, 0x00, 0x00, 0x00, 0x04, 0x00,
        0x0F, 0x27, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00, 0xD2, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    ]);
    decodes_to_itself(&w);
    assert_eq!(command::pwm_set(5000, 0, 0x11).unwrap().words(), &[9999, 0x11, 5000, 0]);
    for duty in [1, 30, 100, u32::MAX] {
        assert!(matches!(command::pwm_set(1234, duty, 3), Err(Forbidden::LaserData(_))), "duty {duty}");
        assert!(matches!(command::pwm_set(1234, duty, 0x11), Err(Forbidden::LaserData(_))), "duty {duty}");
    }
    assert!(matches!(command::pwm_set(1234, 0, 7), Err(Forbidden::Command(_))));

    let mut l = link();
    l.write(&w).unwrap();
    let s = l.poll_status().unwrap();
    assert_eq!((s.word(Status::PwmFreq), s.word(Status::PwmDuty)), (1234, 0));
}

// ---- stop variants, pause, emergency stop --------------------------------------------------

#[test]
fn the_three_stop_clamps() {
    // 100000 / (10000 / 1000) = 10000 before clamping; 0.4 * 20000 = 8000
    let cases = [
        (StopVariant::MainProgram, 10_000, 8000),
        (StopVariant::MainProgramSoftLimit, 10_000, 8000),
        (StopVariant::PendantOrBuilder, 10_000, 10_000),
        // 100000 / 200 = 500 before clamping
        (StopVariant::MainProgram, 200_000, 2000),
        (StopVariant::MainProgramSoftLimit, 200_000, 5000),
        (StopVariant::PendantOrBuilder, 200_000, 2000),
        // slower than 1 mm/s: the quotient is zero and the upper bound is used
        (StopVariant::MainProgram, 500, 8000),
        (StopVariant::PendantOrBuilder, 500, 100_000),
    ];
    for (variant, v_last, vd) in cases {
        assert_eq!(command::stop_decel_word_for(v_last, &p(), variant), vd, "{variant:?} {v_last}");
        let w = command::jog_release_stop_with(&[0, 1], v_last, &p(), variant).unwrap();
        assert_eq!(w.words(), &[1, 0b11, 2, vd, vd * 10], "{variant:?} {v_last}");
        assert_eq!(w.policy(), Policy::Always);
        decodes_to_itself(&w);
    }
    assert_eq!(StopVariant::default(), StopVariant::MainProgram);
    let w = command::jog_release_stop_with(&[0], 10_000, &p(), StopVariant::MainProgram).unwrap();
    #[rustfmt::skip]
    assert_eq!(body(&w), [
        0x00, 0x1C, 0x00, 0x40, 0x65, 0x00, 0x00, 0x00, 0x05, 0x00,
        0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x40, 0x1F, 0x00, 0x00, 0x80, 0x38, 0x01, 0x00,
    ]);
    assert!(command::jog_release_stop_with(&[], 0, &p(), StopVariant::MainProgram).is_err());
    assert!(command::jog_release_stop_with(&[5], 0, &p(), StopVariant::MainProgram).is_err());

    // through the card: a held jog is cancelled by each variant, in any arming state
    for variant in [StopVariant::MainProgram, StopVariant::MainProgramSoftLimit, StopVariant::PendantOrBuilder] {
        let mut l = armed();
        l.write(&command::jog_continuous(0, true, &p(), None).unwrap()).unwrap();
        assert_eq!(l.poll_status().unwrap().word(Status::RunStatus), 1);
        l.gate.arming.disarm("test");
        l.write(&command::jog_release_stop_with(&[0], 200_000, &p(), variant).unwrap()).unwrap();
        l.advance(ms(1));
        assert_eq!(l.poll_status().unwrap().word(Status::RunStatus), 0, "{variant:?}");
    }
}

#[test]
fn pause_is_the_stop_and_an_emergency_stop_adds_the_laser_gate() {
    let stop = command::stop_sequence(&p(), true, 0, 0b1_0001_0000, &[5, 6, 9]);
    let pause = command::pause_sequence(&p(), true, 0, 0b1_0001_0000, &[5, 6, 9]);
    assert_eq!(pause, stop);
    // no laser-gate port assigned: the emergency stop is the stop
    assert_eq!(command::estop_sequence(&p(), true, 0, 0b1_0001_0000, &[5, 6, 9], 0), stop);
    // with one, it goes off right after the motion stop
    let estop = command::estop_sequence(&p(), false, 0, 0b1_0001_0000, &[5, 6, 9], 9);
    assert_eq!(estop[0].words()[..2], [1, 0x1F]);
    assert_eq!(estop[1].words(), &[9999, 2, 0x100, 0]);
    assert_eq!(estop.len(), command::stop_sequence(&p(), false, 0, 0b1_0001_0000, &[5, 6, 9]).len() + 1);
    for w in pause.iter().chain(&estop) {
        assert_eq!(w.policy(), Policy::Always, "{}", w.name());
        assert_ne!(w.words(), [101], "the follower stop is never part of it");
        assert_ne!(w.words(), [102]);
        decodes_to_itself(w);
    }
    // a port number the card does not have adds nothing and takes nothing away
    assert_eq!(command::estop_sequence(&p(), false, 0, 0, &[5, 6, 9], 40).len(), command::stop_sequence(&p(), false, 0, 0, &[5, 6, 9]).len());
}

#[test]
fn an_operator_emergency_stop_latches_and_reaches_the_card() {
    let mut l = armed();
    l.write(&command::do_set(2, true).unwrap()).unwrap();
    l.write(&command::jog_continuous(1, false, &p(), None).unwrap()).unwrap();
    l.poll_status().unwrap();
    let before = l.sim.commands().len();
    let steps = l.emergency_stop(9);
    assert!(steps >= 6, "{steps} steps reached the card");
    let sent = &l.sim.commands()[before..];
    assert_eq!(sent[0][..2], [1, 0x1F], "motion stops first");
    assert_eq!(sent[1], vec![9999, 2, 0x100, 0], "then the laser gate");
    assert!(l.gate.arming.estop_latched());
    assert_eq!(l.gate.arming.state(), ArmState::Disarmed);
    l.advance(ms(1));
    assert_eq!(l.poll_status().unwrap().word(Status::RunStatus), 0);
    // the gas stays as it was: a stop switches nothing on and only the laser ports off
    assert_eq!(l.sim.outputs(), 0b10);
    // nothing moves until the operator has acknowledged and armed again
    assert!(l.gate.arming.arm_motion().is_err());
    assert!(matches!(
        l.write(&command::jog_step(0, 1.0, &p()).unwrap()),
        Err(LinkError::Refused(GateRefusal::NeedsArming { estop_latched: true, .. }))
    ));
    l.gate.arming.acknowledge_estop();
    l.gate.arming.arm_motion().unwrap();
    l.write(&command::jog_step(0, 1.0, &p()).unwrap()).unwrap();
}

#[test]
fn a_paused_job_cannot_be_resumed_only_cleared() {
    let mut l = armed();
    l.write(&command::fifo_clear().unwrap()).unwrap();
    for id in 1..=3 {
        l.write(&command::fifo_frame(id, &vec![FifoItem::tick(1, 0); 90]).unwrap()).unwrap();
    }
    l.write(&command::fifo_start().unwrap()).unwrap();
    l.advance(Duration::from_micros(250 * 100));
    assert_eq!(l.pause(), 6);
    assert!(!l.sim.fifo_running());
    assert_eq!(l.sim.fifo_depth(), 170, "the card holds the rest");
    assert!(l.gate.queue_held());
    assert_eq!(l.gate.arming.state(), ArmState::MotionArmed, "a pause does not disarm");
    // what continues a paused job on this card is not known, so a bare start is refused
    l.poll_status().unwrap();
    assert_eq!(l.write(&command::fifo_start().unwrap()), Err(LinkError::Refused(GateRefusal::QueueHeld)));
    l.advance(ms(100));
    assert_eq!(l.sim.ticks_consumed(), 100);
    l.poll_status().unwrap();
    l.write(&command::fifo_clear().unwrap()).unwrap();
    assert!(!l.gate.queue_held());
    assert_eq!(l.sim.fifo_depth(), 0);
}

// ---- connect, disconnect, typed reads ------------------------------------------------------

#[test]
fn a_connect_only_reads() {
    let mut l = link();
    let r = l.connect(ConnectOptions::default()).unwrap();
    assert_eq!((r.version.program_id, r.version.program_version), (100, 20152));
    assert_eq!((r.system.bus_cycle_us(), r.system.zf_type(), r.system.k()), (250, 1, Some(1000)));
    assert_eq!(r.status.fifo_space_margin(), 60000);
    assert_eq!(r.axis_ro.homed_mask(), 0);
    assert_eq!(r.axis_rw.words.len(), 100);
    assert_eq!(r.follower.as_ref().map(|z| z.state()), Some(ZfState::Ready));
    assert!(!r.prologue_sent);
    // every request was a read; nothing was written, not even the prologue
    let vectors: Vec<Vec<u32>> = l.sim.requests().filter_map(|q| q.vector.clone()).collect();
    assert_eq!(
        vectors,
        [vec![0x30, 1000, 2], vec![0x30, 1000, 36], vec![0x30, 50000, 26], vec![0x30, 2000, 50], vec![0x30, 50200, 100], vec![0x30, 10000, 18]]
    );
    assert!(l.sim.commands().is_empty());
    assert!(l.sim.forbidden_attempts().is_empty());
    // connected is not armed
    assert_eq!(l.gate.arming.state(), ArmState::Disarmed);
    assert!(matches!(l.write(&command::jog_step(0, 1.0, &p()).unwrap()), Err(LinkError::Refused(GateRefusal::NeedsArming { .. }))));
}

#[test]
fn connect_options_prologue_version_and_no_follower() {
    let mut l = link();
    let r = l.connect(ConnectOptions { send_prologue: true, ..ConnectOptions::default() }).unwrap();
    assert!(r.prologue_sent);
    assert_eq!(l.sim.commands(), &[vec![9999, 5, 0, 0]], "the prologue and nothing else");
    // a card older than the software asks for is refused before anything else happens
    let mut l = link();
    let e = l.connect(ConnectOptions { min_version: MIN_PROGRAM_VERSION_SP1, send_prologue: true }).unwrap_err();
    assert_eq!(e, LinkError::Version { found: 20152, needed: 20177 });
    assert!(l.sim.commands().is_empty());
    assert_eq!(l.sim.requests().count(), 1);
    // a machine without an on-board follower: its status block is not asked for
    let mut l = SimLink::new(CardSimulator::new(SimConfig { zf_type: 0, ..SimConfig::default() }));
    let r = l.connect(ConnectOptions::default()).unwrap();
    assert!(r.follower.is_none() && !r.system.has_follower());
    assert!(l.sim.requests().all(|q| q.vector.as_deref() != Some(&[0x30, 10000, 18])));
    // a card that does not answer
    let mut l = link();
    l.sim.reboot(ms(1000), ms(0));
    assert_eq!(l.connect(ConnectOptions::default()), Err(LinkError::Timeout));
}

#[test]
fn a_disconnect_stops_disarms_and_forgets() {
    let mut l = armed();
    l.write(&command::fifo_clear().unwrap()).unwrap();
    l.write(&command::fifo_frame(1, &vec![FifoItem::tick(1, 0); 90]).unwrap()).unwrap();
    l.write(&command::fifo_start().unwrap()).unwrap();
    assert!(l.disconnect() >= 6);
    assert!(!l.sim.fifo_running());
    assert_eq!(l.gate.arming.state(), ArmState::Disarmed);
    assert!(l.gate.last_status_ms.is_none() && !l.gate.homed);
    // arming again is not enough: the card has to be asked how it is first
    l.gate.arming.arm_motion().unwrap();
    assert_eq!(l.write(&command::jog_step(0, 1.0, &p()).unwrap()), Err(LinkError::Refused(GateRefusal::CommLoss)));
    l.poll_status().unwrap();
    l.write(&command::jog_step(0, 1.0, &p()).unwrap()).unwrap();
}

#[test]
fn typed_reads_round_trip_through_the_card() {
    let mut l = armed();
    // axis block: homed bit, busy, fault bits
    l.write(&command::home_axis(0).unwrap()).unwrap();
    assert!(l.read_axis_ro().unwrap().busy(0));
    l.advance(ms(400));
    l.sim.inject_axis_fault(1, axis::SERVO_ALARM | axis::HARD_LIMIT_NEG);
    let ro = l.read_axis_ro().unwrap();
    assert_eq!(ro.homed_mask(), 0b1);
    assert!(!ro.busy(0));
    assert_eq!(ro.faults(1), axis::SERVO_ALARM | axis::HARD_LIMIT_NEG);
    assert_eq!(axis::fault_names(ro.status(1)), ["hard limit -", "servo alarm"]);
    // a jog shows up in the position word
    l.poll_status().unwrap();
    l.write(&command::jog_step(1, -2.5, &p()).unwrap()).unwrap();
    l.advance(ms(400));
    assert_eq!(l.read_axis_ro().unwrap().pulse_position(1), -2500);
    // axis parameters: lead and pulses per revolution give pulses per millimetre
    assert!(l.sim.set_card_word(50200 + 9, 10_000));
    assert!(l.sim.set_card_word(50200 + 10, 20_000));
    let k = l.read_system().unwrap().k().unwrap();
    assert_eq!(l.read_axis_rw().unwrap().pulses_per_mm(0, k), Some(2000.0));
    // the fast block shows the same two blocks in one read
    let fast = l.read_fast().unwrap();
    assert_eq!(fast.axis_ro(), l.read_axis_ro().unwrap());
    assert_eq!(fast.pulses_per_mm(0, k), Some(2000.0));
    // follower status and properties, hardware parameters: read, never written
    assert!(l.sim.set_card_word(10001, 1 << 5));
    assert_eq!(l.read_follower().unwrap().alarm_names(), ["touched the plate"]);
    assert_eq!(l.read_follower_properties().unwrap().len(), 39);
    for i in [0, 1, 31] {
        assert_eq!(l.read_hard_params(i).unwrap(), vec![0; 52], "block {i}");
    }
    assert!(matches!(l.read_hard_params(32), Err(LinkError::Forbidden(_))));
    assert_eq!(l.read_version().unwrap().program_version, 20152);
    // the stimulus cannot touch what the crate may not touch either
    for addr in [150, 151, 59500, 59599, 0x65, 99_999] {
        assert!(!l.sim.set_card_word(addr, 1), "{addr}");
    }
    assert!(l.sim.forbidden_attempts().is_empty());
}

#[test]
fn reads_the_rules_forbid_stay_refused() {
    let mut l = link();
    for (addr, count) in
        [(150u32, 1u16), (151, 1), (149, 3), (59500, 1), (59502, 4), (59599, 1), (59590, 20), (200, 1), (13000, 4), (13200, 4), (100, 1)]
    {
        assert!(matches!(l.read(addr, count), Err(LinkError::Refused(GateRefusal::ReadNotAllowed(_)))), "{addr}/{count}");
    }
    assert_eq!(l.sim.requests().count(), 0, "none of them reached the card");
}

// ---- nothing new slips past the rules ------------------------------------------------------

#[test]
fn everything_that_moves_or_opens_gas_is_refused_until_armed() {
    let gated = vec![
        command::do_set(1, true).unwrap(),
        command::do_set(2, true).unwrap(),
        command::do_set(3, true).unwrap(),
        command::do_set(7, true).unwrap(),
        command::jog_continuous(0, true, &p(), None).unwrap(),
        command::jog_step(1, 1.0, &p()).unwrap(),
        command::move_axis_absolute(0, 10.0, &p()).unwrap(),
        command::go_to(10.0, 10.0, &p()).unwrap(),
        command::home_axis(0).unwrap(),
        command::home_axis(1).unwrap(),
        command::fifo_clear().unwrap(),
        command::fifo_start().unwrap(),
        command::fifo_frame(1, &[FifoItem::tick(1, 1)]).unwrap(),
    ];
    let mut l = link();
    l.connect(ConnectOptions::default()).unwrap();
    for w in &gated {
        assert!(w.policy() >= Policy::Motion, "{}", w.name());
        assert!(
            matches!(l.write(w), Err(LinkError::Refused(GateRefusal::NeedsArming { state: ArmState::Disarmed, .. }))),
            "{} {:?} went past a gate that is not armed",
            w.name(),
            w.words()
        );
    }
    assert!(l.sim.commands().is_empty());
    assert_eq!(l.sim.frames_accepted(), 0);
    assert_eq!(l.sim.outputs(), 0);
    assert!(!l.sim.fifo_running());
    // what always passes is what makes the machine safer
    let always = vec![
        command::do_set(2, false).unwrap(),
        command::da_set(1, 0.0).unwrap(),
        command::pwm_set(1234, 0, 3).unwrap(),
        command::jog_release_stop_with(&[0], 0, &p(), StopVariant::MainProgramSoftLimit).unwrap(),
        command::fifo_stop().unwrap(),
    ];
    for w in &always {
        assert_eq!(w.policy(), Policy::Always, "{}", w.name());
        l.write(w).unwrap_or_else(|e| panic!("{}: {e}", w.name()));
    }
}

#[test]
fn the_vendor_operations_the_rules_forbid_cannot_be_built_decoded_or_served() {
    // (register, words, what the vendor software uses it for)
    let cases: &[(u32, &[u32], &str)] = &[
        (150, &[5555], "follower factory reset"),
        (150, &[9999], "follower parameter commit"),
        (151, &[0], "edge-seek record"),
        (151, &[1, 2, 3], "edge-seek record"),
        (59500, &[1], "licence area"),
        (59502, &[0x1234], "licence area"),
        (59503, &[1], "licence area"),
        (59510, &[2026, 11, 12], "card clock"),
        (59511, &[1], "card clock"),
        (59599, &[1], "licence area"),
        (59600, &[1], "hardware parameter"),
        (59600 + 0xD0 * 31, &[1], "hardware parameter"),
        (11000, &[1], "follower property"),
        (11002, &[1], "follower property"),
        (11076, &[1], "follower property"),
        (100, &[9999], "restart / apply"),
        (100, &[8888], "the unexplained write of a newer release's connect"),
        (5001, &[9999, 9, 0xFFFF, 0], "block 5000"),
        (5008, &[1], "block 5000"),
        (5012, &[1], "block 5000"),
        (200, &[1], "test data"),
        (201, &[1], "test data"),
        (50000, &[1], "system block"),
        (50017, &[1000], "K"),
        (50200, &[1], "axis parameter"),
        (0x65, &[0x65], "follower stop"),
        (0x65, &[0x66], "follower resync"),
        (0x65, &[107], "head calibration"),
        (0x65, &[103, 500, 3000], "follower move"),
        (0x65, &[104, 1, 2], "follow"),
        (0x65, &[109, 500, 3000], "follower lift"),
        (0x65, &[117, 1], "pierce"),
        (0x65, &[118, 5, 0], "follower bookkeeping"),
        (0x65, &[7], "offline upload"),
        (0x65, &[4, 1, 2], "offline start"),
        (0x65, &[1, 8], "stop of one slot without deceleration"),
        (0x65, &[2, 0, 0], "home nothing"),
        (0x65, &[2, 0x10003, 0], "system home"),
        (0x65, &[2, 0x10, 0], "lift-table home"),
        (0x65, &[3, 4, 50_000, 4000, 40_000, 1_000_000], "lift-table jog"),
        (0x65, &[9999, 13, 0xFFFF, 0], "extended outputs"),
        (0x65, &[9999, 13, 1, 1], "extended outputs"),
        (0x65, &[9999, 16], "unexplained"),
        (0x65, &[9999, 1, 0], "old queue clear"),
        (0x65, &[9999, 4, 0, 5000], "analogue output: gas pressure or power"),
        (0x67, &[0], "queue control"),
        (0x67, &[4], "queue control"),
    ];
    let mut sim = CardSimulator::new(SimConfig::default());
    for (i, (addr, words, what)) in cases.iter().enumerate() {
        assert!(SafeWrite::try_from_raw(*addr, words).is_err(), "{what}: {addr} <- {words:?} can be built");
        let frame = hand_built(*addr, words);
        assert!(matches!(Request::decode(&frame), Err(RequestError::Forbidden(_))), "{what}: {addr} <- {words:?} decodes");
        // the simulated card answers with an exception and counts the attempt
        let reply = sim.exchange(&frame).unwrap();
        assert!(RawFrame::parse(&reply).is_exception(), "{what}");
        assert_eq!(sim.forbidden_attempts().len(), i + 1, "{what}");
    }
    assert!(sim.commands().is_empty());
    assert_eq!((sim.reg(150), sim.reg(151), sim.reg(100)), (0, 0, 0));
    assert_eq!(sim.outputs(), 0);
    // laser data is refused as laser data
    for words in [&[9999u32, 2, 0x100, 0x100][..], &[9999, 2, 0x10, 0x10], &[9999, 2, 0x20, 0x20], &[9999, 3, 1234, 30], &[9999, 0x11, 1234, 30, 0]] {
        assert!(matches!(SafeWrite::try_from_raw(0x65, words), Err(Forbidden::LaserData(_))), "{words:?}");
        assert!(RawFrame::parse(&sim.exchange(&hand_built(0x65, words)).unwrap()).is_exception());
    }
    assert_eq!(sim.laser_refusals(), 5);
    assert_eq!(sim.outputs(), 0);
}
