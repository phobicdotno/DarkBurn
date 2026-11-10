//! [`LineMachine`]: one driver for controllers that pace G-code with `ok` replies and do no
//! character counting: Smoothieware, Marlin, Snapmaker and user-defined devices.
//!
//! # Threading model
//!
//! Same as `darkburn_grbl::GrblMachine`. [`LineMachine::connect`] moves the transport into one
//! worker thread and returns at once. The worker owns the link and does all I/O: handshake,
//! status polling, streaming, the stop sequence, the fire timer. Methods validate against the
//! latest snapshot and return `Err` straight away; the worker checks again and reports late
//! refusals as [`MachineEvent::Rejected`]. Events come back over a channel
//! ([`GcodeMachine::poll_events`]) and the state sits in a shared [`MachineSnapshot`].
//! The worker hands out the snapshot and the events that belong to it together, so the
//! snapshot read after an event already shows what the event reports.
//!
//! # Streaming
//!
//! A line is sent when fewer than `window_lines` lines are unanswered and the unanswered bytes
//! stay within `window_bytes`. A window of one line is plain send-and-wait. The controller
//! sends `ok` once a line is accepted into its planner, so a full planner holds the `ok` back
//! and the driver stops sending: that is the flow control. A job ends with the profile's
//! `finish_sync` command (`M400`) so "finished" means the motion is done.
//!
//! # Pause and stop
//!
//! Smoothieware pauses with its documented `suspend`/`resume` console commands. Marlin has no
//! pause for a streamed job: the driver stops sending (the motion already buffered still
//! runs). Stop uses the controller's quick stop where one exists (`M410` on Marlin, the reset
//! byte on Smoothieware), then switches the laser off and waits for the answers.

use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use darkburn_grbl::protocol::{clean_line, fmt_num, is_motion_line};
use darkburn_grbl::{
    Activity, ConnectionState, ConsoleKind, ConsoleLine, FinishPosition, FrameOptions, Job, JobKind, JobProgress, JogStep, MachineState, MoveTarget,
    TravelLimits,
};
use kurbo::Point;

use crate::detect::{FirmwareGuess, identify_lines};
use crate::machine::{Capabilities, GcodeMachine, MachineError, MachineEvent, MachineFamily, MachineSnapshot};
use crate::template::{jog_vars, num, render, render_lines};
use crate::translate::{TranslateOptions, parse_words, translate};

/// How the controller's position and state are polled.
#[derive(Clone, Debug, PartialEq)]
pub enum PollMode {
    /// No polling; the state is derived from what the driver sent.
    None,
    /// Send these raw bytes (no `ok` is expected) and parse `<State,MPos:...>` replies.
    /// Smoothieware: `?`.
    Raw(Vec<u8>),
    /// Send `M114` and parse the position (Marlin).
    M114,
}

/// How settings are read and written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsStyle {
    None,
    /// `cat /sd/config.txt` to read, `config-set sd <key> <value>` to write.
    SmoothieConfig,
    /// `M503` to read, `M92 X80`-style lines to write, `M500` to store.
    MarlinM503,
}

/// What Stop does to empty the controller.
#[derive(Clone, Debug, PartialEq)]
pub enum QuickStop {
    /// Nothing; wait for the lines in flight to be answered.
    None,
    /// Send a real-time byte (Smoothieware `0x18`). The unanswered lines are forgotten.
    Realtime(u8),
    /// Send a line and wait for its answer (Marlin `M410`).
    Line(String),
}

/// Command lines for the actions. Templates use `{name}` placeholders (see [`crate::template`]).
/// An empty string means the action is not available.
#[derive(Clone, Debug, PartialEq)]
pub struct Commands {
    pub home: String,
    /// Clears a halt or alarm.
    pub unlock: String,
    /// Placeholders `{dx} {dy} {dz} {feed}`.
    pub jog: String,
    pub pause: String,
    pub resume: String,
    /// Placeholders `{s}` (S value) and `{percent}`.
    pub fire_on: String,
    pub fire_off: String,
    pub air_on: String,
    pub air_off: String,
    /// Sent after every stop, in this order, to be sure the laser is off.
    pub laser_off: String,
    /// Sent after a quick stop (e.g. `M999` on Smoothieware to clear the halt).
    pub after_stop: String,
    pub emergency: String,
    pub set_origin: String,
    pub clear_origin: String,
    /// Feed override template, `{percent}`.
    pub feed_override: String,
    /// Stores the settings (`M500`).
    pub persist: String,
}

impl Default for Commands {
    fn default() -> Self {
        Self {
            home: "G28".into(),
            unlock: String::new(),
            jog: "G91\nG0 X{dx} Y{dy} Z{dz} F{feed}\nG90".into(),
            pause: String::new(),
            resume: String::new(),
            fire_on: "M3 S{s}".into(),
            fire_off: "M5".into(),
            air_on: String::new(),
            air_off: String::new(),
            laser_off: "M5".into(),
            after_stop: String::new(),
            emergency: "M112".into(),
            set_origin: "G92 X0 Y0".into(),
            clear_origin: "G92.1".into(),
            feed_override: String::new(),
            persist: String::new(),
        }
    }
}

/// Everything that makes one firmware different from another.
#[derive(Clone, Debug)]
pub struct LineConfig {
    pub family: MachineFamily,
    /// Line sent after the banner wait to identify the firmware (`version`, `M115`).
    pub ident: Option<String>,
    /// Firmwares this configuration accepts. An empty list accepts anything.
    pub accept: Vec<FirmwareGuess>,
    /// Lines in flight at most.
    pub window_lines: usize,
    /// Bytes in flight at most (the controller's RX buffer minus a margin).
    pub window_bytes: usize,
    pub poll: PollMode,
    pub poll_interval: Duration,
    pub banner_wait: Duration,
    pub handshake_timeout: Duration,
    /// S value for 100 % power.
    pub s_max: f64,
    pub travel: Option<TravelLimits>,
    pub max_fire_percent: f64,
    pub max_fire_duration: Duration,
    pub commands: Commands,
    pub quick_stop: QuickStop,
    /// Sent after the last job line; its answer ends the job (`M400`). Empty: end on the last `ok`.
    pub finish_sync: String,
    pub settings: SettingsStyle,
    /// Used to write framing moves. Scripts, homing and clustering are cleared.
    pub power: TranslateOptions,
    /// Firmware name for the Laser panel when the device does not report one.
    pub default_name: String,
    /// `G53` machine-coordinate moves exist.
    pub supports_g53: bool,
    /// Tell the driver the controller reports its own state in `<...>` reports.
    pub reports_state: bool,
    /// A halt cannot be cleared without power-cycling (Marlin `M112`).
    pub halt_needs_power_cycle: bool,
}

impl LineConfig {
    /// A bare configuration: plain `ok` streaming, no polling, one line at a time.
    pub fn plain(family: MachineFamily) -> Self {
        Self {
            family,
            ident: None,
            accept: Vec::new(),
            window_lines: 1,
            window_bytes: 100,
            poll: PollMode::None,
            poll_interval: Duration::from_millis(250),
            banner_wait: Duration::from_millis(60),
            handshake_timeout: Duration::from_secs(4),
            s_max: 1000.0,
            travel: None,
            max_fire_percent: 10.0,
            max_fire_duration: Duration::from_secs(10),
            commands: Commands::default(),
            quick_stop: QuickStop::None,
            finish_sync: "M400".into(),
            settings: SettingsStyle::None,
            power: TranslateOptions::default(),
            default_name: family.label().to_owned(),
            supports_g53: false,
            reports_state: false,
            halt_needs_power_cycle: false,
        }
    }

    fn capabilities(&self) -> Capabilities {
        let c = &self.commands;
        Capabilities {
            hardware_pause: matches!(self.family, MachineFamily::Smoothie) && !c.pause.trim().is_empty(),
            jog_cancel: false,
            overrides: !c.feed_override.trim().is_empty(),
            reports_state: self.reports_state,
            read_settings: self.settings != SettingsStyle::None,
            write_settings: self.settings != SettingsStyle::None,
            air_assist: !c.air_on.trim().is_empty(),
            fire: !c.fire_on.trim().is_empty(),
            home: !c.home.trim().is_empty(),
            user_origin: !c.set_origin.trim().is_empty(),
        }
    }
}

// ===================================================================================
// Handle
// ===================================================================================

#[derive(Clone, Debug, PartialEq)]
enum Tag {
    Ident,
    Job(usize),
    JobTail,
    JobSync,
    Plain,
    Console,
    Macro,
    Poll,
    Home,
    Unlock,
    SettingsRead,
    SettingWrite(String, String),
    StopStep,
    Fire,
}

enum Cmd {
    Line { text: String, tag: Tag },
    Lines { lines: Vec<String>, tag: Tag },
    Raw(Vec<u8>),
    StartJob(Job, JobKind),
    Stop,
    Emergency,
    Pause,
    Resume,
    Fire { lines: Vec<String>, until: Instant },
    FireOff,
    SetAir(bool),
    Disconnect,
}

/// Handle to a connected (or connecting) controller. See the module docs.
pub struct LineMachine {
    cmds: Sender<Cmd>,
    events: Receiver<MachineEvent>,
    shared: Arc<Mutex<MachineSnapshot>>,
    /// Set when a job or frame has been handed to the worker and the snapshot does not show it
    /// yet, so a second start is refused at once.
    starting: Arc<AtomicBool>,
    cfg: LineConfig,
    worker: Option<JoinHandle<()>>,
}

impl LineMachine {
    /// Starts the worker thread and the handshake. Returns at once; watch for
    /// [`MachineEvent::Connected`] or [`MachineEvent::ConnectFailed`].
    pub fn connect(transport: Box<dyn darkburn_grbl::Transport>, mut cfg: LineConfig) -> Self {
        cfg.window_lines = cfg.window_lines.max(1);
        cfg.window_bytes = cfg.window_bytes.max(16);
        cfg.poll_interval = cfg.poll_interval.max(Duration::from_millis(100));
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (ev_tx, ev_rx) = mpsc::channel();
        let snap = MachineSnapshot {
            connection: ConnectionState::Connecting,
            port: transport.name(),
            family: Some(cfg.family),
            firmware: cfg.default_name.clone(),
            ..Default::default()
        };
        let shared = Arc::new(Mutex::new(snap.clone()));
        let starting = Arc::new(AtomicBool::new(false));
        let worker = Worker::new(transport, cmd_rx, ev_tx, shared.clone(), starting.clone(), cfg.clone(), snap);
        let handle = std::thread::Builder::new().name("darkburn-gcode".into()).spawn(move || worker.run()).expect("spawn G-code worker thread");
        Self { cmds: cmd_tx, events: ev_rx, shared, starting, cfg, worker: Some(handle) }
    }

    pub fn config(&self) -> &LineConfig {
        &self.cfg
    }

    fn send(&self, cmd: Cmd) -> Result<(), MachineError> {
        self.cmds.send(cmd).map_err(|_| MachineError::NotConnected)
    }

    fn ready(&self) -> Result<MachineSnapshot, MachineError> {
        let s = self.snapshot();
        match s.connection {
            ConnectionState::Connected => Ok(s),
            ConnectionState::Connecting => Err(MachineError::NotReady),
            _ => Err(MachineError::NotConnected),
        }
    }

    fn free(&self) -> Result<MachineSnapshot, MachineError> {
        let s = self.ready()?;
        if s.activity != Activity::Idle || self.starting.load(Ordering::SeqCst) {
            return Err(MachineError::Busy);
        }
        Ok(s)
    }

    /// Claims the right to start a job or frame; the worker releases it.
    fn claim_start(&self) -> Result<(), MachineError> {
        self.starting.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).map(|_| ()).map_err(|_| MachineError::Busy)
    }

    fn send_start(&self, job: Job, kind: JobKind) -> Result<(), MachineError> {
        self.claim_start()?;
        self.send(Cmd::StartJob(job, kind)).inspect_err(|_| self.starting.store(false, Ordering::SeqCst))
    }

    fn motion_ok(&self) -> Result<MachineSnapshot, MachineError> {
        let s = self.free()?;
        if s.alarm.is_some() || s.state.is_alarm() {
            return Err(MachineError::Alarm);
        }
        if s.paused {
            return Err(MachineError::NotIdle(MachineState::Hold(0)));
        }
        Ok(s)
    }

    fn s_value(&self, percent: f64) -> Result<f64, MachineError> {
        if !(0.0..=self.cfg.max_fire_percent).contains(&percent) {
            return Err(MachineError::InvalidArgument(format!("Fire power must be between 0 and {} %.", self.cfg.max_fire_percent)));
        }
        Ok(percent / 100.0 * self.cfg.s_max)
    }

    fn in_travel(&self, p: Point) -> bool {
        self.cfg.travel.is_none_or(|t| {
            let e = 1e-6;
            p.x >= t.xy.x0 - e && p.x <= t.xy.x1 + e && p.y >= t.xy.y0 - e && p.y <= t.xy.y1 + e
        })
    }

    fn lines_of(&self, template: &str, vars: &[(&str, String)]) -> Vec<String> {
        render_lines(template, vars)
    }
}

impl Drop for LineMachine {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl LineMachine {
    fn shutdown(&mut self) {
        let _ = self.cmds.send(Cmd::Disconnect);
        if let Some(h) = self.worker.take() {
            let _ = h.join();
        }
    }
}

impl GcodeMachine for LineMachine {
    fn family(&self) -> MachineFamily {
        self.cfg.family
    }

    fn capabilities(&self) -> Capabilities {
        self.cfg.capabilities()
    }

    fn poll_events(&self) -> Vec<MachineEvent> {
        self.events.try_iter().collect()
    }

    fn next_event(&self, timeout: Duration) -> Option<MachineEvent> {
        self.events.recv_timeout(timeout).ok()
    }

    fn snapshot(&self) -> MachineSnapshot {
        self.shared.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    fn start_job(&self, job: Job) -> Result<(), MachineError> {
        self.motion_ok()?;
        if let Some(b) = &job.bounds {
            b.check().map_err(MachineError::from)?;
        }
        self.send_start(job, JobKind::Job)
    }

    fn pause(&self) -> Result<(), MachineError> {
        let s = self.ready()?;
        if s.activity == Activity::Idle {
            return Ok(());
        }
        self.send(Cmd::Pause)
    }

    fn resume(&self) -> Result<(), MachineError> {
        self.ready()?;
        self.send(Cmd::Resume)
    }

    fn stop(&self) -> Result<(), MachineError> {
        self.ready()?;
        self.send(Cmd::Stop)
    }

    fn emergency_stop(&self) -> Result<(), MachineError> {
        self.ready()?;
        if self.cfg.commands.emergency.trim().is_empty() {
            return self.send(Cmd::Stop);
        }
        self.send(Cmd::Emergency)
    }

    fn frame_outline(&self, points: &[Point], opts: FrameOptions) -> Result<(), MachineError> {
        self.motion_ok()?;
        if points.is_empty() {
            return Err(MachineError::InvalidArgument("Nothing to frame.".into()));
        }
        if opts.feed <= 0.0 {
            return Err(MachineError::InvalidArgument("Framing speed must be above 0.".into()));
        }
        for p in points {
            if !self.in_travel(*p) {
                return Err(MachineError::OutOfBounds(format!("the frame reaches X {:.1}, Y {:.1}, outside the machine", p.x, p.y)));
            }
        }
        let mut src = vec!["G21 G90".to_owned()];
        src.push(format!("G0 X{} Y{}", num(points[0].x), num(points[0].y)));
        if let Some(pct) = opts.laser_percent {
            self.s_value(pct)?;
            let s = pct * 10.0;
            src.push(format!("M3 S{}", num(s.round())));
            let f = num(opts.feed);
            for p in points.iter().skip(1).chain(std::iter::once(&points[0])) {
                src.push(format!("G1 X{} Y{} S{} F{f}", num(p.x), num(p.y), num(s.round())));
            }
            src.push("M5".to_owned());
        } else {
            let f = num(opts.feed);
            for p in points.iter().skip(1).chain(std::iter::once(&points[0])) {
                src.push(format!("G1 X{} Y{} F{f}", num(p.x), num(p.y)));
            }
        }
        let lines = translate(src.iter().map(String::as_str), &self.cfg.power);
        let n = lines.len();
        let job = Job::from_lines(lines).with_total(n);
        self.send_start(job, JobKind::Frame)
    }

    fn jog(&self, step: JogStep) -> Result<(), MachineError> {
        let s = self.motion_ok()?;
        if step.feed <= 0.0 {
            return Err(MachineError::InvalidArgument("Jog speed must be above 0.".into()));
        }
        if step.dx == 0.0 && step.dy == 0.0 && step.dz == 0.0 {
            return Ok(());
        }
        if self.cfg.commands.jog.trim().is_empty() {
            return Err(MachineError::Unsupported("jogging"));
        }
        if self.cfg.travel.is_some() {
            let target = Point::new(s.mpos[0] + step.dx, s.mpos[1] + step.dy);
            if !self.in_travel(target) {
                return Err(MachineError::OutOfBounds("the jog goes past the machine's travel".into()));
            }
        }
        let lines = self.lines_of(&self.cfg.commands.jog, &jog_vars(step.dx, step.dy, step.dz, step.feed));
        if s.fire_on {
            self.send(Cmd::FireOff)?;
        }
        self.send(Cmd::Lines { lines, tag: Tag::Plain })
    }

    fn move_to(&self, t: MoveTarget) -> Result<(), MachineError> {
        let s = self.motion_ok()?;
        if t.x.is_none() && t.y.is_none() && t.z.is_none() {
            return Ok(());
        }
        if t.feed <= 0.0 {
            return Err(MachineError::InvalidArgument("Move speed must be above 0.".into()));
        }
        if t.coords == darkburn_grbl::Coords::Machine && !self.cfg.supports_g53 {
            return Err(MachineError::Unsupported("moves in machine coordinates"));
        }
        let cur = s.wpos;
        let probe = Point::new(t.x.unwrap_or(cur[0]), t.y.unwrap_or(cur[1]));
        if t.coords == darkburn_grbl::Coords::Work && !self.in_travel(probe) && self.cfg.travel.is_some() && s.mpos == s.wpos {
            return Err(MachineError::OutOfBounds(format!("X {:.1}, Y {:.1} is outside the machine", probe.x, probe.y)));
        }
        let mut axes = String::new();
        for (l, v) in [('X', t.x), ('Y', t.y), ('Z', t.z)] {
            if let Some(v) = v {
                axes.push_str(&format!(" {l}{}", num(v)));
            }
        }
        let g53 = if t.coords == darkburn_grbl::Coords::Machine { "G53 " } else { "" };
        if s.fire_on {
            self.send(Cmd::FireOff)?;
        }
        self.send(Cmd::Lines { lines: vec!["G90".into(), format!("{g53}G0{axes} F{}", num(t.feed))], tag: Tag::Plain })
    }

    fn go_to_origin(&self) -> Result<(), MachineError> {
        self.motion_ok()?;
        self.send(Cmd::Lines { lines: vec!["G90".into(), "G0 X0 Y0".into()], tag: Tag::Plain })
    }

    fn home(&self) -> Result<(), MachineError> {
        let s = self.free()?;
        let lines = render_lines(&self.cfg.commands.home, &[]);
        if lines.is_empty() {
            return Err(MachineError::Unsupported("homing"));
        }
        if s.alarm.is_some() && self.cfg.halt_needs_power_cycle {
            return Err(MachineError::Alarm);
        }
        self.send(Cmd::Lines { lines, tag: Tag::Home })
    }

    fn unlock(&self) -> Result<(), MachineError> {
        let s = self.free()?;
        if s.alarm.is_some() && self.cfg.halt_needs_power_cycle {
            return Err(MachineError::Unsupported("this controller must be power-cycled after an emergency stop"));
        }
        let lines = render_lines(&self.cfg.commands.unlock, &[]);
        if lines.is_empty() {
            return Err(MachineError::Unsupported("unlocking"));
        }
        self.send(Cmd::Lines { lines, tag: Tag::Unlock })
    }

    fn set_user_origin(&self) -> Result<(), MachineError> {
        self.motion_ok()?;
        let lines = render_lines(&self.cfg.commands.set_origin, &[]);
        if lines.is_empty() {
            return Err(MachineError::Unsupported("setting the origin"));
        }
        self.send(Cmd::Lines { lines, tag: Tag::Plain })
    }

    fn clear_user_origin(&self) -> Result<(), MachineError> {
        self.motion_ok()?;
        let lines = render_lines(&self.cfg.commands.clear_origin, &[]);
        if lines.is_empty() {
            return Err(MachineError::Unsupported("clearing the origin"));
        }
        self.send(Cmd::Lines { lines, tag: Tag::Plain })
    }

    fn request_status(&self) -> Result<(), MachineError> {
        self.ready()?;
        match &self.cfg.poll {
            PollMode::Raw(b) => self.send(Cmd::Raw(b.clone())),
            PollMode::M114 => self.send(Cmd::Line { text: "M114".into(), tag: Tag::Poll }),
            PollMode::None => Ok(()),
        }
    }

    fn fire(&self, percent: f64, duration: Duration) -> Result<(), MachineError> {
        self.motion_ok()?;
        if self.cfg.commands.fire_on.trim().is_empty() {
            return Err(MachineError::Unsupported("the fire button"));
        }
        let s = self.s_value(percent)?;
        if duration.is_zero() || duration > self.cfg.max_fire_duration {
            return Err(MachineError::InvalidArgument(format!("Fire time must be between 0 and {} s.", self.cfg.max_fire_duration.as_secs_f64())));
        }
        let s_text = if self.cfg.s_max >= 50.0 { format!("{}", s.round() as i64) } else { fmt_num((s * 1000.0).round() / 1000.0) };
        let vars = [("s", s_text), ("percent", num(percent))];
        let lines = self.lines_of(&self.cfg.commands.fire_on, &vars);
        self.send(Cmd::Fire { lines, until: Instant::now() + duration })
    }

    fn fire_off(&self) -> Result<(), MachineError> {
        self.ready()?;
        self.send(Cmd::FireOff)
    }

    fn set_air(&self, on: bool) -> Result<(), MachineError> {
        self.free()?;
        let t = if on { &self.cfg.commands.air_on } else { &self.cfg.commands.air_off };
        if t.trim().is_empty() {
            return Err(MachineError::Unsupported("air assist"));
        }
        self.send(Cmd::SetAir(on))
    }

    fn set_feed_override(&self, percent: u16) -> Result<(), MachineError> {
        self.ready()?;
        if self.cfg.commands.feed_override.trim().is_empty() {
            return Err(MachineError::Unsupported("feed override"));
        }
        if !(10..=200).contains(&percent) {
            return Err(MachineError::InvalidArgument("Feed override must be 10 to 200 %.".into()));
        }
        let line = render(&self.cfg.commands.feed_override, &[("percent", percent.to_string())]);
        self.send(Cmd::Line { text: line, tag: Tag::Plain })
    }

    fn send_console(&self, line: &str) -> Result<(), MachineError> {
        let s = self.ready()?;
        let t = line.trim();
        if t.is_empty() {
            return Ok(());
        }
        if t.chars().any(|c| c.is_control()) {
            return Err(MachineError::InvalidArgument("A console line cannot hold control characters.".into()));
        }
        if t == "?" {
            return self.request_status();
        }
        if s.activity != Activity::Idle {
            return Err(MachineError::Busy);
        }
        if (s.alarm.is_some() || s.state.is_alarm()) && is_motion_line(t) {
            return Err(MachineError::Alarm);
        }
        self.send(Cmd::Line { text: t.to_owned(), tag: Tag::Console })
    }

    fn run_macro(&self, text: &str) -> Result<(), MachineError> {
        self.free()?;
        let lines: Vec<String> = text.lines().filter_map(clean_line).collect();
        if lines.is_empty() {
            return Ok(());
        }
        self.send(Cmd::Lines { lines, tag: Tag::Macro })
    }

    fn read_settings(&self) -> Result<(), MachineError> {
        self.free()?;
        let line = match self.cfg.settings {
            SettingsStyle::None => return Err(MachineError::Unsupported("reading settings")),
            SettingsStyle::SmoothieConfig => "cat /sd/config.txt",
            SettingsStyle::MarlinM503 => "M503",
        };
        self.send(Cmd::Line { text: line.into(), tag: Tag::SettingsRead })
    }

    fn write_setting(&self, key: &str, value: &str) -> Result<(), MachineError> {
        self.free()?;
        let (k, v) = (key.trim(), value.trim());
        let line = match self.cfg.settings {
            SettingsStyle::None => return Err(MachineError::Unsupported("writing settings")),
            SettingsStyle::SmoothieConfig => {
                if k.is_empty() || k.chars().any(|c| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))) {
                    return Err(MachineError::InvalidArgument(format!("'{key}' is not a config key.")));
                }
                if v.is_empty() || v.chars().any(|c| c.is_control() || c == ';' || c == '#') {
                    return Err(MachineError::InvalidArgument(format!("'{value}' is not a valid value.")));
                }
                format!("config-set sd {k} {v}")
            }
            SettingsStyle::MarlinM503 => {
                // Key `M92.X`: command M92, axis letter X.
                let (code, axis) = k.split_once('.').ok_or_else(|| MachineError::InvalidArgument(format!("'{key}' is not a setting like M92.X.")))?;
                let ok_code = code.len() >= 2 && code.starts_with('M') && code[1..].chars().all(|c| c.is_ascii_digit());
                let ok_axis = axis.len() == 1 && axis.chars().all(|c| c.is_ascii_uppercase());
                if !ok_code || !ok_axis {
                    return Err(MachineError::InvalidArgument(format!("'{key}' is not a setting like M92.X.")));
                }
                if v.parse::<f64>().is_err() {
                    return Err(MachineError::InvalidArgument(format!("{key}: '{value}' is not a number.")));
                }
                format!("{code} {axis}{v}")
            }
        };
        self.send(Cmd::Line { text: line, tag: Tag::SettingWrite(k.to_owned(), v.to_owned()) })
    }

    fn persist_settings(&self) -> Result<(), MachineError> {
        self.free()?;
        let lines = render_lines(&self.cfg.commands.persist, &[]);
        if lines.is_empty() {
            return Ok(());
        }
        self.send(Cmd::Lines { lines, tag: Tag::Plain })
    }

    fn disconnect(mut self: Box<Self>) {
        self.shutdown();
    }
}

// ===================================================================================
// Worker
// ===================================================================================

struct Sent {
    len: usize,
    tag: Tag,
    text: String,
}

struct Out {
    text: String,
    tag: Tag,
    /// Sent even when the window is full (pause, resume, fire off).
    urgent: bool,
}

struct ActiveJob {
    kind: JobKind,
    lines: Box<dyn Iterator<Item = String> + Send>,
    staged: Option<Out>,
    source_index: usize,
    exhausted: bool,
    tail: VecDeque<Out>,
    tail_built: bool,
    finish: FinishPosition,
    lines_sent: usize,
    lines_acked: usize,
    total: Option<usize>,
    started: Instant,
    errors: usize,
    stop_on_error: bool,
    sync_acked: bool,
}

impl ActiveJob {
    fn progress(&self) -> JobProgress {
        let elapsed = self.started.elapsed();
        let eta = match self.total {
            Some(t) if self.lines_acked > 0 => Some(elapsed.mul_f64(t.saturating_sub(self.lines_acked) as f64 / self.lines_acked as f64)),
            _ => None,
        };
        JobProgress {
            kind: self.kind,
            lines_sent: self.lines_sent,
            lines_acked: self.lines_acked,
            total_lines: self.total,
            elapsed,
            eta,
            errors: self.errors,
        }
    }

    fn stage(&mut self) {
        if self.staged.is_some() {
            return;
        }
        while !self.exhausted {
            match self.lines.next() {
                Some(raw) => {
                    self.source_index += 1;
                    if let Some(text) = clean_line(&raw) {
                        self.staged = Some(Out { text, tag: Tag::Job(self.source_index), urgent: false });
                        return;
                    }
                }
                None => self.exhausted = true,
            }
        }
        if let Some(o) = self.tail.pop_front() {
            self.staged = Some(o);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum StopPhase {
    Quick,
    Drain,
    Off,
}

struct StopSeq {
    phase: StopPhase,
    since: Instant,
    reason: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    WaitBanner { deadline: Instant },
    Identify { deadline: Instant },
    Ready,
}

struct Worker {
    t: Box<dyn darkburn_grbl::Transport>,
    cmds: Receiver<Cmd>,
    events: Sender<MachineEvent>,
    /// Events raised since the last [`Worker::publish`]. They go out together with the
    /// snapshot, so whoever receives an event finds the state it reports in the snapshot.
    outbox: RefCell<Vec<MachineEvent>>,
    shared: Arc<Mutex<MachineSnapshot>>,
    starting: Arc<AtomicBool>,
    start_seen: bool,
    cfg: LineConfig,
    snap: MachineSnapshot,
    rx_line: Vec<u8>,
    phase: Phase,
    inflight: VecDeque<Sent>,
    inflight_bytes: usize,
    queue: VecDeque<Out>,
    job: Option<ActiveJob>,
    stop: Option<StopSeq>,
    fire_until: Option<Instant>,
    last_poll: Instant,
    last_progress: Instant,
    banner_lines: Vec<String>,
    ident_lines: Vec<String>,
    collect: Vec<String>,
    unknown_pending: Option<String>,
    reckon_relative: bool,
}

enum Flow {
    Continue,
    Quit,
}

impl Worker {
    fn new(
        t: Box<dyn darkburn_grbl::Transport>,
        cmds: Receiver<Cmd>,
        events: Sender<MachineEvent>,
        shared: Arc<Mutex<MachineSnapshot>>,
        starting: Arc<AtomicBool>,
        cfg: LineConfig,
        snap: MachineSnapshot,
    ) -> Self {
        let now = Instant::now();
        Self {
            t,
            cmds,
            events,
            outbox: RefCell::new(Vec::new()),
            shared,
            starting,
            start_seen: false,
            phase: Phase::WaitBanner { deadline: now + cfg.banner_wait },
            cfg,
            snap,
            rx_line: Vec::new(),
            inflight: VecDeque::new(),
            inflight_bytes: 0,
            queue: VecDeque::new(),
            job: None,
            stop: None,
            fire_until: None,
            last_poll: now,
            last_progress: now,
            banner_lines: Vec::new(),
            ident_lines: Vec::new(),
            collect: Vec::new(),
            unknown_pending: None,
            reckon_relative: false,
        }
    }

    /// Queues an event. It is delivered by the next [`Worker::publish`].
    fn emit(&self, e: MachineEvent) {
        self.outbox.borrow_mut().push(e);
    }

    fn note(&self, text: impl Into<String>) {
        self.emit(MachineEvent::Console(ConsoleLine { kind: ConsoleKind::Note, text: text.into() }));
    }

    /// Stores the snapshot and delivers the queued events as one step: the events are sent
    /// while the snapshot lock is held, so a reader never sees an event before the state it
    /// reports, nor the new state before its events are waiting.
    fn publish(&mut self) {
        self.snap.updated = Some(Instant::now());
        {
            let mut shared = self.shared.lock().unwrap_or_else(|p| p.into_inner());
            *shared = self.snap.clone();
            for e in self.outbox.get_mut().drain(..) {
                let _ = self.events.send(e);
            }
        }
        if self.start_seen {
            self.start_seen = false;
            self.starting.store(false, Ordering::SeqCst);
        }
    }

    fn run(mut self) {
        self.run_loop();
        // Whatever ended the loop, the last state and its events go out.
        self.publish();
    }

    fn run_loop(&mut self) {
        let mut buf = [0u8; 512];
        loop {
            loop {
                match self.cmds.try_recv() {
                    Ok(c) => {
                        if matches!(self.handle(c), Flow::Quit) {
                            return;
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        self.shutdown(None);
                        return;
                    }
                }
            }
            match self.t.read(&mut buf, Duration::from_millis(2)) {
                Ok(0) => {}
                Ok(n) => self.ingest(&buf[..n]),
                Err(e) => {
                    self.lost(&e.to_string());
                    return;
                }
            }
            if self.tick().is_err() {
                return;
            }
            self.publish();
        }
    }

    // ----- connection end -----

    fn lost(&mut self, reason: &str) {
        self.abort_job(&format!("The connection was lost: {reason}"));
        self.snap.connection = ConnectionState::Failed(reason.to_owned());
        self.snap.activity = Activity::Idle;
        self.emit(MachineEvent::Disconnected(Some(reason.to_owned())));
        self.publish();
    }

    fn shutdown(&mut self, _why: Option<&str>) {
        self.abort_job("Disconnected");
        if self.snap.connection == ConnectionState::Connected {
            // Make sure the laser is off before letting go.
            for l in render_lines(&self.cfg.commands.laser_off, &[]) {
                let _ = self.t.write_all(format!("{l}\n").as_bytes());
            }
            if self.snap.fire_on {
                for l in render_lines(&self.cfg.commands.fire_off, &[]) {
                    let _ = self.t.write_all(format!("{l}\n").as_bytes());
                }
            }
        }
        self.snap.connection = ConnectionState::Disconnected;
        self.snap.activity = Activity::Idle;
        self.emit(MachineEvent::Disconnected(None));
        self.publish();
    }

    // ----- commands -----

    fn handle(&mut self, c: Cmd) -> Flow {
        match c {
            Cmd::Disconnect => {
                self.shutdown(None);
                return Flow::Quit;
            }
            Cmd::Raw(b) => {
                if self.write(&b).is_err() {
                    return Flow::Quit;
                }
            }
            Cmd::Line { text, tag } => self.enqueue(text, tag),
            Cmd::Lines { lines, tag } => {
                for l in lines {
                    self.enqueue(l, tag.clone());
                }
            }
            Cmd::StartJob(job, kind) => self.start_job(job, kind),
            Cmd::Stop => self.begin_stop("Stopped"),
            Cmd::Emergency => self.emergency(),
            Cmd::Pause => self.pause(),
            Cmd::Resume => self.resume(),
            Cmd::Fire { lines, until } => {
                for l in lines {
                    self.enqueue(l, Tag::Fire);
                }
                self.snap.fire_on = true;
                self.fire_until = Some(until);
            }
            Cmd::FireOff => self.fire_off(),
            Cmd::SetAir(on) => {
                let t = if on { self.cfg.commands.air_on.clone() } else { self.cfg.commands.air_off.clone() };
                for l in render_lines(&t, &[]) {
                    self.enqueue(l, Tag::Plain);
                }
                self.snap.air_on = on;
            }
        }
        Flow::Continue
    }

    fn enqueue(&mut self, text: String, tag: Tag) {
        if self.snap.alarm.is_some() && is_motion_line(&text) && !matches!(tag, Tag::StopStep) {
            self.emit(MachineEvent::Rejected(format!("'{text}' was not sent: the controller is halted.")));
            return;
        }
        if matches!(tag, Tag::Console | Tag::Macro) {
            self.emit(MachineEvent::Console(ConsoleLine { kind: ConsoleKind::Sent, text: text.clone() }));
        }
        self.queue.push_back(Out { text, tag, urgent: false });
    }

    fn start_job(&mut self, job: Job, kind: JobKind) {
        self.start_seen = true;
        if self.job.is_some() || self.stop.is_some() {
            self.emit(MachineEvent::Rejected("A job is already running.".into()));
            return;
        }
        let Job { lines, total_lines, finish, stop_on_error, .. } = job;
        self.job = Some(ActiveJob {
            kind,
            lines,
            staged: None,
            source_index: 0,
            exhausted: false,
            tail: VecDeque::new(),
            tail_built: false,
            finish,
            lines_sent: 0,
            lines_acked: 0,
            total: total_lines,
            started: Instant::now(),
            errors: 0,
            stop_on_error,
            sync_acked: false,
        });
        self.snap.activity = if kind == JobKind::Frame { Activity::Framing } else { Activity::Job };
        self.snap.paused = false;
        self.snap.job = self.job.as_ref().map(ActiveJob::progress);
        self.emit(MachineEvent::JobStarted);
    }

    fn pause(&mut self) {
        if self.job.is_none() || self.snap.paused {
            return;
        }
        self.snap.paused = true;
        for l in render_lines(&self.cfg.commands.pause, &[]) {
            self.enqueue_front(l);
        }
    }

    fn resume(&mut self) {
        if !self.snap.paused {
            return;
        }
        self.snap.paused = false;
        for l in render_lines(&self.cfg.commands.resume, &[]) {
            self.enqueue_front(l);
        }
    }

    fn enqueue_front(&mut self, text: String) {
        self.queue.push_front(Out { text, tag: Tag::Plain, urgent: true });
    }

    fn fire_off(&mut self) {
        self.fire_until = None;
        if self.snap.fire_on || self.stop.is_none() {
            // Drop queued fire-on lines first so they cannot switch the laser on afterwards.
            self.queue.retain(|o| o.tag != Tag::Fire);
            for l in render_lines(&self.cfg.commands.fire_off, &[]) {
                self.queue.push_front(Out { text: l, tag: Tag::StopStep, urgent: true });
            }
        }
        self.snap.fire_on = false;
        self.emit(MachineEvent::FireOff);
    }

    fn emergency(&mut self) {
        let lines = render_lines(&self.cfg.commands.emergency, &[]);
        self.queue.clear();
        for l in lines {
            let _ = self.write_line(&l, Tag::StopStep);
        }
        self.snap.alarm = Some("Emergency stop".to_owned());
        self.snap.state = MachineState::Alarm(None);
        self.snap.fire_on = false;
        self.fire_until = None;
        self.stop = None;
        self.abort_job("Emergency stop");
        self.snap.activity = Activity::Idle;
        self.emit(MachineEvent::Alarm("Emergency stop".to_owned()));
    }

    /// Ends the active job early. Emits `JobAborted` when there was one.
    fn abort_job(&mut self, reason: &str) {
        if let Some(j) = self.job.take() {
            let progress = j.progress();
            self.snap.job = Some(progress.clone());
            self.emit(MachineEvent::JobAborted { progress, reason: reason.to_owned() });
        }
        self.snap.paused = false;
        if self.stop.is_none() {
            self.snap.activity = Activity::Idle;
        }
    }

    fn begin_stop(&mut self, reason: &str) {
        if self.stop.is_some() {
            return;
        }
        self.queue.clear();
        self.fire_until = None;
        self.snap.fire_on = false;
        self.snap.paused = false;
        self.snap.activity = Activity::Stopping;
        // The job object stays until the stop ends so JobAborted carries the final progress.
        if let Some(j) = self.job.as_mut() {
            j.staged = None;
            j.exhausted = true;
            j.tail.clear();
        }
        let mut phase = StopPhase::Drain;
        match self.cfg.quick_stop.clone() {
            QuickStop::None => {}
            QuickStop::Realtime(b) => {
                // Resume first when a suspend is active: a reset also clears it, but be sure.
                if self.write(&[b]).is_err() {
                    return;
                }
                self.inflight.clear();
                self.inflight_bytes = 0;
                self.snap.alarm = None;
                phase = StopPhase::Quick;
            }
            QuickStop::Line(l) => {
                let _ = self.write_line(&l, Tag::StopStep);
            }
        }
        self.stop = Some(StopSeq { phase, since: Instant::now(), reason: reason.to_owned() });
    }

    // ----- low level I/O -----

    fn write(&mut self, data: &[u8]) -> Result<(), ()> {
        if let Err(e) = self.t.write_all(data) {
            self.lost(&e.to_string());
            return Err(());
        }
        Ok(())
    }

    fn write_line(&mut self, text: &str, tag: Tag) -> Result<(), ()> {
        let wire = format!("{text}\n");
        self.write(wire.as_bytes())?;
        if self.cfg.poll == PollMode::None {
            self.reckon(text);
        }
        self.inflight_bytes += wire.len();
        self.inflight.push_back(Sent { len: wire.len(), tag, text: text.to_owned() });
        Ok(())
    }

    /// Dead reckoning for controllers that cannot report a position: follows the X/Y/Z words
    /// of the lines sent (G90/G91, G92, homing). It is the commanded position, not a measurement.
    fn reckon(&mut self, text: &str) {
        let Some(words) = parse_words(&strip_comment_simple(text)) else { return };
        let mut rel = self.reckon_relative;
        let mut pos = self.snap.mpos;
        let (mut set_origin, mut home) = (false, false);
        for (l, v) in &words {
            if *l == 'G' {
                match v.parse::<f64>().ok() {
                    Some(g) if (g - 90.0).abs() < 1e-9 => rel = false,
                    Some(g) if (g - 91.0).abs() < 1e-9 => rel = true,
                    Some(g) if (g - 92.0).abs() < 1e-9 || (g - 28.0).abs() < 1e-9 => {
                        set_origin = (g - 92.0).abs() < 1e-9;
                        home = !set_origin;
                    }
                    _ => {}
                }
            }
        }
        if text.trim_start().starts_with("$H") {
            home = true;
        }
        self.reckon_relative = rel;
        if home {
            pos = [0.0; 3];
        } else {
            for (l, v) in &words {
                let Some(i) = ['X', 'Y', 'Z'].iter().position(|a| a == l) else { continue };
                let Ok(v) = v.parse::<f64>() else { continue };
                pos[i] = if set_origin || !rel { v } else { pos[i] + v };
            }
        }
        self.snap.mpos = pos;
        self.snap.wpos = pos;
    }

    fn ingest(&mut self, data: &[u8]) {
        for &b in data {
            if b == b'\n' || b == b'\r' {
                if !self.rx_line.is_empty() {
                    let line = String::from_utf8_lossy(&self.rx_line).trim().to_owned();
                    self.rx_line.clear();
                    if !line.is_empty() {
                        self.on_line(&line);
                    }
                }
            } else if self.rx_line.len() < 4096 {
                self.rx_line.push(b);
            }
        }
    }

    // ----- reading -----

    fn on_line(&mut self, line: &str) {
        let low = line.to_ascii_lowercase();
        if line.starts_with('<') && line.ends_with('>') {
            self.on_status(line);
            return;
        }
        if low == "ok" || low.starts_with("ok ") || low.starts_with("ok:") {
            if let Some(rest) = line.get(2..)
                && (rest.contains("C:") || rest.contains("X:"))
            {
                self.parse_position(rest, false);
            }
            self.on_ack(None);
            return;
        }
        if low == "!!" || low.starts_with("alarm") || low.contains("printer halted") || low.contains("kill() called") {
            self.on_halt(line);
            return;
        }
        if low.starts_with("error") {
            let msg = line.get(5..).map(|s| s.trim_start_matches(':').trim()).unwrap_or("");
            if self.cfg.family == MachineFamily::Marlin || self.cfg.family == MachineFamily::Snapmaker {
                // Marlin's `Error:` lines describe a problem; the command still gets its `ok`.
                self.console_in(line);
                return;
            }
            self.on_ack(Some(msg.to_owned()));
            return;
        }
        if low == "start" && self.phase == Phase::Ready {
            self.console_in(line);
            self.note("The controller restarted.");
            self.inflight.clear();
            self.inflight_bytes = 0;
            self.abort_job("The controller restarted");
            self.queue.clear();
            self.snap.activity = Activity::Idle;
            self.stop = None;
            return;
        }
        if low.starts_with("echo:unknown command") {
            self.unknown_pending = Some(line.to_owned());
            self.console_in(line);
            return;
        }
        if low.contains("busy: processing") || low.contains("busy:processing") {
            return;
        }
        if low.starts_with("x:") && low.contains("count") {
            self.parse_position(line, true);
            return;
        }
        match self.phase {
            Phase::WaitBanner { .. } => self.banner_lines.push(line.to_owned()),
            Phase::Identify { .. } => self.ident_lines.push(line.to_owned()),
            Phase::Ready => {}
        }
        if self.inflight.front().is_some_and(|s| s.tag == Tag::SettingsRead) {
            self.collect.push(line.to_owned());
            return;
        }
        self.console_in(line);
    }

    fn console_in(&self, line: &str) {
        self.emit(MachineEvent::Console(ConsoleLine { kind: ConsoleKind::Received, text: line.to_owned() }));
    }

    fn on_halt(&mut self, line: &str) {
        self.console_in(line);
        let first = self.snap.alarm.is_none();
        self.snap.alarm = Some(line.to_owned());
        self.snap.state = MachineState::Alarm(None);
        self.snap.fire_on = false;
        self.fire_until = None;
        self.queue.clear();
        if self.stop.is_none() || !matches!(self.cfg.quick_stop, QuickStop::Realtime(_)) {
            // A halt flushes the planner; unanswered lines will never be answered.
            self.inflight.clear();
            self.inflight_bytes = 0;
        }
        if self.stop.is_none() {
            self.abort_job(line);
        }
        if first {
            self.emit(MachineEvent::Alarm(line.to_owned()));
        }
    }

    fn parse_position(&mut self, text: &str, marlin: bool) {
        // `X:1.00 Y:2.00 Z:0.00 E:0.00 Count X:...` or ` C: X:1.0 Y:2.0 Z:0.0`.
        let head = text.split("Count").next().unwrap_or(text);
        let mut pos = self.snap.mpos;
        let mut found = false;
        for tok in head.split_whitespace() {
            let tok = tok.trim_start_matches("C:");
            if let Some((k, v)) = tok.split_once(':')
                && let Ok(v) = v.parse::<f64>()
            {
                match k {
                    "X" => pos[0] = v,
                    "Y" => pos[1] = v,
                    "Z" => pos[2] = v,
                    _ => continue,
                }
                found = true;
            }
        }
        if found {
            self.snap.mpos = pos;
            if marlin || self.snap.mpos == self.snap.wpos || self.snap.wpos == [0.0; 3] {
                self.snap.wpos = pos;
            }
        }
    }

    fn on_status(&mut self, line: &str) {
        let Some((state, mpos, wpos, feed)) = parse_status(line) else {
            self.console_in(line);
            return;
        };
        if let Some(m) = mpos {
            self.snap.mpos = m;
        }
        if let Some(w) = wpos {
            self.snap.wpos = w;
        } else if let Some(m) = mpos {
            self.snap.wpos = m;
        }
        if let Some(f) = feed {
            self.snap.feed = f;
        }
        if state != self.snap.state {
            self.snap.state = state.clone();
            self.emit(MachineEvent::StateChanged(state.clone()));
        }
        if state.is_alarm() {
            if self.snap.alarm.is_none() {
                self.snap.alarm = Some("Alarm".to_owned());
                self.emit(MachineEvent::Alarm("Alarm".to_owned()));
            }
        } else if self.snap.alarm.is_some() && self.phase == Phase::Ready {
            // The controller says it is no longer in alarm.
            self.snap.alarm = None;
        }
    }

    fn on_ack(&mut self, error: Option<String>) {
        let Some(sent) = self.inflight.pop_front() else {
            // An extra `ok` (for a raw status query, say). Nothing to match.
            return;
        };
        self.inflight_bytes = self.inflight_bytes.saturating_sub(sent.len);
        let err_text = error.clone();
        match sent.tag {
            Tag::Ident => self.finish_identify(),
            Tag::Job(idx) => {
                let mut stop_reason = None;
                if let Some(j) = self.job.as_mut() {
                    j.lines_acked += 1;
                    if let Some(msg) = &err_text {
                        j.errors += 1;
                        if j.stop_on_error {
                            stop_reason = Some(format!("The controller refused line {idx}: {msg}"));
                        }
                    }
                }
                if let Some(msg) = err_text.clone() {
                    self.emit(MachineEvent::JobLineError { line_number: idx, line: sent.text.clone(), message: msg });
                } else if let Some(u) = self.unknown_pending.take() {
                    if let Some(j) = self.job.as_mut() {
                        j.errors += 1;
                    }
                    self.emit(MachineEvent::JobLineError { line_number: idx, line: sent.text.clone(), message: u });
                }
                if let Some(r) = stop_reason {
                    self.begin_stop(&r);
                    if let Some(s) = self.stop.as_mut() {
                        s.reason = r;
                    }
                }
                self.progress_event(false);
            }
            Tag::JobSync => {
                if let Some(j) = self.job.as_mut() {
                    j.sync_acked = true;
                }
            }
            Tag::JobTail | Tag::Plain | Tag::StopStep | Tag::Fire | Tag::Macro | Tag::Console => {
                self.unknown_pending = None;
                if let Some(msg) = err_text {
                    self.emit(MachineEvent::Error { message: msg, line: sent.text });
                }
            }
            Tag::Poll => {}
            Tag::Home => {
                if let Some(msg) = err_text {
                    self.emit(MachineEvent::Error { message: msg, line: sent.text });
                } else if self.inflight.iter().all(|s| s.tag != Tag::Home) && !self.queue.iter().any(|o| o.tag == Tag::Home) {
                    self.emit(MachineEvent::Homed);
                }
            }
            Tag::Unlock => {
                if let Some(msg) = err_text {
                    self.emit(MachineEvent::Error { message: msg, line: sent.text });
                } else if self.inflight.iter().all(|s| s.tag != Tag::Unlock) && !self.queue.iter().any(|o| o.tag == Tag::Unlock) {
                    self.snap.alarm = None;
                    if self.snap.state.is_alarm() {
                        self.snap.state = MachineState::Idle;
                    }
                }
            }
            Tag::SettingsRead => {
                let lines = std::mem::take(&mut self.collect);
                let values = match self.cfg.settings {
                    SettingsStyle::SmoothieConfig => parse_smoothie_config(&lines),
                    SettingsStyle::MarlinM503 => parse_marlin_settings(&lines),
                    SettingsStyle::None => Vec::new(),
                };
                self.snap.settings = values.iter().cloned().collect::<BTreeMap<_, _>>();
                self.emit(MachineEvent::Settings(values));
            }
            Tag::SettingWrite(key, value) => {
                let result = match err_text {
                    Some(m) => Err(m),
                    None => {
                        self.snap.settings.insert(key.clone(), value.clone());
                        Ok(())
                    }
                };
                self.emit(MachineEvent::SettingWritten { key, value, result });
            }
        }
    }

    fn progress_event(&mut self, force: bool) {
        if let Some(j) = self.job.as_ref() {
            let p = j.progress();
            self.snap.job = Some(p.clone());
            if force || self.last_progress.elapsed() >= Duration::from_millis(50) {
                self.last_progress = Instant::now();
                self.emit(MachineEvent::JobProgress(p));
            }
        }
    }

    // ----- handshake -----

    fn finish_identify(&mut self) {
        let mut lines = self.banner_lines.clone();
        lines.extend(self.ident_lines.clone());
        let r = identify_lines(&lines);
        if !self.cfg.accept.is_empty() && r.firmware != FirmwareGuess::Unknown && !self.cfg.accept.contains(&r.firmware) {
            self.fail(format!("This is not a {} controller; it looks like {:?} ({}).", self.cfg.family.label(), r.firmware, r.evidence));
            return;
        }
        let name = match (&r.version, r.firmware) {
            (Some(v), fw) if fw != FirmwareGuess::Unknown => format!("{} {v}", self.cfg.family.label()),
            _ => self.cfg.default_name.clone(),
        };
        if r.firmware == FirmwareGuess::Unknown {
            self.note("The controller answered but did not say which firmware it is.");
        }
        self.connected(name);
    }

    fn connected(&mut self, name: String) {
        self.phase = Phase::Ready;
        self.snap.connection = ConnectionState::Connected;
        self.snap.firmware = name.clone();
        self.snap.state = MachineState::Idle;
        self.emit(MachineEvent::Connected(name));
    }

    fn fail(&mut self, msg: String) {
        self.snap.connection = ConnectionState::Failed(msg.clone());
        self.phase = Phase::Ready;
        self.emit(MachineEvent::ConnectFailed(msg));
        self.publish();
    }

    // ----- the loop body -----

    fn tick(&mut self) -> Result<(), ()> {
        let now = Instant::now();
        match self.phase {
            Phase::WaitBanner { deadline } => {
                if now >= deadline {
                    match self.cfg.ident.clone() {
                        Some(l) => {
                            self.phase = Phase::Identify { deadline: now + self.cfg.handshake_timeout };
                            self.write_line(&l, Tag::Ident)?;
                        }
                        None => self.connected(self.cfg.default_name.clone()),
                    }
                }
                return Ok(());
            }
            Phase::Identify { deadline } => {
                if now >= deadline {
                    self.fail("The controller did not answer. Check the port, the baud rate and the cable.".into());
                    return Err(());
                }
                return Ok(());
            }
            Phase::Ready => {}
        }
        if matches!(self.snap.connection, ConnectionState::Failed(_)) {
            return Err(());
        }
        // Fire timer.
        if let Some(u) = self.fire_until
            && now >= u
        {
            self.fire_off();
        }
        self.tick_stop(now);
        self.poll(now)?;
        self.pump_send()?;
        self.check_job_done();
        self.derive_state();
        Ok(())
    }

    fn poll(&mut self, now: Instant) -> Result<(), ()> {
        if now.duration_since(self.last_poll) < self.cfg.poll_interval {
            return Ok(());
        }
        match self.cfg.poll.clone() {
            PollMode::None => {}
            PollMode::Raw(b) => {
                self.last_poll = now;
                self.write(&b)?;
            }
            PollMode::M114 => {
                let busy = self.inflight.iter().any(|s| s.tag == Tag::Poll) || self.queue.iter().any(|o| o.tag == Tag::Poll);
                if !busy && self.stop.is_none() && !(self.snap.alarm.is_some() && self.cfg.halt_needs_power_cycle) {
                    self.last_poll = now;
                    self.queue.push_back(Out { text: "M114".into(), tag: Tag::Poll, urgent: false });
                }
            }
        }
        Ok(())
    }

    fn can_send(&self, len: usize) -> bool {
        if self.inflight.is_empty() {
            return true;
        }
        self.inflight.len() < self.cfg.window_lines && self.inflight_bytes + len <= self.cfg.window_bytes
    }

    fn pump_send(&mut self) -> Result<(), ()> {
        if self.snap.alarm.is_some() && self.cfg.halt_needs_power_cycle {
            return Ok(());
        }
        // A settings read must be alone on the wire.
        if self.inflight.front().is_some_and(|s| s.tag == Tag::SettingsRead) {
            return Ok(());
        }
        loop {
            if let Some(front) = self.queue.front() {
                let len = front.text.len() + 1;
                let alone = front.tag == Tag::SettingsRead || matches!(front.tag, Tag::Home);
                if (alone && !self.inflight.is_empty()) || (!front.urgent && !self.can_send(len)) {
                    return Ok(());
                }
                let Some(o) = self.queue.pop_front() else { return Ok(()) };
                let solo = o.tag == Tag::SettingsRead;
                self.write_line(&o.text, o.tag)?;
                if solo {
                    return Ok(());
                }
                continue;
            }
            // Job lines: not while stopping, paused or halted.
            if self.stop.is_some() || self.snap.paused || self.snap.alarm.is_some() {
                return Ok(());
            }
            let Some(job) = self.job.as_mut() else { return Ok(()) };
            if job.staged.is_none() {
                job.stage();
            }
            if job.staged.is_none() && !job.tail_built {
                job.tail_built = true;
                let mut tail = VecDeque::new();
                let fin = job.finish;
                match fin {
                    FinishPosition::Stay => {}
                    FinishPosition::Origin => {
                        tail.push_back(Out { text: "G90".into(), tag: Tag::JobTail, urgent: false });
                        tail.push_back(Out { text: "G0 X0 Y0".into(), tag: Tag::JobTail, urgent: false });
                    }
                    FinishPosition::Machine(p) => {
                        tail.push_back(Out { text: "G90".into(), tag: Tag::JobTail, urgent: false });
                        tail.push_back(Out { text: format!("G0 X{} Y{}", num(p.x), num(p.y)), tag: Tag::JobTail, urgent: false });
                    }
                }
                for l in render_lines(&self.cfg.commands.laser_off, &[]) {
                    tail.push_back(Out { text: l, tag: Tag::JobTail, urgent: false });
                }
                let sync = self.cfg.finish_sync.trim();
                if !sync.is_empty() {
                    tail.push_back(Out { text: sync.to_owned(), tag: Tag::JobSync, urgent: false });
                }
                job.tail = tail;
                job.exhausted = true;
                job.stage();
            }
            let Some(staged) = job.staged.as_ref() else { return Ok(()) };
            let len = staged.text.len() + 1;
            if !self.can_send(len) {
                return Ok(());
            }
            let Some(job) = self.job.as_mut() else { return Ok(()) };
            let Some(o) = job.staged.take() else { return Ok(()) };
            if matches!(o.tag, Tag::Job(_)) {
                job.lines_sent += 1;
            }
            self.write_line(&o.text, o.tag)?;
        }
    }

    fn check_job_done(&mut self) {
        if self.stop.is_some() {
            return;
        }
        let done = match self.job.as_ref() {
            Some(j) => {
                let no_sync = self.cfg.finish_sync.trim().is_empty();
                j.exhausted
                    && j.tail_built
                    && j.staged.is_none()
                    && j.tail.is_empty()
                    && !self.inflight.iter().any(|s| matches!(s.tag, Tag::Job(_) | Tag::JobTail | Tag::JobSync))
                    && (j.sync_acked || no_sync)
            }
            None => false,
        };
        if done && let Some(j) = self.job.take() {
            let p = j.progress();
            self.snap.job = Some(p.clone());
            self.snap.activity = Activity::Idle;
            self.emit(MachineEvent::JobFinished(p));
        }
    }

    fn tick_stop(&mut self, now: Instant) {
        let Some(s) = self.stop.as_mut() else { return };
        let waited = now.duration_since(s.since);
        match s.phase {
            StopPhase::Quick => {
                // Give the reset a moment, then continue with the clean-up lines.
                if waited >= Duration::from_millis(30) {
                    s.phase = StopPhase::Drain;
                    s.since = now;
                }
            }
            StopPhase::Drain => {
                if self.inflight.is_empty() || waited >= Duration::from_secs(3) {
                    self.inflight.clear();
                    self.inflight_bytes = 0;
                    let mut lines = render_lines(&self.cfg.commands.after_stop, &[]);
                    lines.extend(render_lines(&self.cfg.commands.laser_off, &[]));
                    lines.extend(render_lines(&self.cfg.commands.fire_off, &[]));
                    if let Some(s) = self.stop.as_mut() {
                        s.phase = StopPhase::Off;
                        s.since = now;
                    }
                    for l in lines {
                        self.queue.push_back(Out { text: l, tag: Tag::StopStep, urgent: true });
                    }
                }
            }
            StopPhase::Off => {
                if (self.inflight.is_empty() && self.queue.is_empty()) || waited >= Duration::from_secs(3) {
                    let reason = s.reason.clone();
                    self.inflight.clear();
                    self.inflight_bytes = 0;
                    self.queue.clear();
                    self.stop = None;
                    if !self.cfg.commands.after_stop.trim().is_empty() && !self.cfg.halt_needs_power_cycle {
                        self.snap.alarm = None;
                        if self.snap.state.is_alarm() {
                            self.snap.state = MachineState::Idle;
                        }
                    }
                    self.abort_job(&reason);
                    self.snap.activity = Activity::Idle;
                    self.emit(MachineEvent::Stopped);
                }
            }
        }
    }

    fn derive_state(&mut self) {
        if self.cfg.reports_state && matches!(self.cfg.poll, PollMode::Raw(_)) && !self.snap.paused {
            // The controller reports its own state; keep it but add the driver's knowledge.
            if self.snap.alarm.is_some() && !self.snap.state.is_alarm() {
                self.snap.state = MachineState::Alarm(None);
            }
            return;
        }
        let new = if self.snap.alarm.is_some() {
            MachineState::Alarm(None)
        } else if self.snap.paused {
            MachineState::Hold(0)
        } else if self.inflight.iter().any(|s| s.tag == Tag::Home) {
            MachineState::Home
        } else if self.job.is_some() || self.inflight.iter().any(|s| !matches!(s.tag, Tag::Poll)) || !self.queue.is_empty() {
            MachineState::Run(0)
        } else {
            MachineState::Idle
        };
        if new != self.snap.state {
            self.snap.state = new.clone();
            self.emit(MachineEvent::StateChanged(new));
        }
    }
}

fn strip_comment_simple(t: &str) -> String {
    clean_line(t).unwrap_or_default()
}

// ===================================================================================
// Parsers
// ===================================================================================

type StatusParts = (MachineState, Option<[f64; 3]>, Option<[f64; 3]>, Option<f64>);

fn floats_after(inner: &str, key: &str) -> Option<[f64; 3]> {
    let i = inner.find(key)?;
    let rest = &inner[i + key.len()..];
    let mut out = [0.0; 3];
    let mut n = 0;
    for tok in rest.split([',', '|']) {
        if n == 3 {
            break;
        }
        match tok.trim().parse::<f64>() {
            Ok(v) => {
                out[n] = v;
                n += 1;
            }
            Err(_) => break,
        }
    }
    (n >= 2).then_some(out)
}

/// Parses `<Idle,MPos:0.0000,0.0000,0.0000,WPos:0.0000,0.0000,0.0000>` and the `|` form.
pub fn parse_status(line: &str) -> Option<StatusParts> {
    let inner = line.trim().strip_prefix('<')?.strip_suffix('>')?;
    let state_text = inner.split([',', '|']).next()?.trim();
    if state_text.is_empty() {
        return None;
    }
    let state = MachineState::parse(state_text);
    let mpos = floats_after(inner, "MPos:");
    let wpos = floats_after(inner, "WPos:");
    let feed = inner.find("F:").and_then(|i| inner[i + 2..].split([',', '|', '>']).next()).and_then(|v| v.trim().parse::<f64>().ok());
    Some((state, mpos, wpos, feed))
}

/// Parses the text of `cat /sd/config.txt`: `key value # comment` per line.
pub fn parse_smoothie_config(lines: &[String]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for l in lines {
        let t = l.split('#').next().unwrap_or("").trim();
        if t.is_empty() {
            continue;
        }
        let mut it = t.splitn(2, char::is_whitespace);
        let (Some(k), Some(v)) = (it.next(), it.next()) else { continue };
        out.push((k.to_owned(), v.trim().to_owned()));
    }
    out
}

/// Parses `M503` output: `echo:  M92 X80.00 Y80.00 Z400.00` becomes `M92.X`, `M92.Y`, `M92.Z`.
pub fn parse_marlin_settings(lines: &[String]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for l in lines {
        let t = l.trim().strip_prefix("echo:").unwrap_or(l.trim());
        let t = t.split(';').next().unwrap_or("").trim();
        let mut toks = t.split_whitespace();
        let Some(code) = toks.next() else { continue };
        let is_code = code.len() >= 2 && matches!(code.as_bytes()[0], b'M' | b'G') && code[1..].chars().all(|c| c.is_ascii_digit());
        if !is_code {
            continue;
        }
        for tok in toks {
            let mut ch = tok.chars();
            if let Some(a) = ch.next().filter(char::is_ascii_uppercase) {
                let v: String = ch.collect();
                if v.parse::<f64>().is_ok() {
                    out.push((format!("{code}.{a}"), v));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_both_styles() {
        let (s, m, w, _) = parse_status("<Idle,MPos:1.0000,2.0000,3.0000,WPos:0.5000,1.5000,3.0000>").unwrap();
        assert_eq!(s, MachineState::Idle);
        assert_eq!(m, Some([1.0, 2.0, 3.0]));
        assert_eq!(w, Some([0.5, 1.5, 3.0]));
        let (s, m, _, f) = parse_status("<Run|MPos:1.0,2.0,3.0|WPos:1.0,2.0,3.0|F:6000.0>").unwrap();
        assert_eq!(s, MachineState::Run(0));
        assert_eq!(m, Some([1.0, 2.0, 3.0]));
        assert_eq!(f, Some(6000.0));
        assert!(parse_status("<>").is_none());
    }

    #[test]
    fn smoothie_config_lines() {
        let l: Vec<String> =
            ["# Smoothieware", "alpha_steps_per_mm 80  # steps", "", "laser_module_enable true"].iter().map(|s| s.to_string()).collect();
        assert_eq!(parse_smoothie_config(&l), vec![("alpha_steps_per_mm".into(), "80".into()), ("laser_module_enable".into(), "true".into())]);
    }

    #[test]
    fn marlin_m503_lines() {
        let l: Vec<String> =
            ["echo:; Steps per unit:", "echo:  M92 X80.00 Y80.00 Z400.00", "echo:  M211 S1 ; Min X0.00"].iter().map(|s| s.to_string()).collect();
        let v = parse_marlin_settings(&l);
        assert!(v.contains(&("M92.X".to_owned(), "80.00".to_owned())));
        assert!(v.contains(&("M92.Z".to_owned(), "400.00".to_owned())));
        assert!(v.contains(&("M211.S".to_owned(), "1".to_owned())));
    }
}
