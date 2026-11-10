//! Driver tests against the in-memory emulator. No test opens a real device.

use std::time::{Duration, Instant};

use darkburn_galvo::*;
use kurbo::{BezPath, Rect};

struct Rig {
    m: GalvoMachine,
    emu: EmulatorHandle,
    events: Vec<Event>,
}

fn opts() -> ConnectOptions {
    ConnectOptions { port_poll_interval: Duration::ZERO, ..Default::default() }
}

fn rig_with(settings: DeviceSettings, emu_cfg: EmulatorConfig, o: ConnectOptions) -> Rig {
    let (t, emu) = MemoryTransport::new(GalvoEmulator::new(emu_cfg));
    let m = GalvoMachine::connect(Box::new(t), settings, o);
    let mut r = Rig { m, emu, events: Vec::new() };
    assert!(r.wait(|r| r.m.snapshot().connection == ConnectionState::Connected), "did not connect: {:?}", r.m.snapshot());
    r
}

fn rig(settings: DeviceSettings) -> Rig {
    rig_with(settings, EmulatorConfig { cmds_per_poll: 16, ..Default::default() }, opts())
}

impl Rig {
    fn wait(&mut self, mut f: impl FnMut(&mut Rig) -> bool) -> bool {
        let end = Instant::now() + Duration::from_secs(10);
        while Instant::now() < end {
            self.events.extend(self.m.poll_events());
            if f(self) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        false
    }

    fn wait_event(&mut self, pred: impl Fn(&Event) -> bool) -> bool {
        self.wait(|r| r.events.iter().any(&pred))
    }

    fn has(&self, pred: impl Fn(&Event) -> bool) -> bool {
        self.events.iter().any(pred)
    }

    fn emu<R>(&self, f: impl FnOnce(&mut GalvoEmulator) -> R) -> R {
        f(&mut self.emu.lock().unwrap())
    }
}

fn rect(x: f64, y: f64, w: f64, h: f64) -> BezPath {
    let mut p = BezPath::new();
    p.move_to((x, y));
    p.line_to((x + w, y));
    p.line_to((x + w, y + h));
    p.line_to((x, y + h));
    p.close_path();
    p
}

fn small_job(dev: &DeviceSettings) -> CompiledJob {
    let mut job = Job::new();
    job.layers.push(Layer::new("L", LayerMode::Line, MarkParams::default(), vec![rect(-10.0, -10.0, 20.0, 20.0)]));
    compile(&job, dev, &CompileOptions::default()).unwrap()
}

/// A job long enough to still be running after a few polls.
fn long_job(dev: &DeviceSettings) -> CompiledJob {
    let mut job = Job::new();
    job.layers.push(Layer::new(
        "F",
        LayerMode::Fill(FillSettings { interval_mm: 0.1, ..Default::default() }),
        MarkParams::default(),
        vec![rect(-20.0, -20.0, 40.0, 40.0)],
    ));
    compile(&job, dev, &CompileOptions::default()).unwrap()
}

fn marks_in(job: &CompiledJob) -> usize {
    job.list_commands().iter().filter(|c| matches!(c, ListCmd::MarkTo { .. })).count()
}

fn dev() -> DeviceSettings {
    DeviceSettings::default()
}

#[test]
fn connecting_never_enables_the_laser() {
    let r = rig(dev());
    let s = r.m.snapshot();
    assert_eq!(s.serial, Some(EmulatorConfig::default().serial));
    assert_eq!(s.version, Some(EmulatorConfig::default().version));
    assert!(!s.lens_loaded);
    r.emu(|e| {
        assert!(!e.laser_enabled() && !e.mo_open());
        assert!(!e.log().singles.contains(&SingleCmd::EnableLaser));
        assert!(e.log().singles.contains(&SingleCmd::DisableLaser));
        assert!(e.log().singles.contains(&SingleCmd::WriteCorTable(false)));
    });
}

#[test]
fn job_needs_an_explicit_arm() {
    let mut r = rig(dev());
    let job = small_job(&dev());
    assert_eq!(r.m.start_job(job.clone(), StartOptions::default()), Err(GalvoError::NotArmed));
    r.m.arm().unwrap();
    r.m.disarm().unwrap();
    assert_eq!(r.m.start_job(job, StartOptions::default()), Err(GalvoError::NotArmed));
    std::thread::sleep(Duration::from_millis(30));
    r.events.extend(r.m.poll_events());
    r.emu(|e| {
        assert!(!e.log().singles.contains(&SingleCmd::EnableLaser) && e.log().motions.is_empty());
    });
}

#[test]
fn armed_job_marks_and_spends_the_arm() {
    let mut r = rig(dev());
    let job = small_job(&dev());
    let n = marks_in(&job);
    r.m.arm().unwrap();
    r.m.start_job(job.clone(), StartOptions::default()).unwrap();
    assert!(r.wait_event(|e| matches!(e, Event::JobFinished { .. })), "{:?}", r.events);
    assert!(r.has(|e| matches!(e, Event::JobStarted)));
    r.emu(|e| {
        assert_eq!(e.emitting_marks(), n);
        assert!(!e.laser_enabled() && !e.mo_open() && !e.laser_signal());
        assert_eq!(e.port_out(), 0);
        assert!(!e.is_executing() && e.queued_commands() == 0);
        // fibre defaults: 30 kHz -> 667, 50 % -> 2048
        let m = e.log().motions.iter().find(|m| m.mark).unwrap();
        assert_eq!((m.q_period, m.current), (667, 2048));
    });
    let s = r.m.snapshot();
    assert!(!s.armed && s.activity == Activity::Idle && s.progress.is_none());
    assert_eq!(r.m.start_job(job, StartOptions::default()), Err(GalvoError::NotArmed));
}

#[test]
fn multi_packet_job_streams_completely() {
    let mut r = rig(dev());
    let job = long_job(&dev());
    assert!(job.packet_count() > 4);
    let n = marks_in(&job);
    r.m.arm().unwrap();
    r.m.start_job(job, StartOptions::default()).unwrap();
    assert!(r.wait_event(|e| matches!(e, Event::JobFinished { .. })));
    r.emu(|e| assert_eq!(e.emitting_marks(), n));
}

#[test]
fn frame_is_red_light_only_and_never_emits() {
    let mut r = rig(dev());
    // even if somebody armed the laser, a frame must stay dark
    r.m.arm().unwrap();
    let spec = FrameSpec::from_rect(Rect::new(-10.0, -10.0, 10.0, 10.0));
    let f = compile_frame(&spec, &dev()).unwrap();
    r.m.start_frame(f, true).unwrap();
    assert!(r.wait(|r| r.events.iter().filter(|e| matches!(e, Event::FrameLap)).count() >= 3), "{:?}", r.events);
    r.emu(|e| {
        assert!(!e.laser_enabled() && !e.mo_open());
        assert!(!e.ever_emitted());
        assert!(e.log().light_on_writes > 0);
        assert!(e.log().motions.iter().all(|m| !m.mark));
        assert!(!e.log().singles.contains(&SingleCmd::EnableLaser));
    });
    assert_eq!(r.m.snapshot().framed_key, Some(spec.key));
    r.m.stop().unwrap();
    assert!(r.wait_event(|e| matches!(e, Event::FrameStopped)));
    r.emu(|e| assert_eq!(e.port_out(), 0));
}

#[test]
fn frames_with_marks_are_refused() {
    let r = rig(dev());
    let bad = CompiledFrame { packets: packetize(&[ListCmd::MarkTo { x: 1, y: 1, angle: 0, distance: 1 }]), bounds_mm: Rect::ZERO, key: 0 };
    assert!(matches!(r.m.start_frame(bad, false), Err(GalvoError::Unsafe(_))));
}

#[test]
fn stop_mid_job_turns_everything_off() {
    let mut r = rig_with(dev(), EmulatorConfig { cmds_per_poll: 2, ..Default::default() }, opts());
    let job = long_job(&dev());
    let n = marks_in(&job);
    r.m.arm().unwrap();
    r.m.start_job(job, StartOptions::default()).unwrap();
    assert!(r.wait(|r| r.emu(|e| e.emitting_marks() > 5)));
    r.m.stop().unwrap();
    assert!(r.wait_event(|e| matches!(e, Event::JobStopped)));
    r.emu(|e| {
        assert!(!e.laser_enabled() && !e.mo_open() && !e.laser_signal());
        assert_eq!(e.port_out(), 0);
        assert!(!e.is_executing() && e.queued_commands() == 0);
        assert!(e.emitting_marks() < n);
    });
    let s = r.m.snapshot();
    assert!(!s.armed && s.activity == Activity::Idle);
    // nothing keeps marking afterwards
    let before = r.emu(|e| e.log().motions.len());
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(r.emu(|e| e.log().motions.len()), before);
}

#[test]
fn stop_when_idle_still_runs_the_off_sequence() {
    let r = rig(dev());
    r.emu(|e| e.clear_log());
    r.m.stop().unwrap();
    std::thread::sleep(Duration::from_millis(60));
    r.emu(|e| {
        let s = &e.log().singles;
        for c in [SingleCmd::StopExecute, SingleCmd::FiberSetMo(false), SingleCmd::LaserSignalOff, SingleCmd::WritePort(0), SingleCmd::DisableLaser] {
            assert!(s.contains(&c), "{c:?} missing in {s:?}");
        }
    });
}

#[test]
fn dropping_the_machine_turns_the_laser_off() {
    let mut r = rig_with(dev(), EmulatorConfig { cmds_per_poll: 2, ..Default::default() }, opts());
    r.m.arm().unwrap();
    r.m.start_job(long_job(&dev()), StartOptions::default()).unwrap();
    assert!(r.wait(|r| r.emu(|e| e.emitting_marks() > 2)));
    let emu = r.emu.clone();
    drop(r);
    let e = emu.lock().unwrap();
    assert!(!e.laser_enabled() && !e.mo_open() && e.port_out() == 0 && !e.is_executing());
}

#[test]
fn door_open_blocks_start_and_pauses_a_running_job() {
    let mut d = dev();
    d.ports.door_bit = Some(3); // low = open
    let mut r = rig_with(d.clone(), EmulatorConfig { cmds_per_poll: 2, ..Default::default() }, opts());
    // door open before start
    r.emu(|e| e.set_input_bit(3, false));
    assert!(r.wait(|r| r.m.snapshot().inputs.door_open));
    r.m.arm().unwrap();
    assert_eq!(r.m.start_job(small_job(&d), StartOptions::default()), Err(GalvoError::DoorOpen));
    assert!(matches!(
        r.m.start_frame(compile_frame(&FrameSpec::from_rect(Rect::new(-5.0, -5.0, 5.0, 5.0)), &d).unwrap(), false),
        Err(GalvoError::DoorOpen)
    ));
    // close it and run
    r.emu(|e| e.set_input_bit(3, true));
    assert!(r.wait(|r| !r.m.snapshot().inputs.door_open));
    let job = long_job(&d);
    let n = marks_in(&job);
    r.m.start_job(job, StartOptions::default()).unwrap();
    assert!(r.wait(|r| r.emu(|e| e.emitting_marks() > 5)));
    // open mid-job
    r.emu(|e| e.set_input_bit(3, false));
    assert!(r.wait_event(|e| matches!(e, Event::DoorOpened)), "{:?}", r.events);
    assert_eq!(r.m.snapshot().activity, Activity::PausedDoor);
    r.emu(|e| {
        assert!(!e.mo_open() && !e.laser_signal());
        assert!(e.is_paused());
    });
    let frozen = r.emu(|e| e.log().motions.len());
    std::thread::sleep(Duration::from_millis(40));
    assert_eq!(r.emu(|e| e.log().motions.len()), frozen);
    assert_eq!(r.m.resume(), Err(GalvoError::DoorOpen));
    // close and resume
    r.emu(|e| e.set_input_bit(3, true));
    assert!(r.wait(|r| !r.m.snapshot().inputs.door_open));
    r.m.resume().unwrap();
    assert!(r.wait_event(|e| matches!(e, Event::JobFinished { .. })), "{:?}", r.events);
    r.emu(|e| assert_eq!(e.emitting_marks(), n));
}

#[test]
fn foot_switch_starts_the_job() {
    let mut r = rig(dev());
    r.m.arm().unwrap();
    let job = small_job(&dev());
    let n = marks_in(&job);
    r.m.start_job(job, StartOptions { wait_for_foot: true, ..Default::default() }).unwrap();
    assert!(r.wait_event(|e| matches!(e, Event::WaitingForFoot)));
    assert_eq!(r.m.snapshot().activity, Activity::WaitingFoot);
    std::thread::sleep(Duration::from_millis(40));
    r.emu(|e| assert!(e.log().motions.is_empty()));
    r.emu(|e| e.set_input_bit(15, false)); // pressed (active low)
    assert!(r.wait_event(|e| matches!(e, Event::JobFinished { .. })));
    r.emu(|e| assert_eq!(e.emitting_marks(), n));
}

#[test]
fn repeat_waits_for_the_foot_switch_each_time() {
    let mut r = rig(dev());
    let mut job = Job::new();
    job.layers.push(Layer::new("L", LayerMode::Line, MarkParams::default(), vec![rect(-5.0, -5.0, 10.0, 10.0)]));
    job.repeat = RepeatMarking { count: 2, axis_step: Some((0, 10.0)), wait_foot_each: true };
    let c = compile(&job, &dev(), &CompileOptions::default()).unwrap();
    r.m.arm().unwrap();
    r.m.start_job(c, StartOptions::default()).unwrap();
    // first repeat runs, then the job waits for the foot
    assert!(r.wait_event(|e| matches!(e, Event::WaitingForFoot)));
    let after_first = r.emu(|e| e.emitting_marks());
    assert_eq!(after_first, 4);
    r.emu(|e| e.set_input_bit(15, false));
    assert!(r.wait_event(|e| matches!(e, Event::JobFinished { .. })));
    r.emu(|e| {
        assert_eq!(e.emitting_marks(), 8);
        assert_eq!(e.axis_position(0), 1000); // 10 mm at 100 steps/mm
    });
}

#[test]
fn require_framing_before_start() {
    let mut d = dev();
    d.require_framing = true;
    let mut r = rig(d.clone());
    let job = small_job(&d);
    r.m.arm().unwrap();
    assert_eq!(r.m.start_job(job.clone(), StartOptions::default()), Err(GalvoError::FramingRequired));
    // frame the job's own area once
    let mut j = Job::new();
    j.layers.push(Layer::new("L", LayerMode::Line, MarkParams::default(), vec![rect(-10.0, -10.0, 20.0, 20.0)]));
    let spec = FrameSpec::from_job_bounds(&j, 0.01).unwrap();
    r.m.start_frame(compile_frame(&spec, &d).unwrap(), false).unwrap();
    assert!(r.wait_event(|e| matches!(e, Event::FrameStopped)));
    r.m.start_job(job.clone(), StartOptions::default()).unwrap();
    assert!(r.wait_event(|e| matches!(e, Event::JobFinished { .. })));
    // a different job is not framed
    let mut other = Job::new();
    other.layers.push(Layer::new("L", LayerMode::Line, MarkParams::default(), vec![rect(0.0, 0.0, 5.0, 5.0)]));
    r.m.arm().unwrap();
    assert_eq!(r.m.start_job(compile(&other, &d, &CompileOptions::default()).unwrap(), StartOptions::default()), Err(GalvoError::FramingRequired));
    // the user can override
    r.m.start_job(job, StartOptions { skip_framing_check: true, ..Default::default() }).unwrap();
    assert!(r.wait(|r| r.events.iter().filter(|e| matches!(e, Event::JobFinished { .. })).count() == 2));
}

#[test]
fn done_marking_signal_pulses_the_output() {
    let mut d = dev();
    d.ports.done_bit = Some(5);
    d.ports.done_ms = 0;
    let mut r = rig(d.clone());
    r.m.arm().unwrap();
    r.m.start_job(small_job(&d), StartOptions::default()).unwrap();
    assert!(r.wait_event(|e| matches!(e, Event::JobFinished { .. })));
    r.emu(|e| {
        let s = &e.log().singles;
        let pulse = s.iter().position(|c| *c == SingleCmd::WritePort(1 << 5)).expect("done pulse");
        assert!(s[pulse..].contains(&SingleCmd::WritePort(0)));
        assert_eq!(e.port_out(), 0);
    });
}

#[test]
fn lens_table_loads_at_connect_and_can_change() {
    let t = CorTable::from_fn(|r, c| (r as i32 - c as i32, 3));
    let mut r = rig_with(dev(), EmulatorConfig::default(), ConnectOptions { lens: Some(t.clone()), ..opts() });
    assert!(r.m.snapshot().lens_loaded);
    assert_eq!(r.emu(|e| e.lens_table()), Some(t));
    r.m.set_lens(None).unwrap();
    assert!(r.wait_event(|e| matches!(e, Event::LensLoaded)));
    assert_eq!(r.emu(|e| e.lens_table()), None);
    assert!(!r.m.snapshot().lens_loaded);
}

#[test]
fn rotary_job_moves_the_axis_per_window() {
    let mut r = rig(dev());
    let mut job = Job::new();
    job.layers.push(Layer::new("L", LayerMode::Line, MarkParams::default(), vec![rect(-10.0, 0.0, 20.0, 60.0)]));
    job.rotary = Some(RotaryConfig { mode: RotaryMode::BySlice, max_size_mm: 30.0, axis: 0 });
    let c = compile(&job, &dev(), &CompileOptions::default()).unwrap();
    r.m.arm().unwrap();
    r.m.start_job(c, StartOptions::default()).unwrap();
    assert!(r.wait_event(|e| matches!(e, Event::JobFinished { .. })), "{:?}", r.events);
    // last window centre is 45 mm -> 4500 steps
    r.emu(|e| assert_eq!(e.axis_position(0), 4500));
}

#[test]
fn manual_axis_move_and_out_of_range() {
    let mut r = rig(dev());
    r.m.move_axis(0, -12.5).unwrap();
    assert!(r.wait(|r| r.emu(|e| e.axis_position(0)) == -1250));
    assert!(matches!(r.m.move_axis(0, 100000.0), Err(GalvoError::Invalid(_))));
    assert!(matches!(r.m.move_axis(5, 1.0), Err(GalvoError::Invalid(_))));
}

#[test]
fn sliced_image_moves_z_and_returns() {
    let mut r = rig(dev());
    let img = SlicedImage {
        width_px: 2,
        height_px: 2,
        depth: vec![255, 120, 255, 120],
        origin: kurbo::Point::new(-1.0, -1.0),
        size_mm: (2.0, 2.0),
        slices: 2,
        z_step_mm: 0.5,
        interval_mm: 1.0,
        bidirectional: true,
        cleanup: None,
    };
    let mut job = Job::new();
    job.layers.push(Layer::new("3D", LayerMode::Sliced(img), MarkParams::default(), vec![]));
    let c = compile(&job, &dev(), &CompileOptions::default()).unwrap();
    r.m.arm().unwrap();
    r.m.start_job(c, StartOptions::default()).unwrap();
    assert!(r.wait_event(|e| matches!(e, Event::JobFinished { .. })));
    r.emu(|e| {
        assert_eq!(e.z_data(), 0);
        assert!(e.log().singles.contains(&SingleCmd::SetZData(50)));
    });
}

#[test]
fn pause_and_resume_by_the_user() {
    let mut r = rig_with(dev(), EmulatorConfig { cmds_per_poll: 2, ..Default::default() }, opts());
    let job = long_job(&dev());
    let n = marks_in(&job);
    r.m.arm().unwrap();
    r.m.start_job(job, StartOptions::default()).unwrap();
    assert!(r.wait(|r| r.emu(|e| e.emitting_marks() > 3)));
    r.m.pause().unwrap();
    assert!(r.wait_event(|e| matches!(e, Event::Paused)));
    r.emu(|e| assert!(!e.mo_open() && e.is_paused()));
    let frozen = r.emu(|e| e.log().motions.len());
    std::thread::sleep(Duration::from_millis(40));
    assert_eq!(r.emu(|e| e.log().motions.len()), frozen);
    r.m.resume().unwrap();
    assert!(r.wait_event(|e| matches!(e, Event::JobFinished { .. })));
    r.emu(|e| assert_eq!(e.emitting_marks(), n));
}

#[test]
fn link_failure_mid_job_is_reported_and_leaves_state_off() {
    let mut r = rig_with(dev(), EmulatorConfig { cmds_per_poll: 2, ..Default::default() }, opts());
    r.m.arm().unwrap();
    r.m.start_job(long_job(&dev()), StartOptions::default()).unwrap();
    assert!(r.wait(|r| r.emu(|e| e.emitting_marks() > 2)));
    r.emu(|e| e.disconnect());
    assert!(r.wait_event(|e| matches!(e, Event::JobFailed(_))), "{:?}", r.events);
    assert!(matches!(r.m.snapshot().connection, ConnectionState::Failed(_)));
    assert!(!r.m.snapshot().armed);
    assert_eq!(r.m.start_job(small_job(&dev()), StartOptions::default()), Err(GalvoError::NotConnected));
}

#[test]
fn red_light_toggle() {
    let r = rig(dev());
    r.m.set_light(true).unwrap();
    let mut r = r;
    assert!(r.wait(|r| r.emu(|e| e.port_out()) == 1 << 8));
    r.m.set_light(false).unwrap();
    assert!(r.wait(|r| r.emu(|e| e.port_out()) == 0));
}

#[test]
fn no_answer_fails_the_connection() {
    struct Mute;
    impl Transport for Mute {
        fn write(&mut self, _: &[u8]) -> std::io::Result<()> {
            Ok(())
        }
        fn read(&mut self, _: Duration) -> std::io::Result<Option<[u8; 8]>> {
            std::thread::sleep(Duration::from_millis(5));
            Ok(None)
        }
        fn name(&self) -> String {
            "mute".into()
        }
    }
    let m = GalvoMachine::connect(Box::new(Mute), dev(), ConnectOptions { read_timeout: Duration::from_millis(40), ..Default::default() });
    let end = Instant::now() + Duration::from_secs(5);
    while Instant::now() < end && !matches!(m.snapshot().connection, ConnectionState::Failed(_)) {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(matches!(m.snapshot().connection, ConnectionState::Failed(_)));
    assert_eq!(m.arm(), Err(GalvoError::NotConnected));
}

#[test]
fn drill_job_fires_at_each_point_and_only_when_armed() {
    let mut job = Job::new();
    job.layers.push(Layer::new(
        "D",
        LayerMode::Drill(DrillSettings { dwell_ms: 3.0, spacing_mm: 5.0 }),
        MarkParams { power_pct: 80.0, ..Default::default() },
        vec![rect(-10.0, -10.0, 20.0, 20.0)],
    ));
    let compiled = compile(&job, &dev(), &CompileOptions::default()).unwrap();
    let mut r = rig(dev());
    assert_eq!(r.m.start_job(compiled.clone(), StartOptions::default()), Err(GalvoError::NotArmed));
    r.emu(|e| assert!(e.log().shots.is_empty()));
    r.m.arm().unwrap();
    r.m.start_job(compiled, StartOptions::default()).unwrap();
    assert!(r.wait_event(|e| matches!(e, Event::JobFinished { .. })), "{:?}", r.events);
    r.emu(|e| {
        let log = e.log();
        assert_eq!(log.shots.len(), 16, "80 mm round every 5 mm");
        assert_eq!(log.points, 16);
        assert!(log.shots.iter().all(|s| s.emitting && s.dwell == 300), "3 ms each, laser on: {:?}", log.shots);
        assert_eq!(e.emitting_marks(), 0, "nothing is drawn between the points");
        // 16 different places.
        let mut places: Vec<(u16, u16)> = log.shots.iter().map(|s| s.at).collect();
        places.sort();
        places.dedup();
        assert_eq!(places.len(), 16);
        // 80 % power on a fibre source.
        assert_eq!(log.shots[0].current, (0.8f64 * 4095.0).round() as u16);
        assert!(log.time_us >= 16.0 * 3000.0, "the dwell is counted in the run time");
        assert!(!e.laser_enabled() && !e.mo_open(), "all off afterwards");
    });
}
