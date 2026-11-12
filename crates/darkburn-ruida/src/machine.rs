//! The Ruida driver: a blocking API over any [`Transport`].
//!
//! [`RuidaMachine`] does the packet work (swizzling, checksums, MTU-sized chunks, ACK waits,
//! retries, timeouts, reply matching) and offers machine-level operations: connect, status,
//! start/pause/resume/stop, home, go to origin, frame, jog, file list/upload/run/delete,
//! job time, settings read/write with backup and restore, controller info, laser 2 and U axis.
//!
//! The calls block. For an egui app wrap the machine in [`crate::worker::RuidaHandle`], which
//! runs it on its own thread and is polled once per frame.

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

use kurbo::{Point, Rect};

use crate::command::{Axis, Command, JogKey, PanelCommand, PanelKey, RapidOption, encode_commands, parse_commands, split_commands};
use crate::encoding::{decode_cstring, mm_to_um, um_to_mm};
use crate::error::{Result, RuidaError};
use crate::packet::{chunks, frame};
use crate::rd::RdJob;
use crate::settings::{ControllerInfo, Laser2Settings, MachineSettings, MachineStatus, StatusWord, UAxisSettings, addr, board_name, writable};
use crate::swizzle::{Magic, Swizzler};
use crate::transport::{Channel, Framing, Transport};

/// Driver configuration.
#[derive(Debug, Clone)]
pub struct MachineConfig {
    /// Swizzle magic. `None` probes the controller on connect.
    pub magic: Option<Magic>,
    /// Time to wait for an ACK or a reply.
    pub timeout: Duration,
    /// Time to wait for a reply while probing magic values.
    pub probe_timeout: Duration,
    /// Resends after a timeout, a NAK or an ERR on the first chunk.
    pub retries: u32,
    /// Payload bytes per datagram; `None` uses the transport's value.
    pub max_payload: Option<usize>,
    /// Longest file name sent to the controller. UNCERTAIN: MeerK40t's emulator truncates to 8.
    pub max_name_len: usize,
    /// Most files the controller holds (LightBurn warns at 99).
    pub max_files: usize,
}

impl Default for MachineConfig {
    fn default() -> Self {
        MachineConfig {
            magic: Some(Magic::DEFAULT),
            timeout: Duration::from_secs(3),
            probe_timeout: Duration::from_millis(300),
            retries: 3,
            max_payload: None,
            max_name_len: 8,
            max_files: 99,
        }
    }
}

/// A job stored on the controller.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ControllerFile {
    /// 1-based index used by run/delete/calc-time.
    pub index: u16,
    pub name: String,
}

/// What to do when a file name already exists on the controller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum OverwriteChoice {
    /// Delete the old file and store the new one under the same name.
    Overwrite,
    /// Store under the next free numbered name.
    Rename,
    /// Do not send.
    Cancel,
}

/// Options for [`RuidaMachine::send_job`].
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct SendOptions {
    /// Always append the next free number to the name (`JOB` -> `JOB1`, `JOB2`, ...).
    pub auto_increment: bool,
    /// Run the job as soon as it is stored.
    pub start_immediately: bool,
}

/// Result of a send.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SendOutcome {
    pub name: String,
    pub index: u16,
    pub started: bool,
    pub bytes: usize,
}

/// Which axes to home.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum HomeAxes {
    XY,
    Z,
    U,
}

/// The driver.
pub struct RuidaMachine<T: Transport = Box<dyn Transport>> {
    t: T,
    cfg: MachineConfig,
    swz: Swizzler,
    inbox: VecDeque<Command>,
    stream_buf: Vec<u8>,
    info: Option<ControllerInfo>,
    bed_um: Option<(u32, u32)>,
    /// Bytes sent on the data channel (logical), for progress reporting.
    pub bytes_sent: usize,
    /// Resends performed, for diagnostics.
    pub resends: usize,
    /// Copies of a datagram that were sent and never answered in time. Each may still be
    /// acknowledged late; such an acknowledge must not be taken for the next datagram's.
    late_acks: usize,
    /// The same on the jog port: how many answers to key commands may still arrive (copies
    /// sent, less answers seen), and the time after which they are given up.
    panel_owed: usize,
    panel_owed_until: Instant,
}

impl<T: Transport> RuidaMachine<T> {
    pub fn new(transport: T, cfg: MachineConfig) -> Self {
        let swz = Swizzler::new(cfg.magic.unwrap_or_default());
        RuidaMachine {
            t: transport,
            cfg,
            swz,
            inbox: VecDeque::new(),
            stream_buf: Vec::new(),
            info: None,
            bed_um: None,
            bytes_sent: 0,
            resends: 0,
            late_acks: 0,
            panel_owed: 0,
            panel_owed_until: Instant::now(),
        }
    }

    pub fn config(&self) -> &MachineConfig {
        &self.cfg
    }
    pub fn magic(&self) -> Magic {
        self.swz.magic()
    }
    pub fn transport(&self) -> &T {
        &self.t
    }
    pub fn transport_mut(&mut self) -> &mut T {
        &mut self.t
    }
    pub fn is_connected(&self) -> bool {
        self.info.is_some()
    }
    pub fn info(&self) -> Option<&ControllerInfo> {
        self.info.as_ref()
    }
    /// Bed size in mm, known after [`connect`](Self::connect).
    pub fn bed_size_mm(&self) -> Option<(f64, f64)> {
        self.bed_um.map(|(x, y)| (x as f64 / 1000.0, y as f64 / 1000.0))
    }

    // ---- connection -----------------------------------------------------------------------

    /// Probe the magic if needed, say hello (ENQ) and read controller info and bed size.
    pub fn connect(&mut self) -> Result<ControllerInfo> {
        if self.cfg.magic.is_none() {
            let m = self.detect_magic()?;
            self.swz = Swizzler::new(m);
        }
        self.send_commands(&[Command::Enq])?;
        let info = self.read_controller_info()?;
        let bx = self.read_setting(addr::BED_X)?;
        let by = self.read_setting(addr::BED_Y)?;
        self.bed_um = Some((bx, by));
        self.info = Some(info.clone());
        Ok(info)
    }

    pub fn disconnect(&mut self) {
        self.info = None;
    }

    /// Find the controller's magic by asking for the card ID with candidate values, the common
    /// ones (0x88, 0x11, 0x38) first and twice.
    ///
    /// Answers are judged by what they hold, never by when they arrive: on a busy network (and
    /// on Windows loopback) a datagram can be hundreds of milliseconds late, later than the
    /// answers to the probes sent after it. So:
    /// * the one-byte acknowledge a controller sends for every datagram comes back scrambled
    ///   with the controller's own magic, whichever magic the probe used, and so names it;
    /// * a card ID reply names its magic through its first byte;
    /// * a magic found this way is then checked with a fresh question at the normal timeout
    ///   and with the normal resends, and whatever is still queued is thrown away afterwards.
    pub fn detect_magic(&mut self) -> Result<Magic> {
        let mut order: Vec<Magic> = Magic::COMMON.to_vec();
        order.extend(Magic::COMMON);
        order.extend((0..=255u8).map(Magic).filter(|m| !Magic::COMMON.contains(m)));
        let udp = self.t.framing() == Framing::Udp;
        let probe = Command::GetSetting { addr: addr::CARD_ID }.encode()?;
        let mut refuted = [false; 256];
        for m in order {
            self.swz = Swizzler::new(m);
            self.inbox.clear();
            self.stream_buf.clear();
            let wire = self.swz.swizzle(&probe);
            if self.t.send(Channel::Data, &if udp { frame(&wire) } else { wire }).is_err() {
                continue;
            }
            let deadline = Instant::now() + self.cfg.probe_timeout;
            loop {
                let left = deadline.saturating_duration_since(Instant::now());
                let Ok(Some(d)) = self.t.recv(Channel::Data, left) else { break };
                let candidate = if !udp {
                    // A stream has no acknowledge: only the right magic gets an answer.
                    self.stream_buf.extend(self.swz.unswizzle(&d));
                    self.drain_stream();
                    self.inbox.iter().any(|c| matches!(c, Command::SettingReply { addr: a, .. } if *a == addr::CARD_ID)).then_some(m)
                } else if let [ack] = d.as_slice() {
                    Some(crate::swizzle::magic_from_wire(*ack, 0xCC))
                } else {
                    // A reply: it starts with DA under the magic it was scrambled with.
                    d.first().map(|b| crate::swizzle::magic_from_wire(*b, 0xDA)).filter(
                        |k| matches!(Command::decode(&Swizzler::new(*k).unswizzle(&d)), Command::SettingReply { addr: a, .. } if a == addr::CARD_ID),
                    )
                };
                if let Some(k) = candidate.filter(|k| !refuted[k.0 as usize]) {
                    if self.confirm_magic(k) {
                        return Ok(k);
                    }
                    refuted[k.0 as usize] = true;
                    self.swz = Swizzler::new(m);
                }
                if Instant::now() >= deadline {
                    break;
                }
            }
        }
        Err(RuidaError::Timeout("no answer to a card ID probe with any magic value".into()))
    }

    /// Ask for the card ID with magic `m` at the normal timeout and resends. On success the
    /// driver keeps `m` and nothing stale is left queued.
    fn confirm_magic(&mut self, m: Magic) -> bool {
        self.swz = Swizzler::new(m);
        self.inbox.clear();
        self.stream_buf.clear();
        let ok = matches!(self.read_setting_raw(addr::CARD_ID), Ok(Command::SettingReply { .. }));
        self.discard_queued();
        self.inbox.clear();
        self.stream_buf.clear();
        ok
    }

    /// Throw away whatever is waiting on the data channel (answers that came too late).
    fn discard_queued(&mut self) {
        for _ in 0..64 {
            if !matches!(self.t.recv(Channel::Data, Duration::from_millis(1)), Ok(Some(_))) {
                break;
            }
        }
    }

    // ---- packet layer ---------------------------------------------------------------------

    /// Send commands on the data channel.
    pub fn send_commands(&mut self, cmds: &[Command]) -> Result<()> {
        let bytes = encode_commands(cmds)?;
        self.send_logical(&bytes)
    }

    /// Send logical bytes on the data channel: swizzled, chunked to the MTU, and (on UDP)
    /// checksummed and acknowledged chunk by chunk.
    pub fn send_logical(&mut self, logical: &[u8]) -> Result<()> {
        self.send_logical_with_progress(logical, &mut |_, _| {})
    }

    /// As [`send_logical`](Self::send_logical), calling `progress(sent, total)` after every chunk.
    pub fn send_logical_with_progress(&mut self, logical: &[u8], progress: &mut dyn FnMut(usize, usize)) -> Result<()> {
        let max = self.cfg.max_payload.unwrap_or_else(|| self.t.max_payload());
        let total = logical.len();
        let mut sent = 0;
        for (i, chunk) in chunks(logical, max).enumerate() {
            let wire = self.swz.swizzle(chunk);
            match self.t.framing() {
                Framing::Stream => self.t.send(Channel::Data, &wire)?,
                Framing::Udp => self.send_datagram(&frame(&wire), i == 0)?,
            }
            sent += chunk.len();
            self.bytes_sent += chunk.len();
            progress(sent, total);
        }
        Ok(())
    }

    fn send_datagram(&mut self, datagram: &[u8], first: bool) -> Result<()> {
        self.drain_late_acks()?;
        let mut attempt = 0;
        loop {
            self.t.send(Channel::Data, datagram)?;
            let outcome = self.wait_ack()?;
            if matches!(outcome, Ack::Timeout) {
                self.late_acks += 1;
            }
            match outcome {
                Ack::Ok => return Ok(()),
                Ack::Timeout if attempt < self.cfg.retries => {}
                Ack::Timeout => return Err(RuidaError::Timeout("no ACK from the controller".into())),
                Ack::Nak if attempt < self.cfg.retries => {}
                Ack::Nak => return Err(RuidaError::ChecksumRejected),
                Ack::Err if first && attempt < self.cfg.retries => {}
                Ack::Err => return Err(RuidaError::ControllerError("ERR after a data chunk".into())),
            }
            attempt += 1;
            self.resends += 1;
        }
    }

    /// Datagrams carry no number, so an acknowledge cannot be matched to the datagram it is
    /// for. After a datagram went unanswered and was sent again, the answer to the earlier copy
    /// may still be on its way; it is waited for here (at most the normal timeout) before a new
    /// datagram goes out. Otherwise it would be taken as the acknowledge of that new datagram,
    /// which could then be lost without anyone noticing.
    fn drain_late_acks(&mut self) -> Result<()> {
        if self.late_acks == 0 {
            return Ok(());
        }
        let deadline = Instant::now() + self.cfg.timeout;
        while self.late_acks > 0 {
            let left = deadline.saturating_duration_since(Instant::now());
            let Some(d) = self.t.recv(Channel::Data, left)? else { break };
            let logical = self.swz.unswizzle(&d);
            match logical.as_slice() {
                [0xCC] | [0xCF] | [0xCD] => self.late_acks -= 1,
                _ => self.inbox.extend(parse_commands(&logical)),
            }
            if Instant::now() >= deadline {
                break;
            }
        }
        self.late_acks = 0;
        Ok(())
    }

    fn wait_ack(&mut self) -> Result<Ack> {
        let deadline = Instant::now() + self.cfg.timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let Some(d) = self.t.recv(Channel::Data, left)? else { return Ok(Ack::Timeout) };
            let logical = self.swz.unswizzle(&d);
            match logical.as_slice() {
                [0xCC] => return Ok(Ack::Ok),
                [0xCF] => return Ok(Ack::Nak),
                [0xCD] => return Ok(Ack::Err),
                _ => self.inbox.extend(parse_commands(&logical)),
            }
            if Instant::now() >= deadline {
                return Ok(Ack::Timeout);
            }
        }
    }

    /// Wait for a reply command matching `pred`.
    fn wait_reply(&mut self, pred: &dyn Fn(&Command) -> bool) -> Result<Option<Command>> {
        if let Some(i) = self.inbox.iter().position(pred) {
            return Ok(self.inbox.remove(i));
        }
        let deadline = Instant::now() + self.cfg.timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let Some(d) = self.t.recv(Channel::Data, left)? else { return Ok(None) };
            let logical = self.swz.unswizzle(&d);
            match self.t.framing() {
                Framing::Udp => {
                    if !matches!(logical.as_slice(), [0xCC] | [0xCD] | [0xCF]) {
                        self.inbox.extend(parse_commands(&logical));
                    }
                }
                Framing::Stream => {
                    self.stream_buf.extend_from_slice(&logical);
                    self.drain_stream();
                }
            }
            if let Some(i) = self.inbox.iter().position(pred) {
                return Ok(self.inbox.remove(i));
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
        }
    }

    /// Move complete commands from the stream buffer to the inbox.
    fn drain_stream(&mut self) {
        let buf = std::mem::take(&mut self.stream_buf);
        let slices = split_commands(&buf);
        let n = slices.len();
        for (i, s) in slices.into_iter().enumerate() {
            let c = Command::decode(s);
            // A USB serial port hands a reply over in pieces. The last slice may be a reply
            // that is still arriving, and some partial replies decode as something shorter.
            let complete = match (s, &c) {
                // A setting reply is DA 01 and two address bytes, then five value bytes, or for
                // the firmware version a NUL-terminated string of any length (so its first 9 or
                // 14 bytes must not be taken for a number or a settings write).
                ([0xDA, 0x01, hi, lo, rest @ ..], _) => {
                    let a = (u16::from(*hi & 0x7F) << 7) | u16::from(*lo & 0x7F);
                    if a == addr::FIRMWARE { rest.last() == Some(&0) } else { rest.len() >= 5 }
                }
                ([0xDA, 0x01, ..], _) => false,
                (_, Command::Raw(_)) => false,
                // E8 01, two index bytes, the name, NUL. The first four bytes alone read as the
                // request for a name, which a controller never sends.
                (_, Command::GetFileName(_)) => false,
                (_, Command::FileName { .. }) => s.len() >= 5 && s.last() == Some(&0),
                _ => true,
            };
            if i + 1 == n && !complete {
                self.stream_buf = s.to_vec();
            } else {
                self.inbox.push_back(c);
            }
        }
    }

    /// Send `cmd` and wait for a reply matching `pred`, resending on timeout.
    pub fn query(&mut self, cmd: &Command, pred: &dyn Fn(&Command) -> bool) -> Result<Command> {
        let mut attempt = 0;
        loop {
            self.send_commands(std::slice::from_ref(cmd))?;
            if let Some(r) = self.wait_reply(pred)? {
                return Ok(r);
            }
            if attempt >= self.cfg.retries {
                return Err(RuidaError::Timeout(format!("no reply to {cmd:?}")));
            }
            attempt += 1;
            self.resends += 1;
        }
    }

    /// Send one command on the jog / wireless-panel port and wait for its `CC`.
    /// Only on UDP; on USB use the data-channel equivalents.
    ///
    /// A key release is held to more than a key press. The answers carry no number, so the
    /// answer to an earlier command that was sent twice may still arrive and cannot be told
    /// from the answer to this one. For a press the worst case is a key that was not pressed.
    /// A release that is lost while such a leftover answer passes for its own leaves the key
    /// held down on the controller (the laser firing, an axis moving) with nobody the wiser.
    /// So a release counts as answered only once more answers have come than earlier commands
    /// can still be owed. It is never held back for that: it goes out at once, and is sent
    /// again while its answer is in doubt, which does no harm.
    pub fn send_panel(&mut self, cmd: PanelCommand) -> Result<()> {
        if self.t.framing() != Framing::Udp {
            return Err(RuidaError::Unsupported("the jog port exists only over Ethernet".into()));
        }
        let release = matches!(cmd, PanelCommand::KeyUp(_));
        if Instant::now() >= self.panel_owed_until {
            self.panel_owed = 0;
        }
        // What waits on the port now was sent before this command went out: not its answer.
        while self.t.recv(Channel::Jog, Duration::ZERO)?.is_some() {
            self.panel_owed = self.panel_owed.saturating_sub(1);
        }
        // Answers of earlier commands that may still come, and answers seen since this one
        // first went out.
        let earlier = self.panel_owed;
        let mut seen = 0;
        let mut attempt = 0;
        loop {
            self.t.send(Channel::Jog, &cmd.encode())?;
            self.panel_owed += 1;
            self.panel_owed_until = Instant::now() + self.cfg.timeout;
            if let Some(r) = self.t.recv(Channel::Jog, self.cfg.timeout)?
                && r.first() == Some(&0xCC)
            {
                self.panel_owed = self.panel_owed.saturating_sub(1);
                seen += 1;
                if !release || seen > earlier {
                    return Ok(());
                }
                // A release, and this answer may be an earlier command's: sent again at once.
            }
            if attempt >= self.cfg.retries {
                return Err(RuidaError::Timeout("no reply on the jog port".into()));
            }
            attempt += 1;
            self.resends += 1;
        }
    }

    // ---- settings -------------------------------------------------------------------------

    /// Read one setting as a number.
    pub fn read_setting(&mut self, a: u16) -> Result<u32> {
        match self.read_setting_raw(a)? {
            Command::SettingReply { value, .. } => Ok(value),
            other => Err(RuidaError::Decode(format!("setting 0x{a:03X} is not a number: {other:?}"))),
        }
    }

    /// Read one setting, returning the reply command as is (numbers or strings).
    pub fn read_setting_raw(&mut self, a: u16) -> Result<Command> {
        let pred = move |c: &Command| matches!(c, Command::SettingReply { addr, .. } | Command::SettingReplyRaw { addr, .. } if *addr == a);
        self.query(&Command::GetSetting { addr: a }, &pred)
    }

    /// Write one setting (`DA 01 addr v v`).
    pub fn write_setting(&mut self, a: u16, value: u32) -> Result<()> {
        self.send_commands(&[Command::SetSetting { addr: a, value, value2: value }])
    }

    /// Read many settings.
    pub fn read_settings(&mut self, addrs: &[u16]) -> Result<BTreeMap<u16, u32>> {
        addrs.iter().map(|&a| Ok((a, self.read_setting(a)?))).collect()
    }

    /// Read every writable register into a backup.
    pub fn backup_settings(&mut self) -> Result<MachineSettings> {
        let addrs: Vec<u16> = writable(None).map(|r| r.addr).collect();
        let values = self.read_settings(&addrs)?;
        let card_id = self.read_setting(addr::CARD_ID).ok();
        let firmware = self.read_firmware().ok();
        Ok(MachineSettings { card_id, firmware, values })
    }

    /// Write a backup back. Only addresses of known writable registers are written; the
    /// rest are skipped and returned. Values are read back to verify.
    pub fn restore_settings(&mut self, s: &MachineSettings) -> Result<Vec<u16>> {
        let mut skipped = Vec::new();
        for (&a, &v) in &s.values {
            if crate::settings::register(a).is_some_and(|r| r.writable) {
                self.write_setting(a, v)?;
            } else {
                skipped.push(a);
            }
        }
        for (&a, &v) in &s.values {
            if !skipped.contains(&a) {
                let got = self.read_setting(a)?;
                if got != v {
                    return Err(RuidaError::ControllerError(format!("setting 0x{a:03X} reads {got} after writing {v}")));
                }
            }
        }
        Ok(skipped)
    }

    fn read_firmware(&mut self) -> Result<String> {
        match self.read_setting_raw(addr::FIRMWARE)? {
            Command::SettingReplyRaw { data, .. } => Ok(decode_cstring(&data).0),
            Command::SettingReply { value, .. } => Ok(format!("{value:08X}")),
            _ => Ok(String::new()),
        }
    }

    /// Card ID, firmware and the run-time counters.
    pub fn read_controller_info(&mut self) -> Result<ControllerInfo> {
        let card_id = self.read_setting(addr::CARD_ID)?;
        let firmware = self.read_firmware().unwrap_or_default();
        Ok(ControllerInfo {
            card_id,
            board: board_name(card_id).map(str::to_string),
            firmware,
            total_on_time_s: self.read_setting(addr::TOTAL_OPEN_TIME)?,
            total_job_time_s: self.read_setting(addr::TOTAL_WORK_TIME)?,
            total_laser_on_time_s: self.read_setting(addr::TOTAL_LASER_ON_TIME)?,
            jobs_run: self.read_setting(addr::TOTAL_WORK_NUMBER)?,
            last_job_time_ms: self.read_setting(addr::PREVIOUS_WORK_TIME)?,
        })
    }

    pub fn read_laser2(&mut self) -> Result<Laser2Settings> {
        let mut v = [0u32; 5];
        for (slot, a) in v.iter_mut().zip(Laser2Settings::ADDRS) {
            *slot = self.read_setting(a)?;
        }
        Ok(Laser2Settings::from_values(v))
    }

    pub fn write_laser2(&mut self, s: Laser2Settings) -> Result<()> {
        for (a, v) in Laser2Settings::ADDRS.into_iter().zip(s.to_values()) {
            self.write_setting(a, v)?;
        }
        Ok(())
    }

    pub fn read_u_axis(&mut self) -> Result<UAxisSettings> {
        let mut v = [0u32; 12];
        for (slot, a) in v.iter_mut().zip(UAxisSettings::ADDRS) {
            *slot = self.read_setting(a)?;
        }
        Ok(UAxisSettings::from_values(v))
    }

    pub fn write_u_axis(&mut self, s: UAxisSettings) -> Result<()> {
        for (a, v) in UAxisSettings::ADDRS.into_iter().zip(s.to_values()) {
            self.write_setting(a, v)?;
        }
        Ok(())
    }

    // ---- status and control ---------------------------------------------------------------

    pub fn status(&mut self) -> Result<MachineStatus> {
        let word = StatusWord(self.read_setting(addr::MACHINE_STATUS)?);
        Ok(MachineStatus {
            word,
            state: word.state(),
            x: self.read_setting(addr::POS_X)? as i32,
            y: self.read_setting(addr::POS_Y)? as i32,
            z: self.read_setting(addr::POS_Z)? as i32,
            u: self.read_setting(addr::POS_U)? as i32,
        })
    }

    /// Current XY position in mm.
    pub fn position_mm(&mut self) -> Result<Point> {
        let s = self.status()?;
        Ok(Point::new(um_to_mm(s.x), um_to_mm(s.y)))
    }

    /// Start (or restart) the current job.
    pub fn start(&mut self) -> Result<()> {
        self.send_commands(&[Command::Start])
    }
    pub fn pause(&mut self) -> Result<()> {
        self.send_commands(&[Command::Pause])
    }
    pub fn resume(&mut self) -> Result<()> {
        self.send_commands(&[Command::Resume])
    }
    pub fn stop(&mut self) -> Result<()> {
        self.send_commands(&[Command::Stop])
    }

    pub fn home(&mut self, axes: HomeAxes) -> Result<()> {
        self.send_commands(&[match axes {
            HomeAxes::XY => Command::HomeXY,
            HomeAxes::Z => Command::HomeZ,
            HomeAxes::U => Command::HomeU,
        }])
    }

    /// Rapid move to the job origin. UNCERTAIN: sent as `D9 10 00 0 0` (rapid XY with the
    /// "origin" option); a real controller may need the panel Origin key instead.
    pub fn go_to_origin(&mut self) -> Result<()> {
        self.send_commands(&[Command::RapidXY { option: RapidOption::Origin, x: 0, y: 0 }])
    }

    /// Rapid move to a point in mm. Checked against the bed size when known.
    pub fn move_to(&mut self, p: Point, light: bool) -> Result<()> {
        self.check_bounds(p)?;
        let option = if light { RapidOption::Light } else { RapidOption::None };
        self.send_commands(&[Command::RapidXY { option, x: mm_to_um(p.x), y: mm_to_um(p.y) }])
    }

    fn check_bounds(&self, p: Point) -> Result<()> {
        // `NaN < 0.0` and `NaN > bed` are both false, and NaN turns into 0 µm when it is encoded.
        if !p.x.is_finite() || !p.y.is_finite() {
            return Err(RuidaError::Encode("the position is not a number".into()));
        }
        if let Some((bx, by)) = self.bed_size_mm()
            && (p.x < 0.0 || p.y < 0.0 || p.x > bx || p.y > by)
        {
            return Err(RuidaError::Encode(format!("({:.3}, {:.3}) mm is outside the {bx} x {by} mm bed", p.x, p.y)));
        }
        Ok(())
    }

    /// Trace a rectangle with rapid moves (laser off, or red light when `light`).
    pub fn frame_rect(&mut self, r: Rect, light: bool) -> Result<()> {
        let pts = [Point::new(r.x0, r.y0), Point::new(r.x1, r.y0), Point::new(r.x1, r.y1), Point::new(r.x0, r.y1), Point::new(r.x0, r.y0)];
        self.frame_outline(&pts, light)
    }

    /// Trace any outline with rapid moves. Every point is checked before anything moves.
    pub fn frame_outline(&mut self, pts: &[Point], light: bool) -> Result<()> {
        for &p in pts {
            self.check_bounds(p)?;
        }
        let option = if light { RapidOption::Light } else { RapidOption::None };
        let cmds: Vec<Command> = pts.iter().map(|p| Command::RapidXY { option, x: mm_to_um(p.x), y: mm_to_um(p.y) }).collect();
        self.send_commands(&cmds)
    }

    /// Ask the controller to frame its current job (panel "Frame", jog port, UDP only).
    pub fn frame_on_controller(&mut self) -> Result<()> {
        self.send_panel(PanelCommand::Frame)
    }

    /// Jog one axis by `distance_mm` (relative, rapid).
    pub fn jog_step(&mut self, axis: Axis, distance_mm: f64) -> Result<()> {
        let s = self.status()?;
        let cur = match axis {
            Axis::X => s.x,
            Axis::Y => s.y,
            Axis::Z => s.z,
            Axis::U => s.u,
        };
        if !distance_mm.is_finite() {
            return Err(RuidaError::Encode("the jog distance is not a number".into()));
        }
        let target =
            cur.checked_add(mm_to_um(distance_mm)).ok_or_else(|| RuidaError::Encode(format!("a jog of {distance_mm} mm is out of range")))?;
        if matches!(axis, Axis::X | Axis::Y) {
            let p = if axis == Axis::X { Point::new(um_to_mm(target), um_to_mm(s.y)) } else { Point::new(um_to_mm(s.x), um_to_mm(target)) };
            self.check_bounds(p)?;
        }
        self.send_commands(&[Command::RapidAxis { axis, option: RapidOption::None, pos: target }])
    }

    /// Start continuous jog (key down). Call [`jog_stop`](Self::jog_stop) to end it.
    pub fn jog_start(&mut self, axis: Axis, positive: bool) -> Result<()> {
        self.send_commands(&[Command::KeyDown(JogKey::for_axis(axis, positive))])
    }

    /// End continuous jog (key up).
    pub fn jog_stop(&mut self, axis: Axis, positive: bool) -> Result<()> {
        self.send_commands(&[Command::KeyUp(JogKey::for_axis(axis, positive))])
    }

    /// Press or release a wireless-panel key on the jog port (UDP only).
    pub fn panel_key(&mut self, key: PanelKey, down: bool) -> Result<()> {
        self.send_panel(if down { PanelCommand::KeyDown(key) } else { PanelCommand::KeyUp(key) })
    }

    // ---- files ----------------------------------------------------------------------------

    /// Files stored on the controller.
    pub fn list_files(&mut self) -> Result<Vec<ControllerFile>> {
        let n = self.read_setting(addr::FILE_COUNT)?;
        let mut out = Vec::new();
        for i in 1..=n.min(0x3FFF) as u16 {
            let pred = move |c: &Command| matches!(c, Command::FileName { index, .. } if *index == i);
            if let Command::FileName { name, .. } = self.query(&Command::GetFileName(i), &pred)? {
                out.push(ControllerFile { index: i, name });
            }
        }
        Ok(out)
    }

    /// Store logical job bytes under `name` (no checks; see [`send_job`](Self::send_job)).
    pub fn upload_logical(&mut self, name: &str, job: &[u8], progress: &mut dyn FnMut(usize, usize)) -> Result<()> {
        if job.last() != Some(&0xD7) {
            return Err(RuidaError::Encode("job bytes must end with the end-of-file command".into()));
        }
        let mut bytes = encode_commands(&[Command::BeginFileTransfer, Command::SetFilename(name.to_string())])?;
        bytes.extend_from_slice(job);
        self.send_logical_with_progress(&bytes, progress)
    }

    /// Store an existing `.rd` file (swizzled with `file_magic`, or detected when `None`).
    /// RDWorks calls this "download" to the controller. There is no documented command to
    /// read a stored job back from the controller.
    pub fn upload_rd_file(
        &mut self,
        name: &str,
        rd_file: &[u8],
        file_magic: Option<Magic>,
        opts: &SendOptions,
        prompt: &mut dyn FnMut(&str) -> OverwriteChoice,
    ) -> Result<SendOutcome> {
        let magic =
            file_magic.or_else(|| crate::swizzle::detect_magic_histogram(rd_file)).ok_or_else(|| RuidaError::Decode("empty .rd file".into()))?;
        let logical = Swizzler::new(magic).unswizzle(rd_file);
        self.send_logical_job(name, &logical, opts, prompt, &mut |_, _| {})
    }

    /// Build a job and store it on the controller under `name`, with the overwrite prompt
    /// `prompt` called if the name already exists (unless `opts.auto_increment`).
    pub fn send_job(&mut self, name: &str, job: &RdJob, opts: &SendOptions, prompt: &mut dyn FnMut(&str) -> OverwriteChoice) -> Result<SendOutcome> {
        let logical = job.to_logical()?;
        self.send_logical_job(name, &logical, opts, prompt, &mut |_, _| {})
    }

    /// As [`send_job`](Self::send_job) for logical bytes, with progress.
    pub fn send_logical_job(
        &mut self,
        name: &str,
        logical: &[u8],
        opts: &SendOptions,
        prompt: &mut dyn FnMut(&str) -> OverwriteChoice,
        progress: &mut dyn FnMut(usize, usize),
    ) -> Result<SendOutcome> {
        let files = self.list_files()?;
        let names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
        let clean = sanitize_name(name, self.cfg.max_name_len);
        let mut target = if opts.auto_increment { next_name(&clean, &names, self.cfg.max_name_len) } else { clean.clone() };
        let mut replace: Option<u16> = None;
        if let Some(existing) = files.iter().find(|f| f.name == target) {
            match prompt(&target) {
                OverwriteChoice::Overwrite => replace = Some(existing.index),
                OverwriteChoice::Rename => target = next_name(&clean, &names, self.cfg.max_name_len),
                OverwriteChoice::Cancel => return Err(RuidaError::Cancelled),
            }
        }
        if replace.is_none() && files.len() >= self.cfg.max_files {
            return Err(RuidaError::ControllerError(format!("the controller already holds {} files", files.len())));
        }
        if let Some(i) = replace {
            self.delete_files(&[i])?;
        }
        self.upload_logical(&target, logical, progress)?;
        let index = self
            .list_files()?
            .into_iter()
            .find(|f| f.name == target)
            .map(|f| f.index)
            .ok_or_else(|| RuidaError::NoSuchFile(format!("{target} did not appear after upload")))?;
        if opts.start_immediately {
            self.run_file(index)?;
        }
        Ok(SendOutcome { name: target, index, started: opts.start_immediately, bytes: logical.len() })
    }

    /// Stream a job straight to the controller and run it without storing it under a name.
    pub fn run_job_now(&mut self, job: &RdJob) -> Result<()> {
        let logical = job.to_logical()?;
        self.send_logical(&logical)
    }

    pub fn run_file(&mut self, index: u16) -> Result<()> {
        self.send_commands(&[Command::RunFile(index)])
    }

    /// Delete one or many files by index. Deleted from the highest index down so the
    /// remaining indices stay valid.
    pub fn delete_files(&mut self, indices: &[u16]) -> Result<()> {
        let mut v: Vec<u16> = indices.iter().copied().filter(|&i| i > 0).collect();
        v.sort_unstable();
        v.dedup();
        for i in v.into_iter().rev() {
            self.send_commands(&[Command::DeleteFiles { first: i, last: i }])?;
        }
        Ok(())
    }

    pub fn delete_all_files(&mut self) -> Result<()> {
        self.send_commands(&[Command::DeleteFiles { first: 0, last: 0 }])
    }

    /// Ask the controller to calculate a stored job's run time.
    /// UNCERTAIN: the result address (0x390 + index) and its unit (taken as ms) come from
    /// MeerK40t's emulator and need checking.
    pub fn calc_time(&mut self, index: u16) -> Result<Duration> {
        let result = addr::FILE_TIME_BASE.checked_add(index).ok_or_else(|| RuidaError::NoSuchFile(format!("there is no file number {index}")))?;
        self.send_commands(&[Command::CalcFileTime(index)])?;
        let ms = self.read_setting(result)?;
        Ok(Duration::from_millis(ms as u64))
    }
}

enum Ack {
    Ok,
    Nak,
    Err,
    Timeout,
}

/// Make a name safe for the controller: printable ASCII without spaces, at most `max` chars.
pub fn sanitize_name(name: &str, max: usize) -> String {
    let s: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || "-_.".contains(c) { c } else { '_' }).take(max.max(1)).collect();
    if s.is_empty() { "JOB".into() } else { s }
}

/// The next free numbered name: `base` with trailing digits replaced by the smallest number
/// above any existing one for that base, truncated so it fits `max`.
pub fn next_name(base: &str, existing: &[&str], max: usize) -> String {
    let stem = base.trim_end_matches(|c: char| c.is_ascii_digit());
    let stem = if stem.is_empty() { "JOB" } else { stem };
    let mut n = existing
        .iter()
        .filter_map(|e| {
            let digits = e.trim_start_matches(|c: char| !c.is_ascii_digit());
            let prefix = &e[..e.len() - digits.len()];
            let full = stem.len().min(max.saturating_sub(digits.len()));
            (!digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) && stem.starts_with(prefix) && prefix.len() == full)
                .then(|| digits.parse::<u32>().ok())
                .flatten()
        })
        .max()
        .unwrap_or(0)
        + 1;
    loop {
        let num = n.to_string();
        let keep = max.saturating_sub(num.len()).max(1).min(stem.len());
        let candidate = format!("{}{}", &stem[..keep], num);
        if !existing.contains(&candidate.as_str()) {
            return candidate;
        }
        n += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(sanitize_name("my job!", 8), "my_job_");
        assert_eq!(sanitize_name("", 8), "JOB");
        assert_eq!(sanitize_name("ABCDEFGHIJ", 8), "ABCDEFGH");
        assert_eq!(next_name("JOB", &[], 8), "JOB1");
        assert_eq!(next_name("JOB", &["JOB1", "JOB2"], 8), "JOB3");
        assert_eq!(next_name("JOB7", &["JOB7"], 8), "JOB8");
        assert_eq!(next_name("LONGNAME", &["LONGNAME"], 8), "LONGNAM1");
        assert_eq!(next_name("LONGNAME", &["LONGNAM9"], 8), "LONGNA10");
    }
}
