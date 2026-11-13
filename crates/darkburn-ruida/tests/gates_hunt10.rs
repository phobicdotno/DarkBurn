//! Bug hunt 10, the gate matrix of bug hunt 9 for the Ruida driver: stop, disconnect, a second
//! start, a frame or a jog asked in every phase (idle, framing, jogging, firing, running,
//! paused). RuidaMachine against RuidaEmulator over MemoryTransport. No network, no USB.

mod common;

use common::*;
use darkburn_ruida::emulator::RunState;
use darkburn_ruida::*;
use kurbo::{Point, Rect};

/// A connected machine whose emulator runs a job one command at a time (`advance`).
fn rig() -> (RuidaMachine<MemoryTransport>, Emu) {
    udp_machine_with(EmulatorConfig { step_mode: true, ..Default::default() }, MachineConfig::default())
}

fn key_is_down(emu: &Emu, key: PanelKey) -> bool {
    let e = lock(emu);
    let last = e.panel_received.iter().rev().find(|c| matches!(c, PanelCommand::KeyDown(k) | PanelCommand::KeyUp(k) if *k == key));
    matches!(last, Some(PanelCommand::KeyDown(_)))
}

/// Start a stored job and run a little of it.
fn running(m: &mut RuidaMachine<MemoryTransport>, emu: &Emu) {
    let opts = SendOptions { start_immediately: true, ..Default::default() };
    m.send_job("GATE", &square_job((10.0, 10.0), 50.0, 20.0), &opts, &mut never).unwrap();
    lock(emu).advance(6);
    assert_eq!(lock(emu).state(), RunState::Running);
    assert_eq!(m.status().unwrap().state, MachineState::Running);
}

/// Everything that moves the head or starts a job is refused, and the job is left as it was.
fn nothing_starts_on_top(m: &mut RuidaMachine<MemoryTransport>, emu: &Emu, phase: RunState, what: &str) {
    let (pos, progress, moves) = {
        let e = lock(emu);
        (e.position(), e.job_progress(), e.motion.len())
    };
    assert!(m.frame_rect(Rect::new(0.0, 0.0, 20.0, 20.0), false).is_err(), "{what}: a frame was let through");
    assert!(m.frame_outline(&[Point::new(1.0, 1.0), Point::new(2.0, 2.0)], true).is_err(), "{what}: an outline frame was let through");
    assert!(m.move_to(Point::new(100.0, 100.0), false).is_err(), "{what}: a move was let through");
    assert!(m.go_to_origin().is_err(), "{what}: go-to-origin was let through");
    assert!(m.jog_step(Axis::X, 5.0).is_err(), "{what}: a jog step was let through");
    assert!(m.run_file(1).is_err(), "{what}: a second job was started on top");
    assert!(m.run_job_now(&square_job((1.0, 1.0), 5.0, 20.0)).is_err(), "{what}: a streamed job was started on top");
    let e = lock(emu);
    assert_eq!(e.position(), pos, "{what}: the head moved");
    assert_eq!(e.motion.len(), moves, "{what}: a move was made");
    assert_eq!(e.job_progress(), progress, "{what}: the job was replaced");
    assert_eq!(e.state(), phase, "{what}");
}

// ----- the job running and paused -----

#[test]
fn nothing_moves_or_starts_on_top_of_a_running_job() {
    let (mut m, emu) = rig();
    running(&mut m, &emu);
    nothing_starts_on_top(&mut m, &emu, RunState::Running, "running");
    // A second Start does not restart the job either.
    let progress = lock(&emu).job_progress();
    m.start().unwrap();
    assert_eq!(lock(&emu).job_progress(), progress);
}

#[test]
fn nothing_moves_or_starts_on_top_of_a_paused_job() {
    let (mut m, emu) = rig();
    running(&mut m, &emu);
    m.pause().unwrap();
    assert_eq!(m.status().unwrap().state, MachineState::Paused);
    nothing_starts_on_top(&mut m, &emu, RunState::Paused, "paused");
}

#[test]
fn a_stopped_job_does_not_show_as_running() {
    for paused in [false, true] {
        let (mut m, emu) = rig();
        running(&mut m, &emu);
        if paused {
            m.pause().unwrap();
        }
        m.stop().unwrap();
        assert_eq!(lock(&emu).state(), RunState::Idle);
        assert_eq!(m.status().unwrap().state, MachineState::Idle);
        // Resume and a further Stop do not bring the burn back.
        let cuts = lock(&emu).motion.len();
        m.resume().unwrap();
        lock(&emu).advance(10);
        assert_eq!(lock(&emu).motion.len(), cuts, "paused {paused}: the job came back after a stop");
        // And the head can be framed again.
        m.frame_rect(Rect::new(0.0, 0.0, 10.0, 10.0), false).unwrap();
    }
}

#[test]
fn after_the_job_ends_the_head_can_be_framed_and_jogged_again() {
    let (mut m, emu) = rig();
    running(&mut m, &emu);
    lock(&emu).run_to_end();
    assert_eq!(m.status().unwrap().state, MachineState::Idle);
    m.frame_rect(Rect::new(0.0, 0.0, 10.0, 10.0), false).unwrap();
    m.jog_step(Axis::X, 1.0).unwrap();
    m.go_to_origin().unwrap();
}

// ----- idle -----

#[test]
fn stop_and_a_second_start_while_idle_start_nothing() {
    let (mut m, emu) = rig();
    m.stop().unwrap();
    m.start().unwrap();
    assert_eq!(lock(&emu).state(), RunState::Idle);
    assert!(lock(&emu).motion.is_empty());
    m.frame_rect(Rect::new(0.0, 0.0, 10.0, 10.0), false).unwrap();
}

// ----- firing: the Pulse key held -----

#[test]
fn stop_lets_the_pulse_key_go() {
    let (mut m, emu) = rig();
    m.panel_key(PanelKey::Pulse, true).unwrap();
    assert!(key_is_down(&emu, PanelKey::Pulse));
    m.stop().unwrap();
    assert!(!key_is_down(&emu, PanelKey::Pulse), "Stop left the Pulse key held down");
}

#[test]
fn disconnect_lets_the_pulse_key_go() {
    let (mut m, emu) = rig();
    m.panel_key(PanelKey::Pulse, true).unwrap();
    m.disconnect();
    assert!(!key_is_down(&emu, PanelKey::Pulse), "a disconnect left the Pulse key held down");
    assert!(!m.is_connected());
}

#[test]
fn starting_a_job_lets_the_pulse_key_go() {
    let (mut m, emu) = rig();
    m.panel_key(PanelKey::Pulse, true).unwrap();
    m.start().unwrap();
    assert!(!key_is_down(&emu, PanelKey::Pulse), "Start left the Pulse key held down");
}

// ----- jogging: a jog key held -----

#[test]
fn stop_and_disconnect_let_a_held_jog_key_go() {
    for disconnect in [false, true] {
        let (mut m, emu) = rig();
        m.panel_key(PanelKey::PlusX, true).unwrap();
        if disconnect {
            m.disconnect();
        } else {
            m.stop().unwrap();
        }
        assert!(!key_is_down(&emu, PanelKey::PlusX), "disconnect {disconnect}: the jog key stays held");
    }
}
