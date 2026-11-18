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

/// Sits between the driver and the emulator. Once tripped, the next input read (`ReadPort`)
/// stops before it reaches the board: it says so on `entered` and goes on when `release` fires.
/// This holds the worker exactly inside the door read of a job start, after its first look at
/// the arm, without any timing.
/// Tripped through `reply_tripped` instead, it holds the answer to the next `EnableLaser` the same
/// way: the word has reached the board and the worker waits for the reply.
struct Gate {
    inner: MemoryTransport,
    tripped: std::sync::Arc<std::sync::atomic::AtomicBool>,
    reply_tripped: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Holds the next EnableLaser write itself: the worker has looked at the arm and is inside
    /// the transport write.
    write_tripped: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// EnableLaser writes that have reached the board.
    enables: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    /// Holds the worker at the driver's hold point right before a resume restarts the list.
    restart_tripped: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Holds the next list packet write itself, before it reaches the board.
    packet_tripped: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// List packets that have reached the board.
    packets: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    last_was_enable: bool,
    entered: std::sync::mpsc::Sender<()>,
    release: std::sync::mpsc::Receiver<()>,
}

struct GateCtl {
    tripped: std::sync::Arc<std::sync::atomic::AtomicBool>,
    reply_tripped: std::sync::Arc<std::sync::atomic::AtomicBool>,
    write_tripped: std::sync::Arc<std::sync::atomic::AtomicBool>,
    enables: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    restart_tripped: std::sync::Arc<std::sync::atomic::AtomicBool>,
    packet_tripped: std::sync::Arc<std::sync::atomic::AtomicBool>,
    packets: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    entered: std::sync::mpsc::Receiver<()>,
    release: std::sync::mpsc::Sender<()>,
}

impl Transport for Gate {
    fn write(&mut self, data: &[u8]) -> std::io::Result<()> {
        if data == SingleCmd::ReadPort.to_command().to_bytes() && self.tripped.swap(false, std::sync::atomic::Ordering::SeqCst) {
            let _ = self.entered.send(());
            let _ = self.release.recv();
        }
        self.last_was_enable = data == SingleCmd::EnableLaser.to_command().to_bytes();
        if self.last_was_enable && self.write_tripped.swap(false, std::sync::atomic::Ordering::SeqCst) {
            let _ = self.entered.send(());
            let _ = self.release.recv();
        }
        let is_packet = data.len() == darkburn_galvo::codec::PACKET_LEN;
        if is_packet && self.packet_tripped.swap(false, std::sync::atomic::Ordering::SeqCst) {
            let _ = self.entered.send(());
            let _ = self.release.recv();
        }
        let r = self.inner.write(data);
        if self.last_was_enable {
            self.enables.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        if is_packet {
            self.packets.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        r
    }
    fn read(&mut self, timeout: Duration) -> std::io::Result<Option<[u8; 8]>> {
        if std::mem::take(&mut self.last_was_enable) && self.reply_tripped.swap(false, std::sync::atomic::Ordering::SeqCst) {
            let _ = self.entered.send(());
            let _ = self.release.recv();
        }
        self.inner.read(timeout)
    }
    fn name(&self) -> String {
        self.inner.name()
    }
    fn before_restart_list(&mut self) {
        if self.restart_tripped.swap(false, std::sync::atomic::Ordering::SeqCst) {
            let _ = self.entered.send(());
            let _ = self.release.recv();
        }
    }
}

/// A door-checking rig whose input reads can be held (see [`Gate`]).
fn gated_rig() -> (Rig, GateCtl) {
    gated_rig_with(with_door())
}

fn gated_rig_with(settings: DeviceSettings) -> (Rig, GateCtl) {
    let (t, emu) = MemoryTransport::new(slow_emu());
    let tripped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (etx, erx) = std::sync::mpsc::channel();
    let (rtx, rrx) = std::sync::mpsc::channel();
    let reply_tripped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let write_tripped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let enables = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let restart_tripped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let packet_tripped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let packets = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let gate = Gate {
        inner: t,
        tripped: tripped.clone(),
        reply_tripped: reply_tripped.clone(),
        write_tripped: write_tripped.clone(),
        enables: enables.clone(),
        restart_tripped: restart_tripped.clone(),
        packet_tripped: packet_tripped.clone(),
        packets: packets.clone(),
        last_was_enable: false,
        entered: etx,
        release: rrx,
    };
    let m = GalvoMachine::connect(Box::new(gate), settings, no_routine_poll());
    let mut r = Rig { m, emu, events: Vec::new() };
    assert!(r.wait(|r| r.m.snapshot().connection == ConnectionState::Connected), "did not connect: {:?}", r.m.snapshot());
    (r, GateCtl { tripped, reply_tripped, write_tripped, enables, restart_tripped, packet_tripped, packets, entered: erx, release: rtx })
}

/// Arms, starts, and returns once the worker is inside the start's door read: it has already
/// seen the arm set.
fn start_and_hold_in_door_read(r: &mut Rig, g: &GateCtl) {
    g.tripped.store(true, std::sync::atomic::Ordering::SeqCst);
    r.m.arm().unwrap();
    r.m.start_job(small_job(&with_door()), StartOptions::default()).unwrap();
    g.entered.recv_timeout(Duration::from_secs(10)).expect("the start never read the door");
}

#[test]
fn a_disconnect_that_follows_a_start_at_once_means_the_laser_is_never_enabled() {
    let (mut r, g) = gated_rig();
    start_and_hold_in_door_read(&mut r, &g);
    // The disconnect is asked for while the worker is still reading the door.
    r.m.begin_disconnect();
    g.release.send(()).unwrap();
    r.m.disconnect();
    assert_eq!(r.singles(SingleCmd::EnableLaser), 0, "the laser was enabled after the disconnect had been asked for");
    r.emu(|e| assert!(!e.laser_enabled() && !e.mo_open() && !e.ever_emitted()));
}

#[test]
fn a_stop_during_the_door_read_of_a_start_means_the_laser_is_never_enabled() {
    let (mut r, g) = gated_rig();
    start_and_hold_in_door_read(&mut r, &g);
    r.m.stop().unwrap();
    g.release.send(()).unwrap();
    r.settle();
    assert_eq!(r.singles(SingleCmd::EnableLaser), 0, "the laser was enabled after Stop had been pressed: {:?}", r.events);
    assert!(!r.has(|e| matches!(e, Event::JobStarted)), "{:?}", r.events);
    r.emu(|e| assert!(!e.laser_enabled() && !e.mo_open() && !e.ever_emitted()));
    let s = r.m.snapshot();
    assert!(!s.armed && s.activity == Activity::Idle, "{s:?}");
}

#[test]
fn a_disarm_during_the_door_read_of_a_start_means_the_laser_is_never_enabled() {
    let (mut r, g) = gated_rig();
    start_and_hold_in_door_read(&mut r, &g);
    r.m.disarm().unwrap();
    g.release.send(()).unwrap();
    r.settle();
    assert_eq!(r.singles(SingleCmd::EnableLaser), 0, "the laser was enabled after the disarm: {:?}", r.events);
    assert!(!r.has(|e| matches!(e, Event::JobStarted)), "{:?}", r.events);
    r.emu(|e| assert!(!e.laser_enabled() && !e.mo_open() && !e.ever_emitted()));
    assert_eq!(r.m.snapshot().activity, Activity::Idle);
}

#[test]
fn a_stop_does_not_wait_for_the_board_to_answer_the_laser_on_word() {
    let (mut r, g) = gated_rig();
    g.reply_tripped.store(true, std::sync::atomic::Ordering::SeqCst);
    r.m.arm().unwrap();
    r.m.start_job(small_job(&with_door()), StartOptions::default()).unwrap();
    // The worker has written EnableLaser and waits for an answer that does not come yet.
    g.entered.recv_timeout(Duration::from_secs(10)).expect("the start never sent EnableLaser");
    // If the stop waited on that answer, this lets it go after two seconds; the stop must not need it.
    let release = g.release.clone();
    let watchdog = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(2));
        let _ = release.send(());
    });
    let t0 = Instant::now();
    r.m.stop().unwrap();
    let took = t0.elapsed();
    assert!(!r.m.snapshot().armed, "the arm is gone as soon as stop returns");
    assert!(took < Duration::from_millis(500), "stop waited {took:?} for the board to answer");
    watchdog.join().unwrap();
    // The queued stop turns the laser that was just enabled off again.
    assert!(r.wait(|r| r.emu(|e| !e.laser_enabled() && !e.mo_open()) && r.m.snapshot().activity == Activity::Idle), "{:?}", r.events);
    r.settle();
    r.emu(|e| assert!(!e.laser_enabled() && !e.mo_open() && !e.ever_emitted()));
    let s = r.m.snapshot();
    assert!(!s.armed && s.activity == Activity::Idle, "{s:?}");
}

/// Arms, starts, and returns once the worker has looked at the arm (it was set) and is inside
/// the transport write of EnableLaser, held there. A helper lets the write go after `after`, so a
/// cancel that does not wait for the write returns before it has reached the board.
fn start_and_hold_in_enable_write(r: &mut Rig, g: &GateCtl, after: Duration) -> std::thread::JoinHandle<()> {
    g.write_tripped.store(true, std::sync::atomic::Ordering::SeqCst);
    r.m.arm().unwrap();
    r.m.start_job(small_job(&with_door()), StartOptions::default()).unwrap();
    g.entered.recv_timeout(Duration::from_secs(10)).expect("the start never wrote EnableLaser");
    assert_eq!(g.enables.load(std::sync::atomic::Ordering::SeqCst), 0);
    let release = g.release.clone();
    std::thread::spawn(move || {
        std::thread::sleep(after);
        let _ = release.send(());
    })
}

#[test]
fn a_stop_between_the_arm_check_and_the_laser_on_write_returns_only_after_that_write() {
    let (mut r, g) = gated_rig();
    let helper = start_and_hold_in_enable_write(&mut r, &g, Duration::from_millis(300));
    r.m.stop().unwrap();
    // The write that had passed its check has gone out before stop returned; none comes later.
    let at_return = g.enables.load(std::sync::atomic::Ordering::SeqCst);
    assert_eq!(at_return, 1, "stop returned while a laser-on write that passed its check was still on its way");
    assert!(!r.m.snapshot().armed);
    helper.join().unwrap();
    // The stop queued behind that write turns the laser off again.
    assert!(r.wait(|r| r.emu(|e| !e.laser_enabled() && !e.mo_open()) && r.m.snapshot().activity == Activity::Idle), "{:?}", r.events);
    r.settle();
    assert_eq!(g.enables.load(std::sync::atomic::Ordering::SeqCst), at_return, "EnableLaser was written after stop returned");
    assert_eq!(r.singles(SingleCmd::EnableLaser), at_return);
    r.emu(|e| assert!(!e.laser_enabled() && !e.mo_open() && !e.ever_emitted()));
}

#[test]
fn a_disconnect_between_the_arm_check_and_the_laser_on_write_returns_only_after_that_write() {
    let (mut r, g) = gated_rig();
    let helper = start_and_hold_in_enable_write(&mut r, &g, Duration::from_millis(300));
    r.m.begin_disconnect();
    let at_return = g.enables.load(std::sync::atomic::Ordering::SeqCst);
    assert_eq!(at_return, 1, "begin_disconnect returned while a laser-on write that passed its check was still on its way");
    helper.join().unwrap();
    r.m.disconnect();
    assert_eq!(g.enables.load(std::sync::atomic::Ordering::SeqCst), at_return, "EnableLaser was written after the disconnect was asked for");
    r.emu(|e| assert!(!e.laser_enabled() && !e.mo_open() && !e.ever_emitted()));
}

#[test]
fn an_arm_after_a_disconnect_was_asked_for_is_refused_and_the_laser_never_enabled() {
    let (mut r, g) = gated_rig();
    start_and_hold_in_door_read(&mut r, &g);
    r.m.begin_disconnect();
    // The snapshot still says Connected until the worker is joined; the arm must not take anyway.
    assert_eq!(r.m.arm(), Err(GalvoError::NotConnected));
    assert!(!r.m.snapshot().armed);
    g.release.send(()).unwrap();
    r.m.disconnect();
    assert_eq!(r.singles(SingleCmd::EnableLaser), 0, "the laser was enabled after the disconnect had been asked for");
    r.emu(|e| assert!(!e.laser_enabled() && !e.mo_open() && !e.ever_emitted()));
}

#[test]
fn a_job_does_not_start_on_an_old_reading_of_a_door_that_has_opened_since() {
    let dev = with_door();
    let mut r = rig_with(dev.clone(), slow_emu(), no_routine_poll());
    r.emu(|e| e.set_input_bit(DOOR_BIT, false)); // the door opens after the last reading
    r.m.arm().unwrap();
    let _ = r.m.start_job(small_job(&dev), StartOptions::default());
    r.settle();
    assert_eq!(r.singles(SingleCmd::EnableLaser), 0, "the laser was enabled with the door open: {:?}", r.events);
    r.emu(|e| assert!(!e.ever_emitted() && !e.mo_open()));
    assert!(r.has(|e| matches!(e, Event::Rejected(m) if m.contains("door"))), "{:?}", r.events);
    assert_eq!(r.m.snapshot().activity, Activity::Idle);
    assert!(r.m.snapshot().inputs.door_open, "the refusal leaves the fresh reading in the snapshot");
}

#[test]
fn a_frame_does_not_start_on_an_old_reading_of_a_door_that_has_opened_since() {
    let dev = with_door();
    let mut r = rig_with(dev.clone(), slow_emu(), no_routine_poll());
    r.emu(|e| e.set_input_bit(DOOR_BIT, false));
    let _ = r.m.start_frame(frame(&dev), true);
    r.settle();
    assert!(!r.has(|e| matches!(e, Event::FrameStarted)), "{:?}", r.events);
    r.emu(|e| assert!(e.log().motions.is_empty() && e.log().light_on_writes == 0));
    assert!(r.has(|e| matches!(e, Event::Rejected(m) if m.contains("door"))), "{:?}", r.events);
    assert_eq!(r.m.snapshot().activity, Activity::Idle);
}

#[test]
fn a_paused_job_does_not_resume_on_an_old_reading_of_a_door_that_has_opened_since() {
    let dev = with_door();
    let mut r = rig_with(dev.clone(), slow_emu(), no_routine_poll());
    r.m.arm().unwrap();
    r.m.start_job(long_job(&dev), StartOptions::default()).unwrap();
    assert!(r.wait(|r| r.emu(|e| e.emitting_marks() > 5)), "{:?}", r.events);
    r.m.pause().unwrap();
    assert!(r.wait(|r| r.has(|e| matches!(e, Event::Paused))), "{:?}", r.events);
    let marks = r.emu(|e| {
        e.set_input_bit(DOOR_BIT, false); // the door opens while the job is paused
        e.emitting_marks()
    });
    let _ = r.m.resume();
    r.settle();
    assert!(!r.has(|e| matches!(e, Event::Resumed)), "the job went on with the door open: {:?}", r.events);
    r.emu(|e| {
        assert_eq!(e.emitting_marks(), marks);
        assert!(!e.mo_open() && e.is_paused());
    });
    assert!(r.has(|e| matches!(e, Event::Rejected(m) if m.contains("door"))), "{:?}", r.events);
    assert_eq!(r.m.snapshot().activity, Activity::Paused);
    assert!(r.m.snapshot().inputs.door_open, "the refusal leaves the fresh reading in the snapshot");
    r.m.stop().unwrap();
    assert!(r.wait(|r| r.has(|e| matches!(e, Event::JobStopped))), "{:?}", r.events);
}

#[test]
fn the_next_list_of_a_job_is_not_opened_on_an_old_reading_of_the_door() {
    let dev = with_door();
    let emu = GalvoEmulator::new(EmulatorConfig { cmds_per_poll: 2, axis_polls: 150, ..Default::default() });
    let mut r = rig_with(dev.clone(), emu, no_routine_poll());
    let mut job = small_job(&dev);
    let marks = job.list_commands().iter().filter(|c| matches!(c, ListCmd::MarkTo { .. })).count();
    job.steps.insert(0, Step::MoveAxis { axis: 0, mm: 1.0 });
    r.m.arm().unwrap();
    r.m.start_job(job, StartOptions::default()).unwrap();
    // The door opens while the axis is still moving, before the list is prepared.
    assert!(r.wait(|r| r.emu(|e| e.log().singles.iter().any(|s| matches!(s, SingleCmd::MoveAxisTo { .. })))), "{:?}", r.events);
    let reads = r.emu(|e| {
        e.set_input_bit(DOOR_BIT, false);
        e.log().singles.iter().filter(|s| **s == SingleCmd::ReadPort).count()
    });
    // Either the driver looks at the door again before the list (and waits), or it marks.
    assert!(r.wait(|r| r.emu(|e| e.ever_emitted() || e.log().singles.iter().filter(|s| **s == SingleCmd::ReadPort).count() >= reads + 5)));
    r.emu(|e| assert!(!e.ever_emitted() && !e.mo_open(), "the list was opened and marked with the door open"));
    r.emu(|e| e.set_input_bit(DOOR_BIT, true));
    assert!(r.wait(|r| r.has(|e| matches!(e, Event::JobFinished { .. }))), "{:?}", r.events);
    r.emu(|e| assert_eq!(e.emitting_marks(), marks));
}

#[test]
fn a_job_that_waits_for_the_foot_switch_does_not_start_on_an_old_press() {
    let dev = DeviceSettings::default();
    let mut emu = slow_emu();
    emu.set_input_bit(15, false); // pressed while connecting (active low)
    let mut r = rig_with(dev.clone(), emu, no_routine_poll());
    assert!(r.m.snapshot().inputs.foot_pressed);
    r.emu(|e| e.set_input_bit(15, true)); // let go before the job is started
    r.m.arm().unwrap();
    r.m.start_job(small_job(&dev), StartOptions { wait_for_foot: true, ..Default::default() }).unwrap();
    assert!(r.wait(|r| r.has(|e| matches!(e, Event::WaitingForFoot))), "{:?}", r.events);
    r.settle();
    r.emu(|e| assert!(!e.ever_emitted() && !e.mo_open(), "the job started on a press from before it was asked to wait"));
    assert_eq!(r.m.snapshot().activity, Activity::WaitingFoot);
    r.m.stop().unwrap();
    assert!(r.wait(|r| r.has(|e| matches!(e, Event::JobStopped))));
}

#[test]
fn taking_the_arm_back_while_a_job_marks_stops_the_job() {
    let dev = DeviceSettings::default();
    let mut r = rig_with(dev.clone(), slow_emu(), polling());
    let job = long_job(&dev);
    let total = job.list_commands().iter().filter(|c| matches!(c, ListCmd::MarkTo { .. })).count();
    r.m.arm().unwrap();
    r.m.start_job(job, StartOptions::default()).unwrap();
    assert!(r.wait(|r| r.emu(|e| e.emitting_marks() > 5)), "{:?}", r.events);
    r.m.disarm().unwrap();
    assert!(!r.m.snapshot().armed);
    // "Not armed: the laser cannot fire" must be true from here on.
    assert!(r.wait(|r| r.has(|e| matches!(e, Event::JobStopped | Event::JobFinished { .. }))), "{:?}", r.events);
    assert!(r.has(|e| matches!(e, Event::JobStopped)), "the job went on marking without the arm: {:?}", r.events);
    r.emu(|e| {
        assert!(e.emitting_marks() < total);
        assert!(!e.laser_enabled() && !e.mo_open() && !e.laser_signal() && !e.is_executing());
        assert_eq!(e.port_out(), 0);
    });
    assert_eq!(r.m.snapshot().activity, Activity::Idle);
}

#[test]
fn pause_stop_and_resume_in_one_breath_leave_the_job_stopped() {
    let dev = DeviceSettings::default();
    let mut r = rig_with(dev.clone(), slow_emu(), polling());
    r.m.arm().unwrap();
    r.m.start_job(long_job(&dev), StartOptions::default()).unwrap();
    assert!(r.wait(|r| r.emu(|e| e.emitting_marks() > 5)), "{:?}", r.events);
    let marks = {
        let held = r.emu.lock().unwrap();
        std::thread::sleep(Duration::from_millis(80));
        r.m.pause().unwrap();
        r.m.stop().unwrap();
        r.m.resume().unwrap();
        held.emitting_marks()
    };
    assert!(r.wait(|r| r.has(|e| matches!(e, Event::JobStopped))), "{:?}", r.events);
    r.settle();
    assert!(!r.has(|e| matches!(e, Event::Resumed)), "{:?}", r.events);
    r.emu(|e| {
        // at most the commands of the poll the worker was held in ran after the requests
        assert!(e.emitting_marks() <= marks + 2, "{} marks before, {} after", marks, e.emitting_marks());
        assert!(!e.laser_enabled() && !e.mo_open() && !e.laser_signal() && !e.is_executing() && !e.is_paused());
        assert_eq!((e.port_out(), e.queued_commands()), (0, 0));
    });
    let s = r.m.snapshot();
    assert!(!s.armed && s.activity == Activity::Idle && s.progress.is_none(), "{s:?}");
}

#[test]
fn a_frame_and_a_second_job_are_refused_while_a_job_runs() {
    let dev = DeviceSettings::default();
    let mut r = rig_with(dev.clone(), slow_emu(), polling());
    r.m.arm().unwrap();
    {
        // Both starts are asked for before the worker has seen the first.
        let _held = r.emu.lock().unwrap();
        std::thread::sleep(Duration::from_millis(80));
        r.m.start_job(long_job(&dev), StartOptions::default()).unwrap();
        assert_eq!(r.m.start_job(small_job(&dev), StartOptions::default()), Err(GalvoError::Busy));
        assert_eq!(r.m.start_frame(frame(&dev), true), Err(GalvoError::Busy));
        assert_eq!(r.m.move_axis(0, 1.0), Err(GalvoError::Busy));
        assert_eq!(r.m.set_light(true), Err(GalvoError::Busy));
    }
    assert!(r.wait(|r| r.emu(|e| e.emitting_marks() > 5)), "{:?}", r.events);
    assert_eq!(r.m.start_frame(frame(&dev), true), Err(GalvoError::Busy));
    assert_eq!(r.singles(SingleCmd::EnableLaser), 1);
    r.emu(|e| assert_eq!(e.log().light_on_writes, 0));
    r.m.stop().unwrap();
    assert!(r.wait(|r| r.has(|e| matches!(e, Event::JobStopped))));
}

#[test]
fn a_link_that_drops_during_a_start_leaves_nothing_running_and_the_arm_spent() {
    let dev = DeviceSettings::default();
    let mut r = rig_with(dev.clone(), slow_emu(), polling());
    r.m.arm().unwrap();
    {
        let mut held = r.emu.lock().unwrap();
        std::thread::sleep(Duration::from_millis(80));
        r.m.start_job(small_job(&dev), StartOptions::default()).unwrap();
        held.disconnect();
    }
    assert!(r.wait(|r| matches!(r.m.snapshot().connection, ConnectionState::Failed(_))), "{:?}", r.m.snapshot());
    r.settle();
    let s = r.m.snapshot();
    assert!(!s.armed && s.activity == Activity::Idle && s.progress.is_none(), "{s:?}");
    assert!(!r.has(|e| matches!(e, Event::JobStarted)), "{:?}", r.events);
    assert_eq!(r.m.start_job(small_job(&dev), StartOptions::default()), Err(GalvoError::NotConnected));
    assert_eq!(r.m.arm(), Err(GalvoError::NotConnected));
    r.emu(|e| assert!(!e.ever_emitted()));
}

/// The source of a resume needs no settle time here, so the worker looks for a stop once and goes
/// straight on to restart the list. A stop that lands right after that look must still win: the
/// held list is never restarted and the job does not say it resumed.
#[test]
fn a_stop_right_before_a_resumed_list_restarts_means_the_list_never_restarts() {
    let dev = DeviceSettings { open_mo_delay_units: 0, ..with_door() };
    let (mut r, g) = gated_rig_with(dev.clone());
    r.m.arm().unwrap();
    r.m.start_job(long_job(&dev), StartOptions::default()).unwrap();
    assert!(r.wait(|r| r.emu(|e| e.emitting_marks() > 5)), "{:?}", r.events);
    r.m.pause().unwrap();
    assert!(r.wait(|r| r.has(|e| matches!(e, Event::Paused))), "{:?}", r.events);
    g.restart_tripped.store(true, std::sync::atomic::Ordering::SeqCst);
    r.m.resume().unwrap();
    // The worker has made its last look for a stop and is about to restart the list.
    g.entered.recv_timeout(Duration::from_secs(10)).expect("the resume never reached the list restart");
    r.m.stop().unwrap();
    g.release.send(()).unwrap();
    assert!(r.wait(|r| r.has(|e| matches!(e, Event::JobStopped))), "{:?}", r.events);
    r.settle();
    assert_eq!(r.singles(SingleCmd::RestartList), 0, "the list was restarted after Stop had been pressed: {:?}", r.events);
    assert!(!r.has(|e| matches!(e, Event::Resumed)), "{:?}", r.events);
    r.emu(|e| assert!(!e.laser_enabled() && !e.mo_open()));
    let s = r.m.snapshot();
    assert!(!s.armed && s.activity == Activity::Idle, "{s:?}");
}

#[test]
fn a_stop_while_a_marking_packet_is_written_means_no_more_marking_packets_go_out() {
    let dev = DeviceSettings::default();
    let (mut r, g) = gated_rig_with(dev.clone());
    let mut job = Job::new();
    job.layers.push(Layer::new(
        "F",
        LayerMode::Fill(FillSettings { interval_mm: 0.02, ..Default::default() }),
        MarkParams::default(),
        vec![rect(-20.0, -20.0, 40.0, 40.0)],
    ));
    let job = compile(&job, &dev, &CompileOptions::default()).unwrap();
    let total = job.packet_count();
    assert!(total >= 10, "the job needs many list packets, has {total}");
    let packets = |g: &GateCtl| g.packets.load(std::sync::atomic::Ordering::SeqCst);
    r.m.arm().unwrap();
    r.m.start_job(job, StartOptions::default()).unwrap();
    // The list is running and marking before the worker is held in its next packet write.
    assert!(r.wait(|r| packets(&g) >= 4 && r.emu(|e| e.is_executing())), "{:?}", r.events);
    g.packet_tripped.store(true, std::sync::atomic::Ordering::SeqCst);
    g.entered.recv_timeout(Duration::from_secs(10)).expect("the job never wrote another list packet");
    let held_at = packets(&g);
    assert!(held_at < total);
    // The held write has passed its look at the arm. Another thread lets it go only once stop()
    // has cleared the arm (the snapshot reads the arm flag itself), so stop() is then past the
    // clear and in, or about to enter, its wait on the held write: no timing is assumed.
    let release = g.release.clone();
    let m = &r.m;
    let at_return = std::thread::scope(|s| {
        s.spawn(move || {
            let until = std::time::Instant::now() + Duration::from_secs(10);
            while m.snapshot().armed && std::time::Instant::now() < until {
                std::thread::yield_now();
            }
            let _ = release.send(());
        });
        m.stop().unwrap();
        packets(&g)
    });
    assert_eq!(at_return, held_at + 1, "stop returned while a marking packet that passed its check was still on its way");
    assert!(!r.m.snapshot().armed);
    assert!(r.wait(|r| r.has(|e| matches!(e, Event::JobStopped)) && r.m.snapshot().activity == Activity::Idle), "{:?}", r.events);
    r.settle();
    assert_eq!(packets(&g), at_return, "a marking list packet was written after stop returned");
    r.emu(|e| {
        assert!(!e.laser_enabled() && !e.mo_open() && !e.laser_signal() && !e.is_executing());
        assert_eq!(e.port_out(), 0);
    });
}
