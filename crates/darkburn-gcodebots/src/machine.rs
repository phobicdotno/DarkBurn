//! The [`GcodeMachine`] trait and the types shared by every G-code controller driver.
//!
//! The application holds a `Box<dyn GcodeMachine>` and does not care whether GRBL, Smoothieware,
//! Marlin or a custom device is behind it. Every implementation follows the threading model of
//! `darkburn_grbl::GrblMachine`: methods validate against the latest snapshot and return at
//! once, a worker thread owns the link, events are drained with
//! [`GcodeMachine::poll_events`] once per frame and the state is read with
//! [`GcodeMachine::snapshot`].

use std::collections::BTreeMap;
use std::fmt;
use std::time::{Duration, Instant};

use darkburn_grbl::{
    Activity, ConnectionState, ConsoleLine, FrameOptions, GrblError, Job, JobProgress, JogStep, LaserStatus, MachineState, MoveTarget,
};
use kurbo::{Point, Rect};

/// Which kind of controller is behind a [`GcodeMachine`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MachineFamily {
    Grbl,
    GrblHal,
    FluidNc,
    Smoothie,
    Marlin,
    Snapmaker,
    LinuxCnc,
    Custom,
}

impl MachineFamily {
    pub fn label(self) -> &'static str {
        match self {
            Self::Grbl => "GRBL",
            Self::GrblHal => "grblHAL",
            Self::FluidNc => "FluidNC",
            Self::Smoothie => "Smoothieware",
            Self::Marlin => "Marlin",
            Self::Snapmaker => "Snapmaker",
            Self::LinuxCnc => "LinuxCNC",
            Self::Custom => "Custom G-code",
        }
    }
}

/// What a controller can do, so the UI can grey out buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Capabilities {
    /// Pause holds the motion at once (a real feed hold) instead of only stopping new lines.
    pub hardware_pause: bool,
    /// A running jog can be cancelled.
    pub jog_cancel: bool,
    /// Real-time feed and power overrides.
    pub overrides: bool,
    /// The controller reports its own state (Idle/Run/Hold/Alarm) rather than the driver guessing.
    pub reports_state: bool,
    /// Settings can be read.
    pub read_settings: bool,
    /// Settings can be written.
    pub write_settings: bool,
    pub air_assist: bool,
    pub fire: bool,
    pub home: bool,
    /// Work origin can be set from the current position.
    pub user_origin: bool,
}

/// Errors returned straight from [`GcodeMachine`] methods.
#[derive(Clone, Debug, PartialEq)]
pub enum MachineError {
    NotConnected,
    NotReady,
    Busy,
    Alarm,
    NotIdle(MachineState),
    OutOfBounds(String),
    Unsupported(&'static str),
    InvalidArgument(String),
}

impl fmt::Display for MachineError {
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

impl std::error::Error for MachineError {}

impl From<GrblError> for MachineError {
    fn from(e: GrblError) -> Self {
        match e {
            GrblError::NotConnected => Self::NotConnected,
            GrblError::NotReady => Self::NotReady,
            GrblError::Busy => Self::Busy,
            GrblError::Alarm => Self::Alarm,
            GrblError::NotIdle(s) => Self::NotIdle(s),
            GrblError::OutOfBounds(m) => Self::OutOfBounds(m),
            GrblError::Unsupported(m) => Self::Unsupported(m),
            GrblError::InvalidArgument(m) => Self::InvalidArgument(m),
        }
    }
}

/// The latest machine state, refreshed by the worker. Positions are millimetres.
#[derive(Clone, Debug, Default)]
pub struct MachineSnapshot {
    pub connection: ConnectionState,
    pub port: String,
    pub family: Option<MachineFamily>,
    /// Firmware name and version as reported, e.g. `Smoothieware edge-94de3d9` or `GRBL 1.1h`.
    pub firmware: String,
    pub state: MachineState,
    pub mpos: [f64; 3],
    pub wpos: [f64; 3],
    /// Current feed (mm/min) and spindle/laser S value, when the controller reports them.
    pub feed: f64,
    pub spindle: f64,
    pub activity: Activity,
    pub paused: bool,
    pub job: Option<JobProgress>,
    /// Settings last read (`$N` for GRBL, config keys for Smoothieware, `M92.X` style for Marlin).
    pub settings: BTreeMap<String, String>,
    pub fire_on: bool,
    pub air_on: bool,
    /// Set while the controller is halted or in alarm until cleared.
    pub alarm: Option<String>,
    pub updated: Option<Instant>,
}

impl MachineSnapshot {
    /// Status for the Laser panel.
    pub fn laser_status(&self) -> LaserStatus {
        match self.connection {
            ConnectionState::Disconnected | ConnectionState::Failed(_) => return LaserStatus::Disconnected,
            ConnectionState::Connecting => return LaserStatus::Connecting,
            ConnectionState::Connected => {}
        }
        if self.alarm.is_some() || self.state.is_alarm() {
            return LaserStatus::Alarm;
        }
        match self.activity {
            Activity::Framing => LaserStatus::Framing,
            Activity::Job if self.paused => LaserStatus::Paused,
            Activity::Job | Activity::Stopping => LaserStatus::Busy,
            Activity::Idle => match self.state {
                MachineState::Hold(_) | MachineState::Door(_) => LaserStatus::Paused,
                MachineState::Run(_) | MachineState::Jog | MachineState::Home => LaserStatus::Busy,
                _ => LaserStatus::Ready,
            },
        }
    }

    pub fn is_connected(&self) -> bool {
        self.connection == ConnectionState::Connected
    }
}

/// Something that happened. Drain with [`GcodeMachine::poll_events`].
#[derive(Clone, Debug, PartialEq)]
pub enum MachineEvent {
    /// Handshake done. Carries the firmware name.
    Connected(String),
    ConnectFailed(String),
    /// The link closed: `None` when asked to, `Some(reason)` when lost.
    Disconnected(Option<String>),
    StateChanged(MachineState),
    Console(ConsoleLine),
    Alarm(String),
    /// An error reply for a console, macro or internal line.
    Error {
        message: String,
        line: String,
    },
    Settings(Vec<(String, String)>),
    SettingWritten {
        key: String,
        value: String,
        result: Result<(), String>,
    },
    JobStarted,
    JobProgress(JobProgress),
    JobFinished(JobProgress),
    JobLineError {
        line_number: usize,
        line: String,
        message: String,
    },
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

/// One interface for job start/pause/resume/stop, jog, home, frame, status, console and macros.
pub trait GcodeMachine: Send {
    fn family(&self) -> MachineFamily;
    fn capabilities(&self) -> Capabilities;

    /// Everything that happened since the last call. Never blocks.
    fn poll_events(&self) -> Vec<MachineEvent>;
    /// Waits up to `timeout` for the next event (tests, command-line tools).
    fn next_event(&self, timeout: Duration) -> Option<MachineEvent>;
    fn snapshot(&self) -> MachineSnapshot;

    // jobs
    fn start_job(&self, job: Job) -> Result<(), MachineError>;
    fn pause(&self) -> Result<(), MachineError>;
    fn resume(&self) -> Result<(), MachineError>;
    /// Stops the job, turns the laser off and empties what the controller can empty.
    fn stop(&self) -> Result<(), MachineError>;
    /// The controller's emergency stop (M112 on Smoothieware and Marlin, a soft reset on GRBL).
    /// Marlin must be reset by hand afterwards. Always allowed while connected.
    fn emergency_stop(&self) -> Result<(), MachineError>;

    // framing and motion
    /// Traces a closed outline in work coordinates and returns to the first point.
    fn frame_outline(&self, points: &[Point], opts: FrameOptions) -> Result<(), MachineError>;
    fn frame_rect(&self, rect: Rect, opts: FrameOptions) -> Result<(), MachineError> {
        self.frame_outline(
            &[Point::new(rect.x0, rect.y0), Point::new(rect.x1, rect.y0), Point::new(rect.x1, rect.y1), Point::new(rect.x0, rect.y1)],
            opts,
        )
    }
    fn jog(&self, step: JogStep) -> Result<(), MachineError>;
    fn jog_cancel(&self) -> Result<(), MachineError> {
        Ok(())
    }
    fn move_to(&self, target: MoveTarget) -> Result<(), MachineError>;
    fn go_to_origin(&self) -> Result<(), MachineError>;
    fn home(&self) -> Result<(), MachineError>;
    /// Clears an alarm or halt without homing (`$X`, `M999`).
    fn unlock(&self) -> Result<(), MachineError>;
    fn set_user_origin(&self) -> Result<(), MachineError>;
    fn clear_user_origin(&self) -> Result<(), MachineError>;
    fn request_status(&self) -> Result<(), MachineError>;

    // laser and accessories
    /// Fire button: laser on at `percent` for at most `duration`, capped by the driver's limits.
    fn fire(&self, percent: f64, duration: Duration) -> Result<(), MachineError>;
    fn fire_off(&self) -> Result<(), MachineError>;
    fn set_air(&self, on: bool) -> Result<(), MachineError>;
    fn set_feed_override(&self, _percent: u16) -> Result<(), MachineError> {
        Err(MachineError::Unsupported("feed override"))
    }
    fn set_power_override(&self, _percent: u16) -> Result<(), MachineError> {
        Err(MachineError::Unsupported("power override"))
    }

    // console, macros, settings
    fn send_console(&self, line: &str) -> Result<(), MachineError>;
    fn run_macro(&self, text: &str) -> Result<(), MachineError>;
    fn read_settings(&self) -> Result<(), MachineError>;
    fn write_setting(&self, key: &str, value: &str) -> Result<(), MachineError>;
    /// Stores the settings in the controller's non-volatile memory where there is such a step
    /// (Marlin `M500`).
    fn persist_settings(&self) -> Result<(), MachineError> {
        Ok(())
    }

    /// Stops any job, turns the laser off and closes the link.
    fn disconnect(self: Box<Self>);
}

impl dyn GcodeMachine + '_ {
    /// Polls the snapshot until `pred` holds or `timeout` passes (does not consume events).
    pub fn wait_until(&self, timeout: Duration, pred: impl Fn(&MachineSnapshot) -> bool) -> bool {
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
}
