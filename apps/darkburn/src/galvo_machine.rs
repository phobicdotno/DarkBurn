//! The galvo board behind [`MachineHandle`]: [`GalvoAdapter`] over `darkburn_galvo::GalvoMachine`
//! [GALVO-01, GALVO-09..13, LASER-21].
//!
//! * The simulator link ([`open_simulator`]) runs against the library's in-memory board. There is
//!   no real USB link in this build: `Link::GalvoUsb` is refused with an explanation until the
//!   hardware checks in `docs/hardware-checks-galvo.md` are done.
//! * The laser is never enabled without an explicit arm ([`MachineCommand::GalvoArm`]); a job
//!   refuses to start unarmed. The arm is spent when the job ends or stops. A red-light frame
//!   contains no marking commands and never needs an arm.
//! * Coordinates: the app's bed for a galvo device is the square field, bottom-left origin, mm.
//!   The board takes field-centred millimetres, so the adapter shifts by half the field.

#![allow(dead_code, reason = "the MCP backend and some helpers use parts of this file")]

use std::sync::Arc;
use std::time::{Duration, Instant};

use darkburn_galvo as galvo;
use kurbo::{Point, Rect};

use crate::galvo_settings::GalvoDeviceSettings;
use crate::machine_api::{
    ConnectConfig, ConsoleEntry, ConsoleKind, DriverAdapter, Family, JobData, JobProgress, LaserState, LinkState, MachineCommand, MachineError,
    MachineEvent, Snapshot,
};

/// What the galvo panel shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GalvoStatus {
    /// The laser will be enabled by the next job.
    pub armed: bool,
    pub activity: String,
    /// The last red-light lap that completed, by the job it frames.
    pub framed_key: Option<u64>,
    pub door_open: bool,
    pub foot_pressed: bool,
    pub lens_loaded: bool,
    pub serial: Option<[u16; 4]>,
    pub version: Option<u16>,
    pub last_error: Option<String>,
    pub require_framing: bool,
    pub packets: (usize, usize),
    pub estimate_s: f64,
}

/// The bed of a galvo device: the square field.
pub fn bed_of(s: &GalvoDeviceSettings) -> (f64, f64) {
    (s.field_size_mm, s.field_size_mm)
}

/// Bed coordinates (bottom-left origin) to the field-centred coordinates of the board.
pub fn to_field(p: Point, bed: (f64, f64)) -> Point {
    Point::new(p.x - bed.0 / 2.0, p.y - bed.1 / 2.0)
}

/// The key that ties a frame to a job: the bounds in field coordinates.
pub fn key_for_bounds(r: Rect) -> u64 {
    galvo::bounds_key(r)
}

fn map_err(e: galvo::GalvoError) -> MachineError {
    use galvo::GalvoError as G;
    match e {
        G::NotConnected => MachineError::NotConnected,
        G::Busy => MachineError::Busy,
        G::NotArmed | G::FramingRequired | G::DoorOpen | G::Unsafe(_) => MachineError::Refused(e.to_string()),
        G::Invalid(m) => MachineError::Invalid(m),
    }
}

pub struct GalvoAdapter {
    m: Option<galvo::GalvoMachine>,
    settings: Arc<GalvoDeviceSettings>,
    simulator: bool,
    snap: Snapshot,
    job_started: Option<Instant>,
    out: Vec<MachineEvent>,
}

impl GalvoAdapter {
    pub fn new(transport: Box<dyn galvo::Transport>, settings: Arc<GalvoDeviceSettings>, simulator: bool) -> Self {
        let lens = match settings.load_lens() {
            Ok(l) => l,
            Err(e) => {
                log::warn!("lens file not loaded: {e}");
                None
            }
        };
        let port = transport.name();
        let opts = galvo::ConnectOptions { lens, ..Default::default() };
        let m = galvo::GalvoMachine::connect(transport, settings.to_library(), opts);
        let bed = bed_of(&settings);
        let snap = Snapshot {
            family: Family::Galvo,
            simulator,
            link: LinkState::Connecting,
            state: LaserState::Connecting,
            port,
            bed: Some(bed),
            galvo: Some(GalvoStatus { require_framing: settings.require_framing, ..Default::default() }),
            ..Default::default()
        };
        Self { m: Some(m), settings, simulator, snap, job_started: None, out: Vec::new() }
    }

    fn machine(&self) -> Result<&galvo::GalvoMachine, MachineError> {
        self.m.as_ref().ok_or(MachineError::NotConnected)
    }

    fn bed(&self) -> (f64, f64) {
        bed_of(&self.settings)
    }

    fn note(&mut self, kind: ConsoleKind, text: impl Into<String>) {
        self.out.push(MachineEvent::Console(ConsoleEntry { kind, text: text.into() }));
    }

    fn refresh(&mut self) {
        let Some(m) = &self.m else { return };
        let g = m.snapshot();
        let s = &mut self.snap;
        s.link = match &g.connection {
            galvo::ConnectionState::Disconnected => LinkState::Disconnected,
            galvo::ConnectionState::Connecting => LinkState::Connecting,
            galvo::ConnectionState::Connected => LinkState::Connected,
            galvo::ConnectionState::Failed(e) => LinkState::Failed(e.clone()),
        };
        s.state = match (&g.connection, g.activity) {
            (galvo::ConnectionState::Connecting, _) => LaserState::Connecting,
            (galvo::ConnectionState::Connected, a) => match a {
                galvo::Activity::Idle => LaserState::Ready,
                galvo::Activity::Framing => LaserState::Framing,
                galvo::Activity::Job | galvo::Activity::WaitingDoor | galvo::Activity::WaitingFoot => LaserState::Busy,
                galvo::Activity::Paused | galvo::Activity::PausedDoor => LaserState::Paused,
            },
            _ => LaserState::Disconnected,
        };
        s.port = g.transport.clone();
        s.firmware = match g.version {
            Some(v) => format!("Galvo board v{v}"),
            None => String::new(),
        };
        s.job = g.progress.filter(|_| matches!(s.state, LaserState::Busy | LaserState::Paused)).map(|p| JobProgress {
            fraction: (p.packets_total > 0).then(|| p.packets_sent as f64 / p.packets_total as f64),
            elapsed: p.elapsed,
            remaining: (p.estimate_s > 0.0).then(|| Duration::from_secs_f64((p.estimate_s - p.elapsed.as_secs_f64()).max(0.0))),
            lines_done: p.packets_sent,
            total_lines: Some(p.packets_total),
            errors: 0,
            framing: false,
        });
        s.alarm = g.last_error.clone().filter(|_| matches!(g.connection, galvo::ConnectionState::Failed(_)));
        s.galvo = Some(GalvoStatus {
            armed: g.armed,
            activity: format!("{:?}", g.activity),
            framed_key: g.framed_key,
            door_open: g.inputs.door_open,
            foot_pressed: g.inputs.foot_pressed,
            lens_loaded: g.lens_loaded,
            serial: g.serial,
            version: g.version,
            last_error: g.last_error,
            require_framing: self.settings.require_framing,
            packets: g.progress.map(|p| (p.packets_sent, p.packets_total)).unwrap_or_default(),
            estimate_s: g.progress.map(|p| p.estimate_s).unwrap_or_default(),
        });
    }
}

impl DriverAdapter for GalvoAdapter {
    fn poll(&mut self) -> Vec<MachineEvent> {
        let Some(m) = &self.m else { return Vec::new() };
        let events = m.poll_events();
        for ev in events {
            use galvo::Event as E;
            match ev {
                E::Connected { serial, version } => {
                    self.out.push(MachineEvent::Connected(format!("Galvo board v{version} ({:04X}{:04X})", serial[0], serial[1])));
                }
                E::Disconnected(r) => self.out.push(MachineEvent::Disconnected(r)),
                E::JobStarted => {
                    self.job_started = Some(Instant::now());
                    self.out.push(MachineEvent::JobStarted);
                }
                E::JobFinished { elapsed } => {
                    self.job_started = None;
                    self.out.push(MachineEvent::JobFinished(JobProgress {
                        fraction: Some(1.0),
                        elapsed,
                        remaining: Some(Duration::ZERO),
                        ..Default::default()
                    }));
                }
                E::JobStopped => {
                    self.job_started = None;
                    self.out.push(MachineEvent::JobAborted("stopped".into()));
                    self.out.push(MachineEvent::Stopped);
                }
                E::JobFailed(m) => {
                    self.job_started = None;
                    self.out.push(MachineEvent::JobAborted(m));
                }
                E::FrameStarted => self.out.push(MachineEvent::FrameStarted),
                E::FrameLap => self.note(ConsoleKind::Note, "Red-light lap done"),
                E::FrameStopped => self.note(ConsoleKind::Note, "Red-light frame stopped"),
                E::WaitingForFoot => self.note(ConsoleKind::Note, "Waiting for the foot switch"),
                E::WaitingForDoor => self.note(ConsoleKind::Note, "Waiting for the door to close"),
                E::DoorOpened => self.out.push(MachineEvent::Error("The door opened: the job is paused.".into())),
                E::Paused => self.note(ConsoleKind::Note, "Paused"),
                E::Resumed => self.note(ConsoleKind::Note, "Resumed"),
                E::LensLoaded => self.note(ConsoleKind::Note, "Lens correction loaded"),
                E::Rejected(m) => self.out.push(MachineEvent::Error(m)),
                E::Console(t) => self.note(ConsoleKind::Received, t),
            }
        }
        self.refresh();
        std::mem::take(&mut self.out)
    }

    fn snapshot(&self) -> Snapshot {
        self.snap.clone()
    }

    fn command(&mut self, cmd: MachineCommand) -> Result<(), MachineError> {
        use MachineCommand as C;
        let bed = self.bed();
        let unsupported = |what: &str| Err(MachineError::Unsupported(format!("{what}: a galvo board has no gantry axes")));
        let m = self.machine()?;
        match cmd {
            C::StartJob(req) => {
                let JobData::Galvo { job, wait_for_foot, skip_framing_check } = req.data else {
                    return Err(MachineError::Unsupported("a galvo board runs galvo jobs, not G-code or controller files".into()));
                };
                m.start_job(*job, galvo::StartOptions { wait_for_foot, skip_framing_check }).map_err(map_err)
            }
            C::Pause => m.pause().map_err(map_err),
            C::Resume => m.resume().map_err(map_err),
            C::Stop | C::EmergencyStop => m.stop().map_err(map_err),
            C::FrameRect(r, _) => {
                let rect = Rect::new(r.x0 - bed.0 / 2.0, r.y0 - bed.1 / 2.0, r.x1 - bed.0 / 2.0, r.y1 - bed.1 / 2.0);
                let spec = galvo::FrameSpec::from_rect(rect);
                let frame = galvo::compile_frame(&spec, &self.settings.to_library()).map_err(|e| MachineError::Invalid(e.to_string()))?;
                m.start_frame(frame, false).map_err(map_err)
            }
            C::FrameOutline(pts, _) => {
                if pts.len() < 2 {
                    return Err(MachineError::Invalid("There is nothing to frame.".into()));
                }
                let outline: Vec<Point> = pts.iter().map(|p| to_field(*p, bed)).collect();
                let bounds = outline.iter().fold(Rect::from_points(outline[0], outline[0]), |r, p| r.union_pt(*p));
                let spec = galvo::FrameSpec { outline, ..galvo::FrameSpec::from_rect(bounds) };
                let frame = galvo::compile_frame(&spec, &self.settings.to_library()).map_err(|e| MachineError::Invalid(e.to_string()))?;
                m.start_frame(frame, false).map_err(map_err)
            }
            C::GalvoFrame { frame, repeat } => m.start_frame(*frame, repeat).map_err(map_err),
            C::GalvoArm(true) => m.arm().map_err(map_err),
            C::GalvoArm(false) => m.disarm().map_err(map_err),
            C::GalvoLight(on) => m.set_light(on).map_err(map_err),
            C::GalvoLens(t) => m.set_lens(t.map(|b| *b)).map_err(map_err),
            C::GalvoAxis { axis, mm } => m.move_axis(axis, mm).map_err(map_err),
            C::FireOff => m.stop().map_err(map_err),
            C::Fire { .. } => Err(MachineError::Refused(
                "A galvo board never fires from a button. Arm the laser and run a job; the red light shows where it will mark.".into(),
            )),
            C::GetPosition => Ok(()),
            C::JogStep(_) | C::JogStart { .. } | C::JogStop => unsupported("Jog"),
            C::Home => unsupported("Home"),
            C::Unlock => Ok(()),
            C::GoToOrigin => unsupported("Go to origin"),
            C::MoveTo(_) => unsupported("Move"),
            C::SetUserOrigin | C::ClearUserOrigin => unsupported("Origin"),
            C::SetOverrides { .. } => unsupported("Overrides"),
            C::Console(_) | C::Macro(_) => Err(MachineError::Unsupported("a galvo board has no console".into())),
            C::ShowAll(_) => Ok(()),
            _ => Err(MachineError::Unsupported("that is not a galvo board command".into())),
        }
    }

    fn close(&mut self) {
        if let Some(mut m) = self.m.take() {
            let _ = m.stop();
            m.disconnect();
        }
        self.snap.link = LinkState::Disconnected;
        self.snap.state = LaserState::Disconnected;
        let _ = self.simulator;
    }
}

impl Drop for GalvoAdapter {
    fn drop(&mut self) {
        self.close();
    }
}

/// The in-memory galvo board. Safe: nothing leaves the process.
pub fn open_simulator(cfg: &ConnectConfig) -> Result<Box<dyn DriverAdapter>, MachineError> {
    let settings = cfg.galvo.clone().unwrap_or_default();
    let (t, _handle) = galvo::MemoryTransport::new(galvo::GalvoEmulator::new(galvo::EmulatorConfig::default()));
    Ok(Box::new(GalvoAdapter::new(Box::new(t), settings, true)))
}

/// A simulator with its board handle, so tests can look at what the board saw.
pub fn simulator_with_handle(settings: Arc<GalvoDeviceSettings>) -> (GalvoAdapter, galvo::EmulatorHandle) {
    let (t, handle) = galvo::MemoryTransport::new(galvo::GalvoEmulator::new(galvo::EmulatorConfig::default()));
    (GalvoAdapter::new(Box::new(t), settings, true), handle)
}
