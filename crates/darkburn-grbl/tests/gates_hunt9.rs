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
