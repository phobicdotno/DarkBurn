//! The GRBL driver: [`GrblMachine`].
//!
//! # Threading model
//!
//! [`GrblMachine::connect`] moves the [`Transport`] into one worker thread and returns at
//! once. The worker owns the link and does all I/O: handshake, status polling (at most 5 Hz),
//! character-counting streaming, stop sequences, the fire timer. The UI side never blocks:
//!
//! * commands go to the worker over an `mpsc` channel (every method on [`GrblMachine`]);
//!   methods validate against the latest snapshot first and return `Err` straight away when
//!   the request is not allowed (alarm, busy, out of bounds, ...); the worker checks again
//!   when it runs the command and reports late refusals as [`Event::Rejected`];
//! * events come back over a second channel: call [`GrblMachine::poll_events`] once per
//!   frame;
//! * the latest state (status, job progress, settings, firmware) sits in a shared
//!   [`Snapshot`] behind a mutex: [`GrblMachine::snapshot`] clones it.
//!
//! Dropping the [`GrblMachine`] (or calling [`GrblMachine::disconnect`]) stops any job, turns
//! the laser off where the controller allows it and joins the worker.
//!
//! # Streaming
//!
//! Character counting as described in the GRBL v1.1 interface docs: the driver remembers the
//! length (plus newline) of every line it has sent and not yet seen `ok`/`error` for, and only
//! sends the next line while that total stays within the controller's RX buffer
//! (`[OPT:...]` size, 128 by default, one byte kept free). `$` commands are sent one at a
//! time with nothing else in flight ("send-response"), preceded by `G4 P0` when motion may
//! still be queued. [`StreamMode::Synchronous`] sends every line that way.
//!
//! # User origin
//!
//! [`OriginStrategy::G10L20`] (default) stores the origin in the G54 work offset with
//! `G10 L20 P1 X0 Y0`; GRBL keeps it in EEPROM, so it survives resets and power cycles and
//! shows in `WCO`. Clearing writes `G10 L2 P1 X0 Y0`. [`OriginStrategy::G92`] uses
//! `G92 X0 Y0` / `G92.1`, which GRBL does not store, so a power cycle clears it.

use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use kurbo::{Point, Rect};

use crate::codes::{SettingInfo, alarm_is_critical, alarm_message, error_message, setting_info};
use crate::protocol::{
    Axes, FirmwareInfo, FirmwareKind, MachineState, RapidOverride, Response, StatusReport, apply_build_info, clean_line, feed_override_bytes,
    fmt_num, is_motion_line, parse_response, power_override_bytes, rt,
};
use crate::transport::Transport;

/// How lines are paced.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum StreamMode {
    /// Fill the controller's RX buffer (fast, keeps the planner busy).
    #[default]
    CharacterCounting,
    /// One line at a time: send, wait for `ok`, send the next. Slower; for flaky links.
    Synchronous,
}

/// How "set origin" is stored. See the module docs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum OriginStrategy {
    #[default]
    G10L20,
    G92,
}

/// Machine travel in machine coordinates (mm), used for jog, move and frame checks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TravelLimits {
    pub xy: Rect,
    pub z: Option<(f64, f64)>,
}

/// Connection options.
#[derive(Clone, Debug)]
pub struct ConnectOptions {
    /// Pulse DTR low then high when connecting (resets Arduino-based boards).
    pub toggle_dtr: bool,
    /// DTR level to hold while connected ("Enable DTR").
    pub dtr: bool,
    /// How long to wait for the start-up banner before sending a soft reset (and again after).
    pub banner_timeout: Duration,
    /// Read `$$` during the handshake.
    pub read_settings: bool,
    pub stream_mode: StreamMode,
    /// Status poll interval; values under 200 ms are raised to 200 ms (5 Hz).
    pub poll_interval: Duration,
    /// S value for 100 % power. `None` uses `$30` (or 1000 when unknown).
    pub s_max: Option<f64>,
    /// Machine travel. `None` derives it from `$130`/`$131`/`$132` as 0..max.
    pub travel: Option<TravelLimits>,
    /// Highest power the fire button and laser-on framing may use, in percent.
    pub max_fire_percent: f64,
    /// Longest the fire button may keep the laser on.
    pub max_fire_duration: Duration,
    pub origin_strategy: OriginStrategy,
    /// Echo every streamed line and its `ok` to the console.
    pub show_all: bool,
}

impl Default for ConnectOptions {
    fn default() -> Self {
        Self {
            toggle_dtr: false,
            dtr: true,
            banner_timeout: Duration::from_millis(2500),
            read_settings: true,
            stream_mode: StreamMode::CharacterCounting,
            poll_interval: Duration::from_millis(200),
            s_max: None,
            travel: None,
            max_fire_percent: 10.0,
            max_fire_duration: Duration::from_secs(10),
            origin_strategy: OriginStrategy::G10L20,
            show_all: false,
        }
    }
}

const MIN_POLL: Duration = Duration::from_millis(200);
const POLL_STALE: Duration = Duration::from_secs(1);
const STOP_STEP_TIMEOUT: Duration = Duration::from_secs(3);
/// How long `disconnect` (and dropping the handle) waits for the worker to finish.
const DISCONNECT_WAIT: Duration = Duration::from_secs(1);
/// How long it then waits for the worker to let the link go once it was told to give up.
const GIVE_UP_WAIT: Duration = Duration::from_millis(500);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(6);

/// Errors returned straight from [`GrblMachine`] methods.
#[derive(Clone, Debug, PartialEq)]
pub enum GrblError {
    NotConnected,
    /// Still doing the handshake.
    NotReady,
    /// A job, frame or stop is running.
    Busy,
    /// The machine is in alarm; home or unlock first.
    Alarm,
    /// The machine must be idle for this.
    NotIdle(MachineState),
    OutOfBounds(String),
    Unsupported(&'static str),
    InvalidArgument(String),
}

impl fmt::Display for GrblError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotConnected => write!(f, "The laser is not connected."),
            Self::NotReady => write!(f, "The laser is still connecting."),
            Self::Busy => write!(f, "The laser is busy with a job."),
            Self::Alarm => write!(f, "The laser is in alarm. Home or unlock it first."),
            Self::NotIdle(s) => write!(f, "The laser must be idle (it is {s})."),
            Self::OutOfBounds(m) => write!(f, "Out of bounds: {m}"),
            Self::Unsupported(m) => write!(f, "Not supported by this controller: {m}"),
            Self::InvalidArgument(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for GrblError {}

/// Connection state shown in the UI.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum ConnectionState {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    Failed(String),
}

/// What the driver is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Activity {
    #[default]
    Idle,
    Job,
    Framing,
    Stopping,
}

/// The kind of streamed program.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobKind {
    Job,
    Frame,
}

/// Laser status for the Laser panel (LASER-01).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaserStatus {
    Disconnected,
    Connecting,
    Ready,
    Busy,
    Paused,
    Framing,
    Alarm,
}

/// Override percentages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Overrides {
    pub feed: u16,
    pub rapid: u16,
    pub power: u16,
}

impl Default for Overrides {
    fn default() -> Self {
        Self { feed: 100, rapid: 100, power: 100 }
    }
}

/// The latest machine status, in millimetres.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct MachineStatus {
    pub state: MachineState,
    pub mpos: Axes,
    pub wpos: Axes,
    pub wco: Axes,
    pub axes: usize,
    /// Current feed (mm/min) and S value.
    pub feed: f64,
    pub spindle: f64,
    /// `Bf:` free planner blocks and RX bytes, when `$10` asks for them.
    pub planner_free: Option<u32>,
    pub rx_free: Option<u32>,
    pub overrides: Overrides,
    /// Triggered input pins (`X`, `Y`, `Z`, `P`, `D`, `H`, `R`, `S`).
    pub pins: String,
    /// Accessory state (`S` M3, `C` M4, `F` flood, `M` mist).
    pub accessories: String,
    /// When the last report arrived.
    pub updated: Option<Instant>,
}

/// Job progress (LASER-02).
#[derive(Clone, Debug, PartialEq)]
pub struct JobProgress {
    pub kind: JobKind,
    /// Source lines (as given to the job) sent so far.
    pub lines_sent: usize,
    /// Source lines answered with `ok` or `error`.
    pub lines_acked: usize,
    pub total_lines: Option<usize>,
    pub elapsed: Duration,
    /// Remaining time from the job's ETA hook, or a linear estimate from line counts.
    pub eta: Option<Duration>,
    pub errors: usize,
}

impl JobProgress {
    /// 0..1 when the total is known.
    pub fn fraction(&self) -> Option<f64> {
        self.total_lines.filter(|t| *t > 0).map(|t| (self.lines_acked as f64 / t as f64).min(1.0))
    }
}

/// A `$` setting with its human label.
#[derive(Clone, Debug, PartialEq)]
pub struct SettingValue {
    pub number: u16,
    pub value: String,
    pub info: Option<&'static SettingInfo>,
}

impl SettingValue {
    pub fn new(number: u16, value: String) -> Self {
        Self { number, value, info: setting_info(number) }
    }
    pub fn label(&self) -> String {
        self.info.map_or_else(|| format!("${}", self.number), |i| i.label.to_owned())
    }
}

/// Everything the UI shows, refreshed by the worker.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub connection: ConnectionState,
    pub port: String,
    pub firmware: Option<FirmwareInfo>,
    pub status: MachineStatus,
    pub activity: Activity,
    pub paused: bool,
    pub job: Option<JobProgress>,
    /// Last `$$` values (raw text).
    pub settings: BTreeMap<u16, String>,
    pub fire_on: bool,
    /// G-code is locked by an alarm until homing or unlock.
    pub alarm_locked: bool,
    pub last_alarm: Option<(u16, String)>,
    /// Bytes sent and not yet acknowledged (character counting).
    pub rx_in_flight: usize,
    pub rx_in_flight_peak: usize,
    pub rx_capacity: usize,
    pub show_all: bool,
    /// A known diode laser recognised from the welcome text or the `$I` reply.
    pub detected: Option<crate::detect::DetectedLaser>,
}

impl Snapshot {
    pub fn laser_status(&self) -> LaserStatus {
        match self.connection {
            ConnectionState::Disconnected | ConnectionState::Failed(_) => return LaserStatus::Disconnected,
            ConnectionState::Connecting => return LaserStatus::Connecting,
            ConnectionState::Connected => {}
        }
        if self.alarm_locked || self.status.state.is_alarm() {
            return LaserStatus::Alarm;
        }
        match self.activity {
            Activity::Framing => LaserStatus::Framing,
            Activity::Job if self.paused => LaserStatus::Paused,
            Activity::Job | Activity::Stopping => LaserStatus::Busy,
            Activity::Idle => match self.status.state {
                MachineState::Hold(_) | MachineState::Door(_) => LaserStatus::Paused,
                MachineState::Run(_) | MachineState::Jog | MachineState::Home => LaserStatus::Busy,
                _ => LaserStatus::Ready,
            },
        }
    }
    fn s_max(&self, opts: &ConnectOptions) -> f64 {
        opts.s_max.or_else(|| self.settings.get(&30).and_then(|v| v.parse().ok())).filter(|v| *v > 0.0).unwrap_or(1000.0)
    }
    fn travel(&self, opts: &ConnectOptions) -> Option<TravelLimits> {
        if opts.travel.is_some() {
            return opts.travel;
        }
        let g = |n: u16| self.settings.get(&n).and_then(|v| v.parse::<f64>().ok());
        let (x, y) = (g(130)?, g(131)?);
        Some(TravelLimits { xy: Rect::new(0.0, 0.0, x, y), z: g(132).filter(|z| *z > 0.0).map(|z| (0.0, z)) })
    }
    fn is_v11(&self) -> bool {
        self.firmware.as_ref().is_none_or(FirmwareInfo::is_v11)
    }
}

/// Direction of a console line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsoleKind {
    Sent,
    Received,
    /// A note from DarkBurn (translations, refusals).
    Note,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConsoleLine {
    pub kind: ConsoleKind,
    pub text: String,
}

/// Something that happened. Drain with [`GrblMachine::poll_events`].
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Connected(FirmwareInfo),
    ConnectFailed(String),
    /// The link closed: `None` when asked to, `Some(reason)` when lost.
    Disconnected(Option<String>),
    StateChanged(MachineState),
    Console(ConsoleLine),
    Alarm {
        code: u16,
        message: String,
    },
    /// An `error:N` for a console, macro or internal line.
    Error {
        code: u16,
        message: String,
        line: String,
    },
    Settings(Vec<SettingValue>),
    SettingWritten {
        number: u16,
        value: String,
        result: Result<(), String>,
    },
    JobStarted(JobKind),
    JobProgress(JobProgress),
    JobFinished(JobProgress),
    JobLineError {
        line_number: usize,
        line: String,
        code: u16,
        message: String,
    },
    /// Job ended early by stop, alarm, reset or link loss.
    JobAborted {
        progress: JobProgress,
        reason: String,
    },
    /// The stop sequence finished; the laser is off.
    Stopped,
    Homed,
    FireOff,
    Rejected(String),
}

/// Where a job should leave the head.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum FinishPosition {
    #[default]
    Stay,
    /// Work origin (0, 0).
    Origin,
    /// A point in machine coordinates.
    Machine(Point),
}

/// Bounds check: refuse a job whose extent (`job`) is not inside `machine` (both in the same
/// coordinates, mm).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoundsCheck {
    pub job: Rect,
    pub machine: Rect,
}

impl BoundsCheck {
    pub fn check(&self) -> Result<(), GrblError> {
        let (j, m) = (self.job, self.machine);
        let eps = 1e-6;
        if j.x0 < m.x0 - eps || j.y0 < m.y0 - eps || j.x1 > m.x1 + eps || j.y1 > m.y1 + eps {
            return Err(GrblError::OutOfBounds(format!(
                "the job spans X {:.1}..{:.1}, Y {:.1}..{:.1} but the machine is X {:.1}..{:.1}, Y {:.1}..{:.1}",
                j.x0, j.x1, j.y0, j.y1, m.x0, m.x1, m.y0, m.y1
            )));
        }
        Ok(())
    }
}

/// Remaining-time estimator called with the current progress.
pub type EtaHook = Box<dyn FnMut(&JobProgress) -> Option<Duration> + Send>;

/// A G-code program to stream.
pub struct Job {
    pub lines: Box<dyn Iterator<Item = String> + Send>,
    pub total_lines: Option<usize>,
    pub bounds: Option<BoundsCheck>,
    pub finish: FinishPosition,
    /// Stop the job (feed hold, reset, laser off) on the first `error:N`.
    pub stop_on_error: bool,
    pub eta: Option<EtaHook>,
}

impl Job {
    pub fn from_lines<I>(lines: I) -> Self
    where
        I: IntoIterator<Item = String>,
        I::IntoIter: Send + 'static,
    {
        Self { lines: Box::new(lines.into_iter()), total_lines: None, bounds: None, finish: FinishPosition::Stay, stop_on_error: true, eta: None }
    }
    /// Convenience for a G-code text (counts the lines).
    pub fn from_text(text: &str) -> Self {
        let lines: Vec<String> = text.lines().map(str::to_owned).collect();
        let n = lines.len();
        let mut j = Self::from_lines(lines);
        j.total_lines = Some(n);
        j
    }
    pub fn with_total(mut self, n: usize) -> Self {
        self.total_lines = Some(n);
        self
    }
    pub fn with_bounds(mut self, b: BoundsCheck) -> Self {
        self.bounds = Some(b);
        self
    }
    pub fn with_finish(mut self, f: FinishPosition) -> Self {
        self.finish = f;
        self
    }
    pub fn with_stop_on_error(mut self, stop: bool) -> Self {
        self.stop_on_error = stop;
        self
    }
    pub fn with_eta(mut self, eta: EtaHook) -> Self {
        self.eta = Some(eta);
        self
    }
}

/// A relative jog.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct JogStep {
    pub dx: f64,
    pub dy: f64,
    pub dz: f64,
    /// mm/min
    pub feed: f64,
}

/// Direction of a continuous jog: -1, 0 or 1 per axis (diagonals allowed).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct JogDirection {
    pub x: i8,
    pub y: i8,
    pub z: i8,
}

/// Which coordinates a [`MoveTarget`] is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Coords {
    #[default]
    Work,
    Machine,
}

/// Move to a position (MOVE-09, MOVE-11).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct MoveTarget {
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub z: Option<f64>,
    pub coords: Coords,
    /// mm/min
    pub feed: f64,
}

/// Framing options.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameOptions {
    /// mm/min
    pub feed: f64,
    /// Laser on at this power (percent) while tracing, limited by `max_fire_percent`.
    pub laser_percent: Option<f64>,
}

impl Default for FrameOptions {
    fn default() -> Self {
        Self { feed: 3000.0, laser_percent: None }
    }
}

/// A `$$` backup that can be saved as text and restored.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct SettingsBackup {
    pub firmware: String,
    pub values: Vec<(u16, String)>,
}

impl SettingsBackup {
    pub fn from_snapshot(s: &Snapshot) -> Self {
        Self {
            firmware: s.firmware.as_ref().map(FirmwareInfo::display_name).unwrap_or_default(),
            values: s.settings.iter().map(|(k, v)| (*k, v.clone())).collect(),
        }
    }
    /// Text with one `$N=value` per line and the setting label as a comment.
    pub fn to_text(&self) -> String {
        let mut out = String::from("; DarkBurn GRBL settings backup\n");
        if !self.firmware.is_empty() {
            out.push_str(&format!("; firmware: {}\n", self.firmware));
        }
        for (n, v) in &self.values {
            match setting_info(*n) {
                Some(i) if i.unit.is_empty() => out.push_str(&format!("${n}={v} ; {}\n", i.label)),
                Some(i) => out.push_str(&format!("${n}={v} ; {} ({})\n", i.label, i.unit)),
                None => out.push_str(&format!("${n}={v}\n")),
            }
        }
        out
    }
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut b = Self::default();
        for (i, raw) in text.lines().enumerate() {
            let t = raw.trim();
            if let Some(fw) = t.strip_prefix("; firmware:") {
                b.firmware = fw.trim().to_owned();
                continue;
            }
            let Some(line) = clean_line(t) else { continue };
            let bad = || format!("line {}: expected $N=value, got '{raw}'", i + 1);
            let (n, v) = line.strip_prefix('$').and_then(|r| r.split_once('=')).ok_or_else(bad)?;
            let n: u16 = n.trim().parse().map_err(|_| bad())?;
            let v = v.trim();
            v.parse::<f64>().map_err(|_| bad())?;
            b.values.push((n, v.to_owned()));
        }
        Ok(b)
    }
}

enum Cmd {
    Line {
        text: String,
        origin: Origin,
    },
    /// `keep_fire`: the lines switch an accessory only, so the fire button stays as it is.
    Lines {
        lines: Vec<String>,
        origin: Origin,
        keep_fire: bool,
    },
    Realtime(Vec<u8>),
    StartJob(Job, JobKind),
    Stop,
    Fire {
        s: f64,
        until: Instant,
    },
    FireOff,
    JogCancel,
    SetShowAll(bool),
    Disconnect,
}

/// Handle to a connected (or connecting) GRBL controller. See the module docs.
pub struct GrblMachine {
    cmds: Sender<Cmd>,
    events: Receiver<Event>,
    shared: Arc<Mutex<Snapshot>>,
    opts: ConnectOptions,
    worker: Option<JoinHandle<()>>,
    /// Set when `disconnect` stops waiting: the worker then resets the controller and lets the
    /// link go at once.
    give_up: Arc<AtomicBool>,
}

impl GrblMachine {
    /// Starts the worker thread and the handshake. Returns at once; watch for
    /// [`Event::Connected`] or [`Event::ConnectFailed`] (or use [`wait_until`](Self::wait_until)).
    pub fn connect(transport: Box<dyn Transport>, mut opts: ConnectOptions) -> Self {
        opts.poll_interval = opts.poll_interval.max(MIN_POLL);
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (ev_tx, ev_rx) = mpsc::channel();
        let snap = Snapshot {
            connection: ConnectionState::Connecting,
            port: transport.name(),
            rx_capacity: 127,
            show_all: opts.show_all,
            ..Default::default()
        };
        let shared = Arc::new(Mutex::new(snap.clone()));
        let give_up = Arc::new(AtomicBool::new(false));
        let mut worker = Worker::new(transport, cmd_rx, ev_tx, shared.clone(), opts.clone(), snap);
        worker.give_up = give_up.clone();
        let handle = std::thread::Builder::new().name("darkburn-grbl".into()).spawn(move || worker.run()).expect("spawn GRBL worker thread");
        Self { cmds: cmd_tx, events: ev_rx, shared, opts, worker: Some(handle), give_up }
    }

    /// Everything that happened since the last call. Never blocks.
    pub fn poll_events(&self) -> Vec<Event> {
        self.events.try_iter().collect()
    }

    /// Waits up to `timeout` for the next event (for tests and command-line tools).
    pub fn next_event(&self, timeout: Duration) -> Option<Event> {
        self.events.recv_timeout(timeout).ok()
    }

    pub fn snapshot(&self) -> Snapshot {
        self.shared.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    pub fn status(&self) -> MachineStatus {
        self.snapshot().status
    }

    /// Polls the snapshot until `pred` holds or `timeout` passes (does not consume events).
    pub fn wait_until(&self, timeout: Duration, pred: impl Fn(&Snapshot) -> bool) -> bool {
        let end = Instant::now() + timeout;
        loop {
            if pred(&self.snapshot()) {
                return true;
            }
            if Instant::now() >= end {
                return false;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    pub fn is_connected(&self) -> bool {
        self.snapshot().connection == ConnectionState::Connected
    }

    /// Stops any job, turns the laser off and closes the link.
    pub fn disconnect(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        let _ = self.cmds.send(Cmd::Disconnect);
        if let Some(h) = self.worker.take() {
            // The worker stops the job and turns the laser off before it lets the link go. With
            // a controller that no longer answers that takes several seconds of waiting, and
            // the caller is usually the UI thread: wait a little, then tell the worker to give
            // up. It sends a reset (the laser goes off with it) and closes the link, so the
            // port is free for the next connection when this returns.
            let wait = |d: Duration| {
                let end = Instant::now() + d;
                while !h.is_finished() && Instant::now() < end {
                    std::thread::sleep(Duration::from_millis(2));
                }
            };
            wait(DISCONNECT_WAIT);
            if !h.is_finished() {
                self.give_up.store(true, Ordering::SeqCst);
                wait(GIVE_UP_WAIT);
            }
            if h.is_finished() {
                let _ = h.join();
            }
        }
    }

    fn send(&self, cmd: Cmd) -> Result<(), GrblError> {
        self.cmds.send(cmd).map_err(|_| GrblError::NotConnected)
    }

    fn ready(&self) -> Result<Snapshot, GrblError> {
        let s = self.snapshot();
        match s.connection {
            ConnectionState::Connected => Ok(s),
            ConnectionState::Connecting => Err(GrblError::NotReady),
            _ => Err(GrblError::NotConnected),
        }
    }

    /// Connected and not running a job, frame or stop.
    fn free(&self) -> Result<Snapshot, GrblError> {
        let s = self.ready()?;
        if s.activity != Activity::Idle {
            return Err(GrblError::Busy);
        }
        Ok(s)
    }

    /// Free, not in alarm and idle (or jogging when `allow_jog`).
    fn motion_ok(&self, allow_jog: bool) -> Result<Snapshot, GrblError> {
        let s = self.free()?;
        if s.alarm_locked || s.status.state.is_alarm() {
            return Err(GrblError::Alarm);
        }
        match s.status.state {
            MachineState::Idle | MachineState::Unknown | MachineState::Check => Ok(s),
            MachineState::Jog if allow_jog => Ok(s),
            ref other => Err(GrblError::NotIdle(other.clone())),
        }
    }

    fn internal(&self, text: String) -> Result<(), GrblError> {
        self.send(Cmd::Line { text, origin: Origin::Internal(Tag::Plain) })
    }

    // ----- jobs -----

    /// Streams a job. Refused in alarm, while busy, when not idle, or when the bounds check
    /// fails.
    pub fn start_job(&self, job: Job) -> Result<(), GrblError> {
        self.motion_ok(false)?;
        if let Some(b) = &job.bounds {
            b.check()?;
        }
        self.send(Cmd::StartJob(job, JobKind::Job))
    }

    /// Feed hold.
    pub fn pause(&self) -> Result<(), GrblError> {
        self.ready()?;
        self.send(Cmd::Realtime(vec![rt::FEED_HOLD]))
    }

    /// Cycle start / resume.
    pub fn resume(&self) -> Result<(), GrblError> {
        self.ready()?;
        self.send(Cmd::Realtime(vec![rt::CYCLE_START]))
    }

    /// Stops everything: feed hold, wait for the machine to stop, soft reset, then `M5`.
    /// Always allowed while connected.
    pub fn stop(&self) -> Result<(), GrblError> {
        self.ready()?;
        self.send(Cmd::Stop)
    }

    /// Soft reset (0x18) right away, without the feed hold first.
    pub fn soft_reset(&self) -> Result<(), GrblError> {
        self.ready()?;
        self.send(Cmd::Realtime(vec![rt::SOFT_RESET]))
    }

    // ----- frame -----

    /// Traces a rectangle (work coordinates) at `opts.feed`, optionally with the laser on at
    /// low power, and returns to the first corner.
    pub fn frame_rect(&self, rect: Rect, opts: FrameOptions) -> Result<(), GrblError> {
        let pts = [Point::new(rect.x0, rect.y0), Point::new(rect.x1, rect.y0), Point::new(rect.x1, rect.y1), Point::new(rect.x0, rect.y1)];
        self.frame_outline(&pts, opts)
    }

    /// Traces a closed outline (e.g. the convex hull of the design).
    pub fn frame_outline(&self, points: &[Point], opts: FrameOptions) -> Result<(), GrblError> {
        // Check the arguments before the machine state, so a bad request is always reported as
        // bad, whatever the machine happens to be doing.
        if points.is_empty() {
            return Err(GrblError::InvalidArgument("Nothing to frame.".into()));
        }
        if !positive(opts.feed) {
            return Err(GrblError::InvalidArgument("Framing speed must be above 0.".into()));
        }
        if points.iter().any(|p| !p.x.is_finite() || !p.y.is_finite()) {
            return Err(GrblError::InvalidArgument("The frame has a position that is not a number.".into()));
        }
        if let Some(pct) = opts.laser_percent
            && !(0.0..=self.opts.max_fire_percent).contains(&pct)
        {
            return Err(GrblError::InvalidArgument(format!("Fire power must be between 0 and {} %.", self.opts.max_fire_percent)));
        }
        let s = self.motion_ok(false)?;
        if let Some(t) = s.travel(&self.opts) {
            let wco = s.status.wco;
            for p in points {
                let m = Point::new(p.x + wco[0], p.y + wco[1]);
                if !contains(t.xy, m) {
                    return Err(GrblError::OutOfBounds(format!("the frame reaches X {:.1}, Y {:.1}, outside the machine", m.x, m.y)));
                }
            }
        }
        let mut lines = vec!["G21 G90".to_owned(), "M5 S0".to_owned()];
        lines.push(format!("G0 X{} Y{}", fmt_num(points[0].x), fmt_num(points[0].y)));
        if let Some(pct) = opts.laser_percent {
            let sv = self.fire_s(&s, pct)?;
            lines.push(format!("M3 S{}", fmt_num(sv)));
        }
        let f = fmt_num(opts.feed);
        for p in points.iter().skip(1).chain(std::iter::once(&points[0])) {
            lines.push(format!("G1 X{} Y{} F{f}", fmt_num(p.x), fmt_num(p.y)));
        }
        lines.push("M5 S0".to_owned());
        let n = lines.len();
        let job = Job::from_lines(lines).with_total(n);
        self.send(Cmd::StartJob(job, JobKind::Frame))
    }

    // ----- motion -----

    /// Relative jog (`$J=G91`), works for X/Y diagonals and Z. Turns the fire button off first.
    pub fn jog(&self, step: JogStep) -> Result<(), GrblError> {
        let s = self.motion_ok(true)?;
        if !positive(step.feed) {
            return Err(GrblError::InvalidArgument("Jog speed must be above 0.".into()));
        }
        if !(step.dx.is_finite() && step.dy.is_finite() && step.dz.is_finite()) {
            return Err(GrblError::InvalidArgument("The jog distance is not a number.".into()));
        }
        if step.dx == 0.0 && step.dy == 0.0 && step.dz == 0.0 {
            return Ok(());
        }
        if let Some(t) = s.travel(&self.opts) {
            let p = s.status.mpos;
            let target = Point::new(p[0] + step.dx, p[1] + step.dy);
            if !contains(t.xy, target) {
                return Err(GrblError::OutOfBounds("the jog goes past the machine's travel".into()));
            }
        }
        let mut axes = String::new();
        for (l, v) in [('X', step.dx), ('Y', step.dy), ('Z', step.dz)] {
            if v != 0.0 {
                axes.push_str(&format!(" {l}{}", fmt_num(v)));
            }
        }
        self.fire_off_if_on(&s)?;
        if s.is_v11() {
            self.send(Cmd::Line { text: format!("$J=G21 G91{axes} F{}", fmt_num(step.feed)), origin: Origin::Internal(Tag::Plain) })
        } else {
            self.send(Cmd::Lines {
                lines: vec!["M5".into(), format!("G21 G91 G1{axes} F{}", fmt_num(step.feed)), "G90".into()],
                origin: Origin::Internal(Tag::Plain),
                keep_fire: false,
            })
        }
    }

    /// Jogs towards the edge of travel until [`jog_cancel`](Self::jog_cancel) (key released).
    pub fn jog_continuous(&self, dir: JogDirection, feed: f64) -> Result<(), GrblError> {
        let s = self.motion_ok(true)?;
        if !s.is_v11() {
            return Err(GrblError::Unsupported("continuous jog needs GRBL 1.1"));
        }
        if !positive(feed) {
            return Err(GrblError::InvalidArgument("Jog speed must be above 0.".into()));
        }
        let t =
            s.travel(&self.opts).ok_or_else(|| GrblError::InvalidArgument("Machine travel is unknown; read the settings or set the size.".into()))?;
        let p = s.status.mpos;
        let room = |d: i8, pos: f64, lo: f64, hi: f64| match d.signum() {
            1 => Some((hi - pos).max(0.0)),
            -1 => Some((pos - lo).max(0.0)),
            _ => None,
        };
        let mut d = f64::INFINITY;
        for r in [room(dir.x, p[0], t.xy.x0, t.xy.x1), room(dir.y, p[1], t.xy.y0, t.xy.y1)].into_iter().flatten() {
            d = d.min(r);
        }
        if dir.z != 0 {
            let (lo, hi) = t.z.unwrap_or((0.0, 0.0));
            if let Some(r) = room(dir.z, p[2], lo, hi) {
                d = d.min(r);
            }
        }
        if !d.is_finite() || d < 0.01 {
            return Ok(());
        }
        let step = JogStep { dx: f64::from(dir.x.signum()) * d, dy: f64::from(dir.y.signum()) * d, dz: f64::from(dir.z.signum()) * d, feed };
        self.jog(step)
    }

    /// Jog cancel (0x85): stops a jog at once and drops queued jogs.
    pub fn jog_cancel(&self) -> Result<(), GrblError> {
        self.ready()?;
        self.send(Cmd::JogCancel)
    }

    /// `$H`. Allowed in alarm.
    pub fn home(&self) -> Result<(), GrblError> {
        self.free()?;
        self.send(Cmd::Line { text: "$H".into(), origin: Origin::Internal(Tag::Home) })
    }

    /// `$X`: clears the alarm lock without homing.
    pub fn unlock(&self) -> Result<(), GrblError> {
        self.free()?;
        self.send(Cmd::Line { text: "$X".into(), origin: Origin::Internal(Tag::Unlock) })
    }

    /// Rapid to the work origin (X0 Y0).
    pub fn go_to_origin(&self) -> Result<(), GrblError> {
        self.motion_ok(false)?;
        self.internal("G21 G90 G0 X0 Y0".into())
    }

    /// Makes the current position the work origin (see [`OriginStrategy`]).
    pub fn set_user_origin(&self) -> Result<(), GrblError> {
        self.motion_ok(false)?;
        self.internal(match self.opts.origin_strategy {
            OriginStrategy::G10L20 => "G10 L20 P1 X0 Y0".into(),
            OriginStrategy::G92 => "G92 X0 Y0".into(),
        })
    }

    /// Puts the work origin back at the machine origin.
    pub fn clear_user_origin(&self) -> Result<(), GrblError> {
        self.motion_ok(false)?;
        self.internal(match self.opts.origin_strategy {
            OriginStrategy::G10L20 => "G10 L2 P1 X0 Y0".into(),
            OriginStrategy::G92 => "G92.1".into(),
        })
    }

    /// Moves to a position (jog motion on GRBL 1.1, so it can be cancelled).
    pub fn move_to(&self, target: MoveTarget) -> Result<(), GrblError> {
        let s = self.motion_ok(false)?;
        if target.x.is_none() && target.y.is_none() && target.z.is_none() {
            return Ok(());
        }
        if !positive(target.feed) {
            return Err(GrblError::InvalidArgument("Move speed must be above 0.".into()));
        }
        if [target.x, target.y, target.z].into_iter().flatten().any(|v| !v.is_finite()) {
            return Err(GrblError::InvalidArgument("The position is not a number.".into()));
        }
        if let Some(t) = s.travel(&self.opts) {
            let off = if target.coords == Coords::Machine { [0.0; 4] } else { s.status.wco };
            let cur = s.status.mpos;
            let m = Point::new(target.x.map_or(cur[0], |x| x + off[0]), target.y.map_or(cur[1], |y| y + off[1]));
            if !contains(t.xy, m) {
                return Err(GrblError::OutOfBounds(format!("X {:.1}, Y {:.1} is outside the machine", m.x, m.y)));
            }
        }
        let mut axes = String::new();
        for (l, v) in [('X', target.x), ('Y', target.y), ('Z', target.z)] {
            if let Some(v) = v {
                axes.push_str(&format!(" {l}{}", fmt_num(v)));
            }
        }
        let g53 = if target.coords == Coords::Machine { " G53" } else { "" };
        self.fire_off_if_on(&s)?;
        if s.is_v11() {
            self.internal(format!("$J=G21 G90{g53}{axes} F{}", fmt_num(target.feed)))
        } else {
            self.internal(format!("G21 G90{g53} G0{axes}"))
        }
    }

    /// Asks for a status report now (still limited to 5 Hz).
    pub fn request_status(&self) -> Result<(), GrblError> {
        self.ready()?;
        self.send(Cmd::Realtime(vec![rt::STATUS]))
    }

    // ----- fire button -----

    fn fire_s(&self, s: &Snapshot, percent: f64) -> Result<f64, GrblError> {
        if !(0.0..=self.opts.max_fire_percent).contains(&percent) {
            return Err(GrblError::InvalidArgument(format!("Fire power must be between 0 and {} %.", self.opts.max_fire_percent)));
        }
        Ok((percent / 100.0 * s.s_max(&self.opts)).round())
    }

    /// Turns the laser on at `percent` power (constant power, M3) for at most `duration`; it
    /// turns off by itself afterwards. Refused above `max_fire_percent` / `max_fire_duration`.
    pub fn fire(&self, percent: f64, duration: Duration) -> Result<(), GrblError> {
        let s = self.motion_ok(false)?;
        let sv = self.fire_s(&s, percent)?;
        if duration.is_zero() || duration > self.opts.max_fire_duration {
            return Err(GrblError::InvalidArgument(format!("Fire time must be between 0 and {} s.", self.opts.max_fire_duration.as_secs_f64())));
        }
        self.send(Cmd::Fire { s: sv, until: Instant::now() + duration })
    }

    /// Turns the fire button off (`M5 S0`).
    pub fn fire_off(&self) -> Result<(), GrblError> {
        self.ready()?;
        self.send(Cmd::FireOff)
    }

    fn fire_off_if_on(&self, s: &Snapshot) -> Result<(), GrblError> {
        if s.fire_on { self.send(Cmd::FireOff) } else { Ok(()) }
    }

    // ----- overrides -----

    fn overrides_ok(&self) -> Result<(), GrblError> {
        let s = self.ready()?;
        if !s.is_v11() {
            return Err(GrblError::Unsupported("overrides need GRBL 1.1"));
        }
        Ok(())
    }

    /// Feed override, 10-200 %.
    pub fn set_feed_override(&self, percent: u16) -> Result<(), GrblError> {
        self.overrides_ok()?;
        self.send(Cmd::Realtime(feed_override_bytes(percent)))
    }

    /// Laser power override, 10-200 %.
    pub fn set_power_override(&self, percent: u16) -> Result<(), GrblError> {
        self.overrides_ok()?;
        self.send(Cmd::Realtime(power_override_bytes(percent)))
    }

    pub fn set_rapid_override(&self, level: RapidOverride) -> Result<(), GrblError> {
        self.overrides_ok()?;
        self.send(Cmd::Realtime(vec![level.byte()]))
    }

    // ----- settings -----

    /// Reads `$$`; answers with [`Event::Settings`].
    pub fn read_settings(&self) -> Result<(), GrblError> {
        self.free()?;
        self.send(Cmd::Line { text: "$$".into(), origin: Origin::Internal(Tag::SettingsRead { handshake: false }) })
    }

    /// Writes one `$N=value`; answers with [`Event::SettingWritten`].
    pub fn write_setting(&self, number: u16, value: &str) -> Result<(), GrblError> {
        self.free()?;
        let v = value.trim();
        if v.parse::<f64>().is_err() {
            return Err(GrblError::InvalidArgument(format!("${number}: '{value}' is not a number.")));
        }
        self.send(Cmd::Line { text: format!("${number}={v}"), origin: Origin::Internal(Tag::SettingWrite(number, v.to_owned())) })
    }

    /// Writes every value of a backup, one at a time, then reads `$$` back.
    pub fn restore_settings(&self, backup: &SettingsBackup) -> Result<(), GrblError> {
        self.free()?;
        for (n, v) in &backup.values {
            self.write_setting(*n, v)?;
        }
        self.read_settings()
    }

    /// The last settings read, as a backup.
    pub fn backup_settings(&self) -> SettingsBackup {
        SettingsBackup::from_snapshot(&self.snapshot())
    }

    // ----- console and macros -----

    /// Sends a console line. `?`, `!` and `~` go out as real-time bytes. Motion lines are
    /// refused while the machine is in alarm.
    pub fn send_console(&self, line: &str) -> Result<(), GrblError> {
        let s = self.ready()?;
        let t = line.trim();
        match t {
            "?" | "!" | "~" => return self.send(Cmd::Realtime(vec![t.as_bytes()[0]])),
            "" => return Ok(()),
            _ => {}
        }
        if s.activity != Activity::Idle {
            return Err(GrblError::Busy);
        }
        if (s.alarm_locked || s.status.state.is_alarm()) && is_motion_line(t) {
            return Err(GrblError::Alarm);
        }
        self.send(Cmd::Line { text: t.to_owned(), origin: Origin::Console })
    }

    /// Runs a multi-line macro, one line at a time (`$` lines wait for the previous `ok`).
    /// Motion lines are dropped at send time if the machine is still in alarm then (so a
    /// macro may start with `$X` or `$H`).
    pub fn run_macro(&self, text: &str) -> Result<(), GrblError> {
        self.free()?;
        let lines: Vec<String> = text.lines().filter_map(clean_line).collect();
        if lines.is_empty() {
            return Ok(());
        }
        self.send(Cmd::Lines { lines, origin: Origin::Macro, keep_fire: false })
    }

    /// Runs the lines that switch an accessory (air assist: `M8`, `M9` or the device's own
    /// commands). Unlike [`run_macro`](Self::run_macro) it leaves the fire button as it is.
    pub fn run_accessory(&self, text: &str) -> Result<(), GrblError> {
        self.free()?;
        let lines: Vec<String> = text.lines().filter_map(clean_line).collect();
        if lines.is_empty() {
            return Ok(());
        }
        self.send(Cmd::Lines { lines, origin: Origin::Macro, keep_fire: true })
    }

    /// Echo every sent line (including job lines) and its `ok`.
    pub fn set_show_all(&self, on: bool) -> Result<(), GrblError> {
        self.send(Cmd::SetShowAll(on))
    }
}

impl Drop for GrblMachine {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// A usable speed: a real number above 0 (`NaN <= 0.0` is false, so a plain comparison lets
/// NaN through).
fn positive(v: f64) -> bool {
    v.is_finite() && v > 0.0
}

fn contains(r: Rect, p: Point) -> bool {
    let eps = 1e-6;
    p.x >= r.x0 - eps && p.x <= r.x1 + eps && p.y >= r.y0 - eps && p.y <= r.y1 + eps
}

// ===================================================================================
// Worker
// ===================================================================================

#[derive(Clone, Debug, PartialEq)]
enum Tag {
    Info,
    SettingsRead { handshake: bool },
    SettingWrite(u16, String),
    Sync,
    Home,
    Unlock,
    Fire,
    Plain,
}

#[derive(Clone, Debug, PartialEq)]
enum Origin {
    Job(usize),
    JobTail { last: bool },
    Console,
    Macro,
    Internal(Tag),
}

#[derive(Clone, Debug)]
struct Outgoing {
    text: String,
    origin: Origin,
    /// Send only with nothing else in flight, and send nothing after it until it is answered.
    sync: bool,
    /// Needs an empty planner (a `$` command): insert `G4 P0` first if motion may be queued.
    needs_idle: bool,
}

impl Outgoing {
    fn new(text: String, origin: Origin) -> Self {
        let dollar = text.starts_with('$') && !text.to_ascii_uppercase().starts_with("$J=");
        Self { text, origin, sync: dollar, needs_idle: dollar }
    }
}

struct Pending {
    len: usize,
    origin: Origin,
    text: String,
}

struct ActiveJob {
    kind: JobKind,
    lines: Box<dyn Iterator<Item = String> + Send>,
    staged: Option<Outgoing>,
    source_index: usize,
    exhausted: bool,
    tail: VecDeque<String>,
    lines_sent: usize,
    lines_acked: usize,
    total: Option<usize>,
    started: Instant,
    errors: usize,
    stop_on_error: bool,
    eta: Option<EtaHook>,
}

impl ActiveJob {
    fn progress(&mut self) -> JobProgress {
        let mut p = JobProgress {
            kind: self.kind,
            lines_sent: self.lines_sent,
            lines_acked: self.lines_acked,
            total_lines: self.total,
            elapsed: self.started.elapsed(),
            eta: None,
            errors: self.errors,
        };
        p.eta = match &mut self.eta {
            Some(f) => f(&p),
            None => match p.total_lines {
                Some(t) if p.lines_acked > 0 => {
                    let left = t.saturating_sub(p.lines_acked) as f64;
                    Some(p.elapsed.mul_f64(left / p.lines_acked as f64))
                }
                _ => None,
            },
        };
        p
    }

    /// Fills `staged` with the next line to send, if any.
    fn stage(&mut self) {
        if self.staged.is_some() {
            return;
        }
        while !self.exhausted {
            match self.lines.next() {
                Some(raw) => {
                    self.source_index += 1;
                    if let Some(text) = clean_line(&raw) {
                        self.staged = Some(Outgoing::new(text, Origin::Job(self.source_index)));
                        return;
                    }
                }
                None => self.exhausted = true,
            }
        }
        if let Some(text) = self.tail.pop_front() {
            let last = self.tail.is_empty();
            self.staged = Some(Outgoing { text, origin: Origin::JobTail { last }, sync: false, needs_idle: false });
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum StopPhase {
    Hold,
    Reset,
    AwaitStatus,
}

struct StopSeq {
    phase: StopPhase,
    since: Instant,
    /// Where the last status report after the feed hold put the machine. GRBL 0.9 does not
    /// say when a hold is complete, so the reset waits until two reports agree.
    held_at: Option<Axes>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    WaitBanner { deadline: Instant, reset_sent: bool },
    Handshake { deadline: Instant },
    Ready,
}

struct Worker {
    t: Box<dyn Transport>,
    cmds: Receiver<Cmd>,
    events: Sender<Event>,
    shared: Arc<Mutex<Snapshot>>,
    opts: ConnectOptions,
    snap: Snapshot,
    rx_line: Vec<u8>,
    phase: Phase,
    in_flight: VecDeque<Pending>,
    in_flight_bytes: usize,
    queue: VecDeque<Outgoing>,
    job: Option<ActiveJob>,
    motion_pending: bool,
    poll_sent: Option<Instant>,
    last_poll: Instant,
    echo_status: bool,
    settings_collect: Vec<SettingValue>,
    stop: Option<StopSeq>,
    fire_until: Option<Instant>,
    last_progress: Instant,
    alarm_count: u64,
    home_alarm_count: u64,
    outbox: RefCell<Vec<Event>>,
    detector: crate::detect::LaserDetector,
    give_up: Arc<AtomicBool>,
}

impl Worker {
    fn new(
        t: Box<dyn Transport>,
        cmds: Receiver<Cmd>,
        events: Sender<Event>,
        shared: Arc<Mutex<Snapshot>>,
        opts: ConnectOptions,
        snap: Snapshot,
    ) -> Self {
        let now = Instant::now();
        Self {
            t,
            cmds,
            events,
            shared,
            phase: Phase::WaitBanner { deadline: now + opts.banner_timeout, reset_sent: false },
            opts,
            snap,
            rx_line: Vec::new(),
            in_flight: VecDeque::new(),
            in_flight_bytes: 0,
            queue: VecDeque::new(),
            job: None,
            motion_pending: false,
            poll_sent: None,
            last_poll: now,
            echo_status: false,
            settings_collect: Vec::new(),
            stop: None,
            fire_until: None,
            last_progress: now,
            alarm_count: 0,
            home_alarm_count: 0,
            outbox: RefCell::new(Vec::new()),
            detector: crate::detect::LaserDetector::default(),
            give_up: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Queues an event; [`publish`](Self::publish) sends it after the snapshot is updated, so
    /// the UI never sees an event before the state it describes.
    fn emit(&self, e: Event) {
        self.outbox.borrow_mut().push(e);
    }

    fn note(&self, text: impl Into<String>) {
        self.emit(Event::Console(ConsoleLine { kind: ConsoleKind::Note, text: text.into() }));
    }

    fn publish(&mut self) {
        self.snap.rx_in_flight = self.in_flight_bytes;
        self.snap.fire_on = self.fire_until.is_some();
        self.snap.job = self.job.as_mut().map(ActiveJob::progress);
        self.snap.activity = if self.stop.is_some() {
            Activity::Stopping
        } else {
            match self.job.as_ref().map(|j| j.kind) {
                Some(JobKind::Job) => Activity::Job,
                Some(JobKind::Frame) => Activity::Framing,
                None => Activity::Idle,
            }
        };
        *self.shared.lock().unwrap_or_else(|p| p.into_inner()) = self.snap.clone();
        for e in self.outbox.borrow_mut().drain(..) {
            let _ = self.events.send(e);
        }
    }

    fn capacity(&self) -> usize {
        self.snap.rx_capacity
    }

    fn write(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.t.write_all(bytes).map_err(|e| e.to_string())
    }

    // ----- main loop -----

    fn run(mut self) {
        if let Err(e) = self.start() {
            self.fail_connect(format!("Could not open the link: {e}"));
            return;
        }
        loop {
            let mut exit = false;
            loop {
                match self.cmds.try_recv() {
                    Ok(Cmd::Disconnect) | Err(TryRecvError::Disconnected) => {
                        exit = true;
                        break;
                    }
                    Ok(cmd) => {
                        if let Err(e) = self.command(cmd) {
                            self.lost(e);
                            return;
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                }
            }
            if exit {
                self.shutdown();
                return;
            }
            match self.iterate() {
                Ok(true) => {}
                Ok(false) => return,
                Err(e) => {
                    self.lost(e);
                    return;
                }
            }
        }
    }

    fn start(&mut self) -> Result<(), String> {
        if self.opts.toggle_dtr {
            self.t.set_dtr(false).map_err(|e| e.to_string())?;
            std::thread::sleep(Duration::from_millis(100));
            self.t.set_dtr(true).map_err(|e| e.to_string())?;
            if !self.opts.dtr {
                self.t.set_dtr(false).map_err(|e| e.to_string())?;
            }
        } else {
            self.t.set_dtr(self.opts.dtr).map_err(|e| e.to_string())?;
        }
        self.phase = Phase::WaitBanner { deadline: Instant::now() + self.opts.banner_timeout, reset_sent: false };
        self.publish();
        Ok(())
    }

    /// One pass: read, handle lines, timers, send. Returns `Ok(false)` when the worker should
    /// end (handshake failed).
    fn iterate(&mut self) -> Result<bool, String> {
        let mut buf = [0u8; 512];
        let n = self.t.read(&mut buf, Duration::from_millis(2)).map_err(|e| e.to_string())?;
        for &b in &buf[..n] {
            if b == b'\n' {
                let line = String::from_utf8_lossy(&self.rx_line).trim_end_matches('\r').to_owned();
                self.rx_line.clear();
                if !line.trim().is_empty() {
                    self.on_line(line.trim())?;
                }
            } else if self.rx_line.len() < 4096 {
                self.rx_line.push(b);
            }
        }
        if !self.tick()? {
            return Ok(false);
        }
        self.pump()?;
        self.publish();
        Ok(true)
    }

    fn fail_connect(&mut self, reason: String) {
        self.snap.connection = ConnectionState::Failed(reason.clone());
        self.emit(Event::ConnectFailed(reason));
        self.publish();
    }

    fn lost(&mut self, reason: String) {
        if let Some(mut j) = self.job.take() {
            self.emit(Event::JobAborted { progress: j.progress(), reason: "Connection lost".into() });
        }
        self.fire_until = None;
        self.stop = None;
        let was_connecting = self.snap.connection == ConnectionState::Connecting;
        self.snap.connection = ConnectionState::Disconnected;
        if was_connecting {
            self.emit(Event::ConnectFailed(reason));
        } else {
            self.emit(Event::Disconnected(Some(reason)));
        }
        self.publish();
    }

    fn shutdown(&mut self) {
        if self.phase == Phase::Ready {
            let busy = self.job.is_some() || self.snap.status.state.is_moving() || self.stop.is_some();
            if busy && self.stop.is_none() {
                let _ = self.begin_stop();
            }
            let gave_up = |w: &Self| w.give_up.load(Ordering::SeqCst);
            let end = Instant::now() + STOP_STEP_TIMEOUT * 2;
            while self.stop.is_some() && Instant::now() < end && !gave_up(self) {
                if self.iterate().is_err() {
                    break;
                }
            }
            if self.stop.is_some() && gave_up(self) {
                // The caller stopped waiting for a controller that does not answer the stop.
                // A reset is the last thing that can still turn the laser off; then the link
                // is let go, so the port is not held for seconds more.
                if !self.snap.alarm_locked {
                    let _ = self.write(&[rt::SOFT_RESET]);
                }
                self.stop = None;
            }
            // Laser off where the controller accepts G-code; an alarm already keeps it off.
            else if !self.snap.alarm_locked && self.in_flight_bytes + 6 <= self.capacity() && self.write(b"M5 S0\n").is_ok() {
                self.in_flight.push_back(Pending { len: 6, origin: Origin::Internal(Tag::Plain), text: "M5 S0".into() });
                self.in_flight_bytes += 6;
                let end = Instant::now() + Duration::from_millis(500);
                while !self.in_flight.is_empty() && Instant::now() < end && !gave_up(self) {
                    if self.iterate().is_err() {
                        break;
                    }
                }
            } else if !self.snap.alarm_locked {
                let _ = self.write(&[rt::SOFT_RESET]);
            }
        }
        self.fire_until = None;
        self.snap.connection = ConnectionState::Disconnected;
        self.emit(Event::Disconnected(None));
        self.publish();
    }

    // ----- commands from the UI -----

    fn reject(&self, why: impl Into<String>) {
        let why = why.into();
        self.note(why.clone());
        self.emit(Event::Rejected(why));
    }

    fn command(&mut self, cmd: Cmd) -> Result<(), String> {
        let busy = self.job.is_some() || self.stop.is_some();
        match cmd {
            Cmd::Line { text, origin } => {
                if busy {
                    self.reject(format!("Busy, not sent: {text}"));
                } else {
                    if origin == Origin::Console {
                        self.emit(Event::Console(ConsoleLine { kind: ConsoleKind::Sent, text: text.clone() }));
                    }
                    self.fire_off_before_command();
                    self.queue.push_back(Outgoing::new(text, origin));
                }
            }
            Cmd::Lines { lines, origin, keep_fire } => {
                if busy {
                    self.reject("Busy, macro not run.");
                } else {
                    if !keep_fire {
                        self.fire_off_before_command();
                    }
                    for text in lines {
                        if origin == Origin::Macro {
                            self.emit(Event::Console(ConsoleLine { kind: ConsoleKind::Sent, text: text.clone() }));
                        }
                        self.queue.push_back(Outgoing::new(text, origin.clone()));
                    }
                }
            }
            Cmd::Realtime(bytes) => {
                if bytes == [rt::STATUS] {
                    self.echo_status = true;
                    self.poll(true)?;
                } else {
                    if bytes.contains(&rt::SOFT_RESET) {
                        self.fire_until = None;
                    }
                    self.write(&bytes)?;
                    if bytes == [rt::FEED_HOLD] && self.job.is_some() {
                        self.snap.paused = true;
                    }
                    if bytes == [rt::CYCLE_START] {
                        self.snap.paused = false;
                    }
                }
            }
            Cmd::StartJob(job, kind) => self.start_job(job, kind)?,
            Cmd::Stop => {
                if self.stop.is_none() {
                    self.begin_stop()?;
                }
            }
            Cmd::Fire { s, until } => {
                if busy || self.snap.alarm_locked {
                    self.reject("The fire button is not available now.");
                } else {
                    self.queue.push_back(Outgoing {
                        text: format!("G1 F1000 M3 S{}", fmt_num(s)),
                        origin: Origin::Internal(Tag::Fire),
                        sync: false,
                        needs_idle: false,
                    });
                    self.fire_until = Some(until);
                }
            }
            Cmd::FireOff => {
                // A job controls the laser itself; never inject M5 between its lines.
                if self.job.is_none() {
                    self.fire_off(false);
                }
            }
            Cmd::JogCancel => {
                self.queue.retain(|o| !o.text.to_ascii_uppercase().starts_with("$J="));
                if self.snap.is_v11() {
                    self.write(&[rt::JOG_CANCEL])?;
                }
            }
            Cmd::SetShowAll(on) => {
                self.opts.show_all = on;
                self.snap.show_all = on;
            }
            Cmd::Disconnect => {}
        }
        Ok(())
    }

    fn fire_off(&mut self, timed_out: bool) {
        if self.fire_until.take().is_some() || !timed_out {
            self.queue.push_front(Outgoing { text: "M5 S0".into(), origin: Origin::Internal(Tag::Plain), sync: false, needs_idle: false });
        }
        if timed_out {
            self.emit(Event::FireOff);
        }
    }

    /// The fire button never stays on while something else runs: a homing cycle or another
    /// command that waits for the machine would hold back the timed `M5` until it is done.
    fn fire_off_before_command(&mut self) {
        if self.fire_until.is_some() {
            self.fire_off(false);
        }
    }

    fn start_job(&mut self, job: Job, kind: JobKind) -> Result<(), String> {
        if self.job.is_some() || self.stop.is_some() {
            self.reject("A job is already running.");
            return Ok(());
        }
        if self.snap.alarm_locked || self.snap.status.state.is_alarm() {
            self.reject("The machine is in alarm; the job was not started.");
            return Ok(());
        }
        if let Some(b) = &job.bounds
            && let Err(e) = b.check()
        {
            self.reject(e.to_string());
            return Ok(());
        }
        if self.fire_until.is_some() {
            self.fire_off(false);
        }
        let mut tail = VecDeque::new();
        tail.push_back("M5 S0".to_owned());
        match job.finish {
            FinishPosition::Stay => {}
            FinishPosition::Origin => tail.push_back("G90 G0 X0 Y0".into()),
            FinishPosition::Machine(p) => tail.push_back(format!("G90 G53 G0 X{} Y{}", fmt_num(p.x), fmt_num(p.y))),
        }
        tail.push_back("G4 P0".into());
        self.snap.paused = false;
        self.job = Some(ActiveJob {
            kind,
            lines: job.lines,
            staged: None,
            source_index: 0,
            exhausted: false,
            tail,
            lines_sent: 0,
            lines_acked: 0,
            total: job.total_lines,
            started: Instant::now(),
            errors: 0,
            stop_on_error: job.stop_on_error,
            eta: job.eta,
        });
        self.last_progress = Instant::now();
        self.emit(Event::JobStarted(kind));
        Ok(())
    }

    // ----- stop sequence -----

    fn begin_stop(&mut self) -> Result<(), String> {
        if let Some(mut j) = self.job.take() {
            self.emit(Event::JobAborted { progress: j.progress(), reason: "Stopped".into() });
        }
        self.queue.clear();
        self.fire_until = None;
        self.snap.paused = false;
        let state = &self.snap.status.state;
        let may_move = state.is_moving() || matches!(state, MachineState::Hold(_)) || self.motion_pending || !self.in_flight.is_empty();
        // A homing cycle ignores feed hold and answers no status requests, so its state here
        // is stale; only a reset stops it.
        let homing = self.in_flight.iter().any(|p| p.origin == Origin::Internal(Tag::Home) || p.text.trim().eq_ignore_ascii_case("$H"));
        let now = Instant::now();
        // Feed hold first on every GRBL version: a reset while the machine moves raises an
        // alarm and loses the position.
        if may_move && !homing && !matches!(state, MachineState::Home | MachineState::Alarm(_)) {
            self.write(&[rt::FEED_HOLD])?;
            self.stop = Some(StopSeq { phase: StopPhase::Hold, since: now, held_at: None });
        } else {
            self.write(&[rt::SOFT_RESET])?;
            self.stop = Some(StopSeq { phase: StopPhase::Reset, since: now, held_at: None });
        }
        self.publish();
        Ok(())
    }

    fn stop_reset(&mut self) -> Result<(), String> {
        self.write(&[rt::SOFT_RESET])?;
        self.stop = Some(StopSeq { phase: StopPhase::Reset, since: Instant::now(), held_at: None });
        Ok(())
    }

    fn stop_after_reset(&mut self) -> Result<(), String> {
        self.stop = Some(StopSeq { phase: StopPhase::AwaitStatus, since: Instant::now(), held_at: None });
        self.poll(true)
    }

    fn finish_stop(&mut self) {
        self.stop = None;
        if !self.snap.alarm_locked {
            self.queue.push_front(Outgoing { text: "M5 S0".into(), origin: Origin::Internal(Tag::Plain), sync: false, needs_idle: false });
        }
        self.emit(Event::Stopped);
    }

    // ----- timers -----

    fn tick(&mut self) -> Result<bool, String> {
        let now = Instant::now();
        match self.phase {
            Phase::WaitBanner { deadline, reset_sent } if now >= deadline => {
                if reset_sent {
                    self.fail_connect("No GRBL start-up message. Check the port, baud rate and DTR setting.".into());
                    return Ok(false);
                }
                self.write(&[rt::SOFT_RESET])?;
                self.phase = Phase::WaitBanner { deadline: now + self.opts.banner_timeout, reset_sent: true };
            }
            Phase::Handshake { deadline } if now >= deadline => {
                self.fail_connect("The controller did not answer $I / $$.".into());
                return Ok(false);
            }
            _ => {}
        }
        if self.phase == Phase::Ready || matches!(self.phase, Phase::Handshake { .. }) {
            let stale = self.poll_sent.is_some_and(|t| now - t > POLL_STALE);
            if stale {
                self.poll_sent = None;
            }
            if self.poll_sent.is_none() && now - self.last_poll >= self.opts.poll_interval {
                self.poll(false)?;
            }
        }
        if let Some(stop) = &self.stop
            && now - stop.since > STOP_STEP_TIMEOUT
        {
            match stop.phase {
                StopPhase::Hold => self.stop_reset()?,
                StopPhase::Reset => {
                    self.note("No reset reply from the controller.");
                    self.stop_after_reset()?;
                }
                StopPhase::AwaitStatus => self.finish_stop(),
            }
        }
        if self.fire_until.is_some_and(|t| now >= t) {
            self.fire_off(true);
        }
        if self.job.is_some() && now - self.last_progress >= Duration::from_millis(100) {
            self.last_progress = now;
            let p = self.job.as_mut().map(ActiveJob::progress);
            if let Some(p) = p {
                self.emit(Event::JobProgress(p));
            }
        }
        Ok(true)
    }

    /// Sends `?`, respecting the 5 Hz limit unless `now` is set for a one-off request that has
    /// no poll outstanding.
    fn poll(&mut self, now: bool) -> Result<(), String> {
        let t = Instant::now();
        if self.poll_sent.is_some() {
            return Ok(());
        }
        if !now && t - self.last_poll < self.opts.poll_interval {
            return Ok(());
        }
        self.write(&[rt::STATUS])?;
        self.poll_sent = Some(t);
        self.last_poll = t;
        Ok(())
    }

    // ----- sending -----

    fn peek(&mut self) -> Option<&Outgoing> {
        if !self.queue.is_empty() {
            return self.queue.front();
        }
        let job = self.job.as_mut()?;
        job.stage();
        job.staged.as_ref()
    }

    fn take(&mut self) -> Option<Outgoing> {
        if let Some(o) = self.queue.pop_front() {
            return Some(o);
        }
        self.job.as_mut()?.staged.take()
    }

    fn pump(&mut self) -> Result<(), String> {
        if matches!(self.phase, Phase::WaitBanner { .. }) {
            return Ok(());
        }
        if self.stop.is_some() {
            return Ok(());
        }
        loop {
            if self.in_flight.iter().any(|p| is_sync_origin(&p.origin, &p.text)) {
                return Ok(());
            }
            let alarm = self.snap.alarm_locked;
            let motion_pending = self.motion_pending;
            let in_flight_empty = self.in_flight.is_empty();
            let sync_mode = self.opts.stream_mode == StreamMode::Synchronous;
            let cap = self.capacity();
            let used = self.in_flight_bytes;
            let Some(next) = self.peek() else { return Ok(()) };
            if next.needs_idle && motion_pending && !alarm {
                self.queue.push_front(Outgoing { text: "G4 P0".into(), origin: Origin::Internal(Tag::Sync), sync: false, needs_idle: false });
                self.motion_pending = false;
                continue;
            }
            if (next.sync || sync_mode) && !in_flight_empty {
                return Ok(());
            }
            let len = next.text.len() + 1;
            if len > cap {
                let o = self.take().expect("peeked");
                self.line_too_long(o);
                continue;
            }
            if used + len > cap {
                return Ok(());
            }
            let o = self.take().expect("peeked");
            if alarm && is_motion_line(&o.text) && !matches!(o.origin, Origin::Internal(Tag::Sync)) {
                self.note(format!("Not sent while in alarm: {}", o.text));
                if matches!(o.origin, Origin::Job(_) | Origin::JobTail { .. }) {
                    self.abort_job("The machine is in alarm.");
                }
                continue;
            }
            self.send_line(o)?;
        }
    }

    fn line_too_long(&mut self, o: Outgoing) {
        let msg = format!("Line is longer than the controller's buffer and was not sent: {}", o.text);
        self.note(msg.clone());
        if matches!(o.origin, Origin::Job(_)) {
            self.abort_job(&msg);
            let _ = self.begin_stop();
        }
    }

    fn send_line(&mut self, o: Outgoing) -> Result<(), String> {
        let mut bytes = o.text.clone().into_bytes();
        bytes.push(b'\n');
        self.write(&bytes)?;
        let len = bytes.len();
        let up = o.text.to_ascii_uppercase();
        if (!up.starts_with('$') || up.starts_with("$J=")) && o.origin != Origin::Internal(Tag::Sync) {
            self.motion_pending = true;
        }
        if o.origin == Origin::Internal(Tag::Home) {
            self.home_alarm_count = self.alarm_count;
        }
        let echo = match &o.origin {
            Origin::Job(_) | Origin::JobTail { .. } | Origin::Internal(_) => self.opts.show_all,
            Origin::Console | Origin::Macro => false, // echoed when queued
        };
        if echo {
            self.emit(Event::Console(ConsoleLine { kind: ConsoleKind::Sent, text: o.text.clone() }));
        }
        if let (Origin::Job(n), Some(j)) = (&o.origin, self.job.as_mut()) {
            j.lines_sent = *n;
        }
        self.in_flight.push_back(Pending { len, origin: o.origin, text: o.text });
        self.in_flight_bytes += len;
        self.snap.rx_in_flight_peak = self.snap.rx_in_flight_peak.max(self.in_flight_bytes);
        Ok(())
    }

    // ----- receiving -----

    fn front_is_user(&self) -> bool {
        self.in_flight.front().is_some_and(|p| matches!(p.origin, Origin::Console | Origin::Macro))
    }

    fn received(&self, text: &str) {
        self.emit(Event::Console(ConsoleLine { kind: ConsoleKind::Received, text: text.to_owned() }));
    }

    fn on_line(&mut self, line: &str) -> Result<(), String> {
        // Vendor tags and welcome lines name the machine; plain replies are skipped cheaply.
        if line.starts_with('[') || line.ends_with('!') {
            self.detector.feed(line);
            self.snap.detected = self.detector.result();
        }
        let resp = parse_response(line);
        match resp {
            Response::Ok => self.on_answer(line, None),
            Response::Error(code) => self.on_answer(line, Some((code, error_message(code)))),
            Response::ErrorText(t) => self.on_answer(line, Some((0, t))),
            Response::Alarm(code) => {
                let msg = alarm_message(code, self.kind());
                self.received(&format!("{line} ({msg})"));
                self.on_alarm(code, msg)
            }
            Response::AlarmText(t) => {
                self.received(line);
                let code = if t.contains("limit") {
                    1
                } else if t.contains("Abort") {
                    3
                } else {
                    0
                };
                self.on_alarm(code, t)
            }
            Response::Status(r) => {
                if self.echo_status || self.opts.show_all {
                    self.echo_status = false;
                    self.received(line);
                }
                self.on_status(r)
            }
            Response::Banner(info) => {
                self.received(line);
                self.on_banner(info)
            }
            Response::Setting { number, value } => {
                if self.front_is_user() || self.opts.show_all {
                    self.received(line);
                }
                self.snap.settings.insert(number, value.clone());
                self.settings_collect.push(SettingValue::new(number, value));
                Ok(())
            }
            Response::Message(m) => {
                self.received(line);
                if m.contains("to unlock") || m.contains("Reset to continue") {
                    self.snap.alarm_locked = true;
                }
                if m.contains("Caution: Unlocked") {
                    self.snap.alarm_locked = false;
                }
                Ok(())
            }
            Response::Feedback { tag, body } => {
                if self.front_is_user() || self.opts.show_all {
                    self.received(line);
                }
                let tag_is_info = self.in_flight.front().is_some_and(|p| p.origin == Origin::Internal(Tag::Info));
                if tag_is_info && let Some(fw) = self.snap.firmware.as_mut() {
                    apply_build_info(fw, &tag, &body);
                }
                Ok(())
            }
            Response::StartupLine(_) | Response::Other(_) => {
                self.received(line);
                Ok(())
            }
        }
    }

    fn kind(&self) -> FirmwareKind {
        self.snap.firmware.as_ref().map_or(FirmwareKind::Grbl, |f| f.kind)
    }

    fn on_answer(&mut self, line: &str, error: Option<(u16, String)>) -> Result<(), String> {
        let Some(p) = self.in_flight.pop_front() else {
            // Answer to a line flushed by a reset; nothing to account.
            return Ok(());
        };
        self.in_flight_bytes -= p.len;
        let user = matches!(p.origin, Origin::Console | Origin::Macro);
        if let Some((code, msg)) = &error {
            self.received(&format!("{line} ({msg})"));
            match &p.origin {
                Origin::Job(n) => {
                    let stop = if let Some(j) = self.job.as_mut() {
                        j.errors += 1;
                        j.lines_acked = *n;
                        j.stop_on_error
                    } else {
                        false
                    };
                    self.emit(Event::JobLineError { line_number: *n, line: p.text.clone(), code: *code, message: msg.clone() });
                    if stop {
                        self.begin_stop()?;
                    }
                }
                Origin::Internal(Tag::SettingWrite(n, v)) => {
                    self.emit(Event::SettingWritten { number: *n, value: v.clone(), result: Err(msg.clone()) })
                }
                _ => self.emit(Event::Error { code: *code, message: msg.clone(), line: p.text.clone() }),
            }
        } else if user || self.opts.show_all {
            self.received(line);
        }
        match p.origin {
            Origin::Job(n) => {
                if let Some(j) = self.job.as_mut() {
                    j.lines_acked = j.lines_acked.max(n);
                }
            }
            Origin::JobTail { last: true } => {
                self.motion_pending = false;
                if let Some(mut j) = self.job.take() {
                    let p = j.progress();
                    self.snap.paused = false;
                    // The last line (a wait for motion to finish) has been answered, so the
                    // machine is idle now. Say so before announcing the end, instead of leaving
                    // the state of the last status report in place until the next poll.
                    if matches!(self.snap.status.state, MachineState::Run(_)) {
                        self.snap.status.state = MachineState::Idle;
                    }
                    self.last_poll = Instant::now().checked_sub(self.opts.poll_interval).unwrap_or_else(Instant::now);
                    self.emit(Event::JobProgress(p.clone()));
                    self.emit(Event::JobFinished(p));
                }
            }
            Origin::JobTail { last: false } => {}
            Origin::Internal(Tag::Info) => {
                if self.opts.read_settings {
                    self.settings_collect.clear();
                    self.queue.push_front(Outgoing::new("$$".into(), Origin::Internal(Tag::SettingsRead { handshake: true })));
                } else {
                    self.finish_handshake();
                }
            }
            Origin::Internal(Tag::SettingsRead { handshake }) => {
                let list = std::mem::take(&mut self.settings_collect);
                if error.is_none() {
                    self.emit(Event::Settings(list));
                }
                if handshake {
                    self.finish_handshake();
                }
            }
            Origin::Internal(Tag::SettingWrite(n, v)) => {
                if error.is_none() {
                    self.snap.settings.insert(n, v.clone());
                    self.emit(Event::SettingWritten { number: n, value: v, result: Ok(()) });
                }
            }
            Origin::Internal(Tag::Sync) => {}
            Origin::Internal(Tag::Home) => {
                self.motion_pending = false;
                // Homing failures arrive as ALARM:N before the `ok`.
                if error.is_none() && self.alarm_count == self.home_alarm_count {
                    self.snap.alarm_locked = false;
                    if self.snap.status.state.is_alarm() {
                        self.snap.status.state = MachineState::Idle;
                    }
                    self.emit(Event::Homed);
                }
            }
            Origin::Internal(Tag::Unlock) => {
                if error.is_none() {
                    self.snap.alarm_locked = false;
                    if self.snap.status.state.is_alarm() {
                        self.snap.status.state = MachineState::Idle;
                    }
                }
            }
            Origin::Internal(Tag::Fire) => {
                if error.is_some() {
                    self.fire_until = None;
                }
            }
            Origin::Internal(Tag::Plain) | Origin::Console | Origin::Macro => {}
        }
        Ok(())
    }

    fn finish_handshake(&mut self) {
        if self.phase == Phase::Ready {
            return;
        }
        self.phase = Phase::Ready;
        self.snap.connection = ConnectionState::Connected;
        if let Some(fw) = self.snap.firmware.clone() {
            self.snap.rx_capacity = fw.rx_buffer_size.saturating_sub(1).max(16);
            self.emit(Event::Connected(fw));
        }
    }

    fn on_banner(&mut self, info: FirmwareInfo) -> Result<(), String> {
        // Any banner means the controller reset: its RX buffer and planner are empty.
        self.in_flight.clear();
        self.in_flight_bytes = 0;
        self.motion_pending = false;
        self.settings_collect.clear();
        self.fire_until = None;
        self.snap.status.overrides = Overrides::default();
        self.snap.paused = false;
        match self.phase {
            Phase::WaitBanner { .. } | Phase::Handshake { .. } => {
                let mut fw = info;
                if let Some(old) = &self.snap.firmware
                    && old.kind == FirmwareKind::GrblHal
                    && fw.kind == FirmwareKind::Grbl
                {
                    fw.kind = FirmwareKind::GrblHal;
                }
                self.snap.rx_capacity = fw.rx_buffer_size.saturating_sub(1).max(16);
                self.snap.firmware = Some(fw);
                self.queue.clear();
                self.queue.push_back(Outgoing::new("$I".into(), Origin::Internal(Tag::Info)));
                self.phase = Phase::Handshake { deadline: Instant::now() + HANDSHAKE_TIMEOUT };
                self.poll_sent = None;
            }
            Phase::Ready => {
                if self.stop.is_some() {
                    self.stop_after_reset()?;
                } else {
                    self.queue.clear();
                    self.abort_job("The controller reset.");
                    self.note("The controller reset.");
                }
            }
        }
        Ok(())
    }

    fn abort_job(&mut self, reason: &str) {
        if let Some(mut j) = self.job.take() {
            self.emit(Event::JobAborted { progress: j.progress(), reason: reason.to_owned() });
        }
        self.snap.paused = false;
    }

    fn on_alarm(&mut self, code: u16, message: String) -> Result<(), String> {
        self.alarm_count += 1;
        self.snap.alarm_locked = true;
        self.snap.last_alarm = Some((code, message.clone()));
        self.snap.status.state = MachineState::Alarm(None);
        self.fire_until = None;
        self.queue.clear();
        self.abort_job(&message);
        self.emit(Event::Alarm { code, message });
        if self.stop.is_none() && alarm_is_critical(code) && self.phase == Phase::Ready {
            // GRBL waits for a reset after a hard or soft limit; reset so the machine reaches
            // the normal (unlockable) alarm state. A reset never moves the machine.
            self.stop_reset()?;
        }
        Ok(())
    }

    fn on_status(&mut self, r: StatusReport) -> Result<(), String> {
        let poll_time = self.poll_sent.take();
        let k = if self.snap.settings.get(&13).is_some_and(|v| v.trim() == "1") { 25.4 } else { 1.0 };
        let scale = |a: Axes| [a[0] * k, a[1] * k, a[2] * k, a[3] * k];
        let st = &mut self.snap.status;
        if let Some(w) = r.wco {
            st.wco = scale(w);
        }
        match (r.mpos, r.wpos) {
            (Some(m), Some(w)) => {
                st.mpos = scale(m);
                st.wpos = scale(w);
                for i in 0..4 {
                    st.wco[i] = st.mpos[i] - st.wpos[i];
                }
            }
            (Some(m), None) => {
                st.mpos = scale(m);
                st.wpos = std::array::from_fn(|i| st.mpos[i] - st.wco[i]);
            }
            (None, Some(w)) => {
                st.wpos = scale(w);
                st.mpos = std::array::from_fn(|i| st.wpos[i] + st.wco[i]);
            }
            (None, None) => {}
        }
        if r.axes > 0 {
            st.axes = r.axes;
        }
        st.feed = r.feed.map_or(st.feed, |f| f * k);
        st.spindle = r.spindle.unwrap_or(st.spindle);
        if let Some((b, rx)) = r.buffer {
            st.planner_free = Some(b);
            st.rx_free = Some(rx);
        }
        if let Some((f, ra, s)) = r.overrides {
            st.overrides = Overrides { feed: f, rapid: ra, power: s };
            st.accessories = r.accessories.unwrap_or_default();
        }
        st.pins = r.pins.unwrap_or_default();
        st.updated = Some(Instant::now());
        let changed = st.state != r.state;
        st.state = r.state.clone();
        if r.state.is_alarm() {
            self.snap.alarm_locked = true;
        } else if r.state != MachineState::Unknown {
            self.snap.alarm_locked = false;
        }
        if changed {
            self.emit(Event::StateChanged(r.state.clone()));
        }
        let v11 = self.snap.is_v11();
        let mpos = self.snap.status.mpos;
        if let Some(stop) = &mut self.stop {
            match stop.phase {
                StopPhase::Hold => {
                    let after_hold = poll_time.is_some_and(|t| t > stop.since);
                    // GRBL 1.1 reports `Hold:1` while it slows down. GRBL 0.9 reports a plain
                    // `Hold` all the way, so there the machine stands still once two reports
                    // in a row give the same position.
                    let still = v11 || !matches!(r.state, MachineState::Hold(_)) || stop.held_at == Some(mpos);
                    if after_hold {
                        stop.held_at = Some(mpos);
                    }
                    if after_hold && !r.state.is_moving() && still {
                        self.stop_reset()?;
                    }
                }
                StopPhase::AwaitStatus => {
                    // The reset byte went out before this status request, and the controller
                    // takes both straight off the wire: one that answers the request has also
                    // reset, with or without a start-up message. Its buffer is empty.
                    self.in_flight.clear();
                    self.in_flight_bytes = 0;
                    self.motion_pending = false;
                    self.finish_stop();
                }
                StopPhase::Reset => {}
            }
        }
        Ok(())
    }
}

fn is_sync_origin(origin: &Origin, text: &str) -> bool {
    matches!(origin, Origin::Internal(Tag::Info | Tag::SettingsRead { .. } | Tag::SettingWrite(..) | Tag::Home | Tag::Unlock))
        || (text.starts_with('$') && !text.to_ascii_uppercase().starts_with("$J="))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_text_round_trip() {
        let b = SettingsBackup { firmware: "GRBL 1.1h".into(), values: vec![(32, "1".into()), (110, "6000.000".into()), (130, "400.000".into())] };
        let text = b.to_text();
        assert!(text.contains("$110=6000.000 ; X max rate (mm/min)"));
        assert!(text.contains("$32=1 ; Laser mode"));
        assert_eq!(SettingsBackup::parse(&text).unwrap(), b);
        assert!(SettingsBackup::parse("$x=1").is_err());
        assert!(SettingsBackup::parse("$1=abc").is_err());
    }

    #[test]
    fn bounds() {
        let m = Rect::new(0.0, 0.0, 400.0, 400.0);
        assert!(BoundsCheck { job: Rect::new(10.0, 10.0, 100.0, 100.0), machine: m }.check().is_ok());
        assert!(BoundsCheck { job: Rect::new(-1.0, 10.0, 100.0, 100.0), machine: m }.check().is_err());
        assert!(BoundsCheck { job: Rect::new(10.0, 10.0, 100.0, 401.0), machine: m }.check().is_err());
    }

    #[test]
    fn eta_estimate() {
        let mut j = ActiveJob {
            kind: JobKind::Job,
            lines: Box::new(std::iter::empty()),
            staged: None,
            source_index: 0,
            exhausted: true,
            tail: VecDeque::new(),
            lines_sent: 50,
            lines_acked: 50,
            total: Some(100),
            started: Instant::now() - Duration::from_secs(10),
            errors: 0,
            stop_on_error: true,
            eta: None,
        };
        let p = j.progress();
        let eta = p.eta.unwrap().as_secs_f64();
        assert!((eta - 10.0).abs() < 0.5, "{eta}");
        assert_eq!(p.fraction(), Some(0.5));
        j.eta = Some(Box::new(|_| Some(Duration::from_secs(42))));
        assert_eq!(j.progress().eta, Some(Duration::from_secs(42)));
    }
}
