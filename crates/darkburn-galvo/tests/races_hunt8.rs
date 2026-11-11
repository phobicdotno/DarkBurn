//! Bug hunt pass 8 (races): commands that arrive close together, and decisions made on an old
//! reading of the door or the foot switch. Everything runs against the in-memory emulator.
//!
//! Two tricks make the timing exact without a clock:
//! * holding the emulator's lock stops the worker inside its next board transaction, so several
//!   requests can be queued before it sees any of them;
//! * a very long `port_poll_interval` means the worker reads the inputs only where the code asks
//!   for a fresh reading, so a test can change the door after the last routine poll.

use std::time::{Duration, Instant};

use darkburn_galvo::*;
use kurbo::BezPath;

struct Rig {
    m: GalvoMachine,
    emu: EmulatorHandle,
    events: Vec<Event>,
}

const DOOR_BIT: u8 = 3;

fn polling() -> ConnectOptions {
    ConnectOptions { port_poll_interval: Duration::ZERO, ..Default::default() }
}

/// The inputs are read at connect and then only where the driver asks for a fresh reading.
fn no_routine_poll() -> ConnectOptions {
    ConnectOptions { port_poll_interval: Duration::from_secs(3600), ..Default::default() }
}

fn rig_with(settings: DeviceSettings, emu: GalvoEmulator, o: ConnectOptions) -> Rig {
    let (t, emu) = MemoryTransport::new(emu);
    let m = GalvoMachine::connect(Box::new(t), settings, o);
    let mut r = Rig { m, emu, events: Vec::new() };
    assert!(r.wait(|r| r.m.snapshot().connection == ConnectionState::Connected), "did not connect: {:?}", r.m.snapshot());
    r
}

fn slow_emu() -> GalvoEmulator {
    GalvoEmulator::new(EmulatorConfig { cmds_per_poll: 2, ..Default::default() })
}

impl Rig {
    fn wait_for(&mut self, time: Duration, mut f: impl FnMut(&mut Rig) -> bool) -> bool {
        let end = Instant::now() + time;
        while Instant::now() < end {
            self.events.extend(self.m.poll_events());
            if f(self) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        false
    }

    fn wait(&mut self, f: impl FnMut(&mut Rig) -> bool) -> bool {
        self.wait_for(Duration::from_secs(10), f)
    }

    fn has(&self, pred: impl Fn(&Event) -> bool) -> bool {
        self.events.iter().any(pred)
    }

    fn emu<R>(&self, f: impl FnOnce(&mut GalvoEmulator) -> R) -> R {
        f(&mut self.emu.lock().unwrap())
    }

    fn singles(&self, c: SingleCmd) -> usize {
        self.emu(|e| e.log().singles.iter().filter(|s| **s == c).count())
    }

    /// Lets the worker run until it has answered everything queued so far.
    fn settle(&mut self) {
        std::thread::sleep(Duration::from_millis(150));
        self.events.extend(self.m.poll_events());
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

fn with_door() -> DeviceSettings {
    DeviceSettings { ports: PortConfig { door_bit: Some(DOOR_BIT), ..Default::default() }, ..Default::default() }
}

fn frame(dev: &DeviceSettings) -> CompiledFrame {
    compile_frame(&FrameSpec::from_rect(kurbo::Rect::new(-10.0, -10.0, 10.0, 10.0)), dev).unwrap()
}

#[test]
fn a_stop_that_follows_a_start_at_once_means_the_laser_is_never_enabled() {
    let mut r = rig_with(DeviceSettings::default(), slow_emu(), polling());
    let job = small_job(&DeviceSettings::default());
    {
        // The worker is held in its next input poll while arm, start and stop are queued.
        let _held = r.emu.lock().unwrap();
        std::thread::sleep(Duration::from_millis(80));
        r.m.arm().unwrap();
        r.m.start_job(job, StartOptions::default()).unwrap();
        r.m.stop().unwrap();
    }
    r.settle();
    assert_eq!(r.singles(SingleCmd::EnableLaser), 0, "the laser was enabled after Stop had been pressed: {:?}", r.events);
    assert!(!r.has(|e| matches!(e, Event::JobStarted)), "{:?}", r.events);
    r.emu(|e| assert!(!e.laser_enabled() && !e.mo_open() && !e.ever_emitted()));
    let s = r.m.snapshot();
    assert!(!s.armed && s.activity == Activity::Idle, "{s:?}");
}

#[test]
fn a_disconnect_that_follows_a_start_at_once_means_the_laser_is_never_enabled() {
    let mut r = rig_with(DeviceSettings::default(), slow_emu(), polling());
    let job = small_job(&DeviceSettings::default());
    let handle = r.emu.clone();
    let (held_tx, held_rx) = std::sync::mpsc::channel();
    let (go_tx, go_rx) = std::sync::mpsc::channel::<()>();
    let holder = std::thread::spawn(move || {
        let _held = handle.lock().unwrap();
        held_tx.send(()).unwrap();
        // released a moment after the test has queued its requests
        go_rx.recv().unwrap();
        std::thread::sleep(Duration::from_millis(80));
    });
    held_rx.recv().unwrap();
    std::thread::sleep(Duration::from_millis(80));
    r.m.arm().unwrap();
    r.m.start_job(job, StartOptions::default()).unwrap();
    go_tx.send(()).unwrap();
    r.m.disconnect();
    holder.join().unwrap();
    assert_eq!(r.singles(SingleCmd::EnableLaser), 0, "the laser was enabled after the disconnect had been asked for");
    r.emu(|e| assert!(!e.laser_enabled() && !e.mo_open() && !e.ever_emitted()));
}
