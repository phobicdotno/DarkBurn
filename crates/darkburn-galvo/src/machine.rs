//! The galvo driver: [`GalvoMachine`].
//!
//! # Threading model
//!
//! [`GalvoMachine::connect`] moves the [`Transport`] into one worker thread and returns at once.
//! The worker owns the link, does the board bring-up and then runs a small state machine (idle,
//! red-light frame, job) one transaction per tick, so a stop is never stuck behind a long
//! blocking call. The UI side never blocks:
//!
//! * requests go to the worker over an `mpsc` channel (every method here); methods validate
//!   against the latest snapshot first and return `Err` straight away when the request is not
//!   allowed; the worker checks again and reports late refusals as [`Event::Rejected`];
//! * events come back over a second channel: call [`GalvoMachine::poll_events`] once per frame;
//! * the latest state sits in a shared [`Snapshot`]: [`GalvoMachine::snapshot`] clones it.
//!
//! # Safety rules (enforced here, tested against the emulator)
//!
//! * The laser is never enabled at connect (unless `enable_laser_at_init` is set in the device
//!   settings). `EnableLaser` is sent only when a job starts, and only after [`GalvoMachine::arm`].
//!   The arm is spent when the job ends, stops or fails.
//! * A red-light frame is compiled without any mark command, is checked again before it is sent,
//!   and runs with the laser disabled and MO closed.
//! * Door switch: no job or frame starts with the door open; if it opens during a run the list
//!   is paused and MO closed until it is closed and the user resumes.
//! * Foot switch: a job can be made to wait for it, and so can repeats and wait steps.
//! * [`GalvoMachine::stop`] (and drop, and any link error) always runs the off sequence:
//!   stop execution, close MO, reset the list, laser signal off, output port cleared, laser
//!   disabled.
//!
//! The speaking protocol (list per step, `SetEndOfList` after each packet, `ExecuteList` after
//! three packets) follows MeerK40t's `balormk` controller (MIT).

use std::collections::VecDeque;
use std::fmt;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::codec::{ListPacket, Reply, SingleCmd, unpacketize};
use crate::cor::CorTable;
use crate::job::{CompiledFrame, CompiledJob, Step};
use crate::settings::DeviceSettings;
use crate::transport::Transport;

/// Connection options.
#[derive(Clone, Debug)]
pub struct ConnectOptions {
    /// Lens table to upload while connecting; `None` selects the blank table.
    pub lens: Option<CorTable>,
    /// How often the input ports are read (door, foot switch).
    pub port_poll_interval: Duration,
    /// Longest wait for one reply.
    pub read_timeout: Duration,
    /// Worker pause between ticks while a job or frame runs.
    pub active_tick: Duration,
}

impl Default for ConnectOptions {
    fn default() -> Self {
        Self {
            lens: None,
            port_poll_interval: Duration::from_millis(50),
            read_timeout: Duration::from_millis(250),
            active_tick: Duration::from_millis(1),
        }
    }
}

/// Options for one job.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StartOptions {
    /// Wait for the foot switch before the first mark ("Start Marking" trigger).
    pub wait_for_foot: bool,
    /// Skip the require-framing check (the UI asks the user first).
    pub skip_framing_check: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum GalvoError {
    NotConnected,
    Busy,
    NotArmed,
    FramingRequired,
    DoorOpen,
    /// A request would break a safety rule.
    Unsafe(String),
    Invalid(String),
}

impl fmt::Display for GalvoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotConnected => write!(f, "The galvo is not connected."),
            Self::Busy => write!(f, "The galvo is busy."),
            Self::NotArmed => write!(f, "Arm the laser before starting a job."),
            Self::FramingRequired => write!(f, "Frame the job with the red light before starting it."),
            Self::DoorOpen => write!(f, "The door is open."),
            Self::Unsafe(m) | Self::Invalid(m) => write!(f, "{m}"),
        }
    }
}
impl std::error::Error for GalvoError {}

#[derive(Clone, Debug, PartialEq, Default)]
pub enum ConnectionState {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    Failed(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Activity {
    #[default]
    Idle,
    Framing,
    Job,
    /// Paused by the user.
    Paused,
    /// Paused because the door opened.
    PausedDoor,
    WaitingDoor,
    WaitingFoot,
}

/// Input port state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Inputs {
    pub raw: u16,
    pub foot_pressed: bool,
    pub door_open: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct JobProgress {
    pub step: usize,
    pub steps: usize,
    pub packets_sent: usize,
    pub packets_total: usize,
    pub elapsed: Duration,
    pub estimate_s: f64,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Snapshot {
    pub connection: ConnectionState,
    pub activity: Activity,
    pub armed: bool,
    pub serial: Option<[u16; 4]>,
    pub version: Option<u16>,
    pub inputs: Inputs,
    pub progress: Option<JobProgress>,
    /// Key of the last completed frame lap ([`CompiledJob::frame_key`]).
    pub framed_key: Option<u64>,
    pub lens_loaded: bool,
    pub transport: String,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Connected { serial: [u16; 4], version: u16 },
    Disconnected(Option<String>),
    JobStarted,
    JobFinished { elapsed: Duration },
    JobStopped,
    JobFailed(String),
    FrameStarted,
    FrameLap,
    FrameStopped,
    WaitingForFoot,
    WaitingForDoor,
    DoorOpened,
    Paused,
    Resumed,
    LensLoaded,
    Rejected(String),
    Console(String),
}

enum Cmd {
    Arm(bool),
    Job(Box<CompiledJob>, StartOptions),
    Frame(Box<CompiledFrame>, bool),
    Utility(Vec<Step>),
    Pause,
    Resume,
    Stop,
    Lens(Option<CorTable>),
    Light(bool),
    Shutdown,
}

/// Handle to the driver.
pub struct GalvoMachine {
    tx: Sender<Cmd>,
    rx: Receiver<Event>,
    snapshot: Arc<Mutex<Snapshot>>,
    armed: Arc<AtomicBool>,
    settings: DeviceSettings,
    worker: Option<JoinHandle<()>>,
}

impl GalvoMachine {
    /// Starts the worker on `transport`. Returns at once; watch [`Self::snapshot`] and the events.
    pub fn connect(transport: Box<dyn Transport>, settings: DeviceSettings, opts: ConnectOptions) -> Self {
        let (tx, crx) = mpsc::channel();
        let (etx, rx) = mpsc::channel();
        let snapshot = Arc::new(Mutex::new(Snapshot { connection: ConnectionState::Connecting, transport: transport.name(), ..Default::default() }));
        let armed = Arc::new(AtomicBool::new(false));
        let worker = Worker {
            link: Link { transport, settings: settings.clone(), opts, port_shadow: 0, last_inputs_read: None },
            state: State::Idle,
            cmds: crx,
            sh: Shared { snapshot: snapshot.clone(), events: etx, armed: armed.clone() },
            pending: VecDeque::new(),
        };
        let handle = std::thread::Builder::new().name("galvo-worker".into()).spawn(move || worker.run()).expect("spawn galvo worker");
        Self { tx, rx, snapshot, armed, settings, worker: Some(handle) }
    }

    pub fn snapshot(&self) -> Snapshot {
        let mut s = self.snapshot.lock().expect("snapshot").clone();
        s.armed = self.armed.load(Ordering::SeqCst);
        s
    }

    /// Drains pending events.
    pub fn poll_events(&self) -> Vec<Event> {
        self.rx.try_iter().collect()
    }

    pub fn settings(&self) -> &DeviceSettings {
        &self.settings
    }

    fn connected(&self) -> Result<Snapshot, GalvoError> {
        let s = self.snapshot();
        if s.connection != ConnectionState::Connected {
            return Err(GalvoError::NotConnected);
        }
        Ok(s)
    }

    fn send(&self, c: Cmd) -> Result<(), GalvoError> {
        self.tx.send(c).map_err(|_| GalvoError::NotConnected)
    }

    /// Arms the laser for the next job. Without this no job starts and `EnableLaser` is never
    /// sent. The arm is spent when that job ends or stops.
    pub fn arm(&self) -> Result<(), GalvoError> {
        self.connected()?;
        self.armed.store(true, Ordering::SeqCst);
        self.send(Cmd::Arm(true))
    }

    pub fn disarm(&self) -> Result<(), GalvoError> {
        self.armed.store(false, Ordering::SeqCst);
        self.send(Cmd::Arm(false))
    }

    /// Starts a job. Requires an arm; stops a running frame first.
    pub fn start_job(&self, job: CompiledJob, opts: StartOptions) -> Result<(), GalvoError> {
        // An axis move the board cannot make would otherwise stop the job half marked.
        for step in &job.steps {
            match step {
                Step::MoveAxis { axis, mm } => drop(axis_words(&self.settings, *axis, *mm).map_err(GalvoError::Invalid)?),
                Step::MoveZ { mm } => drop(z_word(&self.settings, *mm).map_err(GalvoError::Invalid)?),
                _ => {}
            }
        }
        let mut s = self.snapshot.lock().expect("snapshot");
        if s.connection != ConnectionState::Connected {
            return Err(GalvoError::NotConnected);
        }
        if !matches!(s.activity, Activity::Idle | Activity::Framing) {
            return Err(GalvoError::Busy);
        }
        if !self.armed.load(Ordering::SeqCst) {
            return Err(GalvoError::NotArmed);
        }
        if self.settings.require_framing && !opts.skip_framing_check && s.framed_key != Some(job.frame_key) {
            return Err(GalvoError::FramingRequired);
        }
        if self.settings.ports.door_bit.is_some() && s.inputs.door_open {
            return Err(GalvoError::DoorOpen);
        }
        s.activity = Activity::Job;
        s.progress =
            Some(JobProgress { steps: job.steps.len(), packets_total: job.packet_count(), estimate_s: job.estimate.total_s(), ..Default::default() });
        drop(s);
        self.send(Cmd::Job(Box::new(job), opts))
    }

    /// Starts the red-light frame. `repeat` keeps lapping until [`Self::stop`]. No arm needed:
    /// a frame cannot fire the laser.
    pub fn start_frame(&self, frame: CompiledFrame, repeat: bool) -> Result<(), GalvoError> {
        let mut s = self.snapshot.lock().expect("snapshot");
        if s.connection != ConnectionState::Connected {
            return Err(GalvoError::NotConnected);
        }
        if !matches!(s.activity, Activity::Idle | Activity::Framing) {
            return Err(GalvoError::Busy);
        }
        if unpacketize(&frame.packets).iter().any(|c| c.emits()) {
            return Err(GalvoError::Unsafe("A frame must not contain marking commands.".into()));
        }
        if self.settings.ports.door_bit.is_some() && s.inputs.door_open {
            return Err(GalvoError::DoorOpen);
        }
        s.activity = Activity::Framing;
        drop(s);
        self.send(Cmd::Frame(Box::new(frame), repeat))
    }

    pub fn pause(&self) -> Result<(), GalvoError> {
        self.connected()?;
        self.send(Cmd::Pause)
    }

    pub fn resume(&self) -> Result<(), GalvoError> {
        let s = self.connected()?;
        if self.settings.ports.door_bit.is_some() && s.inputs.door_open {
            return Err(GalvoError::DoorOpen);
        }
        self.send(Cmd::Resume)
    }

    /// Stops everything and turns the laser off. Always allowed, also when not connected state
    /// is stale.
    pub fn stop(&self) -> Result<(), GalvoError> {
        self.armed.store(false, Ordering::SeqCst);
        self.send(Cmd::Stop)
    }

    /// Replaces the lens table (`None` = blank table). Only when idle.
    pub fn set_lens(&self, table: Option<CorTable>) -> Result<(), GalvoError> {
        let s = self.connected()?;
        if s.activity != Activity::Idle {
            return Err(GalvoError::Busy);
        }
        self.send(Cmd::Lens(table))
    }

    /// Red pointer on or off at the current galvo position (the "enable red light" toggle).
    pub fn set_light(&self, on: bool) -> Result<(), GalvoError> {
        let s = self.connected()?;
        if s.activity != Activity::Idle {
            return Err(GalvoError::Busy);
        }
        self.send(Cmd::Light(on))
    }

    /// Moves external axis `axis` (0 or 1) to an absolute position in mm. Only when idle.
    pub fn move_axis(&self, axis: u8, mm: f64) -> Result<(), GalvoError> {
        axis_words(&self.settings, axis, mm).map_err(GalvoError::Invalid)?;
        let mut s = self.snapshot.lock().expect("snapshot");
        if s.connection != ConnectionState::Connected {
            return Err(GalvoError::NotConnected);
        }
        if s.activity != Activity::Idle {
            return Err(GalvoError::Busy);
        }
        s.activity = Activity::Job;
        drop(s);
        self.send(Cmd::Utility(vec![Step::MoveAxis { axis, mm }]))
    }

    pub fn disconnect(&mut self) {
        // A start that is still queued in front of the shutdown finds the arm gone.
        self.armed.store(false, Ordering::SeqCst);
        let _ = self.tx.send(Cmd::Shutdown);
        if let Some(h) = self.worker.take() {
            let _ = h.join();
        }
        self.snapshot.lock().expect("snapshot").connection = ConnectionState::Disconnected;
    }
}

impl Drop for GalvoMachine {
    fn drop(&mut self) {
        self.disconnect();
    }
}

/// Position and invert words for an axis move; errors when the 16-bit word cannot hold it.
pub fn axis_words(s: &DeviceSettings, axis: u8, mm: f64) -> Result<(u16, u16), String> {
    let per = *s.axis_steps_per_mm.get(axis as usize).ok_or("Only axes 0 and 1 exist.")?;
    let steps = (mm * per).round();
    if !steps.is_finite() || steps.abs() > 65535.0 {
        return Err(format!("An axis move of {mm} mm needs {steps} steps, more than the board's 16-bit position word holds."));
    }
    Ok((steps.abs() as u16, (steps < 0.0) as u16))
}

/// The Z word of a Z move; errors when the 16-bit word cannot hold it (it used to be cut short).
pub fn z_word(s: &DeviceSettings, mm: f64) -> Result<u16, String> {
    let steps = (mm * s.z_steps_per_mm).round().abs();
    if !steps.is_finite() || steps > 65535.0 {
        return Err(format!("A Z move of {mm} mm needs {steps} steps, more than the board's 16-bit Z word holds."));
    }
    Ok(steps as u16)
}

// ---------------------------------------------------------------------------------------------
// worker

struct Shared {
    snapshot: Arc<Mutex<Snapshot>>,
    events: Sender<Event>,
    armed: Arc<AtomicBool>,
}

impl Shared {
    fn emit(&self, e: Event) {
        let _ = self.events.send(e);
    }
    fn update(&self, f: impl FnOnce(&mut Snapshot)) {
        f(&mut self.snapshot.lock().expect("snapshot"));
    }
}

struct Link {
    transport: Box<dyn Transport>,
    settings: DeviceSettings,
    opts: ConnectOptions,
    port_shadow: u16,
    last_inputs_read: Option<Instant>,
}

impl Link {
    fn single(&mut self, cmd: SingleCmd) -> io::Result<Reply> {
        self.transport.write(&cmd.to_command().to_bytes())?;
        let deadline = Instant::now() + self.opts.read_timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if let Some(b) = self.transport.read(left)? {
                return Reply::from_bytes(&b).map_err(io::Error::other);
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "the board did not answer"));
            }
        }
    }

    /// Writes a command the board does not answer (lens table rows).
    fn send_only(&mut self, cmd: SingleCmd) -> io::Result<()> {
        self.transport.write(&cmd.to_command().to_bytes())
    }

    fn write_port(&mut self, bits: u16) -> io::Result<()> {
        self.port_shadow = bits;
        self.single(SingleCmd::WritePort(bits)).map(|_| ())
    }

    fn inputs(&mut self) -> io::Result<Inputs> {
        let r = self.single(SingleCmd::ReadPort)?;
        let raw = r.words[1];
        self.last_inputs_read = Some(Instant::now());
        Ok(Inputs { raw, foot_pressed: self.settings.ports.foot_pressed(raw), door_open: self.settings.ports.door_open(raw) })
    }

    fn inputs_due(&self) -> bool {
        self.last_inputs_read.is_none_or(|t| t.elapsed() >= self.opts.port_poll_interval)
    }

    /// The off sequence. Best effort: every step is tried even if one fails.
    fn safe_off(&mut self) {
        for c in [
            SingleCmd::StopExecute,
            SingleCmd::FiberSetMo(false),
            SingleCmd::ResetList,
            SingleCmd::LaserSignalOff,
            SingleCmd::WritePort(0),
            SingleCmd::DisableLaser,
        ] {
            let _ = self.single(c);
        }
        self.port_shadow = 0;
    }

    fn mo(&mut self, on: bool) -> io::Result<()> {
        if self.settings.source.is_fiber() {
            self.single(SingleCmd::FiberSetMo(on))?;
        }
        Ok(())
    }
}

/// Progress of sending one list.
struct ListRun {
    idx: usize,
    executing: bool,
}

enum ListState {
    Working,
    Done,
}

/// One tick of list sending / waiting.
fn list_tick(link: &mut Link, packets: &[ListPacket], run: &mut ListRun) -> io::Result<ListState> {
    if run.idx < packets.len() {
        let st = link.single(SingleCmd::GetListStatus)?;
        if st.is_ready() {
            link.transport.write(packets[run.idx].as_bytes())?;
            link.single(SingleCmd::SetEndOfList(0))?;
            run.idx += 1;
            if !run.executing && (run.idx >= 3 || run.idx == packets.len()) {
                link.single(SingleCmd::ExecuteList)?;
                run.executing = true;
            }
        } else if !run.executing {
            link.single(SingleCmd::ExecuteList)?;
            run.executing = true;
        }
        Ok(ListState::Working)
    } else {
        if !run.executing {
            link.single(SingleCmd::ExecuteList)?;
            run.executing = true;
        }
        let st = link.single(SingleCmd::GetListStatus)?;
        Ok(if st.is_busy() { ListState::Working } else { ListState::Done })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pause {
    User,
    Door,
}

enum Phase {
    /// Choose what to do for `steps[step]`.
    Begin,
    WaitFootFirst,
    Prep,
    Send(ListRun),
    AxisWait,
    WaitDoor,
    WaitFoot,
    /// Done-signal pulse until the deadline.
    Done(Option<Instant>),
}

struct JobRun {
    steps: Vec<Step>,
    step: usize,
    phase: Phase,
    pause: Option<Pause>,
    started: Instant,
    packets_sent_before: usize,
    packets_total: usize,
    estimate_s: f64,
    /// A real marking job (armed, laser enabled) rather than an axis move.
    marking: bool,
    wait_foot: bool,
    announced_wait: bool,
}

struct FrameRun {
    frame: CompiledFrame,
    run: Option<ListRun>,
    started: bool,
    repeat: bool,
    pause: Option<Pause>,
}

enum State {
    Idle,
    Frame(Box<FrameRun>),
    Job(Box<JobRun>),
}

struct Worker {
    link: Link,
    state: State,
    cmds: Receiver<Cmd>,
    sh: Shared,
    pending: VecDeque<Cmd>,
}

impl Worker {
    fn run(mut self) {
        // Settings that cannot be right are refused before a single byte is sent.
        if let Err(msg) = self.link.settings.ports.validate() {
            self.sh.update(|s| {
                s.connection = ConnectionState::Failed(msg.clone());
                s.last_error = Some(msg.clone());
            });
            self.sh.emit(Event::Disconnected(Some(msg)));
            return;
        }
        if let Err(e) = self.bring_up() {
            self.link.safe_off();
            let msg = format!("Could not talk to the galvo board: {e}");
            self.sh.update(|s| {
                s.connection = ConnectionState::Failed(msg.clone());
                s.last_error = Some(msg.clone());
            });
            self.sh.emit(Event::Disconnected(Some(msg)));
            return;
        }
        loop {
            let wait = if matches!(self.state, State::Idle) { Duration::from_millis(20) } else { self.link.opts.active_tick };
            match self.cmds.recv_timeout(wait) {
                Ok(c) => self.pending.push_back(c),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    self.shutdown();
                    return;
                }
            }
            while let Ok(c) = self.cmds.try_recv() {
                self.pending.push_back(c);
            }
            while let Some(c) = self.pending.pop_front() {
                if matches!(c, Cmd::Shutdown) {
                    self.shutdown();
                    return;
                }
                if let Err(e) = self.handle(c) {
                    self.link_failed(e);
                    return;
                }
            }
            if let Err(e) = self.tick() {
                self.link_failed(e);
                return;
            }
        }
    }

    fn shutdown(&mut self) {
        let was_busy = !matches!(self.state, State::Idle);
        self.link.safe_off();
        self.sh.armed.store(false, Ordering::SeqCst);
        if was_busy {
            self.sh.emit(Event::JobStopped);
        }
        self.sh.update(|s| {
            s.activity = Activity::Idle;
            s.progress = None;
            s.connection = ConnectionState::Disconnected;
        });
        self.sh.emit(Event::Disconnected(None));
    }

    fn link_failed(&mut self, e: io::Error) {
        // try to leave the laser off even though the link is bad
        self.link.safe_off();
        self.sh.armed.store(false, Ordering::SeqCst);
        let msg = format!("The galvo link failed: {e}");
        let was_job = matches!(self.state, State::Job(_));
        self.state = State::Idle;
        self.sh.update(|s| {
            s.connection = ConnectionState::Failed(msg.clone());
            s.activity = Activity::Idle;
            s.progress = None;
            s.last_error = Some(msg.clone());
        });
        if was_job {
            self.sh.emit(Event::JobFailed(msg.clone()));
        }
        self.sh.emit(Event::Disconnected(Some(msg)));
    }

    fn bring_up(&mut self) -> io::Result<()> {
        let init = self.link.settings.init_sequence();
        let mut serial = [0u16; 4];
        let mut version = 0u16;
        for c in init {
            let r = self.link.single(c)?;
            match c {
                SingleCmd::GetSerialNo => serial = r.words,
                SingleCmd::GetVersion => version = r.words[0],
                _ => {}
            }
        }
        let lens = self.link.opts.lens.clone();
        self.upload_lens(lens.as_ref())?;
        let inputs = self.link.inputs()?;
        self.sh.update(|s| {
            s.connection = ConnectionState::Connected;
            s.serial = Some(serial);
            s.version = Some(version);
            s.inputs = inputs;
        });
        self.sh.emit(Event::Connected { serial, version });
        Ok(())
    }

    fn upload_lens(&mut self, table: Option<&CorTable>) -> io::Result<()> {
        let cmds = match table {
            Some(t) => t.upload_commands(),
            None => CorTable::clear_commands(),
        };
        for c in cmds {
            // table rows get no reply from the board
            if matches!(c, SingleCmd::WriteCorLine { .. }) {
                self.link.send_only(c)?;
            } else {
                self.link.single(c)?;
            }
        }
        self.sh.update(|s| s.lens_loaded = table.is_some());
        Ok(())
    }

    fn reject(&mut self, msg: &str) {
        self.sh.emit(Event::Rejected(msg.to_string()));
    }

    fn handle(&mut self, c: Cmd) -> io::Result<()> {
        match c {
            Cmd::Arm(on) => {
                // The handle set the flag when it was asked. Setting it again here would give
                // the arm back after a stop that was asked for in between and is still queued.
                if !on {
                    self.sh.armed.store(false, Ordering::SeqCst);
                }
                if !on && matches!(self.state, State::Idle) {
                    self.link.single(SingleCmd::DisableLaser)?;
                }
            }
            Cmd::Stop => self.do_stop(),
            Cmd::Pause => self.do_pause(Pause::User)?,
            Cmd::Resume => self.do_resume()?,
            Cmd::Lens(t) => {
                if matches!(self.state, State::Idle) {
                    self.upload_lens(t.as_ref())?;
                    self.sh.emit(Event::LensLoaded);
                } else {
                    self.reject("The lens table can only change while idle.");
                }
            }
            Cmd::Light(on) => {
                if matches!(self.state, State::Idle) {
                    let bit = 1u16 << self.link.settings.ports.light_bit;
                    let bits = if on { self.link.port_shadow | bit } else { self.link.port_shadow & !bit };
                    self.link.write_port(bits)?;
                } else {
                    self.reject("The red light can only change while idle.");
                }
            }
            Cmd::Frame(f, repeat) => self.start_frame(*f, repeat)?,
            Cmd::Job(j, o) => self.start_job(*j, o)?,
            Cmd::Utility(steps) => self.start_utility(steps),
            Cmd::Shutdown => {}
        }
        Ok(())
    }

    fn door_open(&self) -> bool {
        self.link.settings.ports.door_bit.is_some() && self.sh.snapshot.lock().expect("snapshot").inputs.door_open
    }

    /// Reads the door now and puts the reading in the snapshot. The snapshot alone can be as old
    /// as the poll interval, and older after a long transaction (a lens table upload); nothing
    /// that can lead to a mark is decided on that.
    fn door_open_now(&mut self) -> io::Result<bool> {
        if self.link.settings.ports.door_bit.is_none() {
            return Ok(false);
        }
        let inputs = self.link.inputs()?;
        self.sh.update(|s| s.inputs = inputs);
        Ok(inputs.door_open)
    }

    fn idle_snapshot(&self) {
        self.sh.update(|s| {
            s.activity = Activity::Idle;
            s.progress = None;
        });
    }

    fn do_stop(&mut self) {
        let was = std::mem::replace(&mut self.state, State::Idle);
        self.link.safe_off();
        self.sh.armed.store(false, Ordering::SeqCst);
        self.idle_snapshot();
        match was {
            State::Job(_) => self.sh.emit(Event::JobStopped),
            State::Frame(_) => self.sh.emit(Event::FrameStopped),
            State::Idle => {}
        }
    }

    fn do_pause(&mut self, why: Pause) -> io::Result<()> {
        let activity = if why == Pause::Door { Activity::PausedDoor } else { Activity::Paused };
        match &mut self.state {
            State::Idle => return Ok(()),
            State::Job(j) => {
                if j.pause.is_some() {
                    return Ok(());
                }
                j.pause = Some(why);
            }
            State::Frame(f) => {
                if f.pause.is_some() {
                    return Ok(());
                }
                f.pause = Some(why);
            }
        }
        // hold the list and make sure nothing can emit
        self.link.single(SingleCmd::StopList)?;
        self.link.single(SingleCmd::FiberSetMo(false))?;
        self.link.single(SingleCmd::LaserSignalOff)?;
        self.sh.update(|s| s.activity = activity);
        self.sh.emit(if why == Pause::Door { Event::DoorOpened } else { Event::Paused });
        Ok(())
    }

    fn do_resume(&mut self) -> io::Result<()> {
        if matches!(self.state, State::Idle) {
            return Ok(());
        }
        if self.door_open_now()? {
            self.reject("The door is open.");
            return Ok(());
        }
        let activity = match &mut self.state {
            State::Idle => return Ok(()),
            State::Job(j) => {
                if j.pause.is_none() {
                    return Ok(());
                }
                let marking = j.marking && matches!(j.phase, Phase::Send(_));
                if marking && !self.open_source_and_wait()? {
                    // A stop or a pause came in while the source settled: still paused.
                    self.link.mo(false)?;
                    return Ok(());
                }
                if let State::Job(j) = &mut self.state {
                    j.pause = None;
                }
                Activity::Job
            }
            State::Frame(f) => {
                if f.pause.take().is_none() {
                    return Ok(());
                }
                Activity::Framing
            }
        };
        self.link.single(SingleCmd::RestartList)?;
        self.sh.update(|s| s.activity = activity);
        self.sh.emit(Event::Resumed);
        Ok(())
    }

    /// Open the fibre source for a resume and give it the wait a job start gives it inside its
    /// list (`open_mo_delay_units`): a restarted list has no such delay in front of it. The list
    /// stays held meanwhile. Returns false when a stop, pause or shutdown came in during the
    /// wait; the caller then leaves the job paused.
    fn open_source_and_wait(&mut self) -> io::Result<bool> {
        self.link.mo(true)?;
        if !self.link.settings.source.is_fiber() {
            return Ok(true);
        }
        let end = Instant::now() + Duration::from_micros(u64::from(self.link.settings.open_mo_delay_units) * 10);
        loop {
            while let Ok(c) = self.cmds.try_recv() {
                self.pending.push_back(c);
            }
            if self.pending.iter().any(|c| matches!(c, Cmd::Stop | Cmd::Pause | Cmd::Shutdown)) {
                return Ok(false);
            }
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(true);
            }
            std::thread::sleep(left.min(Duration::from_millis(1)));
        }
    }

    fn start_utility(&mut self, steps: Vec<Step>) {
        if !matches!(self.state, State::Idle) {
            self.reject("The galvo is busy.");
            return;
        }
        let estimate_s = steps.len() as f64 * crate::job::MOTION_STEP_S;
        self.state = State::Job(Box::new(JobRun {
            steps,
            step: 0,
            phase: Phase::Begin,
            pause: None,
            started: Instant::now(),
            packets_sent_before: 0,
            packets_total: 0,
            estimate_s,
            marking: false,
            wait_foot: false,
            announced_wait: false,
        }));
        self.sh.update(|s| s.activity = Activity::Job);
    }

    fn start_job(&mut self, job: CompiledJob, o: StartOptions) -> io::Result<()> {
        let reject = |w: &mut Self, m: &str| {
            w.idle_snapshot_if_idle();
            w.reject(m);
        };
        if matches!(self.state, State::Job(_)) {
            reject(self, "The galvo is busy.");
            return Ok(());
        }
        if !self.sh.armed.load(Ordering::SeqCst) {
            reject(self, "Arm the laser before starting a job.");
            return Ok(());
        }
        if self.link.settings.require_framing && !o.skip_framing_check && self.sh.snapshot.lock().expect("snapshot").framed_key != Some(job.frame_key)
        {
            reject(self, "Frame the job with the red light before starting it.");
            return Ok(());
        }
        if self.door_open_now()? {
            reject(self, "The door is open.");
            return Ok(());
        }
        if matches!(self.state, State::Frame(_)) {
            self.link.single(SingleCmd::StopExecute)?;
            self.link.single(SingleCmd::ResetList)?;
            self.link.write_port(0)?;
            self.sh.emit(Event::FrameStopped);
        }
        self.link.single(SingleCmd::EnableLaser)?;
        let packets_total = job.packet_count();
        self.state = State::Job(Box::new(JobRun {
            steps: job.steps,
            step: 0,
            phase: Phase::Begin,
            pause: None,
            started: Instant::now(),
            packets_sent_before: 0,
            packets_total,
            estimate_s: job.estimate.total_s(),
            marking: true,
            wait_foot: o.wait_for_foot,
            announced_wait: false,
        }));
        self.sh.update(|s| {
            s.activity = Activity::Job;
            s.progress = Some(JobProgress { steps: 0, packets_total, estimate_s: job.estimate.total_s(), ..Default::default() });
        });
        self.sh.emit(Event::JobStarted);
        Ok(())
    }

    fn idle_snapshot_if_idle(&self) {
        let st = self.sh.snapshot.lock().expect("snapshot").activity;
        if matches!(self.state, State::Idle) && st != Activity::Idle {
            self.idle_snapshot();
        } else if matches!(self.state, State::Frame(_)) {
            self.sh.update(|s| s.activity = Activity::Framing);
        }
    }

    fn start_frame(&mut self, frame: CompiledFrame, repeat: bool) -> io::Result<()> {
        if matches!(self.state, State::Job(_)) {
            self.reject("The galvo is busy.");
            return Ok(());
        }
        if unpacketize(&frame.packets).iter().any(|c| c.emits()) {
            self.idle_snapshot_if_idle();
            self.reject("A frame must not contain marking commands.");
            return Ok(());
        }
        if self.door_open_now()? {
            self.idle_snapshot_if_idle();
            self.reject("The door is open.");
            return Ok(());
        }
        // red light only: laser disabled, MO closed, nothing queued
        self.link.single(SingleCmd::StopExecute)?;
        self.link.single(SingleCmd::DisableLaser)?;
        self.link.single(SingleCmd::LaserSignalOff)?;
        self.link.single(SingleCmd::FiberSetMo(false))?;
        self.link.single(SingleCmd::ResetList)?;
        let light = 1u16 << self.link.settings.ports.light_bit;
        self.link.write_port(light)?;
        self.state = State::Frame(Box::new(FrameRun { frame, run: Some(ListRun { idx: 0, executing: false }), started: true, repeat, pause: None }));
        self.sh.update(|s| s.activity = Activity::Framing);
        self.sh.emit(Event::FrameStarted);
        Ok(())
    }

    fn tick(&mut self) -> io::Result<()> {
        // inputs
        if self.link.inputs_due() {
            let inputs = self.link.inputs()?;
            self.sh.update(|s| s.inputs = inputs);
            let door_matters = !matches!(self.state, State::Idle) && self.link.settings.ports.door_bit.is_some();
            if door_matters && inputs.door_open {
                let marking_phase = match &self.state {
                    State::Job(j) => j.marking && matches!(j.phase, Phase::Send(_) | Phase::Prep),
                    State::Frame(_) => true,
                    State::Idle => false,
                };
                if marking_phase {
                    self.do_pause(Pause::Door)?;
                }
            }
        }
        let mut state = std::mem::replace(&mut self.state, State::Idle);
        let res = match &mut state {
            State::Idle => Ok(false),
            State::Frame(f) => self.frame_tick(f),
            State::Job(j) => self.job_tick(j),
        };
        match res {
            Ok(true) => {
                // finished: state stays Idle
            }
            Ok(false) => self.state = state,
            Err(e) => {
                self.state = state;
                return Err(e);
            }
        }
        Ok(())
    }

    /// Returns `Ok(true)` when the frame is over.
    fn frame_tick(&mut self, f: &mut FrameRun) -> io::Result<bool> {
        if f.pause.is_some() || !f.started {
            return Ok(false);
        }
        let Some(run) = f.run.as_mut() else { return Ok(false) };
        if let ListState::Done = list_tick(&mut self.link, &f.frame.packets, run)? {
            let key = f.frame.key;
            self.sh.update(|s| s.framed_key = Some(key));
            self.sh.emit(Event::FrameLap);
            if f.repeat {
                self.link.single(SingleCmd::ResetList)?;
                f.run = Some(ListRun { idx: 0, executing: false });
            } else {
                self.link.single(SingleCmd::ResetList)?;
                self.link.write_port(0)?;
                self.idle_snapshot();
                self.sh.emit(Event::FrameStopped);
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn progress(&self, j: &JobRun, sent: usize) {
        let p = JobProgress {
            step: j.step,
            steps: j.steps.len(),
            packets_sent: j.packets_sent_before + sent,
            packets_total: j.packets_total,
            elapsed: j.started.elapsed(),
            estimate_s: j.estimate_s,
        };
        self.sh.update(|s| s.progress = Some(p));
    }

    fn next_step(&self, j: &mut JobRun) {
        j.step += 1;
        j.phase = Phase::Begin;
        self.progress(j, 0);
    }

    fn finish_job(&mut self, j: &JobRun) -> io::Result<()> {
        self.link.single(SingleCmd::FiberSetMo(false))?;
        self.link.write_port(0)?;
        if j.marking {
            self.link.single(SingleCmd::DisableLaser)?;
            self.sh.armed.store(false, Ordering::SeqCst);
        }
        Ok(())
    }

    fn end_job(&mut self, j: &JobRun) {
        self.idle_snapshot();
        self.sh.emit(Event::JobFinished { elapsed: j.started.elapsed() });
    }

    /// Returns `Ok(true)` when the job is over.
    fn job_tick(&mut self, j: &mut JobRun) -> io::Result<bool> {
        if j.pause.is_some() {
            return Ok(false);
        }
        match &mut j.phase {
            Phase::Begin => {
                if j.wait_foot {
                    j.wait_foot = false;
                    j.phase = Phase::WaitFootFirst;
                    return Ok(false);
                }
                let kind = match j.steps.get(j.step) {
                    None => None,
                    Some(Step::List(_)) => Some(Step::List(Vec::new())),
                    Some(other) => Some(other.clone()),
                };
                let Some(step) = kind else {
                    // all steps done: done-signal pulse
                    self.finish_job(j)?;
                    let ports = self.link.settings.ports;
                    if let (Some(bit), true) = (ports.done_bit, j.marking) {
                        self.link.write_port(1 << bit)?;
                        j.phase = Phase::Done(Some(Instant::now() + Duration::from_millis(ports.done_ms as u64)));
                        return Ok(false);
                    }
                    self.end_job(j);
                    return Ok(true);
                };
                match step {
                    Step::List(_) => j.phase = Phase::Prep,
                    Step::MoveAxis { axis, mm } => {
                        let (pos, inv) = axis_words(&self.link.settings, axis, mm).map_err(io::Error::other)?;
                        self.link.single(SingleCmd::MoveAxisTo { position: pos, invert: inv })?;
                        j.phase = Phase::AxisWait;
                    }
                    Step::MoveZ { mm } => {
                        let steps = z_word(&self.link.settings, mm).map_err(io::Error::other)?;
                        self.link.single(SingleCmd::EnableZ)?;
                        self.link.single(SingleCmd::SetZData(steps))?;
                        self.next_step(j);
                    }
                    Step::WaitDoorClosed => {
                        j.announced_wait = false;
                        j.phase = Phase::WaitDoor;
                    }
                    Step::WaitFootSwitch => {
                        j.announced_wait = false;
                        j.phase = Phase::WaitFoot;
                    }
                }
            }
            Phase::WaitFootFirst | Phase::WaitFoot => {
                let first = matches!(j.phase, Phase::WaitFootFirst);
                if !j.announced_wait {
                    j.announced_wait = true;
                    // A press from before the wait began does not count: read the switch now.
                    let inputs = self.link.inputs()?;
                    self.sh.update(|s| s.inputs = inputs);
                    self.sh.update(|s| s.activity = Activity::WaitingFoot);
                    self.sh.emit(Event::WaitingForFoot);
                }
                if self.sh.snapshot.lock().expect("snapshot").inputs.foot_pressed {
                    j.announced_wait = false;
                    self.sh.update(|s| s.activity = Activity::Job);
                    if first {
                        j.phase = Phase::Begin;
                    } else {
                        self.next_step(j);
                    }
                }
            }
            Phase::WaitDoor => {
                let open = self.door_open_now()?;
                if !j.announced_wait {
                    j.announced_wait = true;
                    self.sh.update(|s| s.activity = Activity::WaitingDoor);
                    if open {
                        self.sh.emit(Event::WaitingForDoor);
                    }
                }
                if !open {
                    j.announced_wait = false;
                    self.sh.update(|s| s.activity = Activity::Job);
                    self.next_step(j);
                }
            }
            Phase::Prep => {
                // The last look at the door before the source is opened is a fresh one.
                if self.door_open_now()? {
                    return Ok(false);
                }
                self.link.single(SingleCmd::ResetList)?;
                self.link.write_port(1u16 << self.link.settings.ports.mark_bit)?;
                self.link.mo(true)?;
                j.phase = Phase::Send(ListRun { idx: 0, executing: false });
            }
            Phase::Send(run) => {
                let Step::List(packets) = &j.steps[j.step] else { unreachable!("Send phase only follows a list step") };
                let sent_before = run.idx;
                let st = list_tick(&mut self.link, packets, run)?;
                let sent = run.idx;
                if sent != sent_before {
                    let p = JobProgress {
                        step: j.step,
                        steps: j.steps.len(),
                        packets_sent: j.packets_sent_before + sent,
                        packets_total: j.packets_total,
                        elapsed: j.started.elapsed(),
                        estimate_s: j.estimate_s,
                    };
                    self.sh.update(|s| s.progress = Some(p));
                }
                if let ListState::Done = st {
                    let n = packets.len();
                    self.link.mo(false)?;
                    self.link.write_port(0)?;
                    j.packets_sent_before += n;
                    self.next_step(j);
                }
            }
            Phase::AxisWait => {
                let st = self.link.single(SingleCmd::GetListStatus)?;
                if !st.axis_moving() {
                    self.next_step(j);
                }
            }
            Phase::Done(deadline) => {
                if deadline.is_none_or(|d| Instant::now() >= d) {
                    self.link.write_port(0)?;
                    self.end_job(j);
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
}
