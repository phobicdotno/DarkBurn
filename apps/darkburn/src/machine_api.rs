//! The app-side machine abstraction: [`MachineHandle`].
//!
//! Everything in DarkBurn that talks to a laser (Laser, Move, Console, File List panels, Machine
//! Settings, the job runner) goes through one [`MachineHandle`] and never sees a driver crate.
//!
//! * One handle holds at most one connected device. It is polled once per frame with
//!   [`MachineHandle::poll`], which drains driver events, refreshes the [`Snapshot`] and returns the
//!   [`MachineEvent`]s for the UI. Nothing here blocks the UI thread.
//! * Every action is a [`MachineCommand`] sent with [`MachineHandle::command`] (the named methods
//!   are thin wrappers). The result is `Ok(())` when the command was accepted; what the machine
//!   did with it arrives later as events and snapshot changes.
//! * Drivers sit behind the [`DriverAdapter`] trait. GRBL ([`GrblAdapter`]) and Ruida
//!   ([`RuidaAdapter`]) are built in. The `GcodeMachine` trait of `darkburn-gcodebots` (not merged
//!   yet) plugs in by writing one more `DriverAdapter` and one more arm in [`open_adapter`].
//! * The two simulator links ([`Link::SimulatorGrbl`], [`Link::SimulatorRuida`]) run the whole
//!   stack against the in-memory emulators, so the entire UI can be exercised with no hardware.
//!
//! Hardware safety: a non-simulator link is only opened when the caller passes a
//! [`RealLinkConsent`], which only the UI constructs after the user clicked Connect and confirmed.
//! In test builds a real link is refused outright, whatever the consent.

#![allow(dead_code, reason = "the panels that use every part of this API arrive in the following modules")]

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use darkburn_grbl as grbl;
use darkburn_ruida as ruida;
use kurbo::{Point, Rect};
use serde::{Deserialize, Serialize};

/// What kind of controller a device has.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Family {
    /// GRBL, grblHAL, FluidNC (G-code streamed over serial or TCP).
    #[default]
    Grbl,
    /// Ruida DSP controller (UDP or USB).
    Ruida,
    /// No laser at all: design only.
    DesignOnly,
    /// EzCad2-style galvo marking board (fibre, MOPA, UV, CO2 galvo).
    Galvo,
    /// Files only: DarkBurn writes the files and another program runs them (Gweike).
    FileOnly,
}

impl Family {
    pub fn label(self) -> &'static str {
        match self {
            Family::Grbl => "G-code",
            Family::Ruida => "Ruida (DSP)",
            Family::DesignOnly => "Design only",
            Family::Galvo => "Galvo",
            Family::FileOnly => "File workflow",
        }
    }
    /// There is no live link at all for this family.
    pub fn no_laser(self) -> bool {
        matches!(self, Family::DesignOnly | Family::FileOnly)
    }
}

/// How the machine is reached.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Link {
    /// The GRBL emulator. Safe: nothing leaves the process.
    SimulatorGrbl,
    /// The Ruida emulator. Safe: nothing leaves the process.
    SimulatorRuida,
    /// USB serial (GRBL).
    Serial { port: String, baud: u32 },
    /// TCP (GRBL over Wi-Fi, FluidNC telnet).
    Tcp { host: String, port: u16 },
    /// Ruida Ethernet.
    Udp { ip: String },
    /// Ruida over USB: the serial port of the controller's FTDI chip (`port`), or the FTDI chip
    /// itself by adapter serial number in builds with the `usb` feature.
    Usb {
        serial: Option<String>,
        #[serde(default)]
        port: Option<String>,
    },
    /// An in-memory Smoothieware, Marlin, Snapmaker, custom or library-GRBL machine. Safe.
    SimulatorGcode(crate::gcode_machines::SimGcode),
    /// The in-memory galvo board. Safe.
    SimulatorGalvo,
    /// A galvo board over USB. Not built into this version: simulator only.
    GalvoUsb,
}

impl Link {
    pub fn is_simulator(&self) -> bool {
        matches!(self, Link::SimulatorGrbl | Link::SimulatorRuida | Link::SimulatorGcode(_) | Link::SimulatorGalvo)
    }
    pub fn family(&self) -> Family {
        match self {
            Link::SimulatorGrbl | Link::Serial { .. } | Link::Tcp { .. } | Link::SimulatorGcode(_) => Family::Grbl,
            Link::SimulatorRuida | Link::Udp { .. } | Link::Usb { .. } => Family::Ruida,
            Link::SimulatorGalvo | Link::GalvoUsb => Family::Galvo,
        }
    }
    pub fn label(&self) -> String {
        match self {
            Link::SimulatorGrbl => "Simulator (GRBL)".into(),
            Link::SimulatorRuida => "Simulator (Ruida)".into(),
            Link::SimulatorGcode(s) => format!("Simulator ({})", s.label()),
            Link::SimulatorGalvo => "Simulator (galvo)".into(),
            Link::GalvoUsb => "Galvo USB".into(),
            Link::Serial { port, baud } => format!("{port} @ {baud}"),
            Link::Tcp { host, port } => format!("tcp://{host}:{port}"),
            Link::Udp { ip } => format!("udp://{ip}"),
            Link::Usb { serial, port } => format!("USB {}", port.as_deref().or(serial.as_deref()).unwrap_or("(first FTDI)")),
        }
    }
}

/// Permission to open a real port, USB device or network connection. Only the UI creates one, from
/// the user's own click on Connect (after the confirmation dialog). Tests cannot use it: in test
/// builds [`open_adapter`] refuses every real link.
#[derive(Debug)]
pub struct RealLinkConsent(());

impl RealLinkConsent {
    /// Call only from the handler of the user's click on the Connect confirmation.
    pub(crate) fn from_user_click() -> Self {
        Self(())
    }
}

/// Connection settings that come from the device profile.
#[derive(Clone, Debug)]
pub struct ConnectConfig {
    pub toggle_dtr: bool,
    pub dtr: bool,
    /// Wait for `ok` after each line instead of filling the controller's buffer.
    pub synchronous: bool,
    /// S value for full power (GRBL `$30`); `None` reads it from the controller.
    pub s_max: Option<f64>,
    /// Work area in mm; used for bounds checks and by the simulators as their bed size.
    pub bed: Option<(f64, f64)>,
    pub max_fire_percent: f64,
    pub max_fire_secs: f64,
    pub show_all: bool,
    /// Simulators only: simulated seconds per real second (jobs stay watchable at 1 to 10).
    pub sim_time_scale: f64,
    /// Ruida: answer overwrite prompts with this instead of asking.
    pub overwrite_default: Option<ruida::OverwriteChoice>,
    /// GRBL simulator only: homing is enabled, so the machine starts locked in alarm until homed.
    pub sim_homing: bool,
    /// Ruida: how long to wait for an acknowledgement or a reply, ms (DS-17).
    pub net_timeout_ms: Option<u32>,
    /// The device profile for the G-code firmwares of `darkburn-gcodebots` (Smoothieware,
    /// Marlin, Snapmaker, custom). `None` for GRBL and Ruida.
    pub gcode: Option<Arc<darkburn_gcodebots::DeviceProfile>>,
    /// The galvo board's settings.
    pub galvo: Option<Arc<crate::galvo_settings::GalvoDeviceSettings>>,
}

impl Default for ConnectConfig {
    fn default() -> Self {
        Self {
            toggle_dtr: false,
            dtr: true,
            synchronous: false,
            s_max: None,
            bed: None,
            max_fire_percent: 10.0,
            max_fire_secs: 10.0,
            show_all: false,
            sim_time_scale: 4.0,
            overwrite_default: None,
            sim_homing: false,
            net_timeout_ms: None,
            gcode: None,
            galvo: None,
        }
    }
}

/// What to connect to.
#[derive(Clone, Debug)]
pub struct ConnectSpec {
    pub link: Link,
    pub config: ConnectConfig,
}

impl ConnectSpec {
    pub fn simulator(family: Family) -> Self {
        let link = if family == Family::Ruida { Link::SimulatorRuida } else { Link::SimulatorGrbl };
        Self { link, config: ConnectConfig::default() }
    }
}

/// Why a command was not accepted.
#[derive(Clone, Debug, PartialEq)]
pub enum MachineError {
    NotConnected,
    /// A job, frame or stop is running.
    Busy,
    /// The machine is in alarm.
    Alarm,
    OutOfBounds(String),
    /// This controller cannot do it.
    Unsupported(String),
    /// A real link was requested without consent, in a test build, or the device has no laser.
    Refused(String),
    Invalid(String),
    Driver(String),
}

impl std::fmt::Display for MachineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotConnected => write!(f, "The laser is not connected."),
            Self::Busy => write!(f, "The laser is busy."),
            Self::Alarm => write!(f, "The laser is in alarm. Home or unlock it first."),
            Self::OutOfBounds(m) => write!(f, "Out of bounds: {m}"),
            Self::Unsupported(m) => write!(f, "Not supported by this controller: {m}"),
            Self::Refused(m) | Self::Invalid(m) | Self::Driver(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for MachineError {}

impl From<grbl::GrblError> for MachineError {
    fn from(e: grbl::GrblError) -> Self {
        use grbl::GrblError as G;
        match e {
            G::NotConnected | G::NotReady => Self::NotConnected,
            G::Busy => Self::Busy,
            G::Alarm => Self::Alarm,
            G::OutOfBounds(m) => Self::OutOfBounds(m),
            G::Unsupported(m) => Self::Unsupported(m.to_owned()),
            G::InvalidArgument(m) => Self::Invalid(m),
            other @ G::NotIdle(_) => Self::Driver(other.to_string()),
        }
    }
}

impl From<ruida::RuidaError> for MachineError {
    fn from(e: ruida::RuidaError) -> Self {
        match e {
            ruida::RuidaError::NotConnected | ruida::RuidaError::WorkerGone => Self::NotConnected,
            ruida::RuidaError::Unsupported(m) => Self::Unsupported(m),
            ruida::RuidaError::Encode(m) if m.contains("outside") => Self::OutOfBounds(m),
            other => Self::Driver(other.to_string()),
        }
    }
}

/// Connection to the device.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum LinkState {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    Failed(String),
}

/// The laser's state as the Laser panel shows it [LASER-01].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum LaserState {
    #[default]
    Disconnected,
    Connecting,
    Ready,
    Busy,
    Paused,
    Framing,
    Alarm,
}

impl LaserState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Disconnected => "Disconnected",
            Self::Connecting => "Connecting",
            Self::Ready => "Ready",
            Self::Busy => "Busy",
            Self::Paused => "Paused",
            Self::Framing => "Framing",
            Self::Alarm => "Alarm",
        }
    }
    /// A job can be started.
    pub fn can_start(self) -> bool {
        self == Self::Ready
    }
    /// A job, frame or pause is in progress.
    pub fn is_working(self) -> bool {
        matches!(self, Self::Busy | Self::Paused | Self::Framing)
    }
}

/// Progress of the running job [LASER-02].
#[derive(Clone, Debug, PartialEq, Default)]
pub struct JobProgress {
    /// 0..1 when known.
    pub fraction: Option<f64>,
    pub elapsed: Duration,
    pub remaining: Option<Duration>,
    pub lines_done: usize,
    pub total_lines: Option<usize>,
    pub errors: usize,
    pub framing: bool,
}

/// Files, info and settings read from a Ruida controller; filled as replies arrive.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RuidaCache {
    pub files: Option<Vec<ruida::ControllerFile>>,
    pub info: Option<ruida::ControllerInfo>,
    pub settings: Option<ruida::MachineSettings>,
    pub laser2: Option<ruida::settings::Laser2Settings>,
    pub u_axis: Option<ruida::settings::UAxisSettings>,
    /// Calculated job times by file index.
    pub times: BTreeMap<u16, Duration>,
    /// Upload progress in bytes while a send is running.
    pub upload: Option<(usize, usize)>,
    /// A send is waiting for the user to decide what to do about an existing file name.
    pub overwrite_prompt: Option<String>,
}

/// What the UI shows. A copy; refreshed by [`MachineHandle::poll`].
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub family: Family,
    pub simulator: bool,
    pub link: LinkState,
    pub state: LaserState,
    pub port: String,
    pub firmware: String,
    pub has_position: bool,
    /// X, Y, Z, U in mm.
    pub machine_pos: [f64; 4],
    pub work_pos: [f64; 4],
    pub job: Option<JobProgress>,
    /// Feed, rapid, power override in percent.
    pub overrides: (u16, u16, u16),
    pub fire_on: bool,
    pub alarm: Option<String>,
    pub grbl_settings: BTreeMap<u16, String>,
    pub bed: Option<(f64, f64)>,
    pub ruida: RuidaCache,
    /// Settings of a Smoothieware or Marlin style controller (config keys, `M92.X`).
    pub gcode_settings: BTreeMap<String, String>,
    /// Set for the controllers that run through `darkburn-gcodebots` (not the built-in GRBL one).
    pub gcode_firmware: Option<darkburn_gcodebots::MachineFamily>,
    pub air_on: bool,
    /// The galvo board's state.
    pub galvo: Option<crate::galvo_machine::GalvoStatus>,
}

impl Snapshot {
    pub fn connected(&self) -> bool {
        self.link == LinkState::Connected
    }
}

/// One console line.
#[derive(Clone, Debug, PartialEq)]
pub struct ConsoleEntry {
    pub kind: ConsoleKind,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsoleKind {
    Sent,
    Received,
    Note,
}

/// Something that happened.
#[derive(Clone, Debug, PartialEq)]
pub enum MachineEvent {
    Connected(String),
    ConnectFailed(String),
    Disconnected(Option<String>),
    Console(ConsoleEntry),
    Alarm(String),
    Error(String),
    JobStarted,
    FrameStarted,
    JobFinished(JobProgress),
    JobAborted(String),
    Stopped,
    Homed,
    SettingsLoaded,
    SettingWritten {
        id: String,
        ok: bool,
        message: String,
    },
    FilesListed,
    InfoLoaded,
    /// A Ruida send needs the user's choice: answer with [`MachineHandle::answer_overwrite`].
    OverwritePrompt(String),
    /// A request that has no data of its own finished (`name`, outcome).
    Done {
        what: String,
        ok: bool,
        message: String,
    },
}

/// What happens to the head when a job ends.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum Finish {
    #[default]
    Stay,
    Origin,
    Machine(Point),
}

/// The program to run.
#[derive(Clone, Debug)]
pub enum JobData {
    /// G-code lines for GRBL-family machines.
    Gcode(Vec<String>),
    /// A neutral Ruida job, streamed and run at once.
    Rd(ruida::RdJob),
    /// A finished `.rd` file (swizzled bytes) stored on the controller under `name`; run if
    /// `start_immediately`.
    RdFile { name: String, data: Vec<u8> },
    /// A compiled galvo job. The laser must be armed first.
    Galvo { job: Box<darkburn_galvo::CompiledJob>, wait_for_foot: bool, skip_framing_check: bool },
}

#[derive(Clone, Debug)]
pub struct JobRequest {
    pub data: JobData,
    /// Name shown in the log and used when the controller stores the job.
    pub name: String,
    /// Job extents in machine millimetres; the job is refused when outside the bed.
    pub bounds: Option<Rect>,
    pub finish: Finish,
    /// Total time estimate, for the remaining-time display.
    pub estimated: Option<Duration>,
    pub stop_on_error: bool,
    /// Ruida: run as soon as it is stored.
    pub start_immediately: bool,
    pub auto_increment_name: bool,
}

impl JobRequest {
    pub fn gcode(lines: Vec<String>) -> Self {
        Self::new(JobData::Gcode(lines))
    }
    pub fn rd(job: ruida::RdJob) -> Self {
        Self::new(JobData::Rd(job))
    }
    pub fn new(data: JobData) -> Self {
        Self {
            data,
            name: "DARKBURN".into(),
            bounds: None,
            finish: Finish::Stay,
            estimated: None,
            stop_on_error: true,
            start_immediately: true,
            auto_increment_name: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameSpec {
    /// mm/min for GRBL; ignored by Ruida.
    pub feed: f64,
    /// Laser on at this power while tracing (GRBL) or red light (Ruida).
    pub laser_percent: Option<f64>,
}

impl Default for FrameSpec {
    fn default() -> Self {
        Self { feed: 3000.0, laser_percent: None }
    }
}

/// A jog by a distance in mm on each axis, `feed` in mm/min.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct JogSpec {
    pub dx: f64,
    pub dy: f64,
    pub dz: f64,
    pub du: f64,
    pub feed: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct MoveSpec {
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub z: Option<f64>,
    /// Coordinates are from machine zero instead of the work origin.
    pub machine_coords: bool,
    pub feed: f64,
}

/// Every action the app can ask of a machine.
#[derive(Clone, Debug)]
pub enum MachineCommand {
    StartJob(Box<JobRequest>),
    Pause,
    Resume,
    Stop,
    FrameRect(Rect, FrameSpec),
    FrameOutline(Vec<Point>, FrameSpec),
    /// Ruida: frame the stored job on the controller.
    FrameOnController,
    JogStep(JogSpec),
    /// Continuous jog: direction per axis (-1, 0, 1) for X, Y, Z, U.
    JogStart {
        dir: [i8; 4],
        feed: f64,
    },
    JogStop,
    Home,
    Unlock,
    GoToOrigin,
    MoveTo(MoveSpec),
    SetUserOrigin,
    ClearUserOrigin,
    GetPosition,
    Fire {
        percent: f64,
        secs: f64,
    },
    FireOff,
    SetOverrides {
        feed: Option<u16>,
        power: Option<u16>,
        rapid: Option<u16>,
    },
    Console(String),
    Macro(String),
    ShowAll(bool),
    ReadSettings,
    /// GRBL: id is the `$` number. Ruida: the register address.
    WriteSetting {
        id: u32,
        value: String,
    },
    RestoreSettings(Box<SettingsBackup>),
    // Ruida only
    ListFiles,
    RunFile(u16),
    DeleteFiles(Vec<u16>),
    DeleteAllFiles,
    CalcTime(u16),
    ReadControllerInfo,
    ReadLaser2,
    WriteLaser2(ruida::settings::Laser2Settings),
    ReadUAxis,
    WriteUAxis(ruida::settings::UAxisSettings),
    AnswerOverwrite(ruida::OverwriteChoice),
    // G-code controllers of darkburn-gcodebots
    /// The controller's own emergency stop (M112 / soft reset). Always allowed while connected.
    EmergencyStop,
    Air(bool),
    /// Store the settings in the controller's memory (Marlin M500).
    PersistSettings,
    /// A setting by its name (config key, `M92.X`).
    WriteGcodeSetting {
        key: String,
        value: String,
    },
    // Galvo only
    /// Arm or disarm the laser for the next job. The laser is never enabled without this.
    GalvoArm(bool),
    /// A red-light frame (no laser). `repeat` keeps lapping until Stop.
    GalvoFrame {
        frame: Box<darkburn_galvo::CompiledFrame>,
        repeat: bool,
    },
    /// The red pointer on or off at the current position.
    GalvoLight(bool),
    /// Replace the lens correction table (`None` clears it).
    GalvoLens(Option<Box<darkburn_galvo::CorTable>>),
    /// Move an external axis (0 or 1) to an absolute position in mm.
    GalvoAxis {
        axis: u8,
        mm: f64,
    },
}

/// A settings backup that can be saved as a `.dbset` file.
#[derive(Clone, Debug, PartialEq)]
pub enum SettingsBackup {
    Grbl(grbl::SettingsBackup),
    Ruida(ruida::MachineSettings),
}

impl SettingsBackup {
    pub fn family(&self) -> Family {
        match self {
            Self::Grbl(_) => Family::Grbl,
            Self::Ruida(_) => Family::Ruida,
        }
    }
    /// GRBL: `$N=value` lines. Ruida: JSON.
    pub fn to_text(&self) -> String {
        match self {
            Self::Grbl(b) => b.to_text(),
            Self::Ruida(s) => serde_json::to_string_pretty(s).unwrap_or_default(),
        }
    }
    pub fn parse(family: Family, text: &str) -> Result<Self, String> {
        match family {
            Family::Grbl => grbl::SettingsBackup::parse(text).map(Self::Grbl),
            Family::Ruida => serde_json::from_str(text).map(Self::Ruida).map_err(|e| format!("settings file: {e}")),
            Family::DesignOnly | Family::FileOnly => Err("this device has no machine settings".into()),
            Family::Galvo => Err("a galvo board has no settings backup file".into()),
        }
    }
    pub fn len(&self) -> usize {
        match self {
            Self::Grbl(b) => b.values.len(),
            Self::Ruida(s) => s.values.len(),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// The seam for drivers. Built in: [`GrblAdapter`], [`RuidaAdapter`]. Later: an adapter over the
/// `GcodeMachine` trait of `darkburn-gcodebots`.
pub trait DriverAdapter {
    /// Drain driver events and advance any simulator. Must not block.
    fn poll(&mut self) -> Vec<MachineEvent>;
    /// The current state (cheap; called after every poll).
    fn snapshot(&self) -> Snapshot;
    fn command(&mut self, cmd: MachineCommand) -> Result<(), MachineError>;
    /// Stop everything, turn the laser off and close the link.
    fn close(&mut self);
}

const CONSOLE_LIMIT: usize = 2000;

/// The one machine connection of the app.
pub struct MachineHandle {
    adapter: Option<Box<dyn DriverAdapter>>,
    snapshot: Snapshot,
    console: VecDeque<ConsoleEntry>,
    last_link: Option<Link>,
}

impl Default for MachineHandle {
    fn default() -> Self {
        Self::new()
    }
}

impl MachineHandle {
    pub fn new() -> Self {
        Self { adapter: None, snapshot: Snapshot::default(), console: VecDeque::new(), last_link: None }
    }

    /// A handle already connected to a simulator (tests, demos). Returns once the connection is
    /// started; poll until [`Snapshot::connected`].
    pub fn simulator(family: Family) -> Self {
        let mut h = Self::new();
        h.connect(&ConnectSpec::simulator(family), None).expect("the simulator always opens");
        h
    }

    pub fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }
    pub fn console(&self) -> &VecDeque<ConsoleEntry> {
        &self.console
    }
    pub fn clear_console(&mut self) {
        self.console.clear();
    }
    pub fn is_connected(&self) -> bool {
        self.snapshot.connected()
    }
    pub fn is_simulator(&self) -> bool {
        self.snapshot.simulator
    }
    /// A real (non-simulator) machine is connected or connecting: show the REAL MACHINE banner.
    pub fn is_real_machine(&self) -> bool {
        self.adapter.is_some() && !self.snapshot.simulator
    }
    /// A job or frame is in progress (closing the window should ask first) [LASER-28].
    pub fn job_running(&self) -> bool {
        self.snapshot.state.is_working()
    }
    pub fn last_link(&self) -> Option<&Link> {
        self.last_link.as_ref()
    }

    /// Open a connection. Starts it and returns; watch [`MachineEvent::Connected`] or
    /// [`MachineEvent::ConnectFailed`]. A real link needs `consent` (see [`RealLinkConsent`]).
    pub fn connect(&mut self, spec: &ConnectSpec, consent: Option<RealLinkConsent>) -> Result<(), MachineError> {
        self.disconnect();
        let adapter = open_adapter(spec, consent)?;
        self.snapshot = adapter.snapshot();
        self.adapter = Some(adapter);
        self.last_link = Some(spec.link.clone());
        self.note(format!("Connecting to {}", spec.link.label()));
        Ok(())
    }

    /// Use an adapter that is already built, for a simulator whose in-memory board the caller
    /// keeps a handle to (tests, demos). Real links never come through here.
    pub fn attach_simulator(&mut self, adapter: Box<dyn DriverAdapter>, link: Link) {
        assert!(link.is_simulator(), "only simulators can be attached");
        self.disconnect();
        self.snapshot = adapter.snapshot();
        self.adapter = Some(adapter);
        self.last_link = Some(link);
    }

    /// Stops any job, turns the laser off, closes the link.
    pub fn disconnect(&mut self) {
        if let Some(mut a) = self.adapter.take() {
            a.close();
            self.note("Disconnected".into());
        }
        let (family, bed) = (self.snapshot.family, self.snapshot.bed);
        self.snapshot = Snapshot { family, bed, ..Default::default() };
    }

    /// Drop the connection and open the same link again (right-click on Devices) [LASER-17].
    /// Real links need fresh consent.
    pub fn reset_connection(&mut self, spec: &ConnectSpec, consent: Option<RealLinkConsent>) -> Result<(), MachineError> {
        self.disconnect();
        self.connect(spec, consent)
    }

    /// Call once per frame.
    pub fn poll(&mut self) -> Vec<MachineEvent> {
        let Some(a) = self.adapter.as_mut() else { return Vec::new() };
        let events = a.poll();
        self.snapshot = a.snapshot();
        let mut gone = false;
        for e in &events {
            match e {
                MachineEvent::Console(c) => self.push_console(c.clone()),
                MachineEvent::Error(m) => self.note(format!("Error: {m}")),
                MachineEvent::Alarm(m) => self.note(format!("Alarm: {m}")),
                MachineEvent::Disconnected(r) => {
                    gone = true;
                    self.note(r.clone().map_or_else(|| "Disconnected".into(), |r| format!("Connection lost: {r}")));
                }
                MachineEvent::ConnectFailed(m) => {
                    gone = true;
                    self.note(format!("Could not connect: {m}"));
                }
                _ => {}
            }
        }
        if gone && let Some(mut a) = self.adapter.take() {
            a.close();
            self.snapshot.state = LaserState::Disconnected;
            if !matches!(self.snapshot.link, LinkState::Failed(_)) {
                self.snapshot.link = LinkState::Disconnected;
            }
            self.snapshot.job = None;
        }
        events
    }

    fn push_console(&mut self, c: ConsoleEntry) {
        if self.console.len() >= CONSOLE_LIMIT {
            self.console.pop_front();
        }
        self.console.push_back(c);
    }

    fn note(&mut self, text: String) {
        self.push_console(ConsoleEntry { kind: ConsoleKind::Note, text });
    }

    /// Send any command.
    pub fn command(&mut self, cmd: MachineCommand) -> Result<(), MachineError> {
        let family = self.snapshot.family;
        if family.no_laser() {
            return Err(MachineError::Refused("This device is design only; there is no laser to command.".into()));
        }
        match self.adapter.as_mut() {
            Some(a) => a.command(cmd),
            None => Err(MachineError::NotConnected),
        }
    }

    // ----- named wrappers -----

    pub fn start_job(&mut self, job: JobRequest) -> Result<(), MachineError> {
        self.command(MachineCommand::StartJob(Box::new(job)))
    }
    /// Always sent while connected.
    pub fn pause(&mut self) -> Result<(), MachineError> {
        self.command(MachineCommand::Pause)
    }
    pub fn resume(&mut self) -> Result<(), MachineError> {
        self.command(MachineCommand::Resume)
    }
    /// Always sent while connected.
    pub fn stop(&mut self) -> Result<(), MachineError> {
        self.command(MachineCommand::Stop)
    }
    pub fn frame_rect(&mut self, r: Rect, spec: FrameSpec) -> Result<(), MachineError> {
        self.command(MachineCommand::FrameRect(r, spec))
    }
    pub fn frame_outline(&mut self, pts: Vec<Point>, spec: FrameSpec) -> Result<(), MachineError> {
        self.command(MachineCommand::FrameOutline(pts, spec))
    }
    pub fn jog_step(&mut self, s: JogSpec) -> Result<(), MachineError> {
        self.command(MachineCommand::JogStep(s))
    }
    pub fn jog_start(&mut self, dir: [i8; 4], feed: f64) -> Result<(), MachineError> {
        self.command(MachineCommand::JogStart { dir, feed })
    }
    pub fn jog_stop(&mut self) -> Result<(), MachineError> {
        self.command(MachineCommand::JogStop)
    }
    pub fn home(&mut self) -> Result<(), MachineError> {
        self.command(MachineCommand::Home)
    }
    pub fn go_to_origin(&mut self) -> Result<(), MachineError> {
        self.command(MachineCommand::GoToOrigin)
    }
    pub fn move_to(&mut self, m: MoveSpec) -> Result<(), MachineError> {
        self.command(MachineCommand::MoveTo(m))
    }
    pub fn set_user_origin(&mut self) -> Result<(), MachineError> {
        self.command(MachineCommand::SetUserOrigin)
    }
    pub fn clear_user_origin(&mut self) -> Result<(), MachineError> {
        self.command(MachineCommand::ClearUserOrigin)
    }
    /// Ask for the position; it arrives in the snapshot.
    pub fn get_position(&mut self) -> Result<(), MachineError> {
        self.command(MachineCommand::GetPosition)
    }
    /// Laser on at `percent` for at most `secs`, then off by itself.
    pub fn fire(&mut self, percent: f64, secs: f64) -> Result<(), MachineError> {
        self.command(MachineCommand::Fire { percent, secs })
    }
    pub fn fire_off(&mut self) -> Result<(), MachineError> {
        self.command(MachineCommand::FireOff)
    }
    pub fn set_overrides(&mut self, feed: Option<u16>, power: Option<u16>, rapid: Option<u16>) -> Result<(), MachineError> {
        self.command(MachineCommand::SetOverrides { feed, power, rapid })
    }
    pub fn send_console(&mut self, line: &str) -> Result<(), MachineError> {
        self.command(MachineCommand::Console(line.to_owned()))
    }
    pub fn run_macro(&mut self, text: &str) -> Result<(), MachineError> {
        self.command(MachineCommand::Macro(text.to_owned()))
    }
    pub fn read_settings(&mut self) -> Result<(), MachineError> {
        self.command(MachineCommand::ReadSettings)
    }
    pub fn write_setting(&mut self, id: u32, value: &str) -> Result<(), MachineError> {
        self.command(MachineCommand::WriteSetting { id, value: value.to_owned() })
    }
    pub fn restore_settings(&mut self, b: SettingsBackup) -> Result<(), MachineError> {
        self.command(MachineCommand::RestoreSettings(Box::new(b)))
    }
    pub fn answer_overwrite(&mut self, c: ruida::OverwriteChoice) -> Result<(), MachineError> {
        self.command(MachineCommand::AnswerOverwrite(c))
    }

    /// The settings read so far, as a backup. `None` before the first read.
    pub fn backup_settings(&self) -> Option<SettingsBackup> {
        let s = &self.snapshot;
        match s.family {
            Family::Grbl if !s.grbl_settings.is_empty() => Some(SettingsBackup::Grbl(grbl::SettingsBackup {
                firmware: s.firmware.clone(),
                values: s.grbl_settings.iter().map(|(k, v)| (*k, v.clone())).collect(),
            })),
            Family::Ruida => s.ruida.settings.clone().map(SettingsBackup::Ruida),
            _ => None,
        }
    }

    /// Poll until `pred` holds or `timeout` passes (tests and scripts, never the UI thread).
    pub fn wait_for(&mut self, timeout: Duration, mut pred: impl FnMut(&Snapshot, &[MachineEvent]) -> bool) -> bool {
        let end = Instant::now() + timeout;
        loop {
            let ev = self.poll();
            if pred(&self.snapshot, &ev) {
                return true;
            }
            if Instant::now() >= end {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

impl Drop for MachineHandle {
    fn drop(&mut self) {
        if let Some(mut a) = self.adapter.take() {
            a.close();
        }
    }
}

/// A small square job for the simulators, so the whole app can be exercised without the job
/// builder. `size` in mm.
pub fn simulator_square(family: Family, size: f64) -> JobRequest {
    match family {
        Family::Ruida => {
            let s = size;
            let layer = ruida::RdLayer {
                settings: ruida::LayerSettings::default(),
                ops: vec![
                    ruida::RdOp::Move(Point::new(10.0, 10.0)),
                    ruida::RdOp::Cut { to: Point::new(10.0 + s, 10.0), power: None, speed: None },
                    ruida::RdOp::Cut { to: Point::new(10.0 + s, 10.0 + s), power: None, speed: None },
                    ruida::RdOp::Cut { to: Point::new(10.0, 10.0 + s), power: None, speed: None },
                    ruida::RdOp::Cut { to: Point::new(10.0, 10.0), power: None, speed: None },
                ],
            };
            let mut j = JobRequest::rd(ruida::RdJob { layers: vec![layer], ..Default::default() });
            j.bounds = Some(Rect::new(10.0, 10.0, 10.0 + s, 10.0 + s));
            j
        }
        _ => {
            let s = size;
            let lines = vec![
                "G21 G90".to_owned(),
                "M4 S100".to_owned(),
                "G0 X10 Y10".to_owned(),
                format!("G1 X{} Y10 F6000", 10.0 + s),
                format!("G1 X{} Y{}", 10.0 + s, 10.0 + s),
                format!("G1 X10 Y{}", 10.0 + s),
                "G1 X10 Y10".to_owned(),
                "M5 S0".to_owned(),
            ];
            let mut j = JobRequest::gcode(lines);
            j.bounds = Some(Rect::new(10.0, 10.0, 10.0 + s, 10.0 + s));
            j
        }
    }
}

// ============================================================================================
// Opening adapters
// ============================================================================================

/// Build the adapter for `spec`. The only place a real link can be opened.
pub fn open_adapter(spec: &ConnectSpec, consent: Option<RealLinkConsent>) -> Result<Box<dyn DriverAdapter>, MachineError> {
    match &spec.link {
        Link::SimulatorGrbl => {
            let mut cfg = grbl::EmulatorConfig::atomstack().with_time_scale(spec.config.sim_time_scale.max(0.1));
            cfg.record_lines = true;
            if spec.config.sim_homing {
                cfg = cfg.with_setting(22, 1.0);
            }
            if let Some((w, h)) = spec.config.bed {
                cfg = cfg.with_setting(130, w).with_setting(131, h);
            }
            let emu = grbl::EmulatorHandle::new(grbl::GrblEmulator::new(cfg));
            Ok(Box::new(GrblAdapter::new(Box::new(emu.transport()), &spec.config, true, Some(emu))))
        }
        Link::SimulatorRuida => {
            let mut cfg = ruida::EmulatorConfig { step_mode: true, ..Default::default() };
            if let Some((w, h)) = spec.config.bed {
                cfg.bed_x_um = (w * 1000.0) as u32;
                cfg.bed_y_um = (h * 1000.0) as u32;
            }
            let emu = Arc::new(Mutex::new(ruida::RuidaEmulator::new(cfg)));
            let transport: Box<dyn ruida::Transport> = Box::new(ruida::MemoryTransport::udp(emu.clone()));
            Ok(Box::new(RuidaAdapter::new(transport, &spec.config, true, Some((emu, spec.config.sim_time_scale.max(0.1))))))
        }
        Link::SimulatorGcode(kind) => crate::gcode_machines::open_simulator(*kind, &spec.config),
        Link::SimulatorGalvo => crate::galvo_machine::open_simulator(&spec.config),
        real => {
            if cfg!(test) {
                return Err(MachineError::Refused("Real connections are disabled in test builds.".into()));
            }
            if consent.is_none() {
                return Err(MachineError::Refused("Connecting to a real machine needs the user's confirmation.".into()));
            }
            open_real(real, &spec.config)
        }
    }
}

/// Opens real hardware. Reached only through [`open_adapter`] with consent, outside tests.
fn open_real(link: &Link, config: &ConnectConfig) -> Result<Box<dyn DriverAdapter>, MachineError> {
    match link {
        #[cfg(feature = "serial")]
        Link::Serial { port, baud } => {
            let t = grbl::SerialTransport::open(port, *baud).map_err(|e| MachineError::Driver(format!("Could not open {port}: {e}")))?;
            Ok(crate::gcode_machines::adapter_for(Box::new(t), config))
        }
        #[cfg(not(feature = "serial"))]
        Link::Serial { .. } => Err(MachineError::Unsupported("this build has no serial port support".into())),
        Link::Tcp { host, port } => {
            use std::net::ToSocketAddrs;
            let addr = (host.as_str(), *port)
                .to_socket_addrs()
                .map_err(|e| MachineError::Driver(format!("Could not resolve {host}: {e}")))?
                .next()
                .ok_or_else(|| MachineError::Driver(format!("No address for {host}")))?;
            let t = grbl::TcpTransport::connect(addr, Duration::from_secs(5))
                .map_err(|e| MachineError::Driver(format!("Could not connect to {addr}: {e}")))?;
            Ok(crate::gcode_machines::adapter_for(Box::new(t), config))
        }
        Link::Udp { ip } => {
            let ip: std::net::IpAddr = ip.parse().map_err(|_| MachineError::Invalid(format!("'{ip}' is not an IP address.")))?;
            let t = ruida::UdpTransport::connect(ip)?;
            Ok(Box::new(RuidaAdapter::new(Box::new(t), config, false, None)))
        }
        Link::Usb { serial, port } => open_ruida_usb(serial.as_deref(), port.as_deref(), config),
        Link::GalvoUsb => Err(MachineError::Unsupported(
            "the galvo USB link is not built into this version. Use the galvo simulator until the hardware checks in docs/hardware-checks-galvo.md are done."
                .into(),
        )),
        Link::SimulatorGrbl | Link::SimulatorRuida | Link::SimulatorGcode(_) | Link::SimulatorGalvo => {
            Err(MachineError::Invalid("not a real link".into()))
        }
    }
}

/// Ruida over USB. The controller's FTDI chip normally shows up as a serial port (a COM port on
/// Windows), which is what gets opened; builds with the `usb` feature can also claim the chip
/// directly. Reached only through [`open_real`].
fn open_ruida_usb(serial: Option<&str>, port: Option<&str>, config: &ConnectConfig) -> Result<Box<dyn DriverAdapter>, MachineError> {
    #[cfg(feature = "serial")]
    if let Some(p) = port {
        let t = ruida::transport::SerialTransport::open(p)?;
        return Ok(Box::new(RuidaAdapter::new(Box::new(t), config, false, None)));
    }
    #[cfg(feature = "usb")]
    {
        let _ = port;
        let t = ruida::transport::UsbTransport::open(serial)?;
        Ok(Box::new(RuidaAdapter::new(Box::new(t), config, false, None)))
    }
    #[cfg(not(feature = "usb"))]
    {
        let _ = (serial, port, config);
        Err(MachineError::Unsupported(NO_RUIDA_USB_PORT.into()))
    }
}

/// Shown when no serial port of a Ruida controller's USB chip is found.
pub const NO_RUIDA_USB_PORT: &str = "No USB serial port of a Ruida controller was found. Plug the controller in, install the FTDI driver if Windows does not show a COM port for it, then press Rescan. You can also type the port (like COM5) in Device Settings.";

// ============================================================================================
// GRBL
// ============================================================================================

pub struct GrblAdapter {
    m: Option<grbl::GrblMachine>,
    cfg: ConnectConfig,
    simulator: bool,
    /// Keeps the emulator alive for as long as the adapter.
    _sim: Option<grbl::EmulatorHandle>,
    snap: Snapshot,
    estimated: Option<Duration>,
    job_started: Option<Instant>,
}

impl GrblAdapter {
    pub fn new(transport: Box<dyn grbl::Transport>, cfg: &ConnectConfig, simulator: bool, sim: Option<grbl::EmulatorHandle>) -> Self {
        let mut o = grbl::ConnectOptions {
            toggle_dtr: cfg.toggle_dtr,
            dtr: cfg.dtr,
            s_max: cfg.s_max,
            max_fire_percent: cfg.max_fire_percent,
            max_fire_duration: Duration::from_secs_f64(cfg.max_fire_secs.clamp(0.1, 60.0)),
            show_all: cfg.show_all,
            stream_mode: if cfg.synchronous { grbl::StreamMode::Synchronous } else { grbl::StreamMode::CharacterCounting },
            ..Default::default()
        };
        if simulator {
            o.banner_timeout = Duration::from_millis(800);
        }
        if let Some((w, h)) = cfg.bed {
            o.travel = Some(grbl::TravelLimits { xy: Rect::new(0.0, 0.0, w, h), z: None });
        }
        let port = transport.name();
        let m = grbl::GrblMachine::connect(transport, o);
        let snap = Snapshot {
            family: Family::Grbl,
            simulator,
            link: LinkState::Connecting,
            state: LaserState::Connecting,
            port,
            bed: cfg.bed,
            ..Default::default()
        };
        Self { m: Some(m), cfg: cfg.clone(), simulator, _sim: sim, snap, estimated: None, job_started: None }
    }

    fn machine(&self) -> Result<&grbl::GrblMachine, MachineError> {
        self.m.as_ref().ok_or(MachineError::NotConnected)
    }

    fn bed_rect(&self) -> Option<Rect> {
        if let Some((w, h)) = self.cfg.bed {
            return Some(Rect::new(0.0, 0.0, w, h));
        }
        let g = |n: u16| self.snap.grbl_settings.get(&n).and_then(|v| v.parse::<f64>().ok());
        Some(Rect::new(0.0, 0.0, g(130)?, g(131)?))
    }

    fn map_state(s: grbl::LaserStatus) -> LaserState {
        match s {
            grbl::LaserStatus::Disconnected => LaserState::Disconnected,
            grbl::LaserStatus::Connecting => LaserState::Connecting,
            grbl::LaserStatus::Ready => LaserState::Ready,
            grbl::LaserStatus::Busy => LaserState::Busy,
            grbl::LaserStatus::Paused => LaserState::Paused,
            grbl::LaserStatus::Framing => LaserState::Framing,
            grbl::LaserStatus::Alarm => LaserState::Alarm,
        }
    }

    fn progress(&self, p: &grbl::JobProgress) -> JobProgress {
        let fraction = p.fraction();
        let remaining = p.eta.or_else(|| {
            self.estimated.map(|e| e.saturating_sub(p.elapsed)).or_else(|| fraction.filter(|f| *f > 0.02).map(|f| p.elapsed.mul_f64((1.0 - f) / f)))
        });
        JobProgress {
            fraction,
            elapsed: p.elapsed,
            remaining,
            lines_done: p.lines_acked,
            total_lines: p.total_lines,
            errors: p.errors,
            framing: p.kind == grbl::JobKind::Frame,
        }
    }

    fn refresh(&mut self) {
        let Some(m) = &self.m else { return };
        let g = m.snapshot();
        let s = &mut self.snap;
        s.link = match &g.connection {
            grbl::ConnectionState::Disconnected => LinkState::Disconnected,
            grbl::ConnectionState::Connecting => LinkState::Connecting,
            grbl::ConnectionState::Connected => LinkState::Connected,
            grbl::ConnectionState::Failed(e) => LinkState::Failed(e.clone()),
        };
        s.state = Self::map_state(g.laser_status());
        s.port = g.port.clone();
        s.firmware = g.firmware.as_ref().map(|f| f.display_name()).unwrap_or_default();
        s.has_position = g.status.updated.is_some();
        s.machine_pos = g.status.mpos;
        s.work_pos = g.status.wpos;
        s.overrides = (g.status.overrides.feed, g.status.overrides.rapid, g.status.overrides.power);
        s.fire_on = g.fire_on;
        s.alarm = g.last_alarm.as_ref().filter(|_| g.alarm_locked || g.status.state.is_alarm()).map(|(_, m)| m.clone());
        s.grbl_settings = g.settings.clone();
        let job = g.job.as_ref().map(|p| self.progress(p));
        self.snap.job = job;
    }
}

impl DriverAdapter for GrblAdapter {
    fn poll(&mut self) -> Vec<MachineEvent> {
        let Some(m) = &self.m else { return Vec::new() };
        let mut out = Vec::new();
        for ev in m.poll_events() {
            use grbl::Event as E;
            match ev {
                E::Connected(fw) => out.push(MachineEvent::Connected(fw.display_name())),
                E::ConnectFailed(e) => out.push(MachineEvent::ConnectFailed(e)),
                E::Disconnected(r) => out.push(MachineEvent::Disconnected(r)),
                E::StateChanged(_) | E::JobProgress(_) | E::FireOff => {}
                E::Console(l) => out.push(MachineEvent::Console(ConsoleEntry {
                    kind: match l.kind {
                        grbl::ConsoleKind::Sent => ConsoleKind::Sent,
                        grbl::ConsoleKind::Received => ConsoleKind::Received,
                        grbl::ConsoleKind::Note => ConsoleKind::Note,
                    },
                    text: l.text,
                })),
                E::Alarm { message, .. } => out.push(MachineEvent::Alarm(message)),
                E::Error { message, line, .. } => out.push(MachineEvent::Error(format!("{message} (line: {line})"))),
                E::Settings(_) => out.push(MachineEvent::SettingsLoaded),
                E::SettingWritten { number, value, result } => out.push(MachineEvent::SettingWritten {
                    id: format!("${number}"),
                    ok: result.is_ok(),
                    message: result.err().unwrap_or_else(|| format!("${number}={value}")),
                }),
                E::JobStarted(k) => {
                    self.job_started = Some(Instant::now());
                    out.push(if k == grbl::JobKind::Frame { MachineEvent::FrameStarted } else { MachineEvent::JobStarted });
                }
                E::JobFinished(p) => {
                    self.job_started = None;
                    out.push(MachineEvent::JobFinished(self.progress(&p)));
                }
                E::JobLineError { line_number, line, message, .. } => {
                    out.push(MachineEvent::Error(format!("Line {line_number} ({line}): {message}")))
                }
                E::JobAborted { reason, .. } => {
                    self.job_started = None;
                    out.push(MachineEvent::JobAborted(reason));
                }
                E::Stopped => out.push(MachineEvent::Stopped),
                E::Homed => out.push(MachineEvent::Homed),
                E::Rejected(m) => out.push(MachineEvent::Error(m)),
            }
        }
        self.refresh();
        out
    }

    fn snapshot(&self) -> Snapshot {
        self.snap.clone()
    }

    fn command(&mut self, cmd: MachineCommand) -> Result<(), MachineError> {
        use MachineCommand as C;
        let m = self.machine()?;
        let unsupported = |what: &str| Err(MachineError::Unsupported(format!("{what} is for DSP controllers; this is a GRBL machine")));
        match cmd {
            C::StartJob(req) => {
                let JobData::Gcode(lines) = req.data else {
                    return Err(MachineError::Unsupported("a GRBL machine runs G-code, not controller files".into()));
                };
                let n = lines.len();
                let mut job = grbl::Job::from_lines(lines).with_total(n).with_stop_on_error(req.stop_on_error);
                if let (Some(b), Some(machine)) = (req.bounds, self.bed_rect()) {
                    job = job.with_bounds(grbl::BoundsCheck { job: b, machine });
                }
                job = job.with_finish(match req.finish {
                    Finish::Stay => grbl::FinishPosition::Stay,
                    Finish::Origin => grbl::FinishPosition::Origin,
                    Finish::Machine(p) => grbl::FinishPosition::Machine(p),
                });
                self.estimated = req.estimated;
                self.machine()?.start_job(job)?;
                Ok(())
            }
            C::Pause => Ok(m.pause()?),
            C::Resume => Ok(m.resume()?),
            C::Stop => Ok(m.stop()?),
            C::FrameRect(r, f) => Ok(m.frame_rect(r, grbl::FrameOptions { feed: f.feed, laser_percent: f.laser_percent })?),
            C::FrameOutline(p, f) => Ok(m.frame_outline(&p, grbl::FrameOptions { feed: f.feed, laser_percent: f.laser_percent })?),
            C::FrameOnController => unsupported("Framing on the controller"),
            C::JogStep(j) => {
                if j.du != 0.0 {
                    return Err(MachineError::Unsupported("this GRBL machine has no rotary axis".into()));
                }
                Ok(m.jog(grbl::JogStep { dx: j.dx, dy: j.dy, dz: j.dz, feed: j.feed })?)
            }
            C::JogStart { dir, feed } => Ok(m.jog_continuous(grbl::JogDirection { x: dir[0], y: dir[1], z: dir[2] }, feed)?),
            C::JogStop => Ok(m.jog_cancel()?),
            C::Home => Ok(m.home()?),
            C::Unlock => Ok(m.unlock()?),
            C::GoToOrigin => Ok(m.go_to_origin()?),
            C::MoveTo(t) => Ok(m.move_to(grbl::MoveTarget {
                x: t.x,
                y: t.y,
                z: t.z,
                coords: if t.machine_coords { grbl::Coords::Machine } else { grbl::Coords::Work },
                feed: t.feed,
            })?),
            C::SetUserOrigin => Ok(m.set_user_origin()?),
            C::ClearUserOrigin => Ok(m.clear_user_origin()?),
            C::GetPosition => Ok(m.request_status()?),
            C::Fire { percent, secs } => Ok(m.fire(percent, Duration::from_secs_f64(secs.max(0.0)))?),
            C::FireOff => Ok(m.fire_off()?),
            C::SetOverrides { feed, power, rapid } => {
                if let Some(f) = feed {
                    m.set_feed_override(f)?;
                }
                if let Some(p) = power {
                    m.set_power_override(p)?;
                }
                if let Some(r) = rapid {
                    m.set_rapid_override(match r {
                        0..=37 => grbl::RapidOverride::Quarter,
                        38..=75 => grbl::RapidOverride::Half,
                        _ => grbl::RapidOverride::Full,
                    })?;
                }
                Ok(())
            }
            C::Console(l) => Ok(m.send_console(&l)?),
            C::Macro(t) => Ok(m.run_macro(&t)?),
            C::ShowAll(on) => Ok(m.set_show_all(on)?),
            C::ReadSettings => Ok(m.read_settings()?),
            C::WriteSetting { id, value } => {
                let n = u16::try_from(id).map_err(|_| MachineError::Invalid(format!("${id} is not a GRBL setting")))?;
                Ok(m.write_setting(n, &value)?)
            }
            C::RestoreSettings(b) => match *b {
                SettingsBackup::Grbl(b) => Ok(m.restore_settings(&b)?),
                SettingsBackup::Ruida(_) => Err(MachineError::Invalid("That backup is from a Ruida controller.".into())),
            },
            C::ListFiles | C::RunFile(_) | C::DeleteFiles(_) | C::DeleteAllFiles | C::CalcTime(_) | C::AnswerOverwrite(_) => {
                unsupported("The controller file list")
            }
            C::ReadControllerInfo => unsupported("Controller info"),
            C::ReadLaser2 | C::WriteLaser2(_) => unsupported("Laser 2 settings"),
            C::ReadUAxis | C::WriteUAxis(_) => unsupported("U axis settings"),
            C::EmergencyStop => Ok(m.stop()?),
            C::Air(on) => Ok(m.run_macro(if on { "M8" } else { "M9" })?),
            C::PersistSettings | C::WriteGcodeSetting { .. } => Err(MachineError::Unsupported("GRBL settings are written with $N=value".into())),
            C::GalvoArm(_) | C::GalvoFrame { .. } | C::GalvoLight(_) | C::GalvoLens(_) | C::GalvoAxis { .. } => {
                Err(MachineError::Unsupported("that is a galvo command; this is a GRBL machine".into()))
            }
        }
    }

    fn close(&mut self) {
        if let Some(m) = self.m.take() {
            m.disconnect();
        }
        self.snap.link = LinkState::Disconnected;
        self.snap.state = LaserState::Disconnected;
    }
}

impl Drop for GrblAdapter {
    fn drop(&mut self) {
        self.close();
    }
}

// ============================================================================================
// Ruida
// ============================================================================================

enum Op {
    Connect,
    BedX,
    BedY,
    Job,
    Time(u16),
    Other(&'static str),
}

pub struct RuidaAdapter {
    h: ruida::RuidaHandle,
    simulator: bool,
    /// The link has the jog port (Ethernet only): the Fire button and the controller's own
    /// Frame need it.
    jog_port: bool,
    sim: Option<(Arc<Mutex<ruida::RuidaEmulator>>, f64)>,
    snap: Snapshot,
    pending: HashMap<u64, Op>,
    overwrite_default: Option<ruida::OverwriteChoice>,
    prompt: Option<u64>,
    job_started: Option<Instant>,
    job_name: String,
    job_estimated: Option<Duration>,
    saw_running: bool,
    last_advance: Instant,
    jog_keys: Vec<(ruida::Axis, bool)>,
    fire: Option<Instant>,
    framing_until: Option<Instant>,
    closed: bool,
    out: Vec<MachineEvent>,
}

impl RuidaAdapter {
    pub fn new(
        transport: Box<dyn ruida::Transport>,
        cfg: &ConnectConfig,
        simulator: bool,
        sim: Option<(Arc<Mutex<ruida::RuidaEmulator>>, f64)>,
    ) -> Self {
        use ruida::Transport as _;
        let jog_port = transport.framing() == ruida::Framing::Udp;
        let described = transport.describe();
        let machine = ruida::RuidaMachine::new(
            transport,
            ruida::MachineConfig {
                timeout: cfg
                    .net_timeout_ms
                    .map_or(ruida::MachineConfig::default().timeout, |ms| Duration::from_millis(u64::from(ms.clamp(100, 60_000)))),
                ..ruida::MachineConfig::default()
            },
        );
        let wc = ruida::WorkerConfig { status_interval: Some(Duration::from_millis(if simulator { 60 } else { 500 })) };
        let mut h = ruida::RuidaHandle::spawn(machine, wc);
        let id = h.request(ruida::Request::Connect);
        let mut pending = HashMap::new();
        pending.insert(id, Op::Connect);
        let snap = Snapshot {
            family: Family::Ruida,
            simulator,
            link: LinkState::Connecting,
            state: LaserState::Connecting,
            port: if simulator { "Simulator (Ruida)".into() } else { described },
            bed: cfg.bed,
            ..Default::default()
        };
        Self {
            h,
            simulator,
            jog_port,
            sim,
            snap,
            pending,
            overwrite_default: cfg.overwrite_default,
            prompt: None,
            job_started: None,
            job_name: String::new(),
            job_estimated: None,
            saw_running: false,
            last_advance: Instant::now(),
            jog_keys: Vec::new(),
            fire: None,
            framing_until: None,
            closed: false,
            out: Vec::new(),
        }
    }

    fn req(&mut self, r: ruida::Request, op: Op) -> Result<(), MachineError> {
        if self.closed || !matches!(self.snap.link, LinkState::Connected) {
            return Err(MachineError::NotConnected);
        }
        let id = self.h.request(r);
        self.pending.insert(id, op);
        Ok(())
    }

    fn note(&mut self, kind: ConsoleKind, text: impl Into<String>) {
        self.out.push(MachineEvent::Console(ConsoleEntry { kind, text: text.into() }));
    }

    fn bed(&self) -> Option<(f64, f64)> {
        // One axis may have been read before the other: a half-known bed is no bed.
        self.snap.bed.filter(|(w, h)| *w > 0.0 && *h > 0.0).or_else(|| self.snap.ruida.settings.as_ref().and_then(|s| s.bed_size_mm()))
    }

    fn finish_job(&mut self) {
        if let Some(t) = self.job_started.take() {
            let p = JobProgress { fraction: Some(1.0), elapsed: t.elapsed(), remaining: Some(Duration::ZERO), ..Default::default() };
            self.out.push(MachineEvent::JobFinished(p));
            self.saw_running = false;
        }
    }

    fn on_status(&mut self, s: ruida::MachineStatus) {
        use ruida::MachineState as S;
        self.snap.has_position = true;
        let mm = |v: i32| f64::from(v) / 1000.0;
        self.snap.machine_pos = [mm(s.x), mm(s.y), mm(s.z), mm(s.u)];
        self.snap.work_pos = self.snap.machine_pos;
        let framing = self.framing_until.is_some_and(|t| Instant::now() < t);
        self.snap.state = match s.state {
            S::Running => LaserState::Busy,
            S::Paused => LaserState::Paused,
            S::Moving if framing => LaserState::Framing,
            S::Moving => LaserState::Busy,
            S::Idle if framing => LaserState::Framing,
            S::Idle => LaserState::Ready,
        };
        if matches!(s.state, S::Running | S::Paused) {
            self.saw_running = true;
        }
        if self.job_started.is_some()
            && s.state == S::Idle
            && (self.saw_running || self.job_started.is_some_and(|t| t.elapsed() > Duration::from_secs(3)))
        {
            self.finish_job();
        }
    }

    fn on_reply(&mut self, id: u64, result: Result<ruida::Response, ruida::RuidaError>) {
        use ruida::Response as R;
        let op = self.pending.remove(&id).unwrap_or(Op::Other("request"));
        match result {
            Err(e) => {
                let msg = e.to_string();
                match op {
                    Op::Connect => {
                        self.snap.link = LinkState::Failed(msg.clone());
                        self.snap.state = LaserState::Disconnected;
                        self.out.push(MachineEvent::ConnectFailed(msg));
                    }
                    Op::Job => {
                        self.job_started = None;
                        self.snap.ruida.upload = None;
                        if e != ruida::RuidaError::Cancelled {
                            self.out.push(MachineEvent::Error(msg));
                        } else {
                            self.out.push(MachineEvent::JobAborted("cancelled".into()));
                        }
                    }
                    Op::Time(_) | Op::Other(_) | Op::BedX | Op::BedY => self.out.push(MachineEvent::Error(msg)),
                }
            }
            Ok(resp) => match resp {
                R::Connected(info) => {
                    self.snap.link = LinkState::Connected;
                    self.snap.state = LaserState::Ready;
                    self.snap.firmware = info.board.clone().map_or_else(|| info.firmware.clone(), |b| format!("{b} {}", info.firmware));
                    self.snap.ruida.info = Some(info.clone());
                    self.out.push(MachineEvent::Connected(self.snap.firmware.clone()));
                    // Two read-only register reads give the bed size for bounds checks.
                    let (bx, by) = (
                        self.h.request(ruida::Request::ReadSetting(ruida::settings::addr::BED_X)),
                        self.h.request(ruida::Request::ReadSetting(ruida::settings::addr::BED_Y)),
                    );
                    self.pending.insert(bx, Op::BedX);
                    self.pending.insert(by, Op::BedY);
                }
                R::Setting(v) if matches!(op, Op::BedX | Op::BedY) => {
                    let mm = f64::from(v) / 1000.0;
                    let cur = self.snap.bed.unwrap_or((0.0, 0.0));
                    self.snap.bed = Some(if matches!(op, Op::BedX) { (mm, cur.1) } else { (cur.0, mm) });
                }
                R::Status(s) => self.on_status(s),
                R::Files(f) => {
                    self.snap.ruida.files = Some(f);
                    self.out.push(MachineEvent::FilesListed);
                }
                R::Sent(o) => {
                    self.snap.ruida.upload = None;
                    self.snap.ruida.overwrite_prompt = None;
                    self.prompt = None;
                    self.note(ConsoleKind::Note, format!("Stored {} as file {}{}", o.name, o.index, if o.started { " and started it" } else { "" }));
                    if o.started {
                        self.job_started = Some(Instant::now());
                        self.saw_running = false;
                        self.out.push(MachineEvent::JobStarted);
                    } else {
                        self.out.push(MachineEvent::Done {
                            what: "send".into(),
                            ok: true,
                            message: format!("Stored {} ({} bytes)", o.name, o.bytes),
                        });
                    }
                    self.pending.insert(self.h.request(ruida::Request::ListFiles), Op::Other("list"));
                }
                R::Time(d) => {
                    if let Op::Time(i) = op {
                        self.snap.ruida.times.insert(i, d);
                    }
                    self.out.push(MachineEvent::Done { what: "calc time".into(), ok: true, message: format!("{:.1} s", d.as_secs_f64()) });
                }
                R::Setting(v) => self.out.push(MachineEvent::Done { what: "read setting".into(), ok: true, message: v.to_string() }),
                R::Settings(s) => {
                    self.snap.bed = self.snap.bed.or_else(|| s.bed_size_mm());
                    self.snap.ruida.settings = Some(s);
                    self.out.push(MachineEvent::SettingsLoaded);
                }
                R::Restored(skipped) => {
                    self.out.push(MachineEvent::Done { what: "restore settings".into(), ok: true, message: format!("{} skipped", skipped.len()) });
                    let id = self.h.request(ruida::Request::BackupSettings);
                    self.pending.insert(id, Op::Other("settings"));
                }
                R::Info(i) => {
                    self.snap.ruida.info = Some(i);
                    self.out.push(MachineEvent::InfoLoaded);
                }
                R::Laser2(l) => {
                    self.snap.ruida.laser2 = Some(l);
                    self.out.push(MachineEvent::InfoLoaded);
                }
                R::UAxis(u) => {
                    self.snap.ruida.u_axis = Some(u);
                    self.out.push(MachineEvent::InfoLoaded);
                }
                R::Done => {
                    if let Op::Job = op {
                        self.job_started = Some(Instant::now());
                        self.saw_running = false;
                        self.out.push(MachineEvent::JobStarted);
                    } else if let Op::Other(w) = op {
                        if w == "home" {
                            self.out.push(MachineEvent::Homed);
                        } else if w == "stop" {
                            self.out.push(MachineEvent::Stopped);
                        }
                        self.out.push(MachineEvent::Done { what: w.into(), ok: true, message: String::new() });
                    }
                }
            },
        }
    }
}

impl DriverAdapter for RuidaAdapter {
    fn poll(&mut self) -> Vec<MachineEvent> {
        if self.closed {
            return Vec::new();
        }
        // Simulator: run the stored job a little at a time so progress is visible.
        if let Some((emu, scale)) = &self.sim {
            let dt = self.last_advance.elapsed();
            self.last_advance = Instant::now();
            let n = (dt.as_secs_f64() * scale * 6.0).ceil() as usize;
            if let Ok(mut e) = emu.lock() {
                e.advance(n.max(1));
            }
        }
        if let Some(t) = self.fire
            && Instant::now() >= t
        {
            self.fire = None;
            let _ = self.h.request(ruida::Request::PanelKey { key: ruida::PanelKey::Pulse, down: false });
            self.snap.fire_on = false;
        }
        for ev in self.h.poll() {
            match ev {
                ruida::Event::Reply { id, result } => self.on_reply(id, result),
                ruida::Event::OverwritePrompt { id, name } => {
                    if let Some(c) = self.overwrite_default {
                        self.h.answer_overwrite(id, c);
                    } else {
                        self.prompt = Some(id);
                        self.snap.ruida.overwrite_prompt = Some(name.clone());
                        self.out.push(MachineEvent::OverwritePrompt(name));
                    }
                }
                ruida::Event::Progress { sent, total, .. } => {
                    self.snap.ruida.upload = Some((sent, total));
                }
                ruida::Event::Status(s) => self.on_status(s),
                ruida::Event::Stopped => {
                    self.closed = true;
                    self.out.push(MachineEvent::Disconnected(Some("the controller thread stopped".into())));
                }
            }
        }
        // Job progress from the simulator's job counter.
        if let (Some(t0), Some((emu, _))) = (self.job_started, &self.sim) {
            let p = emu.lock().ok().and_then(|e| e.job_progress());
            if let Some((pc, len)) = p {
                let f = pc as f64 / len.max(1) as f64;
                let el = t0.elapsed();
                self.snap.job = Some(JobProgress {
                    fraction: Some(f),
                    elapsed: el,
                    remaining: self.job_estimated.map(|e| e.saturating_sub(el)).or_else(|| (f > 0.02).then(|| el.mul_f64((1.0 - f) / f))),
                    lines_done: pc,
                    total_lines: Some(len),
                    errors: 0,
                    framing: false,
                });
            }
        } else if let Some(t0) = self.job_started {
            let el = t0.elapsed();
            self.snap.job = Some(JobProgress { elapsed: el, remaining: self.job_estimated.map(|e| e.saturating_sub(el)), ..Default::default() });
        }
        if self.job_started.is_none() {
            self.snap.job = None;
        }
        std::mem::take(&mut self.out)
    }

    fn snapshot(&self) -> Snapshot {
        self.snap.clone()
    }

    fn command(&mut self, cmd: MachineCommand) -> Result<(), MachineError> {
        use MachineCommand as C;
        use ruida::{Axis, Request as R};
        if self.closed {
            return Err(MachineError::NotConnected);
        }
        // Stop and pause go out in every state while connected.
        let always = matches!(cmd, C::Stop | C::Pause | C::FireOff | C::JogStop | C::AnswerOverwrite(_));
        if !self.snap.connected() && !always {
            return Err(MachineError::NotConnected);
        }
        let working = self.snap.state.is_working();
        let busy = |w: bool| if w { Err(MachineError::Busy) } else { Ok(()) };
        match cmd {
            C::StartJob(req) => {
                busy(working)?;
                if let (Some(b), Some((w, h))) = (req.bounds, self.bed())
                    && (b.x0 < -1e-6 || b.y0 < -1e-6 || b.x1 > w + 1e-6 || b.y1 > h + 1e-6)
                {
                    return Err(MachineError::OutOfBounds(format!(
                        "the job spans X {:.1}..{:.1}, Y {:.1}..{:.1} but the machine is {w:.0} x {h:.0} mm",
                        b.x0, b.x1, b.y0, b.y1
                    )));
                }
                self.job_estimated = req.estimated;
                self.job_name = req.name.clone();
                let options = ruida::SendOptions { auto_increment: req.auto_increment_name, start_immediately: req.start_immediately };
                self.note(ConsoleKind::Sent, format!("Send job {}", req.name));
                match req.data {
                    JobData::Gcode(_) => Err(MachineError::Unsupported("a DSP controller runs .rd jobs, not G-code".into())),
                    JobData::Galvo { .. } => Err(MachineError::Unsupported("a DSP controller cannot run a galvo job".into())),
                    JobData::Rd(job) => self.req(R::RunJobNow(Box::new(job)), Op::Job),
                    JobData::RdFile { name, data } => self.req(R::UploadRdFile { name, data, magic: None, options }, Op::Job),
                }
            }
            C::Pause => {
                self.note(ConsoleKind::Sent, "Pause");
                self.req(R::Pause, Op::Other("pause"))
            }
            C::Resume => {
                self.note(ConsoleKind::Sent, "Resume");
                self.req(R::Resume, Op::Other("resume"))
            }
            C::Stop => {
                self.note(ConsoleKind::Sent, "Stop");
                self.job_started = None;
                self.framing_until = None;
                self.snap.job = None;
                self.out.push(MachineEvent::JobAborted("stopped".into()));
                self.req(R::Stop, Op::Other("stop"))
            }
            C::FrameRect(r, f) => {
                busy(working)?;
                self.framing_until = Some(Instant::now() + Duration::from_millis(300));
                self.req(R::FrameRect { rect: r, light: f.laser_percent.is_some() }, Op::Other("frame"))?;
                self.out.push(MachineEvent::FrameStarted);
                Ok(())
            }
            C::FrameOutline(points, f) => {
                busy(working)?;
                self.framing_until = Some(Instant::now() + Duration::from_millis(300));
                self.req(R::FrameOutline { points, light: f.laser_percent.is_some() }, Op::Other("frame"))?;
                self.out.push(MachineEvent::FrameStarted);
                Ok(())
            }
            C::FrameOnController if !self.jog_port => Err(MachineError::Unsupported(
                "the controller's own Frame needs the Ethernet connection. Over USB, use Frame in DarkBurn or the Frame key on the controller."
                    .into(),
            )),
            C::FrameOnController => self.req(R::FrameOnController, Op::Other("frame")),
            C::JogStep(j) => {
                busy(working)?;
                for (axis, d) in [(Axis::X, j.dx), (Axis::Y, j.dy), (Axis::Z, j.dz), (Axis::U, j.du)] {
                    if d != 0.0 {
                        self.req(R::JogStep { axis, distance_mm: d }, Op::Other("jog"))?;
                    }
                }
                Ok(())
            }
            C::JogStart { dir, .. } => {
                busy(working)?;
                for (axis, d) in [Axis::X, Axis::Y, Axis::Z, Axis::U].into_iter().zip(dir) {
                    if d != 0 {
                        self.req(R::JogStart { axis, positive: d > 0 }, Op::Other("jog"))?;
                        self.jog_keys.push((axis, d > 0));
                    }
                }
                Ok(())
            }
            C::JogStop => {
                for (axis, positive) in std::mem::take(&mut self.jog_keys) {
                    self.req(R::JogStop { axis, positive }, Op::Other("jog"))?;
                }
                Ok(())
            }
            C::Home => {
                busy(working)?;
                self.req(R::Home(ruida::HomeAxes::XY), Op::Other("home"))
            }
            C::Unlock => Ok(()),
            C::GoToOrigin => {
                busy(working)?;
                self.req(R::GoToOrigin, Op::Other("go to origin"))
            }
            C::MoveTo(t) => {
                busy(working)?;
                let cur = self.snap.machine_pos;
                let to = Point::new(t.x.unwrap_or(cur[0]), t.y.unwrap_or(cur[1]));
                if t.x.is_some() || t.y.is_some() {
                    self.req(R::MoveTo { to, light: false }, Op::Other("move"))?;
                }
                if let Some(z) = t.z {
                    self.req(
                        R::Raw(vec![ruida::Command::RapidAxis { axis: Axis::Z, option: ruida::RapidOption::None, pos: (z * 1000.0).round() as i32 }]),
                        Op::Other("move"),
                    )?;
                }
                Ok(())
            }
            C::SetUserOrigin | C::ClearUserOrigin => {
                Err(MachineError::Unsupported("set the origin on the controller panel; this is not confirmed on real controllers yet".into()))
            }
            C::GetPosition => self.req(R::Status, Op::Other("status")),
            C::Fire { .. } if !self.jog_port => {
                Err(MachineError::Unsupported("the Fire button needs the Ethernet connection. Over USB, use the Pulse key on the controller.".into()))
            }
            C::Fire { percent, secs } => {
                let _ = percent;
                busy(working)?;
                if !(0.0..=10.0).contains(&secs) || secs <= 0.0 {
                    return Err(MachineError::Invalid("Fire time must be between 0 and 10 s.".into()));
                }
                self.req(R::PanelKey { key: ruida::PanelKey::Pulse, down: true }, Op::Other("fire"))?;
                self.fire = Some(Instant::now() + Duration::from_secs_f64(secs));
                self.snap.fire_on = true;
                Ok(())
            }
            C::FireOff if !self.jog_port => Ok(()),
            C::FireOff => {
                self.fire = None;
                self.snap.fire_on = false;
                self.req(R::PanelKey { key: ruida::PanelKey::Pulse, down: false }, Op::Other("fire off"))
            }
            C::SetOverrides { .. } => Err(MachineError::Unsupported("speed and power overrides are GRBL only".into())),
            C::Console(l) => {
                self.note(ConsoleKind::Note, format!("A DSP controller has no console; '{l}' was not sent."));
                Err(MachineError::Unsupported("the console is for G-code machines".into()))
            }
            C::Macro(_) => Err(MachineError::Unsupported("macros are for G-code machines".into())),
            C::ShowAll(_) => Ok(()),
            C::ReadSettings => self.req(R::BackupSettings, Op::Other("settings")),
            C::WriteSetting { id, value } => {
                let addr = u16::try_from(id).map_err(|_| MachineError::Invalid(format!("0x{id:X} is not a controller address")))?;
                let v: u32 = value.trim().parse().map_err(|_| MachineError::Invalid(format!("'{value}' is not a whole number")))?;
                self.req(R::WriteSetting { addr, value: v }, Op::Other("write setting"))
            }
            C::RestoreSettings(b) => match *b {
                SettingsBackup::Ruida(s) => self.req(R::RestoreSettings(s), Op::Other("restore")),
                SettingsBackup::Grbl(_) => Err(MachineError::Invalid("That backup is from a GRBL machine.".into())),
            },
            C::ListFiles => self.req(R::ListFiles, Op::Other("list")),
            C::RunFile(i) => {
                busy(working)?;
                self.job_started = Some(Instant::now());
                self.saw_running = false;
                self.out.push(MachineEvent::JobStarted);
                self.req(R::RunFile(i), Op::Other("run file"))
            }
            C::DeleteFiles(v) => {
                self.req(R::DeleteFiles(v), Op::Other("delete"))?;
                self.req(R::ListFiles, Op::Other("list"))
            }
            C::DeleteAllFiles => {
                self.req(R::DeleteAllFiles, Op::Other("delete all"))?;
                self.req(R::ListFiles, Op::Other("list"))
            }
            C::CalcTime(i) => self.req(R::CalcTime(i), Op::Time(i)),
            C::ReadControllerInfo => self.req(R::ControllerInfo, Op::Other("info")),
            C::ReadLaser2 => self.req(R::ReadLaser2, Op::Other("laser2")),
            C::WriteLaser2(s) => {
                self.req(R::WriteLaser2(s), Op::Other("write laser 2"))?;
                self.req(R::ReadLaser2, Op::Other("laser2"))
            }
            C::ReadUAxis => self.req(R::ReadUAxis, Op::Other("u axis")),
            C::WriteUAxis(s) => {
                self.req(R::WriteUAxis(s), Op::Other("write u axis"))?;
                self.req(R::ReadUAxis, Op::Other("u axis"))
            }
            C::AnswerOverwrite(c) => match self.prompt.take() {
                Some(id) => {
                    self.snap.ruida.overwrite_prompt = None;
                    self.h.answer_overwrite(id, c);
                    Ok(())
                }
                None => Err(MachineError::Invalid("No file name is waiting for an answer.".into())),
            },
            C::EmergencyStop => self.req(R::Stop, Op::Other("stop")),
            C::Air(_) | C::PersistSettings | C::WriteGcodeSetting { .. } => {
                Err(MachineError::Unsupported("that is a G-code controller command; this is a Ruida controller".into()))
            }
            C::GalvoArm(_) | C::GalvoFrame { .. } | C::GalvoLight(_) | C::GalvoLens(_) | C::GalvoAxis { .. } => {
                Err(MachineError::Unsupported("that is a galvo command; this is a Ruida controller".into()))
            }
        }
    }

    fn close(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        if self.snap.connected() {
            let _ = self.h.request(ruida::Request::Stop);
            let _ = self.h.request(ruida::Request::Disconnect);
        }
        self.h.shutdown();
        self.snap.link = LinkState::Disconnected;
        self.snap.state = LaserState::Disconnected;
        let _ = self.simulator;
    }
}

impl Drop for RuidaAdapter {
    fn drop(&mut self) {
        self.close();
    }
}
