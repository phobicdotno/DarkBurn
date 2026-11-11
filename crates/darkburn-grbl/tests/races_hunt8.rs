//! Pass 8 of the bug hunt, races: a job or a frame that is started while a jog still runs.
//! In-memory emulator only.

use std::time::{Duration, Instant};

use darkburn_grbl::*;
use kurbo::Rect;

const T: Duration = Duration::from_secs(10);

fn rig() -> (GrblMachine, EmulatorHandle) {
    let emu = EmulatorHandle::new(GrblEmulator::new(EmulatorConfig::atomstack().with_time_scale(5.0)));
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

/// Events until the driver is done with what was asked: finished, aborted or refused.
fn outcome(m: &GrblMachine) -> Vec<Event> {
    let end = Instant::now() + T;
    let mut out = Vec::new();
    while Instant::now() < end {
        if let Some(e) = m.next_event(Duration::from_millis(20)) {
            let done = matches!(e, Event::JobFinished(_) | Event::JobAborted { .. } | Event::Rejected(_));
            out.push(e);
            if done {
                return out;
            }
        }
    }
    panic!("no outcome: {out:?}");
}

const JOB: &str = "G21 G90\nM4 S0\nG0 X10 Y10\nG1 X20 S500 F3000\nG1 Y20\nG1 X10\nG1 Y10\nM5";

#[test]
fn a_job_started_under_a_running_jog_does_not_lose_its_first_lines() {
    let (m, emu) = rig();
    // A long jog; the job is asked for before the next status report shows it.
    m.jog(JogStep { dx: 300.0, feed: 3000.0, ..Default::default() }).unwrap();
    let asked = m.start_job(Job::from_text(JOB));
    let events = if asked.is_ok() { outcome(&m) } else { Vec::new() };
    let refused = asked.is_err() || events.iter().any(|e| matches!(e, Event::Rejected(_)));
    let (errors, lines) = emu.with(|e| (e.stats().errors, e.stats().lines.clone()));
    // Either the start is refused and nothing of the job is sent, or the whole job runs: a
    // controller that jogs answers every job line with an error and drops it.
    assert_eq!(errors, 0, "the controller dropped job lines while it jogged: {lines:?} {events:?}");
    if refused {
        assert!(!lines.iter().any(|l| l.starts_with("G1") || l.starts_with("M4")), "{lines:?}");
    } else {
        assert!(events.iter().any(|e| matches!(e, Event::JobFinished(_))), "{events:?}");
    }
    // Once the jog is over the same job starts and runs to its end.
    assert!(m.wait_until(T, |s| s.status.state == MachineState::Idle && (s.status.mpos[0] - 300.0).abs() < 1e-6), "{:?}", m.snapshot().status);
    let end = Instant::now() + T;
    loop {
        m.start_job(Job::from_text(JOB)).unwrap();
        let events = outcome(&m);
        if events.iter().any(|e| matches!(e, Event::JobFinished(_))) {
            break;
        }
        assert!(Instant::now() < end, "the job never started after the jog: {events:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
    emu.with(|e| assert_eq!(e.stats().errors, 0, "{:?}", e.stats().lines));
    m.disconnect();
}

#[test]
fn a_frame_started_under_a_running_jog_does_not_lose_its_first_lines() {
    let (m, emu) = rig();
    m.jog(JogStep { dx: 300.0, feed: 3000.0, ..Default::default() }).unwrap();
    let asked = m.frame_rect(Rect::new(10.0, 10.0, 50.0, 40.0), FrameOptions::default());
    let events = if asked.is_ok() { outcome(&m) } else { Vec::new() };
    let (errors, lines) = emu.with(|e| (e.stats().errors, e.stats().lines.clone()));
    assert_eq!(errors, 0, "the controller dropped frame lines while it jogged: {lines:?} {events:?}");
    m.disconnect();
}
