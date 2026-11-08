//! An in-memory BJJCZ-style galvo board. Every test talks to this; nothing here touches
//! hardware.
//!
//! The emulator understands the single commands and list commands of [`crate::codec`]. List
//! packets are queued and executed a few commands at a time every time the host polls the board
//! (`GetVersion`, `GetListStatus`, `ReadPort`, ...), so a job stays "busy" over many polls and
//! stop / pause / door tests are deterministic and need no clock.
//!
//! It records everything a safety test cares about: marks with the laser state at that moment,
//! whether the laser was enabled, whether MO was open, and the final output port.

use std::collections::VecDeque;

use crate::codec::{COMMAND_LEN, Command, ListCmd, PACKET_LEN, REPLY_LEN, Reply, STATUS_AXIS, STATUS_BUSY, STATUS_READY, SingleCmd};
use crate::cor::{CorTable, decode_offset};

/// Emulator behaviour knobs.
#[derive(Clone, Debug)]
pub struct EmulatorConfig {
    /// List commands executed per status poll while a list runs.
    pub cmds_per_poll: usize,
    /// Packets the board buffers before `READY` clears.
    pub packet_capacity: usize,
    pub serial: [u16; 4],
    pub version: u16,
    /// Polls an axis move takes before the AXIS bit clears.
    pub axis_polls: u32,
}

impl Default for EmulatorConfig {
    fn default() -> Self {
        Self { cmds_per_poll: 64, packet_capacity: 4, serial: [0x4D45, 0x4552, 0x4B34, 0x3054], version: 0x0103, axis_polls: 2 }
    }
}

/// One executed motion.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Motion {
    pub from: (u16, u16),
    pub to: (u16, u16),
    /// True for `MarkTo`, false for `JumpTo`.
    pub mark: bool,
    /// The laser could really emit: board enabled and MO open and the mark bit set.
    pub emitting: bool,
    /// Fibre current word active at the time.
    pub current: u16,
    /// Q-switch period word active at the time.
    pub q_period: u16,
    pub mark_speed: u16,
}

/// What the emulator saw.
#[derive(Clone, Debug, Default)]
pub struct EmulatorLog {
    /// Every single command received, in order.
    pub singles: Vec<SingleCmd>,
    pub motions: Vec<Motion>,
    pub packets_received: usize,
    /// List commands executed in total.
    pub executed: usize,
    /// Times the red-light bit was set in a port write.
    pub light_on_writes: usize,
    /// `LaserOnPoint` count.
    pub points: usize,
    /// Accumulated simulated mark time in microseconds.
    pub time_us: f64,
}

/// The emulated board.
pub struct GalvoEmulator {
    cfg: EmulatorConfig,
    log: EmulatorLog,
    disconnected: bool,
    // outputs
    laser_enabled: bool,
    laser_signal: bool,
    mo_open: bool,
    port_out: u16,
    port_in: u16,
    pos: (u16, u16),
    // lens table
    cor_loading: bool,
    cor_words: Vec<(u16, u16)>,
    cor_active: Option<Vec<(u16, u16)>>,
    // list engine
    queue: VecDeque<ListCmd>,
    packets_queued: usize,
    executing: bool,
    paused: bool,
    ended: bool,
    // list parameters
    current: u16,
    q_period: u16,
    mark_speed: u16,
    jump_speed: u16,
    // axes
    axis_pos: [i64; 2],
    axis_busy: u32,
    z_data: u16,
    ports_written_by_list: u16,
}

impl Default for GalvoEmulator {
    fn default() -> Self {
        Self::new(EmulatorConfig::default())
    }
}

impl GalvoEmulator {
    pub fn new(cfg: EmulatorConfig) -> Self {
        Self {
            cfg,
            log: EmulatorLog::default(),
            disconnected: false,
            laser_enabled: false,
            laser_signal: false,
            mo_open: false,
            port_out: 0,
            port_in: 0xFFFF,
            pos: (32768, 32768),
            cor_loading: false,
            cor_words: Vec::new(),
            cor_active: None,
            queue: VecDeque::new(),
            packets_queued: 0,
            executing: false,
            paused: false,
            ended: false,
            current: 0,
            q_period: 0,
            mark_speed: 1,
            jump_speed: 1,
            axis_pos: [0; 2],
            axis_busy: 0,
            z_data: 0,
            ports_written_by_list: 0,
        }
    }

    // ----- test controls -------------------------------------------------------------------

    /// Simulates unplugging: every transport call fails afterwards.
    pub fn disconnect(&mut self) {
        self.disconnected = true;
    }
    pub fn is_disconnected(&self) -> bool {
        self.disconnected
    }

    /// Sets the input port word `ReadPort` returns (bit 15 high = foot switch released with
    /// the default polarity; all high by default).
    pub fn set_inputs(&mut self, bits: u16) {
        self.port_in = bits;
    }
    pub fn set_input_bit(&mut self, bit: u8, level: bool) {
        if level {
            self.port_in |= 1 << bit;
        } else {
            self.port_in &= !(1 << bit);
        }
    }

    pub fn log(&self) -> &EmulatorLog {
        &self.log
    }
    pub fn clear_log(&mut self) {
        self.log = EmulatorLog::default();
    }
    pub fn laser_enabled(&self) -> bool {
        self.laser_enabled
    }
    pub fn laser_signal(&self) -> bool {
        self.laser_signal
    }
    pub fn mo_open(&self) -> bool {
        self.mo_open
    }
    pub fn port_out(&self) -> u16 {
        self.port_out
    }
    pub fn position(&self) -> (u16, u16) {
        self.pos
    }
    pub fn is_executing(&self) -> bool {
        self.executing
    }
    pub fn is_paused(&self) -> bool {
        self.paused
    }
    pub fn queued_commands(&self) -> usize {
        self.queue.len()
    }
    pub fn axis_position(&self, axis: usize) -> i64 {
        self.axis_pos[axis]
    }
    pub fn z_data(&self) -> u16 {
        self.z_data
    }
    /// Active lens table as decoded offsets, or `None` for the blank table.
    pub fn lens_table(&self) -> Option<CorTable> {
        self.cor_active.as_ref().map(|w| {
            let mut i = 0;
            CorTable::from_fn(|_, _| {
                let (a, b) = w[i];
                i += 1;
                (decode_offset(a), decode_offset(b))
            })
        })
    }
    /// True if any motion with the laser able to emit has happened.
    pub fn ever_emitted(&self) -> bool {
        self.emitting_marks() > 0 || self.log.points > 0
    }
    /// Marks that were really emitting.
    pub fn emitting_marks(&self) -> usize {
        self.log.motions.iter().filter(|m| m.mark && m.emitting).count()
    }

    /// Runs the list engine for `polls` polls (as if the host polled that often).
    pub fn run_polls(&mut self, polls: usize) {
        for _ in 0..polls {
            self.tick();
        }
    }

    // ----- wire ----------------------------------------------------------------------------

    /// Handles a write of 12 or 3072 bytes. Returns the 8-byte reply for single commands.
    pub fn write(&mut self, data: &[u8]) -> Option<[u8; REPLY_LEN]> {
        match data.len() {
            COMMAND_LEN => {
                let cmd = Command::from_bytes(data).expect("12 bytes");
                if cmd.is_list() {
                    // a list opcode sent as a single command is ignored by the board
                    return Some(self.reply([0; 3]).to_bytes());
                }
                let single = SingleCmd::from_command(cmd);
                self.log.singles.push(single);
                let reply = self.single(single).to_bytes();
                // lens table rows are not answered by the board
                if matches!(single, SingleCmd::WriteCorLine { .. }) { None } else { Some(reply) }
            }
            PACKET_LEN => {
                self.log.packets_received += 1;
                self.packets_queued += 1;
                for chunk in data.chunks_exact(COMMAND_LEN) {
                    self.queue.push_back(ListCmd::from_command(Command::from_bytes(chunk).expect("12 bytes")));
                }
                None
            }
            _ => None,
        }
    }

    fn status(&self) -> u16 {
        let mut s = 0;
        if self.executing || self.paused {
            s |= STATUS_BUSY;
        }
        if self.packets_queued < self.cfg.packet_capacity {
            s |= STATUS_READY;
        }
        if self.axis_busy > 0 {
            s |= STATUS_AXIS;
        }
        s
    }

    fn reply(&self, w: [u16; 3]) -> Reply {
        Reply { words: [w[0], w[1], w[2], self.status()] }
    }

    fn single(&mut self, c: SingleCmd) -> Reply {
        use SingleCmd::*;
        match c {
            GetVersion => {
                self.tick();
                return self.reply([self.cfg.version, 0, 0]);
            }
            GetSerialNo => return Reply { words: [self.cfg.serial[0], self.cfg.serial[1], self.cfg.serial[2], self.cfg.serial[3]] },
            GetListStatus | GetMarkCount => {
                self.tick();
            }
            GetPositionXy => return self.reply([self.pos.0, self.pos.1, 0]),
            ReadPort => {
                self.tick();
                return self.reply([0, self.port_in, 0]);
            }
            GetMarkTime => return self.reply([(self.log.time_us / 10.0) as u16, 0, 0]),
            GetAxisPos(i) => {
                let v = self.axis_pos[(i as usize).min(1)];
                self.tick();
                return self.reply([(v & 0xFFFF) as u16, ((v >> 16) & 0xFFFF) as u16, 0]);
            }
            DisableLaser => self.laser_enabled = false,
            EnableLaser => self.laser_enabled = true,
            LaserSignalOn => self.laser_signal = true,
            LaserSignalOff => self.laser_signal = false,
            FiberSetMo(on) => self.mo_open = on,
            WritePort(v) => self.write_port(v),
            GotoXy { x, y, .. } => self.pos = (x, y),
            ExecuteList => {
                self.executing = true;
                self.paused = false;
            }
            StopExecute => {
                self.executing = false;
                self.paused = false;
                self.queue.clear();
                self.packets_queued = 0;
                self.ended = false;
            }
            StopList => {
                if self.executing {
                    self.paused = true;
                    self.executing = false;
                }
            }
            RestartList => {
                if self.paused {
                    self.paused = false;
                    self.executing = true;
                }
            }
            ResetList => {
                self.executing = false;
                self.paused = false;
                self.queue.clear();
                self.packets_queued = 0;
                self.ended = false;
            }
            Reset => {
                self.executing = false;
                self.paused = false;
                self.queue.clear();
                self.packets_queued = 0;
                self.laser_enabled = false;
                self.laser_signal = false;
                self.mo_open = false;
                self.port_out = 0;
            }
            WriteCorTable(true) => {
                self.cor_loading = true;
                self.cor_words.clear();
            }
            WriteCorTable(false) => {
                self.cor_loading = false;
                self.cor_active = None;
            }
            WriteCorLine { dx, dy, .. } => {
                if self.cor_loading {
                    self.cor_words.push((dx, dy));
                    if self.cor_words.len() == crate::cor::GRID * crate::cor::GRID {
                        self.cor_active = Some(std::mem::take(&mut self.cor_words));
                        self.cor_loading = false;
                    }
                }
            }
            MoveAxisTo { position, invert } => {
                let v = position as i64 * if invert != 0 { -1 } else { 1 };
                self.axis_pos[0] = v;
                self.axis_busy = self.cfg.axis_polls;
            }
            AxisGoOrigin => {
                self.axis_pos[0] = 0;
                self.axis_busy = self.cfg.axis_polls;
            }
            SetZData(v) => self.z_data = v,
            _ => {}
        }
        self.reply([0; 3])
    }

    fn write_port(&mut self, v: u16) {
        if v & (1 << 8) != 0 {
            self.log.light_on_writes += 1;
        }
        self.port_out = v;
    }

    fn can_emit(&self) -> bool {
        self.laser_enabled && self.mo_open && self.port_out & 1 != 0
    }

    /// Advances the list engine and axis timers by one poll.
    fn tick(&mut self) {
        self.axis_busy = self.axis_busy.saturating_sub(1);
        if !self.executing {
            return;
        }
        for _ in 0..self.cfg.cmds_per_poll {
            let Some(cmd) = self.queue.pop_front() else {
                // everything consumed
                if self.ended {
                    self.executing = false;
                    self.ended = false;
                    self.packets_queued = 0;
                }
                return;
            };
            if self.queue.len().is_multiple_of(crate::codec::COMMANDS_PER_PACKET) {
                self.packets_queued = self.packets_queued.saturating_sub(1);
            }
            self.log.executed += 1;
            self.exec(cmd);
        }
    }

    fn exec(&mut self, cmd: ListCmd) {
        match cmd {
            ListCmd::EndOfList => self.ended = true,
            ListCmd::JumpTo { x, y, .. } => {
                // padding after an EndOfList is ignored by design of `ended`; real moves reset it
                self.ended = false;
                self.motion(x, y, false);
            }
            ListCmd::MarkTo { x, y, .. } => {
                self.ended = false;
                self.motion(x, y, true);
            }
            ListCmd::LaserOnPoint { .. } => {
                self.ended = false;
                if self.can_emit() {
                    self.log.points += 1;
                }
            }
            ListCmd::DelayTime { units } => self.log.time_us += units as f64 * 10.0,
            ListCmd::JumpSpeed(v) => self.jump_speed = v.max(1),
            ListCmd::MarkSpeed(v) => self.mark_speed = v.max(1),
            ListCmd::MarkCurrent(v) => self.current = v,
            ListCmd::QSwitchPeriod(v) => self.q_period = v,
            ListCmd::WritePort(v) => {
                self.ports_written_by_list = v;
                self.write_port(v);
            }
            ListCmd::JumpDelay(v) | ListCmd::LaserOnDelay(v) | ListCmd::LaserOffDelay(v) => self.log.time_us += v.max(0) as f64,
            _ => {}
        }
    }

    fn motion(&mut self, x: u16, y: u16, mark: bool) {
        let from = self.pos;
        let d = (x as f64 - from.0 as f64).hypot(y as f64 - from.1 as f64);
        let speed = if mark { self.mark_speed } else { self.jump_speed } as f64;
        self.log.time_us += d / speed * 1000.0;
        self.log.motions.push(Motion {
            from,
            to: (x, y),
            mark,
            emitting: mark && self.can_emit(),
            current: self.current,
            q_period: self.q_period,
            mark_speed: self.mark_speed,
        });
        self.pos = (x, y);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::packetize;

    fn send(e: &mut GalvoEmulator, c: SingleCmd) -> Reply {
        Reply::from_bytes(&e.write(&c.to_command().to_bytes()).unwrap()).unwrap()
    }

    #[test]
    fn singles_reply_with_status() {
        let mut e = GalvoEmulator::default();
        let r = send(&mut e, SingleCmd::GetVersion);
        assert_eq!(r.words[0], 0x0103);
        assert!(r.is_ready() && !r.is_busy());
        assert_eq!(send(&mut e, SingleCmd::GetSerialNo).words, GalvoEmulator::default().cfg.serial);
        assert!(e.write(&[0; 5]).is_none());
    }

    #[test]
    fn list_runs_over_polls_and_finishes() {
        let mut e = GalvoEmulator::new(EmulatorConfig { cmds_per_poll: 8, ..Default::default() });
        let cmds: Vec<ListCmd> = (0..40).map(|i| ListCmd::JumpTo { x: i * 10, y: 5, angle: 0, distance: 10 }).collect();
        for p in packetize(&cmds) {
            e.write(p.as_bytes());
        }
        send(&mut e, SingleCmd::ExecuteList);
        let mut polls = 0;
        while send(&mut e, SingleCmd::GetListStatus).is_busy() {
            polls += 1;
            assert!(polls < 200);
        }
        assert!(polls >= 3);
        assert_eq!(e.log().motions.len(), 40);
        assert_eq!(e.position(), (390, 5));
    }

    #[test]
    fn emission_needs_enable_mo_and_mark_bit() {
        let mark = ListCmd::MarkTo { x: 100, y: 100, angle: 0, distance: 1 };
        let run = |enable: bool, mo: bool, port: u16| {
            let mut e = GalvoEmulator::default();
            if enable {
                send(&mut e, SingleCmd::EnableLaser);
            }
            send(&mut e, SingleCmd::FiberSetMo(mo));
            send(&mut e, SingleCmd::WritePort(port));
            for p in packetize(&[mark]) {
                e.write(p.as_bytes());
            }
            send(&mut e, SingleCmd::ExecuteList);
            e.run_polls(4);
            e.emitting_marks()
        };
        assert_eq!(run(true, true, 1), 1);
        assert_eq!(run(false, true, 1), 0);
        assert_eq!(run(true, false, 1), 0);
        assert_eq!(run(true, true, 0), 0);
    }

    #[test]
    fn stop_and_pause() {
        let mut e = GalvoEmulator::new(EmulatorConfig { cmds_per_poll: 4, ..Default::default() });
        let cmds: Vec<ListCmd> = (0..100).map(|i| ListCmd::JumpTo { x: i, y: 0, angle: 0, distance: 1 }).collect();
        for p in packetize(&cmds) {
            e.write(p.as_bytes());
        }
        send(&mut e, SingleCmd::ExecuteList);
        e.run_polls(2);
        send(&mut e, SingleCmd::StopList);
        assert!(e.is_paused());
        let done = e.log().executed;
        e.run_polls(5);
        assert_eq!(e.log().executed, done);
        send(&mut e, SingleCmd::RestartList);
        e.run_polls(2);
        assert!(e.log().executed > done);
        send(&mut e, SingleCmd::StopExecute);
        assert!(!e.is_executing() && e.queued_commands() == 0);
    }

    #[test]
    fn lens_table_upload_is_captured() {
        let mut e = GalvoEmulator::default();
        let t = CorTable::from_fn(|r, c| (r as i32 - c as i32, 5));
        for c in t.upload_commands() {
            e.write(&c.to_command().to_bytes());
        }
        assert_eq!(e.lens_table().unwrap(), t);
        send(&mut e, SingleCmd::WriteCorTable(false));
        assert!(e.lens_table().is_none());
    }

    #[test]
    fn ports_axes_and_log() {
        let mut e = GalvoEmulator::default();
        e.set_input_bit(15, false);
        assert_eq!(send(&mut e, SingleCmd::ReadPort).words[1] & 0x8000, 0);
        send(&mut e, SingleCmd::WritePort(0x100));
        assert_eq!(e.port_out(), 0x100);
        assert_eq!(e.log().light_on_writes, 1);
        send(&mut e, SingleCmd::MoveAxisTo { position: 500, invert: 1 });
        assert!(send(&mut e, SingleCmd::GetListStatus).axis_moving());
        e.run_polls(3);
        assert!(!send(&mut e, SingleCmd::GetListStatus).axis_moving());
        assert_eq!(e.axis_position(0), -500);
        e.disconnect();
        assert!(e.is_disconnected());
    }
}
