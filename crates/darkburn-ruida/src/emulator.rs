//! A software Ruida controller for tests and demos.
//!
//! It checks checksums, unswizzles with its magic, ACKs/NAKs every UDP datagram, answers
//! settings reads, keeps a file system of uploaded jobs, runs jobs (instantly or step by step),
//! honours start/pause/resume/stop/home/jog/rapid moves and records all motion for assertions.
//!
//! Behaviour is modelled on the public protocol descriptions and on MeerK40t's emulator (MIT).
//! Where the real controller's behaviour is unknown the choice made here is marked UNCERTAIN.

use std::collections::{BTreeMap, BTreeSet};

use crate::command::{Axis, Command, JogKey, Laser, LayerFlag, OverscanMode, PanelCommand, PanelKey, split_commands};
use crate::encoding::{Power, encode_cstring};
use crate::packet::{frame, unframe};
use crate::settings::{StatusWord, addr};
use crate::swizzle::{Magic, Swizzler};

/// Emulator configuration.
#[derive(Debug, Clone)]
pub struct EmulatorConfig {
    pub magic: Magic,
    pub card_id: u32,
    pub firmware: String,
    pub bed_x_um: u32,
    pub bed_y_um: u32,
    /// When true, jobs only progress through [`RuidaEmulator::advance`]; otherwise a job runs
    /// to completion as soon as it starts.
    pub step_mode: bool,
    /// Distance moved by one key-down/key-up pair of continuous jog.
    pub jog_distance_um: i32,
    /// Most files the controller stores.
    pub max_files: usize,
    /// Motion takes time, as on a real controller: the position registers report where the head
    /// was before the latest move outside a job until [`RuidaEmulator::settle`]. Off by default,
    /// where every move is over the moment it arrives.
    pub slow_moves: bool,
}

impl Default for EmulatorConfig {
    fn default() -> Self {
        EmulatorConfig {
            magic: Magic::DEFAULT,
            card_id: 0x6510_6510,
            firmware: "RDC6442S-EMU".into(),
            bed_x_um: 600_000,
            bed_y_um: 400_000,
            step_mode: false,
            jog_distance_um: 10_000,
            max_files: 99,
            slow_moves: false,
        }
    }
}

/// What kind of motion was recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MotionKind {
    /// Laser-off move inside a job.
    Travel,
    /// Laser-on move inside a job.
    Cut,
    /// Rapid (`D9`) move outside a job, for example framing or "go to".
    Rapid,
    /// Continuous or panel jog.
    Jog,
    /// Homing.
    Home,
}

/// One recorded move. Positions are `[x, y, z, u]` in µm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MotionRecord {
    pub kind: MotionKind,
    pub from: [i32; 4],
    pub to: [i32; 4],
    /// Laser 1 max power in effect (cuts only).
    pub power: Power,
    /// Speed in µm/s in effect.
    pub speed: u32,
}

/// A stored job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredFile {
    pub name: String,
    /// Logical (unswizzled) job bytes, without the `E8 02` / `E7 01` prefix.
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunState {
    Idle,
    Running,
    Paused,
}

struct Collect {
    transfer: bool,
    name: Option<String>,
    cmds: Vec<Command>,
}

struct Job {
    cmds: Vec<Command>,
    pc: usize,
    cut_us: u64,
    total_us: u64,
}

/// The emulator. Feed it with [`handle_udp_datagram`](Self::handle_udp_datagram),
/// [`handle_stream_bytes`](Self::handle_stream_bytes) or
/// [`handle_jog_datagram`](Self::handle_jog_datagram), usually through
/// [`crate::transport::MemoryTransport`].
pub struct RuidaEmulator {
    cfg: EmulatorConfig,
    swz: Swizzler,
    mem: BTreeMap<u16, u32>,
    files: Vec<StoredFile>,
    pos: [i32; 4],
    /// What the position registers report while `slow_moves` is on.
    reported: [i32; 4],
    state: RunState,
    pending: Vec<u8>,
    collect: Option<Collect>,
    job: Option<Job>,
    last_job: Option<Vec<Command>>,
    speed: u32,
    power: Power,
    keys_down: BTreeSet<JogKey>,
    panel_keys_down: BTreeSet<PanelKeyOrd>,
    /// Every command received on the data channel, in order (realtime and job bytes).
    pub received: Vec<Command>,
    /// Every panel command received on the jog channel.
    pub panel_received: Vec<PanelCommand>,
    /// Recorded motion.
    pub motion: Vec<MotionRecord>,
    /// Number of datagrams rejected for a bad checksum.
    pub checksum_failures: usize,
    /// Number of uploaded files whose `E5 05` sum did not match.
    pub file_sum_mismatches: usize,
    /// Settings writes, in order.
    pub writes: Vec<(u16, u32)>,
    /// Hardware overscan in effect (`CA 01 00..05`); off again at the end of each layer.
    overscan: OverscanMode,
    /// Every hardware overscan mode a running job turned on, with its layer number.
    pub overscan_modes: Vec<(u8, OverscanMode)>,
    layer: u8,
    /// The user origin (panel Origin key), `[x, y]` in µm.
    origin: [i32; 2],
    /// Added to the absolute coordinates of the running job: zero for `D8 10`, the user origin
    /// for `D8 11`, the head position at the start for `D8 12`.
    job_offset: [i32; 2],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct PanelKeyOrd(u8);

impl RuidaEmulator {
    pub fn new(cfg: EmulatorConfig) -> Self {
        let mut mem = BTreeMap::new();
        for (a, v) in [
            (addr::G0_VELOCITY, 200_000),
            (addr::HOME_VELOCITY, 20_000),
            (addr::LASER1_PWM_FREQ, 20_000),
            (addr::LASER1_MIN_POWER, 100),
            (addr::LASER1_MAX_POWER, 1000),
            (addr::LASER2_PWM_FREQ, 20_000),
            (addr::LASER2_MIN_POWER, 100),
            (addr::LASER2_MAX_POWER, 1000),
            (addr::X_CONTROL, 0x4000),
            (addr::X_PRECISION, 5),
            (addr::X_MAX_VELOCITY, 500_000),
            (addr::X_START_VELOCITY, 10_000),
            (addr::X_MAX_ACC, 3_000_000),
            (addr::BED_X, cfg.bed_x_um),
            (addr::Y_CONTROL, 0x4000),
            (addr::Y_PRECISION, 5),
            (addr::Y_MAX_VELOCITY, 500_000),
            (addr::Y_START_VELOCITY, 10_000),
            (addr::Y_MAX_ACC, 3_000_000),
            (addr::BED_Y, cfg.bed_y_um),
            (addr::Z_RANGE, 50_000),
            (addr::U_PRECISION, 5),
            (addr::U_MAX_VELOCITY, 100_000),
            (addr::U_RANGE, 0),
            (addr::ROTATE_PULSE, 10_000),
            (addr::ROTATE_DIAMETER, 80_000),
            (addr::TURN_VELOCITY, 20_000),
            (addr::SYN_ACC, 3_000_000),
            (addr::G0_ACC, 3_000_000),
            (addr::FOCUS_DEPTH, 5_000),
            (addr::MANUAL_FAST_SPEED, 100_000),
            (addr::MANUAL_SLOW_SPEED, 10_000),
            (addr::TOTAL_OPEN_TIME, 3_600),
        ] {
            mem.insert(a, v);
        }
        RuidaEmulator {
            swz: Swizzler::new(cfg.magic),
            cfg,
            mem,
            files: Vec::new(),
            pos: [0; 4],
            reported: [0; 4],
            state: RunState::Idle,
            pending: Vec::new(),
            collect: None,
            job: None,
            last_job: None,
            speed: 100_000,
            power: Power(0),
            keys_down: BTreeSet::new(),
            panel_keys_down: BTreeSet::new(),
            received: Vec::new(),
            panel_received: Vec::new(),
            motion: Vec::new(),
            checksum_failures: 0,
            file_sum_mismatches: 0,
            writes: Vec::new(),
            overscan: OverscanMode::Off,
            overscan_modes: Vec::new(),
            layer: 0,
            origin: [0; 2],
            job_offset: [0; 2],
        }
    }

    pub fn magic(&self) -> Magic {
        self.cfg.magic
    }
    pub fn config(&self) -> &EmulatorConfig {
        &self.cfg
    }
    pub fn files(&self) -> &[StoredFile] {
        &self.files
    }
    pub fn position(&self) -> [i32; 4] {
        self.pos
    }
    /// Commands run and total of the job now running, for a progress bar.
    pub fn job_progress(&self) -> Option<(usize, usize)> {
        self.job.as_ref().map(|j| (j.pc, j.cmds.len()))
    }
    pub fn state(&self) -> RunState {
        self.state
    }
    /// Set the user origin a `D8 11` job is measured from (test setup; µm).
    pub fn set_origin(&mut self, x: i32, y: i32) {
        self.origin = [x, y];
    }
    /// Hardware overscan mode in effect right now.
    pub fn overscan(&self) -> OverscanMode {
        self.overscan
    }
    pub fn memory(&self, a: u16) -> Option<u32> {
        self.mem.get(&a).copied()
    }
    pub fn set_memory(&mut self, a: u16, v: u32) {
        self.mem.insert(a, v);
    }
    pub fn set_step_mode(&mut self, on: bool) {
        self.cfg.step_mode = on;
    }
    /// Turn [`EmulatorConfig::slow_moves`] on or off. Either way the reported position catches up.
    pub fn set_slow_moves(&mut self, on: bool) {
        self.cfg.slow_moves = on;
        self.settle();
    }
    /// The moves sent so far are over: the position registers report where the head is.
    pub fn settle(&mut self) {
        self.reported = self.pos;
    }
    /// Store a file directly (test setup).
    pub fn add_file(&mut self, name: &str, data: Vec<u8>) {
        self.store(name.to_string(), data);
    }

    fn sw(&self, logical: &[u8]) -> Vec<u8> {
        self.swz.swizzle(logical)
    }

    /// Handle one datagram from UDP port 50200. Returns the datagrams sent back:
    /// first the one-byte ACK/NAK, then any query replies.
    pub fn handle_udp_datagram(&mut self, datagram: &[u8]) -> Vec<Vec<u8>> {
        let payload = match unframe(datagram) {
            Ok(p) => p.to_vec(),
            Err(_) => {
                self.checksum_failures += 1;
                return vec![self.sw(&[0xCF])];
            }
        };
        let mut out = vec![self.sw(&[0xCC])];
        let replies = self.feed(&self.swz.unswizzle(&payload), false);
        out.extend(replies.into_iter().map(|r| self.sw(&r)));
        out
    }

    /// Handle bytes from the USB stream. Returns swizzled reply bytes (no ACKs except for ENQ).
    pub fn handle_stream_bytes(&mut self, data: &[u8]) -> Vec<u8> {
        let logical = self.swz.unswizzle(data);
        let replies = self.feed(&logical, true);
        replies.iter().flat_map(|r| self.sw(r)).collect()
    }

    /// Handle one datagram from UDP port 50207 (unswizzled). Replies `CC` to everything it
    /// understands. UNCERTAIN: a real controller also sends an `A5 68 ...` position frame on connect.
    pub fn handle_jog_datagram(&mut self, data: &[u8]) -> Vec<Vec<u8>> {
        let Some(cmd) = PanelCommand::decode(data) else { return vec![] };
        self.panel_received.push(cmd);
        match cmd {
            PanelCommand::Connect | PanelCommand::KeepAlive | PanelCommand::Disconnect => {}
            PanelCommand::Frame => self.frame_last_job(),
            PanelCommand::KeyDown(k) => {
                self.panel_keys_down.insert(PanelKeyOrd(k.byte()));
                match k {
                    PanelKey::StartPause => match self.state {
                        RunState::Running => self.state = RunState::Paused,
                        RunState::Paused => self.resume(),
                        RunState::Idle => self.restart_last_job(),
                    },
                    PanelKey::Stop | PanelKey::Escape => self.stop(),
                    PanelKey::Origin => self.rapid_to([0, 0, self.pos[2], self.pos[3]], MotionKind::Rapid),
                    PanelKey::Reset => self.home(&[0, 1]),
                    _ => {}
                }
            }
            PanelCommand::KeyUp(k) => {
                if self.panel_keys_down.remove(&PanelKeyOrd(k.byte())) {
                    let d = self.cfg.jog_distance_um;
                    let delta = match k {
                        PanelKey::MinusX => Some((0, -d)),
                        PanelKey::PlusX => Some((0, d)),
                        PanelKey::PlusY => Some((1, d)),
                        PanelKey::MinusY => Some((1, -d)),
                        PanelKey::PlusZ => Some((2, d)),
                        PanelKey::MinusZ => Some((2, -d)),
                        PanelKey::PlusU => Some((3, d)),
                        PanelKey::MinusU => Some((3, -d)),
                        _ => None,
                    };
                    if let Some((i, d)) = delta {
                        let mut to = self.pos;
                        to[i] += d;
                        self.rapid_to(to, MotionKind::Jog);
                    }
                }
            }
        }
        vec![vec![0xCC]]
    }

    /// Run up to `n` job commands (step mode). Returns how many ran.
    pub fn advance(&mut self, n: usize) -> usize {
        let mut ran = 0;
        while ran < n && self.state == RunState::Running {
            if !self.step_job() {
                break;
            }
            ran += 1;
        }
        ran
    }

    /// Run the current job to the end (unless paused or stopped).
    pub fn run_to_end(&mut self) {
        while self.state == RunState::Running && self.step_job() {}
    }

    // ---- stream handling ------------------------------------------------------------------

    fn feed(&mut self, logical: &[u8], stream: bool) -> Vec<Vec<u8>> {
        let mut buf = std::mem::take(&mut self.pending);
        buf.extend_from_slice(logical);
        let slices: Vec<Vec<u8>> = split_commands(&buf).into_iter().map(<[u8]>::to_vec).collect();
        let mut replies = Vec::new();
        let n = slices.len();
        for (i, s) in slices.into_iter().enumerate() {
            if i + 1 == n && !is_complete(&s) {
                self.pending = s;
                break;
            }
            let cmd = Command::decode(&s);
            self.received.push(cmd.clone());
            self.process(cmd, stream, &mut replies);
        }
        replies
    }

    fn process(&mut self, cmd: Command, stream: bool, replies: &mut Vec<Vec<u8>>) {
        use Command::*;
        if let Some(c) = self.collect.as_mut() {
            match cmd {
                SetFilename(name) if c.transfer => c.name = Some(name),
                EndOfFile => {
                    c.cmds.push(EndOfFile);
                    let c = self.collect.take().expect("collecting");
                    self.finish_collect(c);
                }
                GetSetting { .. } => self.answer(cmd, stream, replies),
                cmd => c.cmds.push(cmd),
            }
            return;
        }
        match cmd {
            BeginFileTransfer => self.collect = Some(Collect { transfer: true, name: None, cmds: vec![] }),
            RefPointMode(_) | SetAbsolute => self.collect = Some(Collect { transfer: false, name: None, cmds: vec![cmd] }),
            Start => self.restart_last_job(),
            Stop => self.stop(),
            Pause => {
                if self.state == RunState::Running {
                    self.state = RunState::Paused;
                }
            }
            Resume => self.resume(),
            HomeXY => self.home(&[0, 1]),
            HomeZ => self.home(&[2]),
            HomeU => self.home(&[3]),
            FocusZ => self.focus_z(),
            KeyDown(k) => {
                self.keys_down.insert(k);
            }
            KeyUp(k) => {
                if self.keys_down.remove(&k) {
                    let (axis, dir) = k.axis_dir();
                    let mut to = self.pos;
                    to[axis_idx(axis)] += dir * self.cfg.jog_distance_um;
                    self.rapid_to(to, MotionKind::Jog);
                }
            }
            RapidXY { x, y, .. } => self.rapid_to([x, y, self.pos[2], self.pos[3]], MotionKind::Rapid),
            RapidXYU { x, y, u, .. } => self.rapid_to([x, y, self.pos[2], u], MotionKind::Rapid),
            RapidAxis { axis, pos, .. } => {
                let mut to = self.pos;
                to[axis_idx(axis)] = pos;
                self.rapid_to(to, MotionKind::Rapid);
            }
            RunFile(n) => {
                if let Some(f) = self.files.get((n as usize).wrapping_sub(1)) {
                    let cmds = crate::command::parse_commands(&f.data);
                    self.start_job(cmds);
                }
            }
            DeleteFiles { first, last } => {
                if first == 0 {
                    self.files.clear();
                } else {
                    let (a, b) = (first.min(last) as usize, first.max(last) as usize);
                    let b = b.min(self.files.len());
                    if a >= 1 && a <= b {
                        self.files.drain(a - 1..b);
                    }
                }
            }
            CalcFileTime(n) => {
                let t = self.files.get((n as usize).wrapping_sub(1)).map(|f| estimate_ms(&crate::command::parse_commands(&f.data), self.g0()));
                if let Some(t) = t {
                    self.mem.insert(addr::FILE_TIME_BASE + n, t);
                }
            }
            SetSetting { addr: a, value, .. } => {
                self.writes.push((a, value));
                self.mem.insert(a, value);
            }
            Enq if stream => replies.push(vec![0xCC]),
            GetSetting { .. } | GetFileName(_) => self.answer(cmd, stream, replies),
            c if c.is_motion() => {
                // Loose motion commands outside a job execute immediately.
                self.exec_motion(&c);
            }
            Speed(s) => self.speed = s,
            MaxPower { laser: Laser::L1, power } => self.power = power,
            _ => {}
        }
    }

    fn answer(&mut self, cmd: Command, _stream: bool, replies: &mut Vec<Vec<u8>>) {
        let reply = match cmd {
            Command::GetSetting { addr: a } if a == addr::FIRMWARE => Command::SettingReplyRaw { addr: a, data: encode_cstring(&self.cfg.firmware) },
            Command::GetSetting { addr: a } => Command::SettingReply { addr: a, value: self.read_mem(a) },
            Command::GetFileName(n) => {
                // UNCERTAIN: reply for a missing index is not documented; an empty name is sent.
                let name = self.files.get((n as usize).wrapping_sub(1)).map(|f| f.name.clone()).unwrap_or_default();
                Command::FileName { index: n, name }
            }
            _ => return,
        };
        if let Ok(b) = reply.encode() {
            replies.push(b);
        }
    }

    fn read_mem(&self, a: u16) -> u32 {
        match a {
            addr::MACHINE_STATUS => match self.state {
                RunState::Idle => StatusWord::IDLE_VALUE,
                RunState::Running => StatusWord::RUNNING_VALUE,
                RunState::Paused => StatusWord::PAUSED_VALUE,
            },
            addr::POS_X => self.shown()[0] as u32,
            addr::POS_Y => self.shown()[1] as u32,
            addr::POS_Z => self.shown()[2] as u32,
            addr::POS_U => self.shown()[3] as u32,
            addr::FILE_COUNT => self.files.len() as u32,
            addr::CARD_ID => self.cfg.card_id,
            a => self.mem.get(&a).copied().unwrap_or(0),
        }
    }

    fn finish_collect(&mut self, c: Collect) {
        if c.transfer {
            // Check the E5 05 file sum if present: byte sum of everything before it, plus EOF.
            if let Some(i) = c.cmds.iter().position(|c| matches!(c, Command::FileSum(_))) {
                let Command::FileSum(want) = c.cmds[i] else { unreachable!() };
                let got = crate::rd::file_sum(&c.cmds[..i]).unwrap_or(0).wrapping_add(0xD7);
                if got != want {
                    self.file_sum_mismatches += 1;
                }
            }
            let data = crate::command::encode_commands(&c.cmds).unwrap_or_default();
            let name = c.name.unwrap_or_else(|| format!("FILE{}", self.files.len() + 1));
            self.store(name, data);
        } else {
            self.start_job(c.cmds);
        }
    }

    fn store(&mut self, name: String, data: Vec<u8>) {
        // UNCERTAIN: a real controller's behaviour on a duplicate name is unknown; it is replaced here.
        if let Some(f) = self.files.iter_mut().find(|f| f.name == name) {
            f.data = data;
        } else if self.files.len() < self.cfg.max_files {
            self.files.push(StoredFile { name, data });
        }
    }

    // ---- execution ------------------------------------------------------------------------

    fn g0(&self) -> u32 {
        self.mem.get(&addr::G0_VELOCITY).copied().unwrap_or(200_000).max(1)
    }

    fn start_job(&mut self, cmds: Vec<Command>) {
        self.job_offset = match cmds.iter().find_map(|c| if let Command::RefPointMode(m) = c { Some(*m) } else { None }) {
            Some(0x11) => self.origin,
            Some(0x12) => [self.pos[0], self.pos[1]],
            _ => [0; 2],
        };
        self.last_job = Some(cmds.clone());
        self.job = Some(Job { cmds, pc: 0, cut_us: 0, total_us: 0 });
        self.state = RunState::Running;
        *self.mem.entry(addr::TOTAL_WORK_NUMBER).or_insert(0) += 1;
        if !self.cfg.step_mode {
            self.run_to_end();
        }
    }

    fn restart_last_job(&mut self) {
        match self.state {
            RunState::Paused => self.resume(),
            RunState::Running => {}
            RunState::Idle => {
                if let Some(j) = self.last_job.clone() {
                    self.start_job(j);
                }
            }
        }
    }

    fn resume(&mut self) {
        if self.state == RunState::Paused {
            self.state = RunState::Running;
            if !self.cfg.step_mode {
                self.run_to_end();
            }
        }
    }

    fn stop(&mut self) {
        if let Some(j) = self.job.take() {
            self.account(&j);
        }
        self.state = RunState::Idle;
        self.overscan = OverscanMode::Off;
    }

    fn account(&mut self, j: &Job) {
        self.job_offset = [0; 2];
        let ms = (j.total_us / 1000) as u32;
        self.mem.insert(addr::PREVIOUS_WORK_TIME, ms);
        *self.mem.entry(addr::TOTAL_WORK_TIME).or_insert(0) += (j.total_us / 1_000_000) as u32;
        *self.mem.entry(addr::TOTAL_LASER_ON_TIME).or_insert(0) += (j.cut_us / 1_000_000) as u32;
    }

    /// Execute one job command. Returns false when the job is over.
    fn step_job(&mut self) -> bool {
        let Some(job) = self.job.as_mut() else {
            self.state = RunState::Idle;
            return false;
        };
        let Some(cmd) = job.cmds.get(job.pc).cloned() else {
            let j = self.job.take().expect("job");
            self.account(&j);
            self.state = RunState::Idle;
            return false;
        };
        job.pc += 1;
        match cmd {
            Command::Speed(s) => self.speed = s,
            Command::MaxPower { laser: Laser::L1, power } => self.power = power,
            Command::LayerNumber(n) => self.layer = n,
            Command::Flag(f @ (LayerFlag::LayerEnd | LayerFlag::WorkMode(_))) => {
                if let Some(mode) = f.overscan_mode() {
                    self.overscan = mode;
                    if mode != OverscanMode::Off {
                        self.overscan_modes.push((self.layer, mode));
                    }
                }
            }
            Command::EndOfFile => {
                let j = self.job.take().expect("job");
                self.account(&j);
                self.state = RunState::Idle;
                return false;
            }
            c if c.is_motion() => {
                let (dist, cut) = self.exec_motion(&c);
                let speed = if cut { self.speed.max(1) } else { self.g0() };
                let us = (dist * 1_000_000.0 / speed as f64) as u64;
                if let Some(j) = self.job.as_mut() {
                    j.total_us += us;
                    if cut {
                        j.cut_us += us;
                    }
                }
            }
            _ => {}
        }
        true
    }

    /// Execute a motion command; returns (distance µm, laser on).
    fn exec_motion(&mut self, c: &Command) -> (f64, bool) {
        use Command::*;
        let p = self.pos;
        let [ox, oy] = self.job_offset;
        let (to, kind) = match *c {
            MoveAbs { x, y } => ([x + ox, y + oy, p[2], p[3]], MotionKind::Travel),
            MoveRel { dx, dy } => ([p[0] + dx, p[1] + dy, p[2], p[3]], MotionKind::Travel),
            MoveRelX(d) => ([p[0] + d, p[1], p[2], p[3]], MotionKind::Travel),
            MoveRelY(d) => ([p[0], p[1] + d, p[2], p[3]], MotionKind::Travel),
            CutAbs { x, y } => ([x + ox, y + oy, p[2], p[3]], MotionKind::Cut),
            CutRel { dx, dy } => ([p[0] + dx, p[1] + dy, p[2], p[3]], MotionKind::Cut),
            CutRelX(d) => ([p[0] + d, p[1], p[2], p[3]], MotionKind::Cut),
            CutRelY(d) => ([p[0], p[1] + d, p[2], p[3]], MotionKind::Cut),
            AxisMove { axis, pos } => {
                let mut t = p;
                t[axis_idx(axis)] = pos;
                (t, MotionKind::Travel)
            }
            RapidXY { x, y, .. } => ([x, y, p[2], p[3]], MotionKind::Rapid),
            RapidXYU { x, y, u, .. } => ([x, y, p[2], u], MotionKind::Rapid),
            RapidAxis { axis, pos, .. } => {
                let mut t = p;
                t[axis_idx(axis)] = pos;
                (t, MotionKind::Rapid)
            }
            _ => return (0.0, false),
        };
        let cut = kind == MotionKind::Cut;
        self.record(kind, to);
        (dist(p, to), cut)
    }

    fn record(&mut self, kind: MotionKind, to: [i32; 4]) {
        let from = self.pos;
        let power = if kind == MotionKind::Cut { self.power } else { Power(0) };
        self.motion.push(MotionRecord { kind, from, to, power, speed: self.speed });
        self.pos = to;
        // A slow move outside a job is still under way: the registers show where it started.
        // Inside a job the job's own progress is what is stepped.
        self.reported = if self.cfg.slow_moves && self.job.is_none() { from } else { to };
    }

    /// The position the registers report.
    fn shown(&self) -> [i32; 4] {
        if self.cfg.slow_moves { self.reported } else { self.pos }
    }

    fn rapid_to(&mut self, to: [i32; 4], kind: MotionKind) {
        self.record(kind, to);
    }

    fn home(&mut self, axes: &[usize]) {
        let mut to = self.pos;
        for &a in axes {
            to[a] = 0;
        }
        self.record(MotionKind::Home, to);
    }

    /// Focus Z: the simulated table stops with the head at the focus distance (Z settings).
    fn focus_z(&mut self) {
        let mut to = self.pos;
        to[2] = self.mem.get(&addr::FOCUS_DEPTH).copied().unwrap_or(0) as i32;
        self.record(MotionKind::Home, to);
    }

    /// Trace the bounding box of the last job with the laser off.
    fn frame_last_job(&mut self) {
        let Some(cmds) = self.last_job.clone() else { return };
        let mut min = None;
        let mut max = None;
        for c in &cmds {
            if let Command::DocumentMin { x, y } = c {
                min = Some((*x, *y));
            }
            if let Command::DocumentMax { x, y } = c {
                max = Some((*x, *y));
            }
        }
        if let (Some((x0, y0)), Some((x1, y1))) = (min, max) {
            let (z, u) = (self.pos[2], self.pos[3]);
            for (x, y) in [(x0, y0), (x1, y0), (x1, y1), (x0, y1), (x0, y0)] {
                self.rapid_to([x, y, z, u], MotionKind::Rapid);
            }
        }
    }
}

fn axis_idx(a: Axis) -> usize {
    match a {
        Axis::X => 0,
        Axis::Y => 1,
        Axis::Z => 2,
        Axis::U => 3,
    }
}

fn dist(a: [i32; 4], b: [i32; 4]) -> f64 {
    let dx = (b[0] - a[0]) as f64;
    let dy = (b[1] - a[1]) as f64;
    let dz = (b[2] - a[2]) as f64;
    let du = (b[3] - a[3]) as f64;
    (dx * dx + dy * dy + dz * dz + du * du).sqrt()
}

/// True if `s` is a whole command (the last slice of a datagram may be cut off).
fn is_complete(s: &[u8]) -> bool {
    match Command::decode(s) {
        Command::Raw(_) => false,
        // Host-to-controller DA 01 is always a 14-byte write.
        Command::SettingReply { .. } | Command::SettingReplyRaw { .. } => s.len() >= 14,
        // A file name runs up to its NUL; the datagram or USB write may end before it.
        Command::SetFilename(_) => s.last() == Some(&0),
        _ => true,
    }
}

/// Estimated run time in ms (travel at `g0` µm/s, cuts at the programmed speed).
/// UNCERTAIN: the unit the real controller reports for file times is not documented.
pub fn estimate_ms(cmds: &[Command], g0: u32) -> u32 {
    let mut emu = RuidaEmulator::new(EmulatorConfig::default());
    emu.mem.insert(addr::G0_VELOCITY, g0);
    emu.job = Some(Job { cmds: cmds.to_vec(), pc: 0, cut_us: 0, total_us: 0 });
    emu.state = RunState::Running;
    let mut total = 0;
    while emu.state == RunState::Running {
        if !emu.step_job() {
            break;
        }
        total = emu.job.as_ref().map(|j| j.total_us).unwrap_or(total);
    }
    (total / 1000) as u32
}

/// Encode a reply datagram (used by tests that talk to the emulator by hand).
pub fn udp_datagram(logical: &[u8], magic: Magic) -> Vec<u8> {
    frame(&Swizzler::new(magic).swizzle(logical))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::encode_commands;
    use crate::swizzle::unswizzle;

    fn emu() -> RuidaEmulator {
        RuidaEmulator::new(EmulatorConfig::default())
    }

    #[test]
    fn acks_good_and_naks_bad_checksum() {
        let mut e = emu();
        let d = udp_datagram(&[0xCE], Magic::DEFAULT);
        assert_eq!(e.handle_udp_datagram(&d), vec![vec![0xC6]]);
        let mut bad = d.clone();
        bad[0] ^= 1;
        let r = e.handle_udp_datagram(&bad);
        assert_eq!(unswizzle(&r[0], Magic::DEFAULT), vec![0xCF]);
        assert_eq!(e.checksum_failures, 1);
    }

    #[test]
    fn answers_setting_read() {
        let mut e = emu();
        let q = Command::GetSetting { addr: addr::BED_X }.encode().unwrap();
        let r = e.handle_udp_datagram(&udp_datagram(&q, Magic::DEFAULT));
        assert_eq!(r.len(), 2);
        let reply = Command::decode(&unswizzle(&r[1], Magic::DEFAULT));
        assert_eq!(reply, Command::SettingReply { addr: addr::BED_X, value: 600_000 });
    }

    #[test]
    fn wrong_magic_gets_ack_but_no_reply() {
        let mut e = emu();
        let q = Command::GetSetting { addr: addr::CARD_ID }.encode().unwrap();
        let r = e.handle_udp_datagram(&udp_datagram(&q, Magic(0x11)));
        assert_eq!(r.len(), 1);
    }

    #[test]
    fn command_split_across_datagrams() {
        let mut e = emu();
        let bytes = encode_commands(&[Command::RapidXY { option: crate::command::RapidOption::None, x: 5000, y: 6000 }]).unwrap();
        e.handle_udp_datagram(&udp_datagram(&bytes[..6], Magic::DEFAULT));
        assert!(e.motion.is_empty());
        e.handle_udp_datagram(&udp_datagram(&bytes[6..], Magic::DEFAULT));
        assert_eq!(e.position()[..2], [5000, 6000]);
    }

    #[test]
    fn stream_framing_has_no_ack() {
        let mut e = emu();
        let sw = Swizzler::new(Magic::DEFAULT);
        assert!(e.handle_stream_bytes(&sw.swizzle(&[0xD8, 0x2A])).is_empty());
        let r = e.handle_stream_bytes(&sw.swizzle(&Command::GetSetting { addr: addr::BED_Y }.encode().unwrap()));
        assert_eq!(Command::decode(&sw.unswizzle(&r)), Command::SettingReply { addr: addr::BED_Y, value: 400_000 });
    }

    #[test]
    fn jog_port_replies_and_moves() {
        let mut e = emu();
        assert_eq!(e.handle_jog_datagram(&PanelCommand::Connect.encode()), vec![vec![0xCC]]);
        e.handle_jog_datagram(&PanelCommand::KeyDown(PanelKey::PlusX).encode());
        e.handle_jog_datagram(&PanelCommand::KeyUp(PanelKey::PlusX).encode());
        assert_eq!(e.position()[0], 10_000);
        assert!(e.handle_jog_datagram(&[0x01, 0x02]).is_empty());
    }

    #[test]
    fn estimate_is_distance_over_speed() {
        let cmds = vec![Command::Speed(10_000), Command::CutAbs { x: 100_000, y: 0 }, Command::EndOfFile];
        assert_eq!(estimate_ms(&cmds, 200_000), 10_000);
    }
}
