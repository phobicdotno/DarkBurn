//! A lost or garbled `ok`: a noise byte in front of it is read through, and a lost one ends the
//! job (laser off, no reset) once the controller has plainly finished everything. In-memory
//! emulator only.

use std::time::{Duration, Instant};

use darkburn_grbl::*;

const T: Duration = Duration::from_secs(15);

fn rig(cfg: EmulatorConfig, stall: Duration) -> (GrblMachine, EmulatorHandle) {
    let emu = EmulatorHandle::new(GrblEmulator::new(cfg));
    let opts = ConnectOptions { stall_timeout: stall, ..Default::default() };
    let m = GrblMachine::connect(Box::new(emu.transport()), opts);
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

/// Short moves, no `M5` of its own: the job's tail sends one.
fn job() -> Job {
    let mut v = vec!["G21 G90".to_owned(), "M4 S300".to_owned()];
    for i in 0..20 {
        v.push(format!("G1 X{} Y{} F3000", 5 + i, 5 + i % 3));
    }
    Job::from_lines(v)
}

fn m5_lines(emu: &EmulatorHandle) -> usize {
    emu.with(|e| e.stats().lines.iter().filter(|l| l.starts_with("M5")).count())
}

fn with_bf() -> EmulatorConfig {
    // `$10=3`: reports carry `Bf:` (free planner blocks, free RX bytes).
    EmulatorConfig::atomstack().with_time_scale(20.0).with_setting(10, 3.0)
}

#[test]
fn a_noise_byte_in_front_of_an_ok_does_not_cost_the_answer() {
    let (m, emu) = rig(with_bf(), Duration::from_millis(800));
    emu.with(|e| {
        e.fault_ok(4, Some(&[0xFF]));
        e.fault_ok(9, Some(&[0x00, 0x80]));
        e.fault_ok(12, Some(&[0xC3, 0xA9, 0x07]));
    });
    m.start_job(job()).unwrap();
    let ev = until(&m, |e| matches!(e, Event::JobFinished(_) | Event::JobAborted { .. }));
    assert!(matches!(ev.last(), Some(Event::JobFinished(_))), "{ev:?}");
    assert_eq!(m.snapshot().rx_in_flight, 0);
}

#[test]
fn a_lost_ok_on_an_idle_controller_ends_the_job_with_the_laser_off_and_no_reset() {
    let (m, emu) = rig(with_bf(), Duration::from_millis(800));
    let resets = emu.with(|e| e.stats().soft_resets);
    emu.with(|e| e.fault_ok(6, None));
    m.start_job(job()).unwrap();
    let ev = until(&m, |e| matches!(e, Event::JobFinished(_) | Event::JobAborted { .. }));
    match ev.last() {
        Some(Event::JobAborted { reason, .. }) => assert!(reason.contains("stopped answering"), "{reason}"),
        other => panic!("the job did not end as failed: {other:?}"),
    }
    // The tail's own `M5 S0`, then the laser-off line of the lost answer.
    let end = Instant::now() + T;
    while m5_lines(&emu) < 2 {
        assert!(Instant::now() < end, "no laser-off line after the lost answer");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(m.wait_until(T, |s| s.activity == Activity::Idle && s.rx_in_flight == 0), "{:?}", m.snapshot());
    std::thread::sleep(Duration::from_millis(300));
    emu.with(|e| {
        assert_eq!(e.stats().soft_resets, resets, "the controller was reset");
        assert!(!e.stats().realtime.contains(&0x18), "a soft reset byte went out");
        assert!(!e.laser_on());
    });
    // The driver takes new work again.
    m.start_job(Job::from_text("G0 X1 Y1")).unwrap();
    let ev = until(&m, |e| matches!(e, Event::JobFinished(_) | Event::JobAborted { .. }));
    assert!(matches!(ev.last(), Some(Event::JobFinished(_))), "{ev:?}");
}

#[test]
fn a_lost_ok_without_buffer_reports_is_not_guessed_at() {
    // `$10=1`: no `Bf:`, so whether the controller is empty is not known.
    let (m, emu) = rig(EmulatorConfig::atomstack().with_time_scale(20.0), Duration::from_millis(300));
    emu.with(|e| e.fault_ok(6, None));
    m.start_job(job()).unwrap();
    std::thread::sleep(Duration::from_millis(2000));
    assert_eq!(m.snapshot().activity, Activity::Job, "{:?}", m.snapshot());
    assert_eq!(m5_lines(&emu), 1, "only the job's own laser-off line");
    // Stop still ends it.
    m.stop().unwrap();
    until(&m, |e| matches!(e, Event::Stopped));
}

#[test]
fn a_slow_controller_that_still_runs_never_trips() {
    // Real time: one move of 3 s and a dwell of 1.5 s, against a limit of 0.5 s.
    let (m, emu) = rig(EmulatorConfig::atomstack().with_setting(10, 3.0), Duration::from_millis(500));
    m.start_job(Job::from_lines(["G21 G90", "G1 X50 F1000", "G4 P1.5", "G0 X0"].map(String::from))).unwrap();
    let ev = until(&m, |e| matches!(e, Event::JobFinished(_) | Event::JobAborted { .. }));
    assert!(matches!(ev.last(), Some(Event::JobFinished(_))), "{ev:?}");
    assert!(emu.with(|e| !e.stats().realtime.contains(&0x18)));
}
