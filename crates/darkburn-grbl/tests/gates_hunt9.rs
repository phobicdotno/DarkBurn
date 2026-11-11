//! Pass 9 of the bug hunt, safety gates as a matrix: every kind of gate fault of pass 8, and an
//! alarm arriving in every phase, against the GRBL driver. In-memory emulator only.

use std::time::{Duration, Instant};

use darkburn_grbl::*;
use kurbo::Rect;

const T: Duration = Duration::from_secs(10);

fn rig_with(cfg: EmulatorConfig) -> (GrblMachine, EmulatorHandle) {
    let emu = EmulatorHandle::new(GrblEmulator::new(cfg));
    let m = GrblMachine::connect(Box::new(emu.transport()), ConnectOptions::default());
    let end = Instant::now() + T;
    loop {
        match m.next_event(Duration::from_millis(20)) {
            Some(Event::Connected(_)) => break,
            Some(Event::ConnectFailed(r)) => panic!("connect failed: {r}"),
            _ => assert!(Instant::now() < end, "no handshake"),
        }
    }
    assert!(m.wait_until(T, |s| s.status.state == MachineState::Idle && s.activity == Activity::Idle));
    (m, emu)
}

fn rig() -> (GrblMachine, EmulatorHandle) {
    rig_with(EmulatorConfig::atomstack().with_time_scale(5.0))
}

fn wait_emu(emu: &EmulatorHandle, what: &str, f: impl Fn(&mut GrblEmulator) -> bool) {
    let end = Instant::now() + T;
    while Instant::now() < end {
        if emu.with(&f) {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!("timed out waiting for emulator: {what} (state {})", emu.with(|e| e.state_name()));
}

/// Events until one matches.
fn until(m: &GrblMachine, f: impl Fn(&Event) -> bool) -> Vec<Event> {
    let end = Instant::now() + T;
    let mut out = Vec::new();
    while Instant::now() < end {
        if let Some(e) = m.next_event(Duration::from_millis(20)) {
            let done = f(&e);
            out.push(e);
            if done {
                return out;
            }
        }
    }
    panic!("timed out; got {out:?}");
}

/// A slow burn: long enough to act in the middle of it.
const BURN: &str = "G21 G90\nM4 S600\nG1 X250 F1500\nG1 Y250\nG1 X0\nG1 Y0\nM5";

/// A long job of short lines, so the controller's buffer is full while it runs.
fn long_job() -> Job {
    let mut v = vec!["G21 G90".to_owned(), "M4 S500".to_owned()];
    for i in 0..600 {
        v.push(format!("G1 X{:.2} Y{:.2} F600", 10.0 + (i % 100) as f64, 10.0 + (i / 100) as f64));
    }
    v.push("M5".to_owned());
    Job::from_lines(v)
}

fn burn_lines(emu: &EmulatorHandle) -> usize {
    emu.with(|e| e.stats().lines.iter().filter(|l| l.starts_with("G1") || l.starts_with("M3") || l.starts_with("M4")).count())
}

/// After an alarm: nothing runs, the laser is off, the driver shows no job, and neither an
/// unlock nor a resume brings the burn back.
fn ended_for_good(m: &GrblMachine, emu: &EmulatorHandle, what: &str) {
    assert!(m.wait_until(T, |s| s.activity == Activity::Idle && s.alarm_locked), "{what}: {:?}", m.snapshot());
    let s = m.snapshot();
    assert!(!s.paused && !s.fire_on, "{what}: {s:?}");
    emu.with(|e| assert!(!e.laser_on(), "{what}: the laser is on"));
    emu.with(|e| e.release_limit_switch());
    // The driver may still be resetting the controller after the alarm.
    let end = Instant::now() + T;
    while m.unlock().is_err() {
        assert!(Instant::now() < end, "{what}: unlock never accepted");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(m.wait_until(T, |s| !s.alarm_locked && s.status.state == MachineState::Idle), "{what}: {:?}", m.snapshot().status);
    let before = burn_lines(emu);
    let pos = emu.with(|e| e.machine_position());
    m.resume().unwrap();
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(burn_lines(emu), before, "{what}: burn lines were sent after the alarm was cleared");
    emu.with(|e| {
        assert!(!e.laser_on(), "{what}: the laser came back on");
        assert_eq!(e.machine_position(), pos, "{what}: the machine moved again");
    });
    assert_eq!(m.snapshot().activity, Activity::Idle, "{what}");
}

// ----- an alarm in every phase -----

#[test]
fn a_limit_alarm_while_idle_blocks_everything_that_moves_or_fires() {
    let (m, emu) = rig();
    emu.with(|e| e.trigger_hard_limit('x'));
    until(&m, |e| matches!(e, Event::Alarm { .. }));
    assert!(m.wait_until(T, |s| s.alarm_locked && s.activity == Activity::Idle));
    assert_eq!(m.start_job(Job::from_text(BURN)), Err(GrblError::Alarm));
    assert_eq!(m.frame_rect(Rect::new(0.0, 0.0, 10.0, 10.0), FrameOptions::default()), Err(GrblError::Alarm));
    assert_eq!(m.fire(1.0, Duration::from_millis(200)), Err(GrblError::Alarm));
    assert_eq!(m.jog(JogStep { dx: 1.0, feed: 100.0, ..Default::default() }), Err(GrblError::Alarm));
    assert_eq!(m.go_to_origin(), Err(GrblError::Alarm));
    ended_for_good(&m, &emu, "idle");
    m.disconnect();
}

#[test]
fn a_limit_alarm_while_jogging_ends_the_jog() {
    let (m, emu) = rig();
    m.jog(JogStep { dx: 300.0, feed: 1500.0, ..Default::default() }).unwrap();
    wait_emu(&emu, "jogging", |e| e.machine_position()[0] > 2.0);
    emu.with(|e| e.trigger_hard_limit('x'));
    until(&m, |e| matches!(e, Event::Alarm { .. }));
    ended_for_good(&m, &emu, "jog");
    // No jog is left over: a job can be started.
    m.start_job(Job::from_text("G21 G90\nG0 X1")).unwrap();
    until(&m, |e| matches!(e, Event::JobFinished(_)));
    m.disconnect();
}

#[test]
fn a_limit_alarm_while_framing_ends_the_frame() {
    let (m, emu) = rig();
    let lit = FrameOptions { laser_percent: Some(1.0), feed: 1500.0, ..FrameOptions::default() };
    m.frame_rect(Rect::new(0.0, 0.0, 250.0, 250.0), lit).unwrap();
    wait_emu(&emu, "framing lit", |e| e.laser_on() && e.machine_position()[0] > 2.0);
    emu.with(|e| e.trigger_hard_limit('x'));
    until(&m, |e| matches!(e, Event::JobAborted { .. }));
    ended_for_good(&m, &emu, "frame");
    m.disconnect();
}

#[test]
fn a_limit_alarm_while_streaming_ends_the_job() {
    let (m, emu) = rig();
    m.start_job(long_job()).unwrap();
    wait_emu(&emu, "burning", |e| e.laser_on() && e.machine_position()[0] > 12.0);
    emu.with(|e| e.trigger_hard_limit('y'));
    until(&m, |e| matches!(e, Event::JobAborted { .. }));
    ended_for_good(&m, &emu, "job");
    m.disconnect();
}

#[test]
fn a_limit_alarm_while_paused_ends_the_job() {
    let (m, emu) = rig();
    m.start_job(Job::from_text(BURN)).unwrap();
    wait_emu(&emu, "burning", |e| e.laser_on() && e.machine_position()[0] > 5.0);
    m.pause().unwrap();
    assert!(m.wait_until(T, |s| s.status.state == MachineState::Hold(0) && s.paused), "{:?}", m.snapshot().status);
    emu.with(|e| e.trigger_hard_limit('x'));
    until(&m, |e| matches!(e, Event::JobAborted { .. }));
    ended_for_good(&m, &emu, "pause");
    m.disconnect();
}

#[test]
fn a_limit_alarm_while_stopping_still_ends_stopped() {
    let (m, emu) = rig();
    m.start_job(Job::from_text(BURN)).unwrap();
    wait_emu(&emu, "burning", |e| e.laser_on() && e.machine_position()[0] > 5.0);
    m.stop().unwrap();
    emu.with(|e| e.trigger_hard_limit('x'));
    until(&m, |e| matches!(e, Event::Stopped));
    ended_for_good(&m, &emu, "stop");
    m.disconnect();
}

#[test]
fn a_limit_alarm_under_the_fire_button_ends_the_fire() {
    let (m, emu) = rig();
    m.fire(1.0, Duration::from_secs(8)).unwrap();
    wait_emu(&emu, "firing", |e| e.laser_on());
    assert!(m.wait_until(T, |s| s.fire_on));
    emu.with(|e| e.trigger_hard_limit('x'));
    until(&m, |e| matches!(e, Event::Alarm { .. }));
    assert!(m.wait_until(T, |s| !s.fire_on), "the fire flag outlived the alarm");
    ended_for_good(&m, &emu, "fire");
    m.disconnect();
}

/// The controller's `ALARM:` line is lost on the wire; only its status reports say Alarm.
#[test]
fn an_alarm_seen_only_in_the_status_report_ends_a_streaming_job() {
    let (m, emu) = rig();
    m.start_job(long_job()).unwrap();
    wait_emu(&emu, "burning", |e| e.laser_on() && e.machine_position()[0] > 12.0);
    emu.with(|e| {
        e.trigger_hard_limit('x');
        let _ = e.take_output();
    });
    let end = Instant::now() + Duration::from_secs(4);
    while m.snapshot().activity != Activity::Idle && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(10));
    }
    let s = m.snapshot();
    assert!(s.alarm_locked, "{:?}", s.status);
    assert_eq!(s.activity, Activity::Idle, "the job still shows as running on a machine in alarm");
    emu.with(|e| assert!(!e.laser_on()));
    m.disconnect();
}

#[test]
fn an_alarm_seen_only_in_the_status_report_ends_a_paused_job_and_the_fire_button() {
    let (m, emu) = rig();
    m.start_job(Job::from_text(BURN)).unwrap();
    wait_emu(&emu, "burning", |e| e.laser_on() && e.machine_position()[0] > 5.0);
    m.pause().unwrap();
    assert!(m.wait_until(T, |s| s.status.state == MachineState::Hold(0) && s.paused), "{:?}", m.snapshot().status);
    emu.with(|e| {
        e.trigger_hard_limit('x');
        let _ = e.take_output();
    });
    assert!(m.wait_until(Duration::from_secs(4), |s| s.alarm_locked));
    std::thread::sleep(Duration::from_millis(300));
    let s = m.snapshot();
    assert_eq!(s.activity, Activity::Idle, "the paused job still shows on a machine in alarm");
    assert!(!s.paused);
    m.disconnect();

    let (m, emu) = rig();
    m.fire(1.0, Duration::from_secs(8)).unwrap();
    wait_emu(&emu, "firing", |e| e.laser_on());
    emu.with(|e| {
        e.trigger_hard_limit('x');
        let _ = e.take_output();
    });
    assert!(m.wait_until(Duration::from_secs(4), |s| s.alarm_locked));
    assert!(m.wait_until(Duration::from_secs(1), |s| !s.fire_on), "the fire flag stays on a machine in alarm");
    m.disconnect();
}

// ----- the door -----

fn door(emu: &EmulatorHandle, open: bool) {
    emu.with(|e| e.set_door_switch(open));
}

/// For a third of a second: the laser is off and the head stands still.
fn nothing_runs(emu: &EmulatorHandle, what: &str) {
    let pos = emu.with(|e| e.machine_position());
    std::thread::sleep(Duration::from_millis(300));
    emu.with(|e| {
        assert!(!e.laser_on(), "{what}: the laser is on");
        assert_eq!(e.machine_position(), pos, "{what}: the machine moves");
    });
}

fn refused_by_the_door(r: Result<(), GrblError>, what: &str) {
    assert!(matches!(r, Err(GrblError::NotIdle(MachineState::Door(_)))), "{what}: {r:?}");
}

fn everything_is_refused(m: &GrblMachine, what: &str) {
    refused_by_the_door(m.start_job(Job::from_text(BURN)), &format!("{what}: job"));
    refused_by_the_door(m.frame_rect(Rect::new(0.0, 0.0, 10.0, 10.0), FrameOptions::default()), &format!("{what}: frame"));
    refused_by_the_door(m.fire(1.0, Duration::from_millis(200)), &format!("{what}: fire"));
    refused_by_the_door(m.jog(JogStep { dx: 1.0, feed: 100.0, ..Default::default() }), &format!("{what}: jog"));
    refused_by_the_door(m.go_to_origin(), &format!("{what}: go to origin"));
}

#[test]
fn an_open_door_while_idle_refuses_everything_that_moves_or_fires() {
    let (m, emu) = rig();
    door(&emu, true);
    assert!(m.wait_until(T, |s| s.status.state == MachineState::Door(1)), "{:?}", m.snapshot().status);
    everything_is_refused(&m, "door open");
    // Closed, but the controller still waits to be told to go on.
    door(&emu, false);
    assert!(m.wait_until(T, |s| s.status.state == MachineState::Door(0)), "{:?}", m.snapshot().status);
    everything_is_refused(&m, "door closed, not resumed");
    assert_eq!(m.snapshot().laser_status(), LaserStatus::Paused, "the panel must offer Resume");
    m.resume().unwrap();
    assert!(m.wait_until(T, |s| s.status.state == MachineState::Idle), "{:?}", m.snapshot().status);
    m.start_job(Job::from_text("G21 G90\nG0 X1")).unwrap();
    until(&m, |e| matches!(e, Event::JobFinished(_)));
    m.disconnect();
}

#[test]
fn a_door_opened_in_a_job_a_pause_or_a_frame_holds_it_until_closed_and_resumed() {
    for case in ["job", "paused job", "frame"] {
        let (m, emu) = rig();
        if case == "frame" {
            let lit = FrameOptions { laser_percent: Some(1.0), feed: 1500.0, ..FrameOptions::default() };
            m.frame_rect(Rect::new(0.0, 0.0, 250.0, 250.0), lit).unwrap();
        } else {
            m.start_job(Job::from_text(BURN)).unwrap();
        }
        wait_emu(&emu, "burning", |e| e.laser_on() && e.machine_position()[0] > 5.0);
        if case == "paused job" {
            m.pause().unwrap();
            assert!(m.wait_until(T, |s| s.status.state == MachineState::Hold(0) && s.paused), "{case}: {:?}", m.snapshot().status);
        }
        door(&emu, true);
        wait_emu(&emu, "door", |e| e.state_name() == "Door:1");
        assert!(m.wait_until(T, |s| s.status.state == MachineState::Door(1)), "{case}: {:?}", m.snapshot().status);
        nothing_runs(&emu, case);
        // Held, not running: the panel offers Resume, not Pause.
        assert_ne!(m.snapshot().activity, Activity::Idle, "{case}");
        assert_eq!(m.snapshot().laser_status(), LaserStatus::Paused, "{case}: held by the door");
        // Resume with the door open does nothing, and the job still shows as held.
        m.resume().unwrap();
        nothing_runs(&emu, &format!("{case}: resume with the door open"));
        assert_eq!(m.snapshot().laser_status(), LaserStatus::Paused, "{case}: after a resume with the door open");
        // Closing the door alone does not go on.
        door(&emu, false);
        assert!(m.wait_until(T, |s| s.status.state == MachineState::Door(0)), "{case}: {:?}", m.snapshot().status);
        nothing_runs(&emu, &format!("{case}: door closed, not resumed"));
        assert_eq!(m.snapshot().laser_status(), LaserStatus::Paused, "{case}: door closed, not resumed");
        m.resume().unwrap();
        until(&m, |e| matches!(e, Event::JobFinished(_)));
        assert!(m.wait_until(T, |s| s.activity == Activity::Idle && s.status.state == MachineState::Idle), "{case}: {:?}", m.snapshot().status);
        emu.with(|e| assert!(!e.laser_on(), "{case}: the laser is on after the job"));
        m.disconnect();
    }
}

#[test]
fn a_paused_frame_shows_as_paused() {
    let (m, emu) = rig();
    m.frame_rect(Rect::new(0.0, 0.0, 250.0, 250.0), FrameOptions { feed: 1500.0, ..FrameOptions::default() }).unwrap();
    wait_emu(&emu, "framing", |e| e.machine_position()[0] > 5.0);
    m.pause().unwrap();
    assert!(m.wait_until(T, |s| s.status.state == MachineState::Hold(0)), "{:?}", m.snapshot().status);
    // The panel's button is Pause or Resume by this status: a frame that only ever shows as
    // framing can be paused and never resumed.
    assert_eq!(m.snapshot().laser_status(), LaserStatus::Paused);
    m.resume().unwrap();
    assert!(m.wait_until(T, |s| matches!(s.status.state, MachineState::Run(_))), "{:?}", m.snapshot().status);
    assert_eq!(m.snapshot().laser_status(), LaserStatus::Framing);
    until(&m, |e| matches!(e, Event::JobFinished(_)));
    m.disconnect();
}

#[test]
fn a_stop_with_the_door_open_ends_the_job_for_good() {
    let (m, emu) = rig();
    m.start_job(Job::from_text(BURN)).unwrap();
    wait_emu(&emu, "burning", |e| e.laser_on() && e.machine_position()[0] > 5.0);
    door(&emu, true);
    wait_emu(&emu, "door", |e| e.state_name() == "Door:1");
    m.stop().unwrap();
    until(&m, |e| matches!(e, Event::Stopped));
    assert!(m.wait_until(T, |s| s.activity == Activity::Idle && !s.paused), "{:?}", m.snapshot());
    // The door is still open: the controller is back in its door state after the reset.
    assert!(m.wait_until(T, |s| s.status.state == MachineState::Door(1)), "{:?}", m.snapshot().status);
    everything_is_refused(&m, "stopped, door open");
    door(&emu, false);
    let before = burn_lines(&emu);
    m.resume().unwrap();
    nothing_runs(&emu, "resume after the stop");
    assert_eq!(burn_lines(&emu), before, "burn lines were sent after the stop");
    assert!(m.wait_until(T, |s| s.status.state == MachineState::Idle), "{:?}", m.snapshot().status);
    m.start_job(Job::from_text("G21 G90\nG0 X1")).unwrap();
    until(&m, |e| matches!(e, Event::JobFinished(_)));
    m.disconnect();
}

#[test]
fn a_door_opened_under_the_fire_button_ends_the_fire() {
    let (m, emu) = rig();
    m.fire(1.0, Duration::from_secs(8)).unwrap();
    wait_emu(&emu, "firing", |e| e.laser_on());
    door(&emu, true);
    emu.with(|e| assert!(!e.laser_on()));
    assert!(m.wait_until(Duration::from_secs(2), |s| !s.fire_on), "the fire flag stays on behind an open door");
    door(&emu, false);
    assert!(m.wait_until(T, |s| s.status.state == MachineState::Door(0)), "{:?}", m.snapshot().status);
    m.resume().unwrap();
    assert!(m.wait_until(T, |s| s.status.state == MachineState::Idle), "{:?}", m.snapshot().status);
    nothing_runs(&emu, "the old fire press, after the door is closed and Resume pressed");
    m.disconnect();
}

/// The door opens between the look at the status and the command: the driver's last report
/// still says Idle.
#[test]
fn a_fire_press_as_the_door_opens_lights_nothing_once_it_is_closed() {
    let (m, emu) = rig();
    door(&emu, true);
    let _ = m.fire(1.0, Duration::from_secs(8));
    assert!(m.wait_until(T, |s| s.status.state == MachineState::Door(1)), "{:?}", m.snapshot().status);
    assert!(m.wait_until(Duration::from_secs(2), |s| !s.fire_on), "the fire flag is on behind an open door");
    nothing_runs(&emu, "door open");
    door(&emu, false);
    assert!(m.wait_until(T, |s| s.status.state == MachineState::Door(0)), "{:?}", m.snapshot().status);
    m.resume().unwrap();
    assert!(m.wait_until(T, |s| s.status.state == MachineState::Idle), "{:?}", m.snapshot().status);
    nothing_runs(&emu, "door closed and resumed");
    m.disconnect();
}

/// A job started in the same moment burns nothing behind the open door, and can be stopped.
#[test]
fn a_start_as_the_door_opens_burns_nothing_behind_it() {
    let (m, emu) = rig();
    door(&emu, true);
    let started = m.start_job(Job::from_text(BURN)).is_ok();
    assert!(m.wait_until(T, |s| s.status.state == MachineState::Door(1)), "{:?}", m.snapshot().status);
    nothing_runs(&emu, "door open");
    if started {
        assert_eq!(m.snapshot().laser_status(), LaserStatus::Paused, "a job behind an open door shows as held");
        m.stop().unwrap();
        until(&m, |e| matches!(e, Event::Stopped));
    }
    assert!(m.wait_until(T, |s| s.activity == Activity::Idle), "{:?}", m.snapshot());
    door(&emu, false);
    m.resume().unwrap();
    nothing_runs(&emu, "door closed and resumed");
    m.disconnect();
}

// ----- the fire button behind a line that waits -----

/// Home, then Fire before the status shows the homing: the fire line waits behind `$H`. Its
/// switch-off must not overtake it, or the laser comes on after homing with nothing to end it.
#[test]
fn a_fire_press_caught_behind_homing_never_lights_after_its_release() {
    for timed_out in [false, true] {
        // A machine with homing switched on starts locked: home it once first.
        let emu = EmulatorHandle::new(GrblEmulator::new(EmulatorConfig::atomstack().with_setting(22, 1.0).with_time_scale(5.0)));
        let m = GrblMachine::connect(Box::new(emu.transport()), ConnectOptions::default());
        until(&m, |e| matches!(e, Event::Connected(_)));
        m.home().unwrap();
        until(&m, |e| matches!(e, Event::Homed));
        assert!(m.wait_until(T, |s| s.status.state == MachineState::Idle && !s.alarm_locked), "{:?}", m.snapshot().status);
        m.home().unwrap();
        let _ = m.fire(1.0, Duration::from_millis(if timed_out { 100 } else { 8000 }));
        if !timed_out {
            m.fire_off().unwrap();
        }
        until(&m, |e| matches!(e, Event::Homed));
        assert!(m.wait_until(T, |s| s.status.state == MachineState::Idle && !s.fire_on), "{:?}", m.snapshot().status);
        std::thread::sleep(Duration::from_millis(400));
        emu.with(|e| assert!(!e.laser_on(), "timed out {timed_out}: the laser came on after homing and nothing switches it off"));
        m.disconnect();
    }
}

// ----- stop and disconnect right after start -----

#[test]
fn a_stop_right_after_start_leaves_nothing_burning() {
    let (m, emu) = rig();
    m.start_job(Job::from_text(BURN)).unwrap();
    m.stop().unwrap();
    until(&m, |e| matches!(e, Event::Stopped));
    assert!(m.wait_until(T, |s| s.activity == Activity::Idle));
    std::thread::sleep(Duration::from_millis(300));
    let pos = emu.with(|e| {
        assert!(!e.laser_on());
        assert!(!matches!(e.state_name().as_str(), "Run" | "Jog"), "{}", e.state_name());
        e.machine_position()
    });
    // Nothing of the job is left to run on.
    m.resume().unwrap();
    std::thread::sleep(Duration::from_millis(300));
    emu.with(|e| {
        assert!(!e.laser_on());
        assert_eq!(e.machine_position(), pos);
    });
    m.disconnect();
}

#[test]
fn a_disconnect_right_after_start_leaves_nothing_burning() {
    for frame in [false, true] {
        let (m, emu) = rig();
        if frame {
            let lit = FrameOptions { laser_percent: Some(1.0), ..FrameOptions::default() };
            m.frame_rect(Rect::new(0.0, 0.0, 50.0, 50.0), lit).unwrap();
        } else {
            m.start_job(Job::from_text(BURN)).unwrap();
        }
        m.disconnect();
        std::thread::sleep(Duration::from_millis(300));
        emu.with(|e| {
            assert!(!e.laser_on(), "frame {frame}");
            assert!(e.state_name() != "Run", "frame {frame}: still running after the link closed");
        });
    }
}

#[test]
fn a_disconnect_in_the_middle_of_a_job_a_jog_or_a_fire_stops_it_first() {
    // job
    let (m, emu) = rig();
    m.start_job(Job::from_text(BURN)).unwrap();
    wait_emu(&emu, "burning", |e| e.laser_on() && e.machine_position()[0] > 5.0);
    m.disconnect();
    emu.with(|e| {
        assert!(!e.laser_on());
        assert!(!matches!(e.state_name().as_str(), "Run" | "Jog"), "{}", e.state_name());
    });
    // jog
    let (m, emu) = rig();
    m.jog(JogStep { dx: 300.0, feed: 1500.0, ..Default::default() }).unwrap();
    wait_emu(&emu, "jogging", |e| e.machine_position()[0] > 2.0);
    m.disconnect();
    emu.with(|e| assert!(!matches!(e.state_name().as_str(), "Run" | "Jog"), "the jog ran on after the link closed: {}", e.state_name()));
    // fire
    let (m, emu) = rig();
    m.fire(1.0, Duration::from_secs(8)).unwrap();
    wait_emu(&emu, "firing", |e| e.laser_on());
    m.disconnect();
    emu.with(|e| assert!(!e.laser_on(), "the laser stayed on after the link closed"));
}

// ----- commands while a job is on its way -----

#[test]
fn nothing_else_is_sent_once_a_job_is_asked_for() {
    let (m, emu) = rig();
    m.start_job(Job::from_text(BURN)).unwrap();
    // The snapshot may not show the job yet.
    let _ = m.jog(JogStep { dx: 5.0, feed: 1000.0, ..Default::default() });
    let _ = m.fire(1.0, Duration::from_secs(1));
    let _ = m.home();
    let _ = m.go_to_origin();
    let _ = m.start_job(Job::from_text("G21 G90\nM3 S999\nG1 X7 F100\nM5"));
    let _ = m.send_console("G0 X123");
    let _ = m.run_macro("G0 X124");
    wait_emu(&emu, "burning", |e| e.laser_on() && e.machine_position()[0] > 5.0);
    emu.with(|e| {
        let l = &e.stats().lines;
        assert!(
            !l.iter().any(|l| l.starts_with("$J") || l == "$H" || l.contains("S999") || l.contains("X123") || l.contains("X124") || l.contains("M3")),
            "{l:?}"
        );
        assert_eq!(e.stats().errors, 0, "{l:?}");
    });
    m.stop().unwrap();
    until(&m, |e| matches!(e, Event::Stopped));
    m.disconnect();
}

// ----- the fire button against starts and moves -----

#[test]
fn a_job_a_frame_and_every_move_switch_the_fire_button_off_first() {
    type Act = fn(&GrblMachine);
    let acts: [(&str, Act, &str); 6] = [
        ("job", |m| m.start_job(Job::from_text("G21 G90\nG0 X20")).unwrap(), "G0 X20"),
        ("frame", |m| m.frame_rect(Rect::new(0.0, 0.0, 20.0, 20.0), FrameOptions::default()).unwrap(), "G0 X0 Y0"),
        ("jog", |m| m.jog(JogStep { dx: 20.0, feed: 3000.0, ..Default::default() }).unwrap(), "$J="),
        ("move", |m| m.move_to(MoveTarget { x: Some(20.0), y: None, z: None, coords: Coords::Work, feed: 3000.0 }).unwrap(), "$J="),
        ("origin", |m| m.go_to_origin().unwrap(), "G21 G90 G0 X0 Y0"),
        ("home", |m| m.home().unwrap(), "$H"),
    ];
    for (name, act, first) in acts {
        let (m, emu) = rig();
        m.fire(1.0, Duration::from_secs(8)).unwrap();
        wait_emu(&emu, "firing", |e| e.laser_on());
        assert!(m.wait_until(T, |s| s.fire_on && s.status.state == MachineState::Idle), "{name}: {:?}", m.snapshot().status);
        act(&m);
        wait_emu(&emu, name, |e| e.stats().lines.iter().any(|l| l.starts_with(first)));
        emu.with(|e| {
            let l = &e.stats().lines;
            let go = l.iter().position(|l| l.starts_with(first)).unwrap();
            let on = l.iter().position(|l| l.contains("M3")).unwrap();
            let off = l.iter().skip(on).position(|l| l.starts_with("M5")).map(|i| i + on);
            assert!(off.is_some_and(|i| i < go), "{name}: the laser stayed on: {l:?}");
        });
        assert!(m.wait_until(T, |s| !s.fire_on), "{name}");
        m.disconnect();
    }
}

// ----- a dropped link -----

#[test]
fn a_dropped_link_clears_the_job_the_pause_and_the_fire_flag() {
    let (m, emu) = rig();
    m.fire(1.0, Duration::from_secs(8)).unwrap();
    assert!(m.wait_until(T, |s| s.fire_on));
    emu.with(|e| e.unplug());
    assert!(m.wait_until(T, |s| s.connection != ConnectionState::Connected), "{:?}", m.snapshot().connection);
    let s = m.snapshot();
    assert!(!s.fire_on && !s.paused && s.activity == Activity::Idle, "{s:?}");
    assert!(m.fire(1.0, Duration::from_secs(1)).is_err());
    assert!(m.start_job(Job::from_text(BURN)).is_err());

    let (m, emu) = rig();
    m.start_job(Job::from_text(BURN)).unwrap();
    wait_emu(&emu, "burning", |e| e.laser_on() && e.machine_position()[0] > 5.0);
    m.pause().unwrap();
    assert!(m.wait_until(T, |s| s.paused));
    emu.with(|e| e.unplug());
    assert!(m.wait_until(T, |s| s.connection != ConnectionState::Connected));
    let s = m.snapshot();
    assert!(!s.fire_on && !s.paused && s.activity == Activity::Idle, "{s:?}");
    assert!(m.resume().is_err());
}

// ----- a controller that restarts -----

#[test]
fn a_controller_that_restarts_ends_the_job_the_pause_and_the_fire_button() {
    // streaming
    let (m, emu) = rig();
    m.start_job(long_job()).unwrap();
    wait_emu(&emu, "burning", |e| e.laser_on() && e.machine_position()[0] > 12.0);
    emu.with(|e| e.receive(&[0x18]));
    until(&m, |e| matches!(e, Event::JobAborted { .. }));
    assert!(m.wait_until(T, |s| s.activity == Activity::Idle && !s.paused));
    let before = burn_lines(&emu);
    let _ = m.unlock();
    m.resume().unwrap();
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(burn_lines(&emu), before, "the job went on after the controller restarted");
    emu.with(|e| assert!(!e.laser_on()));
    m.disconnect();
    // fire held
    let (m, emu) = rig();
    m.fire(1.0, Duration::from_secs(8)).unwrap();
    wait_emu(&emu, "firing", |e| e.laser_on());
    emu.with(|e| e.receive(&[0x18]));
    assert!(m.wait_until(T, |s| !s.fire_on), "the fire flag outlived the restart");
    emu.with(|e| assert!(!e.laser_on()));
    m.disconnect();
}
