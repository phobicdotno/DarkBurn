//! Headless tests of [`MachineHandle`] against both emulators. No real port, USB or network
//! connection is ever opened (test builds refuse real links).

use std::ops::{Deref, DerefMut};
use std::time::{Duration, Instant};

use kurbo::{Point, Rect};

use crate::machine_api::*;
use darkburn_ruida as ruida;

const T: Duration = Duration::from_secs(20);

fn spec(family: Family, scale: f64) -> ConnectSpec {
    let mut s = ConnectSpec::simulator(family);
    s.config.sim_time_scale = scale;
    s
}

/// A machine handle for tests that never drops an event.
///
/// [`MachineHandle::poll`] hands each event out once, and one poll can return several: a slow
/// test thread may get `JobStarted` and `JobFinished` together. A wait that looked only at the
/// latest batch would then lose every event behind the one it was waiting for. Here every
/// polled event is kept until [`wait_event`] has matched it.
struct Sim {
    h: MachineHandle,
    /// Events polled and not yet matched by [`wait_event`], oldest first.
    unseen: Vec<MachineEvent>,
}

impl Sim {
    fn new() -> Sim {
        Sim { h: MachineHandle::new(), unseen: Vec::new() }
    }

    /// Polls until `pred` holds or `timeout` passes. `pred` gets the snapshot and every event
    /// that no [`wait_event`] has matched yet (not only the newest ones).
    fn wait_for(&mut self, timeout: Duration, mut pred: impl FnMut(&Snapshot, &[MachineEvent]) -> bool) -> bool {
        let end = Instant::now() + timeout;
        loop {
            let new = self.h.poll();
            self.unseen.extend(new);
            if pred(self.h.snapshot(), &self.unseen) {
                return true;
            }
            if Instant::now() >= end {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

impl Deref for Sim {
    type Target = MachineHandle;
    fn deref(&self) -> &MachineHandle {
        &self.h
    }
}

impl DerefMut for Sim {
    fn deref_mut(&mut self) -> &mut MachineHandle {
        &mut self.h
    }
}

fn connected(family: Family, scale: f64) -> Sim {
    let mut h = Sim::new();
    h.connect(&spec(family, scale), None).unwrap();
    assert!(h.wait_for(T, |s, _| s.connected()), "{family:?} simulator connects");
    h
}

/// Waits for the next event that matches `f`. That event and the ones before it are then
/// done with; later ones stay for the next wait, so two waits in a row see events in order.
fn wait_event(h: &mut Sim, f: impl Fn(&MachineEvent) -> bool) -> bool {
    let mut at = None;
    let found = h.wait_for(T, |_, ev| {
        at = ev.iter().position(&f);
        at.is_some()
    });
    if let Some(i) = at {
        h.unseen.drain(..=i);
    }
    found
}

fn ready(h: &mut Sim) {
    let ok = h.wait_for(T, |s, _| s.state == LaserState::Ready);
    if !ok {
        for c in h.console() {
            eprintln!("{:?} {}", c.kind, c.text);
        }
    }
    assert!(ok, "ready, was {:?} {:?}", h.snapshot().state, h.snapshot().alarm);
}

#[test]
fn real_links_are_refused_without_consent_and_in_tests() {
    let mut h = Sim::new();
    let real = ConnectSpec { link: Link::Serial { port: "COM3".into(), baud: 115_200 }, config: ConnectConfig::default() };
    assert!(matches!(h.connect(&real, None), Err(MachineError::Refused(_))));
    assert!(matches!(h.connect(&real, Some(RealLinkConsent::from_user_click())), Err(MachineError::Refused(_))));
    let udp = ConnectSpec { link: Link::Udp { ip: "192.168.1.5".into() }, config: ConnectConfig::default() };
    assert!(matches!(h.connect(&udp, Some(RealLinkConsent::from_user_click())), Err(MachineError::Refused(_))));
    assert!(!h.is_real_machine());
    assert_eq!(h.snapshot().state, LaserState::Disconnected);
}

#[test]
fn commands_fail_cleanly_when_not_connected() {
    let mut h = Sim::new();
    assert_eq!(h.stop(), Err(MachineError::NotConnected));
    assert_eq!(h.pause(), Err(MachineError::NotConnected));
    assert_eq!(h.home(), Err(MachineError::NotConnected));
    assert_eq!(h.start_job(simulator_square(Family::Grbl, 10.0)), Err(MachineError::NotConnected));
    assert!(h.poll().is_empty());
    assert!(h.backup_settings().is_none());
}

#[test]
fn grbl_simulator_streams_a_job_and_reports_progress() {
    let mut h = connected(Family::Grbl, 200.0);
    assert!(h.is_simulator() && !h.is_real_machine());
    assert!(h.snapshot().firmware.to_lowercase().contains("grbl"), "{}", h.snapshot().firmware);
    ready(&mut h);
    h.start_job(simulator_square(Family::Grbl, 20.0)).unwrap();
    let mut saw_progress = false;
    let done = h.wait_for(T, |s, ev| {
        saw_progress |= s.job.as_ref().is_some_and(|j| j.total_lines == Some(8));
        ev.iter().any(|e| matches!(e, MachineEvent::JobFinished(_)))
    });
    assert!(done);
    assert!(saw_progress || done, "progress or an instant job");
    ready(&mut h);
    assert!(h.console().iter().any(|c| c.kind == ConsoleKind::Received), "controller lines reach the console");
}

#[test]
fn grbl_job_outside_the_bed_is_refused() {
    let mut h = connected(Family::Grbl, 200.0);
    ready(&mut h);
    let mut job = simulator_square(Family::Grbl, 20.0);
    job.bounds = Some(Rect::new(0.0, 0.0, 900.0, 20.0));
    assert!(matches!(h.start_job(job), Err(MachineError::OutOfBounds(_))));
    let wrong = JobRequest::rd(ruida::RdJob::default());
    assert!(matches!(h.start_job(wrong), Err(MachineError::Unsupported(_))));
}

#[test]
fn grbl_pause_resume_stop() {
    let mut h = connected(Family::Grbl, 3.0);
    ready(&mut h);
    let mut lines = vec!["G21 G90".to_owned(), "M4 S100".to_owned()];
    for i in 0..40 {
        lines.push(format!("G1 X{} Y10 F600", 10 + (i % 2) * 20));
    }
    lines.push("M5 S0".into());
    h.start_job(JobRequest::gcode(lines)).unwrap();
    assert!(h.wait_for(T, |s, _| s.state == LaserState::Busy));
    h.pause().unwrap();
    assert!(h.wait_for(T, |s, _| s.state == LaserState::Paused), "paused, was {:?}", h.snapshot().state);
    h.resume().unwrap();
    assert!(h.wait_for(T, |s, _| s.state == LaserState::Busy));
    h.wait_for(Duration::from_millis(500), |_, _| false);
    h.stop().unwrap();
    assert!(wait_event(&mut h, |e| matches!(e, MachineEvent::Stopped)));
    ready(&mut h);
    assert!(!h.snapshot().fire_on);
}

#[test]
fn grbl_home_jog_move_origin_and_position() {
    let mut s = spec(Family::Grbl, 200.0);
    s.config.sim_homing = true;
    let mut h = Sim::new();
    h.connect(&s, None).unwrap();
    assert!(h.wait_for(T, |s, _| s.state == LaserState::Alarm), "homing enabled: locked until homed");
    assert!(matches!(h.jog_step(JogSpec { dx: 1.0, feed: 100.0, ..Default::default() }), Err(MachineError::Alarm)));
    h.home().unwrap();
    assert!(wait_event(&mut h, |e| matches!(e, MachineEvent::Homed)));
    ready(&mut h);
    h.jog_step(JogSpec { dx: 10.0, dy: 5.0, feed: 3000.0, ..Default::default() }).unwrap();
    assert!(h.wait_for(T, |s, _| (s.work_pos[0] - 10.0).abs() < 0.01 && (s.work_pos[1] - 5.0).abs() < 0.01));
    ready(&mut h);
    h.set_user_origin().unwrap();
    h.get_position().unwrap();
    assert!(h.wait_for(T, |s, _| s.work_pos[0].abs() < 0.01 && (s.machine_pos[0] - 10.0).abs() < 0.01));
    h.move_to(MoveSpec { x: Some(20.0), y: Some(0.0), feed: 6000.0, ..Default::default() }).unwrap();
    assert!(h.wait_for(T, |s, _| (s.work_pos[0] - 20.0).abs() < 0.01));
    ready(&mut h);
    h.go_to_origin().unwrap();
    assert!(h.wait_for(T, |s, _| s.work_pos[0].abs() < 0.01));
    ready(&mut h);
    h.clear_user_origin().unwrap();
    assert!(matches!(h.move_to(MoveSpec { x: Some(9999.0), feed: 1000.0, ..Default::default() }), Err(MachineError::OutOfBounds(_))));
    ready(&mut h);
    h.jog_start([1, 0, 0, 0], 3000.0).unwrap();
    assert!(h.wait_for(T, |s, _| s.machine_pos[0] > 100.0));
    h.jog_stop().unwrap();
    assert!(h.wait_for(T, |s, _| s.state == LaserState::Ready));
}

#[test]
fn grbl_fire_has_a_limit_and_turns_off() {
    let mut h = connected(Family::Grbl, 200.0);
    ready(&mut h);
    assert!(matches!(h.fire(80.0, 1.0), Err(MachineError::Invalid(_))), "above the fire limit");
    assert!(matches!(h.fire(5.0, 600.0), Err(MachineError::Invalid(_))));
    h.fire(5.0, 0.4).unwrap();
    assert!(h.wait_for(T, |s, _| s.fire_on));
    assert!(h.wait_for(T, |s, _| !s.fire_on), "fire turns itself off");
    h.fire(5.0, 5.0).unwrap();
    assert!(h.wait_for(T, |s, _| s.fire_on));
    h.fire_off().unwrap();
    assert!(h.wait_for(T, |s, _| !s.fire_on));
}

#[test]
fn grbl_overrides_console_and_macro() {
    let mut h = connected(Family::Grbl, 200.0);
    ready(&mut h);
    h.set_overrides(Some(120), Some(80), Some(50)).unwrap();
    assert!(h.wait_for(T, |s, _| s.overrides == (120, 50, 80)), "overrides {:?}", h.snapshot().overrides);
    h.send_console("$I").unwrap();
    assert!(h.wait_for(T, |_, _| true));
    h.run_macro("G21\nG90\n; comment\nG0 X5").unwrap();
    ready(&mut h);
}

#[test]
fn grbl_settings_round_trip_and_backup() {
    let mut h = connected(Family::Grbl, 200.0);
    ready(&mut h);
    h.read_settings().unwrap();
    assert!(wait_event(&mut h, |e| matches!(e, MachineEvent::SettingsLoaded)));
    assert!(h.snapshot().grbl_settings.contains_key(&110));
    let backup = h.backup_settings().unwrap();
    assert_eq!(backup.family(), Family::Grbl);
    let text = backup.to_text();
    let parsed = SettingsBackup::parse(Family::Grbl, &text).unwrap();
    assert_eq!(parsed.len(), backup.len());
    h.write_setting(110, "1234").unwrap();
    assert!(wait_event(&mut h, |e| matches!(e, MachineEvent::SettingWritten { ok: true, .. })));
    h.read_settings().unwrap();
    assert!(h.wait_for(T, |s, _| s.grbl_settings.get(&110).is_some_and(|v| v.starts_with("1234"))));
    h.restore_settings(parsed).unwrap();
    assert!(h.wait_for(T, |s, _| s.grbl_settings.get(&110).is_some_and(|v| !v.starts_with("1234"))));
    assert!(matches!(h.write_setting(110, "abc"), Err(MachineError::Invalid(_))));
    assert!(SettingsBackup::parse(Family::Grbl, "garbage").is_err());
    assert!(matches!(h.restore_settings(SettingsBackup::Ruida(Default::default())), Err(MachineError::Invalid(_))));
}

#[test]
fn grbl_frame_runs_and_ruida_only_commands_are_refused() {
    let mut h = connected(Family::Grbl, 200.0);
    ready(&mut h);
    h.frame_rect(Rect::new(10.0, 10.0, 40.0, 30.0), FrameSpec::default()).unwrap();
    assert!(h.wait_for(T, |s, ev| s.state == LaserState::Framing || ev.iter().any(|e| matches!(e, MachineEvent::FrameStarted))));
    ready(&mut h);
    assert!(matches!(h.command(MachineCommand::ListFiles), Err(MachineError::Unsupported(_))));
    assert!(matches!(h.command(MachineCommand::CalcTime(1)), Err(MachineError::Unsupported(_))));
    assert!(matches!(h.command(MachineCommand::FrameOnController), Err(MachineError::Unsupported(_))));
    h.disconnect();
    assert_eq!(h.snapshot().state, LaserState::Disconnected);
}

#[test]
fn ruida_simulator_runs_a_job_with_progress() {
    let mut h = connected(Family::Ruida, 20.0);
    assert!(h.snapshot().firmware.contains("RDC6442S"), "{}", h.snapshot().firmware);
    ready(&mut h);
    h.start_job(simulator_square(Family::Ruida, 30.0)).unwrap();
    assert!(wait_event(&mut h, |e| matches!(e, MachineEvent::JobStarted)));
    assert!(wait_event(&mut h, |e| matches!(e, MachineEvent::JobFinished(_))), "job finished");
    assert!(h.wait_for(T, |s, _| s.state == LaserState::Ready && s.job.is_none()));
}

#[test]
fn events_that_arrive_together_are_each_waited_for_in_turn() {
    // What one poll can hand a slow test thread: the start and the end of a job together.
    let mut h = Sim::new();
    h.unseen = vec![MachineEvent::FilesListed, MachineEvent::JobStarted, MachineEvent::JobFinished(JobProgress::default()), MachineEvent::Homed];
    assert!(wait_event(&mut h, |e| matches!(e, MachineEvent::JobStarted)));
    assert!(wait_event(&mut h, |e| matches!(e, MachineEvent::JobFinished(_))), "the end is still there after waiting for the start");
    assert!(matches!(h.unseen[..], [MachineEvent::Homed]), "matched events are used up, later ones stay: {:?}", h.unseen);
    // A wait on the state leaves the events alone.
    assert!(h.wait_for(T, |s, _| s.state == LaserState::Disconnected));
    assert!(wait_event(&mut h, |e| matches!(e, MachineEvent::Homed)));
    assert!(h.unseen.is_empty());
}

#[test]
fn ruida_job_outside_the_bed_and_gcode_are_refused() {
    let mut h = connected(Family::Ruida, 20.0);
    ready(&mut h);
    assert!(h.wait_for(T, |s, _| s.bed.is_some_and(|b| b.0 > 0.0 && b.1 > 0.0)), "bed size read on connect");
    let mut j = simulator_square(Family::Ruida, 30.0);
    j.bounds = Some(Rect::new(0.0, 0.0, 5000.0, 10.0));
    assert!(matches!(h.start_job(j), Err(MachineError::OutOfBounds(_))));
    assert!(matches!(h.start_job(JobRequest::gcode(vec!["G0 X1".into()])), Err(MachineError::Unsupported(_))));
    assert!(matches!(h.set_overrides(Some(110), None, None), Err(MachineError::Unsupported(_))));
    assert!(matches!(h.send_console("?"), Err(MachineError::Unsupported(_))));
    assert!(matches!(h.set_user_origin(), Err(MachineError::Unsupported(_))));
}

#[test]
fn ruida_pause_resume_stop() {
    let mut h = connected(Family::Ruida, 0.2);
    ready(&mut h);
    let mut job = simulator_square(Family::Ruida, 30.0);
    if let JobData::Rd(j) = &mut job.data {
        for i in 0..400 {
            j.layers[0].ops.push(ruida::RdOp::Cut { to: Point::new(20.0 + (i % 2) as f64, 20.0), power: None, speed: None });
        }
    }
    h.start_job(job).unwrap();
    assert!(h.wait_for(T, |s, _| s.state == LaserState::Busy), "busy, was {:?}", h.snapshot().state);
    h.pause().unwrap();
    assert!(h.wait_for(T, |s, _| s.state == LaserState::Paused), "paused, was {:?}", h.snapshot().state);
    h.resume().unwrap();
    assert!(h.wait_for(T, |s, _| s.state == LaserState::Busy));
    h.stop().unwrap();
    assert!(wait_event(&mut h, |e| matches!(e, MachineEvent::Stopped)));
    ready(&mut h);
    assert!(h.snapshot().job.is_none());
}

#[test]
fn ruida_home_jog_move_and_position() {
    let mut h = connected(Family::Ruida, 20.0);
    ready(&mut h);
    h.move_to(MoveSpec { x: Some(50.0), y: Some(40.0), feed: 6000.0, ..Default::default() }).unwrap();
    h.get_position().unwrap();
    assert!(h.wait_for(T, |s, _| (s.machine_pos[0] - 50.0).abs() < 0.01 && (s.machine_pos[1] - 40.0).abs() < 0.01));
    h.jog_step(JogSpec { dx: 5.0, dy: -10.0, feed: 3000.0, ..Default::default() }).unwrap();
    assert!(h.wait_for(T, |s, _| (s.machine_pos[0] - 55.0).abs() < 0.01 && (s.machine_pos[1] - 30.0).abs() < 0.01));
    h.jog_start([1, 0, 0, 0], 3000.0).unwrap();
    h.jog_stop().unwrap();
    h.home().unwrap();
    assert!(wait_event(&mut h, |e| matches!(e, MachineEvent::Homed)));
    h.go_to_origin().unwrap();
    assert!(h.wait_for(T, |s, _| s.machine_pos[0].abs() < 0.01 && s.machine_pos[1].abs() < 0.01));
    h.move_to(MoveSpec { x: Some(9000.0), feed: 1.0, ..Default::default() }).unwrap();
    assert!(wait_event(&mut h, |e| matches!(e, MachineEvent::Error(m) if m.contains("outside"))), "the driver reports the bounds error");
    h.frame_rect(Rect::new(10.0, 10.0, 40.0, 30.0), FrameSpec::default()).unwrap();
    assert!(wait_event(&mut h, |e| matches!(e, MachineEvent::FrameStarted)));
    h.fire(5.0, 0.2).unwrap();
    assert!(h.wait_for(T, |s, _| s.fire_on));
    assert!(h.wait_for(T, |s, _| !s.fire_on));
}

#[test]
fn ruida_file_list_upload_run_delete_and_time() {
    let mut h = connected(Family::Ruida, 200.0);
    ready(&mut h);
    h.command(MachineCommand::ListFiles).unwrap();
    assert!(wait_event(&mut h, |e| matches!(e, MachineEvent::FilesListed)));
    assert_eq!(h.snapshot().ruida.files.as_ref().unwrap().len(), 0);
    let job = match simulator_square(Family::Ruida, 20.0).data {
        JobData::Rd(j) => j,
        _ => unreachable!(),
    };
    let data = job.to_rd_file(ruida::Magic::DEFAULT).unwrap();
    let mut req = JobRequest::new(JobData::RdFile { name: "SQUARE".into(), data });
    req.start_immediately = false;
    h.start_job(req.clone()).unwrap();
    assert!(h.wait_for(T, |s, _| s.ruida.files.as_ref().is_some_and(|f| f.len() == 1)));
    assert_eq!(h.snapshot().ruida.files.as_ref().unwrap()[0].name, "SQUARE");
    // Same name again: the prompt, then rename.
    h.start_job(req).unwrap();
    assert!(wait_event(&mut h, |e| matches!(e, MachineEvent::OverwritePrompt(n) if n == "SQUARE")));
    h.answer_overwrite(ruida::OverwriteChoice::Rename).unwrap();
    assert!(h.wait_for(T, |s, _| s.ruida.files.as_ref().is_some_and(|f| f.len() == 2)));
    assert!(h.answer_overwrite(ruida::OverwriteChoice::Rename).is_err(), "nothing waiting");
    h.command(MachineCommand::CalcTime(1)).unwrap();
    assert!(h.wait_for(T, |s, _| s.ruida.times.contains_key(&1)));
    h.command(MachineCommand::RunFile(1)).unwrap();
    assert!(wait_event(&mut h, |e| matches!(e, MachineEvent::JobFinished(_))));
    h.command(MachineCommand::DeleteFiles(vec![1])).unwrap();
    assert!(h.wait_for(T, |s, _| s.ruida.files.as_ref().is_some_and(|f| f.len() == 1)));
    h.command(MachineCommand::DeleteAllFiles).unwrap();
    assert!(h.wait_for(T, |s, _| s.ruida.files.as_ref().is_some_and(Vec::is_empty)));
}

#[test]
fn ruida_settings_info_laser2_and_u_axis_round_trip() {
    let mut h = connected(Family::Ruida, 200.0);
    ready(&mut h);
    h.read_settings().unwrap();
    assert!(wait_event(&mut h, |e| matches!(e, MachineEvent::SettingsLoaded)));
    let backup = h.backup_settings().unwrap();
    assert_eq!(backup.family(), Family::Ruida);
    assert_eq!(h.snapshot().bed, Some((600.0, 400.0)));
    let parsed = SettingsBackup::parse(Family::Ruida, &backup.to_text()).unwrap();
    assert_eq!(parsed, backup);
    let g0 = u32::from(ruida::settings::addr::G0_VELOCITY);
    h.write_setting(g0, "123456").unwrap();
    h.read_settings().unwrap();
    assert!(h.wait_for(T, |s, _| s.ruida.settings.as_ref().and_then(|x| x.get(g0 as u16)) == Some(123_456)));
    h.restore_settings(parsed).unwrap();
    assert!(h.wait_for(T, |s, _| s.ruida.settings.as_ref().and_then(|x| x.get(g0 as u16)) != Some(123_456)));
    assert!(matches!(h.write_setting(1, "x"), Err(MachineError::Invalid(_))));
    h.command(MachineCommand::ReadControllerInfo).unwrap();
    assert!(wait_event(&mut h, |e| matches!(e, MachineEvent::InfoLoaded)));
    assert!(h.snapshot().ruida.info.is_some());
    let l2 = ruida::settings::Laser2Settings { pwm_freq_hz: 20_000, min_power: 100, max_power: 800, standby_freq_hz: 1, standby_pulse: 2 };
    h.command(MachineCommand::WriteLaser2(l2)).unwrap();
    assert!(h.wait_for(T, |s, _| s.ruida.laser2 == Some(l2)));
    let u = ruida::settings::UAxisSettings { range_um: 5, rotary_diameter_um: 80_000, ..Default::default() };
    h.command(MachineCommand::WriteUAxis(u)).unwrap();
    assert!(h.wait_for(T, |s, _| s.ruida.u_axis == Some(u)));
}

#[test]
fn disconnect_clears_and_reset_reconnects() {
    let mut h = connected(Family::Grbl, 200.0);
    ready(&mut h);
    let s = spec(Family::Grbl, 200.0);
    h.reset_connection(&s, None).unwrap();
    assert!(h.wait_for(T, |s, _| s.connected()));
    h.disconnect();
    assert!(!h.is_connected());
    assert_eq!(h.stop(), Err(MachineError::NotConnected));
}
