//! RuidaMachine against RuidaEmulator over MemoryTransport. No network, no USB.

mod common;

use common::*;
use darkburn_ruida::emulator::{MotionKind, RunState};
use darkburn_ruida::settings::{Laser2Settings, UAxisSettings, addr};
use darkburn_ruida::*;
use kurbo::{Point, Rect};

// ---- connection ---------------------------------------------------------------------------

#[test]
fn connect_reads_info_and_bed() {
    let (m, _emu) = udp_machine();
    let info = m.info().unwrap();
    assert_eq!(info.card_id, 0x6510_6510);
    assert_eq!(info.board.as_deref(), Some("RDC6442S"));
    assert_eq!(info.firmware, "RDC6442S-EMU");
    assert_eq!(m.bed_size_mm(), Some((600.0, 400.0)));
}

#[test]
fn connect_detects_magic() {
    for magic in [Magic(0x88), Magic(0x11), Magic(0x38), Magic(0x5A)] {
        let ecfg = EmulatorConfig { magic, ..Default::default() };
        let mcfg = MachineConfig { magic: None, ..Default::default() };
        let (mut m, _emu) = udp_machine_with(ecfg, mcfg);
        assert_eq!(m.magic(), magic);
        assert_eq!(m.read_setting(addr::BED_X).unwrap(), 600_000);
    }
}

#[test]
fn stream_framing_works_end_to_end() {
    let (mut m, emu) = stream_machine();
    assert_eq!(m.info().unwrap().card_id, 0x6510_6510);
    let out = m.send_job("USB", &square_job((5.0, 5.0), 10.0, 50.0), &SendOptions::default(), &mut never).unwrap();
    assert_eq!(out.index, 1);
    assert_eq!(m.list_files().unwrap()[0].name, "USB");
    m.run_file(1).unwrap();
    assert!(lock(&emu).motion.iter().any(|r| r.kind == MotionKind::Cut));
    // The jog port does not exist on USB.
    assert!(matches!(m.frame_on_controller(), Err(RuidaError::Unsupported(_))));
}

#[test]
fn wrong_magic_times_out() {
    let ecfg = EmulatorConfig { magic: Magic(0x11), ..Default::default() };
    let emu = emulator(ecfg);
    let mut m = RuidaMachine::new(MemoryTransport::udp(emu), MachineConfig { retries: 1, ..Default::default() });
    assert!(matches!(m.connect(), Err(RuidaError::Timeout(_))));
    assert!(!m.is_connected());
}

// ---- jobs and files -----------------------------------------------------------------------

#[test]
fn send_job_stores_named_file_that_reads_back() {
    let (mut m, emu) = udp_machine();
    let job = mixed_job();
    let out = m.send_job("MIXED", &job, &SendOptions::default(), &mut never).unwrap();
    assert_eq!(out, SendOutcome { name: "MIXED".into(), index: 1, started: false, bytes: job.to_logical().unwrap().len() });
    let e = lock(&emu);
    assert_eq!(e.files().len(), 1);
    assert_eq!(e.files()[0].name, "MIXED");
    assert_eq!(RdJob::from_logical(&e.files()[0].data).unwrap(), job.quantized());
    assert_eq!(e.file_sum_mismatches, 0);
    assert_eq!(e.state(), RunState::Idle);
    assert!(e.motion.is_empty(), "storing must not run the job");
}

#[test]
fn long_names_are_cut_and_cleaned() {
    let (mut m, _emu) = udp_machine();
    let out = m.send_job("my long job name", &square_job((1.0, 1.0), 1.0, 10.0), &SendOptions::default(), &mut never).unwrap();
    assert_eq!(out.name, "my_long_");
}

#[test]
fn auto_increment_names() {
    let (mut m, _emu) = udp_machine();
    let opts = SendOptions { auto_increment: true, ..Default::default() };
    let job = square_job((1.0, 1.0), 5.0, 10.0);
    let a = m.send_job("JOB", &job, &opts, &mut never).unwrap();
    let b = m.send_job("JOB", &job, &opts, &mut never).unwrap();
    let c = m.send_job("JOB", &job, &opts, &mut never).unwrap();
    assert_eq!((a.name.as_str(), b.name.as_str(), c.name.as_str()), ("JOB1", "JOB2", "JOB3"));
    assert_eq!(m.list_files().unwrap().len(), 3);
}

#[test]
fn overwrite_prompt_choices() {
    let (mut m, emu) = udp_machine();
    let small = square_job((1.0, 1.0), 5.0, 10.0);
    let big = square_job((1.0, 1.0), 50.0, 10.0);
    m.send_job("CUT", &small, &SendOptions::default(), &mut never).unwrap();

    // Cancel: nothing changes.
    let mut asked = Vec::new();
    let r = m.send_job("CUT", &big, &SendOptions::default(), &mut |n| {
        asked.push(n.to_string());
        OverwriteChoice::Cancel
    });
    assert_eq!(r, Err(RuidaError::Cancelled));
    assert_eq!(asked, vec!["CUT"]);
    assert_eq!(lock(&emu).files().len(), 1);

    // Overwrite: one file, new content.
    let out = m.send_job("CUT", &big, &SendOptions::default(), &mut |_| OverwriteChoice::Overwrite).unwrap();
    assert_eq!(out.name, "CUT");
    {
        let e = lock(&emu);
        assert_eq!(e.files().len(), 1);
        assert_eq!(RdJob::from_logical(&e.files()[0].data).unwrap(), big.quantized());
    }

    // Rename: a second, numbered file.
    let out = m.send_job("CUT", &small, &SendOptions::default(), &mut |_| OverwriteChoice::Rename).unwrap();
    assert_eq!(out.name, "CUT1");
    let names: Vec<String> = m.list_files().unwrap().into_iter().map(|f| f.name).collect();
    assert_eq!(names, vec!["CUT", "CUT1"]);
}

#[test]
fn file_limit_is_enforced() {
    let ecfg = EmulatorConfig::default();
    let (mut m, _emu) = udp_machine_with(ecfg, MachineConfig { max_files: 2, ..Default::default() });
    let job = square_job((1.0, 1.0), 5.0, 10.0);
    let opts = SendOptions { auto_increment: true, ..Default::default() };
    m.send_job("A", &job, &opts, &mut never).unwrap();
    m.send_job("A", &job, &opts, &mut never).unwrap();
    assert!(matches!(m.send_job("A", &job, &opts, &mut never), Err(RuidaError::ControllerError(_))));
}

#[test]
fn start_immediately_runs_and_records_cuts() {
    let (mut m, emu) = udp_machine();
    let job = square_job((10.0, 20.0), 30.0, 50.0);
    let out = m.send_job("SQ", &job, &SendOptions { start_immediately: true, ..Default::default() }, &mut never).unwrap();
    assert!(out.started);
    let e = lock(&emu);
    let cuts: Vec<_> = e.motion.iter().filter(|r| r.kind == MotionKind::Cut).collect();
    assert_eq!(cuts.len(), 4);
    assert_eq!(cuts[0].from[..2], [10_000, 20_000]);
    assert_eq!(cuts[0].to[..2], [40_000, 20_000]);
    assert_eq!(cuts[3].to[..2], [10_000, 20_000]);
    assert!(cuts.iter().all(|c| c.speed == 50_000 && c.power == Power::from_percent(30.0)));
    assert_eq!(e.state(), RunState::Idle);
    assert_eq!(e.memory(addr::TOTAL_WORK_NUMBER), Some(1));
}

#[test]
fn run_job_now_streams_without_storing() {
    let (mut m, emu) = udp_machine();
    m.run_job_now(&square_job((1.0, 1.0), 5.0, 10.0)).unwrap();
    let e = lock(&emu);
    assert!(e.files().is_empty());
    assert_eq!(e.motion.iter().filter(|r| r.kind == MotionKind::Cut).count(), 4);
}

#[test]
fn list_upload_run_delete() {
    let (mut m, emu) = udp_machine();
    let job = square_job((1.0, 1.0), 5.0, 10.0);
    for n in ["A", "B", "C", "D"] {
        m.send_job(n, &job, &SendOptions::default(), &mut never).unwrap();
    }
    // Upload an existing .rd file (as saved to disk, magic 0x11, detected automatically).
    let file = square_job((2.0, 2.0), 3.0, 10.0).to_rd_file(Magic(0x11)).unwrap();
    let out = m.upload_rd_file("DISK", &file, None, &SendOptions::default(), &mut never).unwrap();
    assert_eq!(out.index, 5);
    let names: Vec<String> = m.list_files().unwrap().into_iter().map(|f| f.name).collect();
    assert_eq!(names, vec!["A", "B", "C", "D", "DISK"]);

    m.run_file(5).unwrap();
    assert_eq!(lock(&emu).motion.iter().filter(|r| r.kind == MotionKind::Cut).count(), 4);

    m.delete_files(&[2, 4]).unwrap();
    let names: Vec<String> = m.list_files().unwrap().into_iter().map(|f| f.name).collect();
    assert_eq!(names, vec!["A", "C", "DISK"]);

    m.delete_all_files().unwrap();
    assert!(m.list_files().unwrap().is_empty());
}

#[test]
fn calc_time_matches_distance_over_speed() {
    let (mut m, _emu) = udp_machine();
    // 4 x 100 mm of cut at 10 mm/s = 40 s, plus the travel to the start at 200 mm/s.
    let job = square_job((0.0, 0.0), 100.0, 10.0);
    m.send_job("T", &job, &SendOptions::default(), &mut never).unwrap();
    let t = m.calc_time(1).unwrap();
    assert_eq!(t.as_millis(), 40_000);
}

// ---- control ------------------------------------------------------------------------------

#[test]
fn start_pause_resume_stop_in_step_mode() {
    let (mut m, emu) = udp_machine();
    lock(&emu).set_step_mode(true);
    m.send_job("STEP", &square_job((1.0, 1.0), 5.0, 10.0), &SendOptions::default(), &mut never).unwrap();
    m.run_file(1).unwrap();
    assert_eq!(m.status().unwrap().state, MachineState::Running);
    lock(&emu).advance(3);

    m.pause().unwrap();
    assert_eq!(m.status().unwrap().state, MachineState::Paused);
    assert_eq!(lock(&emu).advance(10), 0, "paused jobs do not move");

    m.resume().unwrap();
    assert_eq!(m.status().unwrap().state, MachineState::Running);
    lock(&emu).advance(1000);
    assert_eq!(m.status().unwrap().state, MachineState::Idle);

    // Start again from the controller's "start" (re-runs the last job), then stop.
    m.start().unwrap();
    assert_eq!(m.status().unwrap().state, MachineState::Running);
    m.stop().unwrap();
    assert_eq!(m.status().unwrap().state, MachineState::Idle);
}

#[test]
fn home_and_origin() {
    let (mut m, emu) = udp_machine();
    m.move_to(Point::new(100.0, 50.0), false).unwrap();
    assert_eq!(lock(&emu).position()[..2], [100_000, 50_000]);
    m.home(HomeAxes::XY).unwrap();
    assert_eq!(lock(&emu).position()[..2], [0, 0]);
    assert_eq!(lock(&emu).motion.last().unwrap().kind, MotionKind::Home);
    m.move_to(Point::new(10.0, 10.0), true).unwrap();
    m.go_to_origin().unwrap();
    assert_eq!(lock(&emu).position()[..2], [0, 0]);
    m.home(HomeAxes::Z).unwrap();
    m.home(HomeAxes::U).unwrap();
    let s = m.status().unwrap();
    assert_eq!((s.x, s.y, s.z, s.u), (0, 0, 0, 0));
}

#[test]
fn frame_rectangle_and_outline() {
    let (mut m, emu) = udp_machine();
    m.frame_rect(Rect::new(10.0, 20.0, 110.0, 70.0), false).unwrap();
    {
        let e = lock(&emu);
        let pts: Vec<[i32; 2]> = e.motion.iter().map(|r| [r.to[0], r.to[1]]).collect();
        assert_eq!(pts, vec![[10_000, 20_000], [110_000, 20_000], [110_000, 70_000], [10_000, 70_000], [10_000, 20_000]]);
        assert!(e.motion.iter().all(|r| r.kind == MotionKind::Rapid && r.power == Power(0)));
    }
    // Out of the bed: refused before anything moves.
    let before = lock(&emu).motion.len();
    assert!(m.frame_rect(Rect::new(500.0, 300.0, 700.0, 350.0), true).is_err());
    assert_eq!(lock(&emu).motion.len(), before);
    // Any outline.
    m.frame_outline(&[Point::new(1.0, 1.0), Point::new(5.0, 9.0), Point::new(1.0, 1.0)], true).unwrap();
    assert_eq!(lock(&emu).motion.len(), before + 3);
}

#[test]
fn frame_on_controller_traces_last_job() {
    let (mut m, emu) = udp_machine();
    m.send_job("F", &square_job((10.0, 10.0), 20.0, 10.0), &SendOptions { start_immediately: true, ..Default::default() }, &mut never).unwrap();
    let before = lock(&emu).motion.len();
    m.frame_on_controller().unwrap();
    let e = lock(&emu);
    let framed: Vec<[i32; 2]> = e.motion[before..].iter().map(|r| [r.to[0], r.to[1]]).collect();
    assert_eq!(framed, vec![[10_000, 10_000], [30_000, 10_000], [30_000, 30_000], [10_000, 30_000], [10_000, 10_000]]);
    assert_eq!(e.panel_received, vec![PanelCommand::Frame]);
}

#[test]
fn jog_step_and_continuous() {
    let (mut m, emu) = udp_machine();
    m.move_to(Point::new(50.0, 50.0), false).unwrap();
    m.jog_step(Axis::X, 5.5).unwrap();
    m.jog_step(Axis::Y, -10.0).unwrap();
    m.jog_step(Axis::Z, 2.0).unwrap();
    assert_eq!(lock(&emu).position(), [55_500, 40_000, 2_000, 0]);
    // Out of the bed is refused.
    assert!(m.jog_step(Axis::X, -100.0).is_err());

    m.jog_start(Axis::X, true).unwrap();
    m.jog_stop(Axis::X, true).unwrap();
    m.jog_start(Axis::U, false).unwrap();
    m.jog_stop(Axis::U, false).unwrap();
    assert_eq!(lock(&emu).position(), [65_500, 40_000, 2_000, -10_000]);
    assert_eq!(lock(&emu).motion.iter().filter(|r| r.kind == MotionKind::Jog).count(), 2);

    // Panel keys on the jog port.
    m.panel_key(PanelKey::PlusY, true).unwrap();
    m.panel_key(PanelKey::PlusY, false).unwrap();
    assert_eq!(lock(&emu).position()[1], 50_000);
}

// ---- settings -----------------------------------------------------------------------------

#[test]
fn settings_read_write() {
    let (mut m, emu) = udp_machine();
    assert_eq!(m.read_setting(addr::BED_Y).unwrap(), 400_000);
    m.write_setting(addr::G0_VELOCITY, 300_000).unwrap();
    assert_eq!(m.read_setting(addr::G0_VELOCITY).unwrap(), 300_000);
    assert_eq!(lock(&emu).writes, vec![(addr::G0_VELOCITY, 300_000)]);
    let many = m.read_settings(&[addr::BED_X, addr::BED_Y]).unwrap();
    assert_eq!(many.values().copied().collect::<Vec<_>>(), vec![600_000, 400_000]);
}

#[test]
fn settings_backup_and_restore() {
    let (mut m, _emu) = udp_machine();
    let backup = m.backup_settings().unwrap();
    assert_eq!(backup.card_id, Some(0x6510_6510));
    assert_eq!(backup.bed_size_mm(), Some((600.0, 400.0)));
    assert!(backup.values.len() >= 40);
    // Survives serialisation.
    let json = serde_json::to_string_pretty(&backup).unwrap();
    let back: MachineSettings = serde_json::from_str(&json).unwrap();
    assert_eq!(back, backup);

    m.write_setting(addr::BED_X, 1).unwrap();
    m.write_setting(addr::X_MAX_VELOCITY, 2).unwrap();
    assert_eq!(m.backup_settings().unwrap().diff(&backup), vec![addr::X_MAX_VELOCITY, addr::BED_X]);

    let mut with_extra = back.clone();
    with_extra.set(addr::CARD_ID, 7); // read-only: skipped
    let skipped = m.restore_settings(&with_extra).unwrap();
    assert_eq!(skipped, vec![addr::CARD_ID]);
    assert!(m.backup_settings().unwrap().diff(&backup).is_empty());
}

#[test]
fn controller_info_counts_jobs_and_laser_time() {
    let (mut m, _emu) = udp_machine();
    let before = m.read_controller_info().unwrap();
    assert_eq!(before.jobs_run, 0);
    // 4 x 100 mm at 10 mm/s = 40 s of laser.
    m.send_job("LONG", &square_job((0.0, 0.0), 100.0, 10.0), &SendOptions { start_immediately: true, ..Default::default() }, &mut never).unwrap();
    let after = m.read_controller_info().unwrap();
    assert_eq!(after.jobs_run, 1);
    assert_eq!(after.total_laser_on_time_s, 40);
    assert_eq!(after.total_job_time_s, 40);
    assert_eq!(after.last_job_time_ms, 40_000);
    assert_eq!(after.total_on_time_s, 3_600);
}

#[test]
fn laser2_and_u_axis_settings() {
    let (mut m, _emu) = udp_machine();
    let l2 = m.read_laser2().unwrap();
    assert_eq!(l2.pwm_freq_hz, 20_000);
    let new = Laser2Settings { pwm_freq_hz: 5_000, min_power: 50, max_power: 800, standby_freq_hz: 2_000, standby_pulse: 3 };
    m.write_laser2(new).unwrap();
    assert_eq!(m.read_laser2().unwrap(), new);

    let u = m.read_u_axis().unwrap();
    assert_eq!(u.rotary_diameter_um, 80_000);
    let new = UAxisSettings { range_um: 360_000, rotary_pulses_per_rotation: 3200, rotary_diameter_um: 60_000, max_velocity: 50_000, ..u };
    m.write_u_axis(new).unwrap();
    assert_eq!(m.read_u_axis().unwrap(), new);
}

// ---- packet layer -------------------------------------------------------------------------

#[test]
fn big_jobs_are_chunked_under_the_mtu() {
    let emu = emulator(EmulatorConfig::default());
    let t = MemoryTransport::udp(emu.clone()).with_max_payload(100);
    let faults = t.faults();
    let mut m = RuidaMachine::new(t, MachineConfig::default());
    m.connect().unwrap();
    let mut job = square_job((1.0, 1.0), 5.0, 10.0);
    for i in 0..200 {
        let p = Point::new(1.0 + (i % 50) as f64, 1.0 + (i / 50) as f64);
        job.layers[0].ops.push(RdOp::Cut { to: p, power: None, speed: None });
    }
    let logical_len = job.to_logical().unwrap().len();
    let before = faults.lock().unwrap().delivered;
    m.send_job("BIG", &job, &SendOptions::default(), &mut never).unwrap();
    let datagrams = faults.lock().unwrap().delivered - before;
    assert!(datagrams > logical_len / 100, "{datagrams} datagrams for {logical_len} bytes");
    let e = lock(&emu);
    assert_eq!(RdJob::from_logical(&e.files()[0].data).unwrap(), job.quantized());
}

#[test]
fn default_chunk_size_fits_one_datagram() {
    const { assert!(packet::DEFAULT_CHUNK + 2 <= packet::MAX_DATAGRAM) };
}

#[test]
fn retries_after_lost_datagram_and_bad_checksum() {
    let emu = emulator(EmulatorConfig::default());
    let t = MemoryTransport::udp(emu.clone());
    let faults = t.faults();
    let mut m = RuidaMachine::new(t, MachineConfig::default());
    m.connect().unwrap();

    faults.lock().unwrap().drop_sends = 1;
    m.home(HomeAxes::XY).unwrap();
    assert_eq!(m.resends, 1);

    faults.lock().unwrap().corrupt_sends = 2;
    m.move_to(Point::new(1.0, 2.0), false).unwrap();
    assert_eq!(m.resends, 3);
    assert_eq!(lock(&emu).checksum_failures, 2);
    assert_eq!(lock(&emu).position()[..2], [1_000, 2_000]);

    // A lost reply to a query is re-asked.
    faults.lock().unwrap().drop_replies = 2; // the ACK and the reply
    assert_eq!(m.read_setting(addr::BED_X).unwrap(), 600_000);
}

#[test]
fn err_on_first_chunk_is_retried_but_not_later() {
    let emu = emulator(EmulatorConfig::default());
    let t = MemoryTransport::udp(emu.clone()).with_max_payload(50);
    let faults = t.faults();
    let mut m = RuidaMachine::new(t, MachineConfig::default());
    m.connect().unwrap();

    faults.lock().unwrap().err_replies = 1;
    m.stop().unwrap();

    // ERR on a later chunk of a multi-chunk send aborts.
    let bytes = square_job((1.0, 1.0), 5.0, 10.0).to_logical().unwrap();
    assert!(bytes.len() > 100);
    let delivered = faults.lock().unwrap().delivered;
    {
        let mut f = faults.lock().unwrap();
        f.err_replies = 0;
    }
    // Make the second datagram fail: let one through, then ERR.
    let mut sent = 0;
    let r = m.send_logical_with_progress(&bytes, &mut |s, _| {
        sent = s;
        if s == 50 {
            faults.lock().unwrap().err_replies = 1;
        }
    });
    assert!(matches!(r, Err(RuidaError::ControllerError(_))), "{r:?}");
    assert_eq!(faults.lock().unwrap().delivered, delivered + 1);
}

#[test]
fn gives_up_after_retries() {
    let emu = emulator(EmulatorConfig::default());
    let t = MemoryTransport::udp(emu);
    let faults = t.faults();
    let mut m = RuidaMachine::new(t, MachineConfig { retries: 2, ..Default::default() });
    m.connect().unwrap();
    faults.lock().unwrap().drop_sends = 10;
    assert!(matches!(m.stop(), Err(RuidaError::Timeout(_))));
    assert_eq!(m.resends, 2);
    faults.lock().unwrap().drop_sends = 0;
    faults.lock().unwrap().corrupt_sends = 10;
    assert_eq!(m.stop(), Err(RuidaError::ChecksumRejected));
}

#[test]
fn raw_commands_pass_through() {
    let (mut m, emu) = udp_machine();
    m.send_commands(&[Command::Raw(vec![0xFE, 0x01, 0x02]), Command::HomeXY]).unwrap();
    let e = lock(&emu);
    assert!(e.received.contains(&Command::Raw(vec![0xFE, 0x01, 0x02])));
    assert_eq!(e.received.last(), Some(&Command::HomeXY));
}
