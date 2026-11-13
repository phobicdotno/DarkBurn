//! Poll-style API for a GUI: the machine runs on its own thread; the UI sends requests and
//! drains events once per frame without ever blocking.
//!
//! ```no_run
//! # use darkburn_ruida::{worker::*, machine::*, transport::*};
//! # fn transport() -> Box<dyn Transport> { unimplemented!() }
//! let machine = RuidaMachine::new(transport(), MachineConfig::default());
//! let mut handle = RuidaHandle::spawn(machine, WorkerConfig::default());
//! let id = handle.request(Request::Connect);
//! // every frame:
//! for event in handle.poll() {
//!     match event {
//!         Event::Reply { id: _, result } => { /* show result */ }
//!         Event::OverwritePrompt { id, name } => {
//!             // ask the user, then:
//!             handle.answer_overwrite(id, OverwriteChoice::Overwrite);
//!         }
//!         Event::Status(_s) => { /* update the laser panel */ }
//!         Event::Progress { .. } | Event::Stopped => {}
//!     }
//! }
//! ```
//!
//! Requests run one at a time in the order sent, except [`Request::Stop`], which goes ahead of
//! everything that is waiting. While a send is waiting for an overwrite answer, other requests
//! wait; a Stop gives that send up. Dropping the handle stops the thread.

use std::collections::VecDeque;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use kurbo::{Point, Rect};

use crate::command::{Axis, Command, PanelKey};
use crate::error::{Result, RuidaError};
use crate::machine::{ControllerFile, HomeAxes, OverwriteChoice, RuidaMachine, SendOptions, SendOutcome};
use crate::rd::RdJob;
use crate::settings::{ControllerInfo, Laser2Settings, MachineSettings, MachineStatus, UAxisSettings};
use crate::swizzle::Magic;
use crate::transport::Transport;

/// Identifies a request; replies carry the same id.
pub type RequestId = u64;

/// Work for the machine thread.
#[derive(Debug, Clone)]
pub enum Request {
    Connect,
    Disconnect,
    Status,
    Start,
    Pause,
    Resume,
    Stop,
    Home(HomeAxes),
    GoToOrigin,
    MoveTo {
        to: Point,
        light: bool,
    },
    FrameRect {
        rect: Rect,
        light: bool,
    },
    FrameOutline {
        points: Vec<Point>,
        light: bool,
    },
    FrameOnController,
    JogStep {
        axis: Axis,
        distance_mm: f64,
    },
    JogStart {
        axis: Axis,
        positive: bool,
    },
    JogStop {
        axis: Axis,
        positive: bool,
    },
    PanelKey {
        key: PanelKey,
        down: bool,
    },
    ListFiles,
    SendJob {
        name: String,
        job: Box<RdJob>,
        options: SendOptions,
    },
    UploadRdFile {
        name: String,
        data: Vec<u8>,
        magic: Option<Magic>,
        options: SendOptions,
    },
    RunJobNow(Box<RdJob>),
    RunFile(u16),
    DeleteFiles(Vec<u16>),
    DeleteAllFiles,
    CalcTime(u16),
    ReadSetting(u16),
    WriteSetting {
        addr: u16,
        value: u32,
    },
    BackupSettings,
    RestoreSettings(MachineSettings),
    ControllerInfo,
    ReadLaser2,
    WriteLaser2(Laser2Settings),
    ReadUAxis,
    WriteUAxis(UAxisSettings),
    /// Send commands as they are.
    Raw(Vec<Command>),
}

/// Successful results.
#[derive(Debug, Clone, PartialEq)]
pub enum Response {
    Done,
    Connected(ControllerInfo),
    Status(MachineStatus),
    Files(Vec<ControllerFile>),
    Sent(SendOutcome),
    Time(Duration),
    Setting(u32),
    Settings(MachineSettings),
    /// Addresses skipped by a restore.
    Restored(Vec<u16>),
    Info(ControllerInfo),
    Laser2(Laser2Settings),
    UAxis(UAxisSettings),
}

/// Something for the UI.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Reply {
        id: RequestId,
        result: std::result::Result<Response, RuidaError>,
    },
    /// A send found the name taken. Answer with [`RuidaHandle::answer_overwrite`].
    OverwritePrompt {
        id: RequestId,
        name: String,
    },
    /// Upload progress in bytes.
    Progress {
        id: RequestId,
        sent: usize,
        total: usize,
    },
    /// Periodic status while connected.
    Status(MachineStatus),
    /// The thread has ended.
    Stopped,
}

/// Worker settings.
#[derive(Debug, Clone)]
pub struct WorkerConfig {
    /// Poll status this often while connected and idle; `None` turns polling off.
    pub status_interval: Option<Duration>,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        WorkerConfig { status_interval: Some(Duration::from_millis(500)) }
    }
}

enum Msg {
    Req(RequestId, Request),
    Overwrite(RequestId, OverwriteChoice),
    Shutdown,
}

/// The UI side.
pub struct RuidaHandle {
    tx: Sender<Msg>,
    rx: Receiver<Event>,
    next: RequestId,
    thread: Option<JoinHandle<()>>,
    backlog: VecDeque<Event>,
}

impl RuidaHandle {
    /// Move `machine` onto a new thread.
    pub fn spawn<T: Transport + 'static>(machine: RuidaMachine<T>, cfg: WorkerConfig) -> RuidaHandle {
        let (tx, mrx) = mpsc::channel::<Msg>();
        let (etx, rx) = mpsc::channel::<Event>();
        let thread = std::thread::Builder::new()
            .name("ruida".into())
            .spawn(move || Worker { m: machine, rx: mrx, tx: etx, cfg, queued: VecDeque::new(), shutdown: false }.run())
            .expect("spawn ruida worker thread");
        RuidaHandle { tx, rx, next: 1, thread: Some(thread), backlog: VecDeque::new() }
    }

    /// Queue a request. Never blocks.
    pub fn request(&mut self, r: Request) -> RequestId {
        let id = self.next;
        self.next += 1;
        if self.tx.send(Msg::Req(id, r)).is_err() {
            self.backlog.push_back(Event::Reply { id, result: Err(RuidaError::WorkerGone) });
        }
        id
    }

    /// Answer an [`Event::OverwritePrompt`].
    pub fn answer_overwrite(&mut self, id: RequestId, choice: OverwriteChoice) {
        let _ = self.tx.send(Msg::Overwrite(id, choice));
    }

    /// All events that arrived since the last call. Never blocks.
    pub fn poll(&mut self) -> Vec<Event> {
        let mut v: Vec<Event> = self.backlog.drain(..).collect();
        v.extend(self.rx.try_iter());
        v
    }

    /// Block until the reply for `id` arrives (for tests and scripts, not for the UI thread).
    /// Other events that arrive meanwhile are kept for the next [`poll`](Self::poll), except
    /// overwrite prompts, which are answered with `on_prompt`.
    pub fn wait(&mut self, id: RequestId, timeout: Duration, on_prompt: &mut dyn FnMut(&str) -> OverwriteChoice) -> Result<Response> {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.rx.recv_timeout(left) {
                Ok(Event::Reply { id: rid, result }) if rid == id => return result,
                Ok(Event::OverwritePrompt { id: pid, name }) if pid == id => {
                    let c = on_prompt(&name);
                    self.answer_overwrite(pid, c);
                }
                Ok(Event::Stopped) => return Err(RuidaError::WorkerGone),
                Ok(other) => self.backlog.push_back(other),
                Err(RecvTimeoutError::Timeout) => return Err(RuidaError::Timeout(format!("request {id}"))),
                Err(RecvTimeoutError::Disconnected) => return Err(RuidaError::WorkerGone),
            }
        }
    }

    /// Stop the thread and wait for it.
    pub fn shutdown(&mut self) {
        let _ = self.tx.send(Msg::Shutdown);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for RuidaHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

struct Worker<T: Transport> {
    m: RuidaMachine<T>,
    rx: Receiver<Msg>,
    tx: Sender<Event>,
    cfg: WorkerConfig,
    queued: VecDeque<(RequestId, Request)>,
    /// The handle asked the thread to end while a request was being handled.
    shutdown: bool,
}

impl<T: Transport> Worker<T> {
    fn run(mut self) {
        let mut last_poll = Instant::now();
        'run: loop {
            if self.shutdown {
                break;
            }
            // Take in everything that is waiting, so a Stop among it can go first.
            loop {
                match self.rx.try_recv() {
                    Ok(Msg::Req(id, r)) => self.queued.push_back((id, r)),
                    Ok(Msg::Overwrite(..)) => {}
                    Ok(Msg::Shutdown) | Err(mpsc::TryRecvError::Disconnected) => break 'run,
                    Err(mpsc::TryRecvError::Empty) => break,
                }
            }
            let stop = self.queued.iter().position(|(_, r)| matches!(r, Request::Stop));
            let next = if let Some(i) = stop {
                // What was asked before the Stop and would move, fire or start a job is given
                // up: done after the Stop, it would undo it (a Pulse key pressed after it).
                let q = self.queued.remove(i);
                let waiting = std::mem::take(&mut self.queued);
                for (n, (id, r)) in waiting.into_iter().enumerate() {
                    if n < i && acts(&r) {
                        let _ = self.tx.send(Event::Reply { id, result: Err(RuidaError::Cancelled) });
                    } else {
                        self.queued.push_back((id, r));
                    }
                }
                q
            } else if let Some(q) = self.queued.pop_front() {
                Some(q)
            } else {
                let wait = match self.cfg.status_interval {
                    Some(iv) if self.m.is_connected() => iv.saturating_sub(last_poll.elapsed()),
                    _ => Duration::from_millis(250),
                };
                match self.rx.recv_timeout(wait) {
                    Ok(Msg::Req(id, r)) => Some((id, r)),
                    Ok(Msg::Overwrite(..)) => None,
                    Ok(Msg::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
                    Err(RecvTimeoutError::Timeout) => None,
                }
            };
            match next {
                Some((id, r)) => {
                    let result = self.handle(id, r);
                    let _ = self.tx.send(Event::Reply { id, result });
                }
                None => {
                    if let Some(iv) = self.cfg.status_interval
                        && self.m.is_connected()
                        && last_poll.elapsed() >= iv
                    {
                        last_poll = Instant::now();
                        if let Ok(s) = self.m.status() {
                            let _ = self.tx.send(Event::Status(s));
                        }
                    }
                }
            }
        }
        let _ = self.tx.send(Event::Stopped);
    }

    fn handle(&mut self, id: RequestId, r: Request) -> Result<Response> {
        use Request as R;
        use Response::*;
        let m = &mut self.m;
        if !matches!(r, R::Connect | R::Disconnect) && !m.is_connected() {
            return Err(RuidaError::NotConnected);
        }
        Ok(match r {
            R::Connect => Connected(m.connect()?),
            R::Disconnect => {
                m.disconnect();
                Done
            }
            R::Status => Status(m.status()?),
            R::Start => m.start().map(|_| Done)?,
            R::Pause => m.pause().map(|_| Done)?,
            R::Resume => m.resume().map(|_| Done)?,
            R::Stop => m.stop().map(|_| Done)?,
            R::Home(a) => m.home(a).map(|_| Done)?,
            R::GoToOrigin => m.go_to_origin().map(|_| Done)?,
            R::MoveTo { to, light } => m.move_to(to, light).map(|_| Done)?,
            R::FrameRect { rect, light } => m.frame_rect(rect, light).map(|_| Done)?,
            R::FrameOutline { points, light } => m.frame_outline(&points, light).map(|_| Done)?,
            R::FrameOnController => m.frame_on_controller().map(|_| Done)?,
            R::JogStep { axis, distance_mm } => m.jog_step(axis, distance_mm).map(|_| Done)?,
            R::JogStart { axis, positive } => m.jog_start(axis, positive).map(|_| Done)?,
            R::JogStop { axis, positive } => m.jog_stop(axis, positive).map(|_| Done)?,
            R::PanelKey { key, down } => m.panel_key(key, down).map(|_| Done)?,
            R::ListFiles => Files(m.list_files()?),
            R::SendJob { name, job, options } => {
                let logical = job.to_logical()?;
                Sent(self.send_logical(id, &name, &logical, &options)?)
            }
            R::UploadRdFile { name, data, magic, options } => {
                let magic =
                    magic.or_else(|| crate::swizzle::detect_magic_histogram(&data)).ok_or_else(|| RuidaError::Decode("empty .rd file".into()))?;
                let logical = crate::swizzle::unswizzle(&data, magic);
                Sent(self.send_logical(id, &name, &logical, &options)?)
            }
            R::RunJobNow(job) => m.run_job_now(&job).map(|_| Done)?,
            R::RunFile(i) => m.run_file(i).map(|_| Done)?,
            R::DeleteFiles(v) => m.delete_files(&v).map(|_| Done)?,
            R::DeleteAllFiles => m.delete_all_files().map(|_| Done)?,
            R::CalcTime(i) => Time(m.calc_time(i)?),
            R::ReadSetting(a) => Setting(m.read_setting(a)?),
            R::WriteSetting { addr, value } => m.write_setting(addr, value).map(|_| Done)?,
            R::BackupSettings => Settings(m.backup_settings()?),
            R::RestoreSettings(s) => Restored(m.restore_settings(&s)?),
            R::ControllerInfo => Info(m.read_controller_info()?),
            R::ReadLaser2 => Laser2(m.read_laser2()?),
            R::WriteLaser2(s) => m.write_laser2(s).map(|_| Done)?,
            R::ReadUAxis => UAxis(m.read_u_axis()?),
            R::WriteUAxis(s) => m.write_u_axis(s).map(|_| Done)?,
            R::Raw(cmds) => m.send_commands(&cmds).map(|_| Done)?,
        })
    }

    fn send_logical(&mut self, id: RequestId, name: &str, logical: &[u8], options: &SendOptions) -> Result<SendOutcome> {
        // The prompt and progress callbacks need the channels while the machine is borrowed,
        // so they are split out of `self` here.
        let Worker { m, rx, tx, queued, shutdown, .. } = self;
        let mut prompt = |n: &str| {
            let _ = tx.send(Event::OverwritePrompt { id, name: n.to_string() });
            loop {
                match rx.recv() {
                    Ok(Msg::Overwrite(aid, c)) if aid == id => return c,
                    Ok(Msg::Overwrite(..)) => {}
                    // Stop never waits for a question to be answered: the send is given up
                    // and Stop is the next thing done.
                    Ok(Msg::Req(rid, Request::Stop)) => {
                        queued.push_back((rid, Request::Stop));
                        return OverwriteChoice::Cancel;
                    }
                    Ok(Msg::Req(rid, r)) => queued.push_back((rid, r)),
                    Ok(Msg::Shutdown) | Err(_) => {
                        // The message is used up here, so the main loop must be told.
                        *shutdown = true;
                        return OverwriteChoice::Cancel;
                    }
                }
            }
        };
        let progress_tx = tx.clone();
        let mut progress = |sent, total| {
            let _ = progress_tx.send(Event::Progress { id, sent, total });
        };
        m.send_logical_job(name, logical, options, &mut prompt, &mut progress)
    }
}

/// Whether a request moves the head, presses a key, fires or starts a job.
fn acts(r: &Request) -> bool {
    use Request as R;
    match r {
        R::Start
        | R::Resume
        | R::Home(_)
        | R::GoToOrigin
        | R::MoveTo { .. }
        | R::FrameRect { .. }
        | R::FrameOutline { .. }
        | R::FrameOnController
        | R::JogStep { .. }
        | R::JogStart { .. }
        | R::RunJobNow(_)
        | R::RunFile(_) => true,
        R::PanelKey { down, .. } => *down,
        R::SendJob { options, .. } | R::UploadRdFile { options, .. } => options.start_immediately,
        R::Raw(cmds) => !cmds.iter().all(|c| matches!(c, Command::KeyUp(_))),
        _ => false,
    }
}
