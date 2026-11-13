//! Bug hunt pass 10, safety gates as a matrix for the galvo driver: every phase (idle, red-light
//! framing, waiting for the foot switch, waiting for the door, marking, paused, a later repeat)
//! against every gate event (a board that stops answering or stays busy for ever, the door, stop,
//! disconnect, unplug, a second start and a frame). In-memory emulator only.

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use darkburn_galvo::*;
use kurbo::BezPath;

const DOOR_BIT: u8 = 3;
const FOOT_BIT: u8 = 15;

/// A link whose board can be made to stop answering: writes still arrive, replies never do.
struct Muted {
    inner: MemoryTransport,
    mute: Arc<AtomicBool>,
}

impl Transport for Muted {
    fn write(&mut self, data: &[u8]) -> io::Result<()> {
        self.inner.write(data)
    }
    fn read(&mut self, timeout: Duration) -> io::Result<Option<[u8; 8]>> {
        let r = self.inner.read(timeout)?;
        if self.mute.load(Ordering::SeqCst) { Ok(None) } else { Ok(r) }
    }
    fn name(&self) -> String {
        "muted emulator".into()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    Idle,
    Framing,
    WaitFoot,
    WaitDoor,
    Marking,
    Paused,
    Repeat,
}

const PHASES: [Phase; 7] = [Phase::Idle, Phase::Framing, Phase::WaitFoot, Phase::WaitDoor, Phase::Marking, Phase::Paused, Phase::Repeat];

struct Rig {
    m: GalvoMachine,
    emu: EmulatorHandle,
    mute: Arc<AtomicBool>,
    events: Vec<Event>,
    dev: DeviceSettings,
}

fn dev() -> DeviceSettings {
    DeviceSettings { ports: PortConfig { door_bit: Some(DOOR_BIT), ..Default::default() }, ..Default::default() }
}

fn rig_with(cmds_per_poll: usize, poll: Duration) -> Rig {
    let dev = dev();
    let (t, emu) = MemoryTransport::new(GalvoEmulator::new(EmulatorConfig { cmds_per_poll, ..Default::default() }));
    let mute = Arc::new(AtomicBool::new(false));
    let opts = ConnectOptions { port_poll_interval: poll, read_timeout: Duration::from_millis(40), ..Default::default() };
    let m = GalvoMachine::connect(Box::new(Muted { inner: t, mute: mute.clone() }), dev.clone(), opts);
    let mut r = Rig { m, emu, mute, events: Vec::new(), dev };
    r.wait("connect", |r| r.m.snapshot().connection == ConnectionState::Connected);
    r
}

fn rig() -> Rig {
    rig_with(2, Duration::ZERO)
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

fn long_job(dev: &DeviceSettings, repeats: u32) -> CompiledJob {
    let mut job = Job::new();
    job.repeat.count = repeats;
    job.layers.push(Layer::new(
        "F",
        LayerMode::Fill(FillSettings { interval_mm: 0.1, ..Default::default() }),
        MarkParams::default(),
        vec![rect(-20.0, -20.0, 40.0, 40.0)],
    ));
    compile(&job, dev, &CompileOptions::default()).unwrap()
}

fn small_job(dev: &DeviceSettings) -> CompiledJob {
    let mut job = Job::new();
    job.layers.push(Layer::new("L", LayerMode::Line, MarkParams::default(), vec![rect(-10.0, -10.0, 20.0, 20.0)]));
    compile(&job, dev, &CompileOptions::default()).unwrap()
}

fn frame(dev: &DeviceSettings) -> CompiledFrame {
    compile_frame(&FrameSpec::from_rect(kurbo::Rect::new(-10.0, -10.0, 10.0, 10.0)), dev).unwrap()
}

fn go() -> StartOptions {
    StartOptions { skip_framing_check: true, ..Default::default() }
}

fn foot_first() -> StartOptions {
    StartOptions { skip_framing_check: true, wait_for_foot: true }
}

impl Rig {
    fn wait(&mut self, what: &str, mut f: impl FnMut(&mut Rig) -> bool) {
        let end = Instant::now() + Duration::from_secs(10);
        while Instant::now() < end {
            self.events.extend(self.m.poll_events());
            if f(self) {
                return;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        panic!("timed out waiting for {what}: {:?}", self.m.snapshot());
    }

    fn settle(&mut self) {
        std::thread::sleep(Duration::from_millis(150));
        self.events.extend(self.m.poll_events());
    }

    fn emu<R>(&self, f: impl FnOnce(&mut GalvoEmulator) -> R) -> R {
        f(&mut self.emu.lock().unwrap())
    }

    fn activity(&self) -> Activity {
        self.m.snapshot().activity
    }

    fn has(&self, pred: impl Fn(&Event) -> bool) -> bool {
        self.events.iter().any(pred)
    }

    /// Puts the machine in `p`.
    fn enter(&mut self, p: Phase) {
        let dev = self.dev.clone();
        match p {
            Phase::Idle => {}
            Phase::Framing => {
                self.m.start_frame(frame(&dev), true).unwrap();
                self.wait("framing", |r| r.has(|e| matches!(e, Event::FrameStarted)));
            }
            Phase::WaitFoot => {
                self.m.arm().unwrap();
                self.m.start_job(long_job(&dev, 1), foot_first()).unwrap();
                self.wait("the foot wait", |r| r.activity() == Activity::WaitingFoot);
            }
            Phase::WaitDoor => {
                let mut job = long_job(&dev, 1);
                job.steps.insert(0, Step::WaitDoorClosed);
                self.m.arm().unwrap();
                self.m.start_job(job, foot_first()).unwrap();
                self.wait("the foot wait", |r| r.activity() == Activity::WaitingFoot);
                self.emu(|e| e.set_input_bit(DOOR_BIT, false));
                self.emu(|e| e.set_input_bit(FOOT_BIT, false));
                self.wait("the door wait", |r| r.activity() == Activity::WaitingDoor);
                self.emu(|e| e.set_input_bit(FOOT_BIT, true));
            }
            Phase::Marking => {
                self.m.arm().unwrap();
                self.m.start_job(long_job(&dev, 1), go()).unwrap();
                self.wait("marking", |r| r.emu(|e| e.is_executing() && e.laser_enabled()));
            }
            Phase::Paused => {
                self.enter(Phase::Marking);
                self.m.pause().unwrap();
                self.wait("the pause", |r| r.activity() == Activity::Paused);
            }
            Phase::Repeat => {
                self.m.arm().unwrap();
                self.m.start_job(long_job(&dev, 3), go()).unwrap();
                self.wait("the second repeat", |r| {
                    r.m.snapshot().progress.is_some_and(|p| p.step >= 1) && r.emu(|e| e.is_executing() && e.laser_enabled())
                });
            }
        }
        self.settle();
    }

    /// The board can fire nothing: laser disabled, no signal, source closed, list not running,
    /// no output port high.
    fn laser_off(&self, what: &str) {
        self.emu(|e| {
            assert!(
                !e.laser_enabled() && !e.laser_signal() && !e.mo_open() && !e.is_executing() && e.port_out() == 0,
                "{what}: enabled {} signal {} mo {} executing {} port {:#x}",
                e.laser_enabled(),
                e.laser_signal(),
                e.mo_open(),
                e.is_executing(),
                e.port_out()
            )
        });
    }

    /// Nothing claims a job: idle, no progress, no arm.
    fn no_job(&self, what: &str) {
        let s = self.m.snapshot();
        assert!(s.activity == Activity::Idle && s.progress.is_none() && !s.armed, "{what}: {s:?}");
    }

    fn running_a_job(p: Phase) -> bool {
        !matches!(p, Phase::Idle | Phase::Framing)
    }
}

#[test]
fn a_stop_in_every_phase_leaves_the_laser_off_and_no_job() {
    for p in PHASES {
        let mut r = rig();
        r.enter(p);
        let before = r.events.len();
        r.m.stop().unwrap();
        r.wait("idle", |r| r.activity() == Activity::Idle);
        r.settle();
        r.laser_off(&format!("{p:?}"));
        r.no_job(&format!("{p:?}"));
        let stopped_job = r.events[before..].iter().any(|e| matches!(e, Event::JobStopped));
        assert_eq!(stopped_job, Rig::running_a_job(p), "{p:?}: {:?}", &r.events[before..]);
        // nothing comes back on its own
        let marks = r.emu(|e| e.emitting_marks());
        r.settle();
        assert_eq!(r.emu(|e| e.emitting_marks()), marks, "{p:?}: marking went on after the stop");
        assert_eq!(r.activity(), Activity::Idle, "{p:?}");
    }
}

#[test]
fn a_disconnect_in_every_phase_leaves_the_laser_off_and_no_job() {
    for p in PHASES {
        let mut r = rig();
        r.enter(p);
        r.m.disconnect();
        r.events.extend(r.m.poll_events());
        r.laser_off(&format!("{p:?}"));
        r.no_job(&format!("{p:?}"));
        assert_eq!(r.m.snapshot().connection, ConnectionState::Disconnected, "{p:?}");
        assert_eq!(r.m.start_frame(frame(&r.dev), false), Err(GalvoError::NotConnected), "{p:?}");
        assert_eq!(r.m.arm(), Err(GalvoError::NotConnected), "{p:?}");
    }
}

#[test]
fn an_unplug_in_every_phase_leaves_no_job_and_no_arm() {
    for p in PHASES {
        let mut r = rig();
        r.enter(p);
        r.emu(|e| e.disconnect());
        r.wait("the link to fail", |r| matches!(r.m.snapshot().connection, ConnectionState::Failed(_)));
        r.settle();
        r.no_job(&format!("{p:?}"));
        assert_eq!(r.has(|e| matches!(e, Event::JobFailed(_))), Rig::running_a_job(p), "{p:?}: {:?}", r.events);
        r.m.arm().unwrap_err();
        assert!(!r.m.snapshot().armed, "{p:?}");
    }
}

#[test]
fn a_board_that_stops_answering_in_every_phase_ends_the_job_with_the_laser_off() {
    for p in PHASES {
        let mut r = rig();
        r.enter(p);
        r.mute.store(true, Ordering::SeqCst);
        r.wait("the link to fail", |r| matches!(r.m.snapshot().connection, ConnectionState::Failed(_)));
        r.settle();
        r.no_job(&format!("{p:?}"));
        // the off sequence still reaches the board, although the board does not answer it
        r.laser_off(&format!("{p:?}"));
        assert_eq!(r.has(|e| matches!(e, Event::JobFailed(_))), Rig::running_a_job(p), "{p:?}: {:?}", r.events);
    }
}

#[test]
fn a_board_busy_for_ever_shows_the_job_until_a_stop_ends_it() {
    // The board takes the list but never runs a command of it.
    let mut r = rig_with(0, Duration::ZERO);
    r.m.arm().unwrap();
    r.m.start_job(small_job(&r.dev), go()).unwrap();
    r.wait("the board busy", |r| r.emu(|e| e.is_executing()));
    r.settle();
    r.settle();
    assert_eq!(r.activity(), Activity::Job, "a job the board still holds shows as running");
    assert!(r.m.start_job(small_job(&r.dev), go()).is_err());
    assert_eq!(r.m.start_frame(frame(&r.dev), false), Err(GalvoError::Busy));
    r.m.stop().unwrap();
    r.wait("idle", |r| r.activity() == Activity::Idle);
    r.settle();
    r.laser_off("busy for ever");
    r.no_job("busy for ever");
}

#[test]
fn the_door_in_every_phase_holds_whatever_could_mark() {
    for p in PHASES {
        let mut r = rig();
        r.enter(p);
        r.emu(|e| e.set_input_bit(DOOR_BIT, false));
        r.settle();
        r.settle();
        let a = r.activity();
        let want = match p {
            Phase::Idle => Activity::Idle,
            Phase::Framing | Phase::Marking | Phase::Repeat => Activity::PausedDoor,
            Phase::WaitFoot => Activity::WaitingFoot,
            Phase::WaitDoor => Activity::WaitingDoor,
            Phase::Paused => Activity::Paused,
        };
        assert_eq!(a, want, "{p:?}");
        r.emu(|e| {
            assert!(!e.laser_signal() && !e.mo_open(), "{p:?}: the source is open behind an open door");
            assert!(!e.is_executing() || e.is_paused(), "{p:?}: the list runs behind an open door");
        });
        let marks = r.emu(|e| e.emitting_marks());
        // neither a resume nor the foot switch brings the mark back while the door is open
        assert!(r.m.resume().is_err(), "{p:?}");
        r.emu(|e| e.set_input_bit(FOOT_BIT, false));
        r.settle();
        assert_eq!(r.emu(|e| e.emitting_marks()), marks, "{p:?}: marked with the door open");
        r.emu(|e| assert!(!e.laser_signal() && !e.mo_open(), "{p:?}: the source opened behind an open door"));
        // nothing new starts behind the open door either
        assert!(r.m.start_frame(frame(&r.dev), false).is_err(), "{p:?}");
        r.m.arm().unwrap();
        assert!(r.m.start_job(small_job(&r.dev), go()).is_err(), "{p:?}");
        r.m.stop().unwrap();
        r.wait("idle", |r| r.activity() == Activity::Idle);
        r.settle();
        r.laser_off(&format!("{p:?} after the stop"));
    }
}

#[test]
fn a_second_start_or_a_frame_in_every_phase_never_runs_on_top_of_a_job() {
    for p in PHASES {
        for frame_it in [false, true] {
            let mut r = rig();
            r.enter(p);
            let packets = r.emu(|e| e.log().packets_received);
            r.m.arm().unwrap();
            let res = if frame_it { r.m.start_frame(frame(&r.dev), false) } else { r.m.start_job(small_job(&r.dev), go()) };
            r.settle();
            if Rig::running_a_job(p) {
                assert_eq!(res, Err(GalvoError::Busy), "{p:?} frame {frame_it}");
                // the job in hand is still the one shown, and nothing new reached the board
                let a = r.activity();
                assert!(a != Activity::Idle && a != Activity::Framing, "{p:?} frame {frame_it}: {a:?}");
                if matches!(p, Phase::WaitFoot | Phase::WaitDoor | Phase::Paused) {
                    assert_eq!(r.emu(|e| e.log().packets_received), packets, "{p:?} frame {frame_it}");
                }
            } else {
                assert_eq!(res, Ok(()), "{p:?} frame {frame_it}");
            }
            r.m.stop().unwrap();
            r.wait("idle", |r| r.activity() == Activity::Idle);
            r.settle();
            r.laser_off(&format!("{p:?} frame {frame_it}"));
            r.no_job(&format!("{p:?} frame {frame_it}"));
        }
    }
}

/// A frame paused by the user, then a job refused because the door had opened: the frame
/// still stood paused on the board, but the machine showed it as framing.
#[test]
fn a_refused_job_over_a_paused_frame_still_shows_the_pause() {
    let mut r = rig_with(2, Duration::from_secs(3600));
    r.m.start_frame(frame(&r.dev), true).unwrap();
    r.wait("framing", |r| r.has(|e| matches!(e, Event::FrameStarted)));
    r.m.arm().unwrap();
    {
        // The worker is held in its next board transaction while the pause and the start are
        // queued, and the door opens before it reads it again.
        let mut e = r.emu.lock().unwrap();
        r.m.pause().unwrap();
        r.m.start_job(small_job(&r.dev), go()).unwrap();
        e.set_input_bit(DOOR_BIT, false);
    }
    r.wait("the refusal", |r| r.has(|e| matches!(e, Event::Rejected(_))));
    r.settle();
    assert!(r.has(|e| matches!(e, Event::Paused)), "{:?}", r.events);
    assert_eq!(r.activity(), Activity::Paused, "the frame is held on the board");
    assert!(r.emu(|e| e.is_paused()));
    r.m.stop().unwrap();
    r.wait("idle", |r| r.activity() == Activity::Idle);
    r.settle();
    r.laser_off("after the stop");
}
