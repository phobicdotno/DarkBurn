//! The gate matrix of bug hunt pass 9, on the Gweike driver: an alarm, an emergency stop and a
//! restart request in every phase of the machine, a stop and a disconnect right after a start,
//! and the three faults the pass found in other drivers: a switch-off that is overtaken by its
//! switch-on, a job the card holds that shows as running, and an alarm that only the status
//! report shows and that leaves the job in place.

use darkburn_mcc::command::{self, MachineParams};
use darkburn_mcc::fifo::FifoItem;
use darkburn_mcc::gate::{ArmState, GateRefusal};
use darkburn_mcc::registers::{MachineState, Status, alarm1, alarm2, derive_machine_state};
use darkburn_mcc::sim::{CardSimulator, LinkError, SimConfig, SimLink};
use std::time::Duration;

fn p() -> MachineParams {
    MachineParams::default()
}

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    Idle,
    HeldJog,
    Homing,
    Queued,
    Streaming,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Event {
    Alarm1Axis,
    Alarm2Command,
    EmergencyStop,
    RestartRequired,
}

/// Motion armed, gas on, and in `phase`.
fn in_phase(phase: Phase) -> SimLink {
    let mut l = SimLink::new(CardSimulator::new(SimConfig { fifo_starvation_alarm: false, ..SimConfig::default() }));
    l.gate.arming.arm_motion().unwrap();
    l.poll_status().unwrap();
    l.write(&command::do_set(2, true).unwrap()).unwrap();
    match phase {
        Phase::Idle => {}
        Phase::HeldJog => l.write(&command::jog_continuous(0, true, &p(), None).unwrap()).unwrap(),
        Phase::Homing => l.write(&command::home_axis(1).unwrap()).unwrap(),
        Phase::Queued | Phase::Streaming => {
            l.write(&command::fifo_clear().unwrap()).unwrap();
            for id in 1..=3 {
                l.write(&command::fifo_frame(id, &vec![FifoItem::tick(1, 0); 90]).unwrap()).unwrap();
            }
            if phase == Phase::Streaming {
                l.write(&command::fifo_start().unwrap()).unwrap();
                l.advance(Duration::from_micros(250 * 20));
            }
        }
    }
    l
}

fn raise(l: &mut SimLink, event: Event) {
    match event {
        Event::Alarm1Axis => l.sim.inject_alarm(Some(alarm1::AXIS_X), None),
        Event::Alarm2Command => l.sim.inject_alarm(None, Some(alarm2::AXIS_COMMAND)),
        Event::EmergencyStop => l.sim.inject_alarm(Some(alarm1::ESTOP), None),
        Event::RestartRequired => l.sim.set_param_status(1),
    }
}

fn clear(l: &mut SimLink) {
    l.sim.inject_alarm(Some(0), Some(0));
    l.sim.set_param_status(0);
}

#[test]
fn every_event_in_every_phase_leaves_the_machine_stopped_and_the_gate_shut() {
    for phase in [Phase::Idle, Phase::HeldJog, Phase::Homing, Phase::Queued, Phase::Streaming] {
        for event in [Event::Alarm1Axis, Event::Alarm2Command, Event::EmergencyStop, Event::RestartRequired] {
            let tag = format!("{phase:?} + {event:?}");
            let mut l = in_phase(phase);
            let ticks_before = l.sim.ticks_consumed();
            raise(&mut l, event);
            l.poll_status().unwrap();

            // An alarm or an emergency stop disarms in every phase. A restart request stops what
            // the gate knows to be running (a stream, a held jog) and blocks everything new.
            let is_alarm = event != Event::RestartRequired;
            if is_alarm {
                assert_eq!(l.gate.arming.state(), ArmState::Disarmed, "{tag}");
            }
            assert_eq!(l.gate.arming.estop_latched(), event == Event::EmergencyStop, "{tag}");
            assert!(l.gate.machine_fault.is_some(), "{tag}");

            // The stream is stopped and stays stopped.
            assert!(!l.sim.fifo_running(), "{tag}");
            l.advance(ms(20));
            if phase == Phase::Streaming {
                assert!(l.sim.ticks_consumed() <= ticks_before + 1, "{tag}: the stream ran on");
            }
            // Axis moves are cancelled (a home under a restart request is the one the gate does
            // not know about and leaves to the card).
            let s = l.poll_status().unwrap();
            if is_alarm || phase != Phase::Homing {
                assert_eq!(s.word(Status::RunStatus), 0, "{tag}: still moving");
            }
            // The laser ports are off and the stop switched nothing on: the gas is as it was.
            assert_eq!(l.sim.outputs(), 0b10, "{tag}");

            // Nothing that moves, streams or opens gas goes out while the event stands.
            for w in [
                command::jog_step(0, 1.0, &p()).unwrap(),
                command::home_axis(0).unwrap(),
                command::fifo_start().unwrap(),
                command::fifo_clear().unwrap(),
                command::do_set(3, true).unwrap(),
            ] {
                assert!(matches!(l.write(&w), Err(LinkError::Refused(_))), "{tag}: {} went out", w.name());
            }
            // Arming again while it stands does not open the gate either.
            if event == Event::EmergencyStop {
                assert!(l.gate.arming.arm_motion().is_err(), "{tag}");
            } else {
                l.gate.arming.arm_motion().unwrap();
                assert!(matches!(l.write(&command::jog_step(0, 1.0, &p()).unwrap()), Err(LinkError::Refused(GateRefusal::MachineFault(_)))), "{tag}");
            }
            // Stops still pass.
            l.write(&command::stop_all_no_decel().unwrap()).unwrap();
            l.write(&command::do_set(2, false).unwrap()).unwrap();

            // The event goes away; the operator acknowledges and arms.
            clear(&mut l);
            l.poll_status().unwrap();
            l.gate.arming.acknowledge_estop();
            l.gate.arming.arm_motion().unwrap();
            // A job that was stopped is not run on by a bare start: the queue has to be cleared.
            let stopped_a_job = match phase {
                Phase::Streaming => true,
                Phase::Queued => is_alarm,
                _ => false,
            };
            if stopped_a_job {
                assert!(l.gate.queue_held(), "{tag}");
                assert_eq!(l.write(&command::fifo_start().unwrap()), Err(LinkError::Refused(GateRefusal::QueueHeld)), "{tag}");
                l.write(&command::fifo_clear().unwrap()).unwrap();
                assert_eq!(l.sim.fifo_depth(), 0, "{tag}");
            }
            // And the machine works again.
            l.advance(ms(400));
            l.poll_status().unwrap();
            l.write(&command::jog_step(0, 1.0, &p()).unwrap()).unwrap_or_else(|e| panic!("{tag}: {e}"));
            assert!(l.sim.forbidden_attempts().is_empty(), "{tag}");
            assert_eq!(l.sim.laser_refusals(), 0, "{tag}");
        }
    }
}

#[test]
fn a_stop_right_after_a_start_stops_and_holds_the_job() {
    let mut l = in_phase(Phase::Queued);
    l.write(&command::fifo_start().unwrap()).unwrap();
    assert_eq!(l.stop_sequence(false), 6);
    assert!(!l.sim.fifo_running());
    l.advance(ms(50));
    assert_eq!(l.sim.ticks_consumed(), 0, "not one tick ran");
    // The card holds the job; it does not show as running anywhere.
    let s = l.poll_status().unwrap();
    assert!(!s.fifo_running());
    assert!(!l.gate.fifo_running);
    assert_eq!(derive_machine_state(&s, &l.read(2000, 50).unwrap()), MachineState::Ready);
    assert_eq!(l.sim.fifo_depth(), 270);
    assert!(l.gate.queue_held());
    assert_eq!(l.write(&command::fifo_start().unwrap()), Err(LinkError::Refused(GateRefusal::QueueHeld)));
}

#[test]
fn a_stop_before_the_start_holds_the_queue_too() {
    // Frames are on the card, the operator presses stop, and only then start: nothing runs.
    let mut l = in_phase(Phase::Queued);
    l.stop_sequence(false);
    l.poll_status().unwrap();
    assert_eq!(l.write(&command::fifo_start().unwrap()), Err(LinkError::Refused(GateRefusal::QueueHeld)));
    assert!(!l.sim.fifo_running());
    // a jog key released in between is not a stop of the job
    let mut l = in_phase(Phase::Queued);
    l.write(&command::jog_release_stop(&[0], 0, &p()).unwrap()).unwrap();
    l.write(&command::fifo_start().unwrap()).unwrap();
    assert!(l.sim.fifo_running());
}

#[test]
fn a_disconnect_right_after_a_start_leaves_nothing_running() {
    let mut l = in_phase(Phase::Queued);
    l.write(&command::fifo_start().unwrap()).unwrap();
    l.disconnect();
    assert!(!l.sim.fifo_running());
    assert_eq!(l.gate.arming.state(), ArmState::Disarmed);
    l.advance(ms(50));
    assert_eq!(l.sim.ticks_consumed(), 0);
    // connecting again does not run what the card still holds
    l.gate.arming.arm_motion().unwrap();
    l.poll_status().unwrap();
    assert_eq!(l.write(&command::fifo_start().unwrap()), Err(LinkError::Refused(GateRefusal::QueueHeld)));
}

#[test]
fn a_switch_off_is_not_overtaken_by_its_switch_on() {
    // The answer to "gas on" is lost. The operator switches the gas off. The card must end with
    // the gas off, and the late or missing answer must not confuse what follows.
    for late in [false, true] {
        let mut l = in_phase(Phase::Idle);
        l.write(&command::do_set(2, false).unwrap()).unwrap();
        l.attempts = 2;
        if late {
            l.sim.delay_replies(ms(5), Some(2));
        } else {
            l.sim.drop_replies(2);
        }
        assert_eq!(l.write(&command::do_set(3, true).unwrap()), Err(LinkError::Timeout), "late {late}");
        assert_eq!(l.sim.outputs(), 0b100, "the card did switch it on");
        l.write(&command::do_set(3, false).unwrap()).unwrap();
        assert_eq!(l.sim.outputs(), 0, "late {late}");
        // the late answers arrive now; they belong to nothing that is waiting
        l.advance(ms(10));
        let s = l.poll_status().unwrap();
        assert_eq!(s.word(Status::Do) & 0xFFFF, 0, "late {late}");
        assert_eq!(l.sim.outputs(), 0, "late {late}: something switched it back on");
        assert_eq!(l.sim.commands().iter().filter(|c| c.as_slice() == [9999, 2, 0b100, 0b100]).count(), 2, "sent twice, never again");
    }
}

#[test]
fn an_alarm_that_only_the_status_shows_stops_the_job_and_says_so() {
    let mut l = in_phase(Phase::Streaming);
    // The card raises an alarm on its own. No write fails, nothing is announced: only the next
    // status read shows it.
    l.sim.inject_alarm(None, Some(alarm2::FTC_COMMAND));
    assert!(l.sim.fifo_running(), "the simulated card runs on until it is told to stop");
    let s = l.poll_status().unwrap();
    assert_eq!(s.alarm2(), alarm2::FTC_COMMAND);
    assert!(!l.sim.fifo_running());
    assert_eq!(l.gate.arming.state(), ArmState::Disarmed);
    // The caller can tell that this was not a job that finished.
    assert!(l.gate.machine_fault.as_deref().is_some_and(|f| f.contains("alarm_2")));
    assert!(l.gate.queue_held());
    assert!(l.sim.fifo_depth() > 0);
    // No more frames are taken for the stopped job.
    assert!(matches!(l.write(&command::fifo_frame(4, &[FifoItem::tick(1, 0)]).unwrap()), Err(LinkError::Refused(_))));
}
