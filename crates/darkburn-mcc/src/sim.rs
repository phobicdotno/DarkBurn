//! In-memory MCC100 card simulator (`CardSimulator`, MCC-05) and a loopback link around it.
//!
//! The simulator takes framed requests as byte vectors and returns framed replies. There are no
//! sockets and no clock: time is a virtual microsecond counter moved by [`CardSimulator::advance`],
//! so tests are deterministic. It is **not** a model of the real firmware. Only the wire format,
//! the function codes, the register addresses and the failure modes seen in the owner's logs are
//! grounded in evidence. Everything it *decides* (register contents, run-status values, FIFO
//! capacity, which exception a misuse returns, DO bit mapping, tick period) is a simulator choice
//! and listed in `docs/hardware-checks-mcc.md`.
//!
//! What it models:
//!
//! * READ of the polled blocks (any sub-range of a configured block, else exception 2).
//! * WRITE to 0x65 with the vocabulary the crate can build: stop, home, jog (relative and
//!   absolute), go-to, connect prologue, DO, DA zero, PWM off. Exception 3 when a motion command
//!   arrives while the card is busy.
//! * 0x66 FIFO frames (duplicate frame ids are acknowledged, not queued) and 0x67 clear / start / stop.
//!   Ticks are consumed at one per tick period while the program runs. Register 1016 reports bytes
//!   free (60000 when empty).
//! * The fast block at 60001 mirrors the axis block at 2000 and the first 14 words of every axis
//!   slot of block 50200. The 32 hardware-parameter blocks and block 1050 read as zeros.
//! * Faults injected by tests: dropped requests, dropped replies, delayed replies, stale replies,
//!   an arbitrary reply override, a reboot (silent, then not-ready), alarms and input changes.
//! * Refusals: a request for a forbidden register or vector is answered with exception 2 and
//!   recorded in [`CardSimulator::forbidden_attempts`]; laser-class data is refused and counted in
//!   [`CardSimulator::laser_refusals`]. A backend under test must keep both lists empty.

use crate::command::{
    ABSOLUTE_BIT, CMD_9999, CMD_GOTO, CMD_HOME, CMD_JOG, CMD_STOP, MISC_DA, MISC_DO, MISC_PROLOGUE, MISC_PWM, MISC_PWM_5V, Policy, SafetyConfig,
};
use crate::fifo::{FifoItem, OP_MISC, parse_fifo_words};
use crate::frame::{DecodeError, Reply, Request, RequestError, decode_request};
use crate::gate::{GateAction, GateRefusal, SafetyGate};
use crate::registers::{
    AXIS_RO_WORDS, AXIS_RW_WORDS_KEPT, AXIS_RW_WORDS_ON_WIRE, AXIS_SLOTS, AxisRoBlock, AxisRwBlock, FIFO_MARGIN_EMPTY, FastBlock, Forbidden,
    HARD_PARAM_BASE, HARD_PARAM_BLOCK_WORDS, HARD_PARAM_STRIDE, MIN_PROGRAM_VERSION, REG_COMMAND, REG_FIFO_CONTROL, REG_FIFO_DATA, Status,
    StatusBlock, SystemBlock, VersionInfo, ZfStatus, alarm2, hard_param_block_addr,
};
use std::collections::{BTreeMap, VecDeque};
use std::time::Duration;

/// Exception returned right after a reboot and for unmapped reads (simulator choice, 08 section 3.3).
pub const EXC_NOT_READY: u8 = 2;
/// Exception returned for motion commands while busy (inferred from the owner's logs, 08 section 4.5).
pub const EXC_BUSY: u8 = 3;

/// Simulator settings. Defaults are simulator choices unless a doc section is cited.
#[derive(Clone, Debug, PartialEq)]
pub struct SimConfig {
    /// Tick period in microseconds (register 50005 default 250; unconfirmed).
    pub tick_us: u64,
    /// Register 1016 with an empty FIFO; also the capacity in bytes.
    pub fifo_margin_empty: u32,
    /// Raise alarm 2 bit 5 when the running FIFO drains. The end of a clean job looks the same, and
    /// whether the real card alarms there is unconfirmed, so the conservative default is on.
    pub fifo_starvation_alarm: bool,
    /// Program id (placeholder).
    pub program_id: u32,
    /// Program version; default is the owner's `MinHardwareVer` (20152).
    pub program_version: u32,
    /// How long a home / jog / go-to keeps the card busy.
    pub motion_time_ms: u64,
    /// Readable / writable blocks `(start, count)`.
    pub blocks: Vec<(u32, u32)>,
    /// Card units per mm (register 50017).
    pub k: u32,
    /// Follower type (register 50006).
    pub zf_type: u32,
    /// Answer a stop while idle with exception 3 (inferred "already stopped").
    pub stop_while_idle_exception: bool,
    /// Refuse motion commands with exception 3 while the FIFO runs.
    pub busy_while_streaming: bool,
    /// Classifier limits used when decoding requests.
    pub safety: SafetyConfig,
    /// Keep this many consumed items for stream diffs (0 = none).
    pub consumed_log_limit: usize,
    /// Keep this many request records (a ring).
    pub request_log_limit: usize,
}

impl Default for SimConfig {
    fn default() -> SimConfig {
        SimConfig {
            tick_us: 250,
            fifo_margin_empty: FIFO_MARGIN_EMPTY,
            fifo_starvation_alarm: true,
            program_id: 100,
            program_version: 20152,
            motion_time_ms: 300,
            blocks: {
                let mut b = vec![(1000, 36), (1050, 3), (2000, 50), (5000, 9), (10000, 18), (11000, 39), (50000, 26), (50200, 100), (60001, 120)];
                b.extend((0..32).map(|i| (HARD_PARAM_BASE + HARD_PARAM_STRIDE * i, HARD_PARAM_BLOCK_WORDS)));
                b
            },
            k: 1000,
            zf_type: 1,
            stop_while_idle_exception: false,
            busy_while_streaming: true,
            safety: SafetyConfig::default(),
            consumed_log_limit: 0,
            request_log_limit: 2000,
        }
    }
}

/// What the simulator did with a datagram.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Answered with a success reply.
    Served,
    /// Answered with this exception code.
    Exception(u8),
    /// Ignored (damaged frame, silent window, injected drop).
    Dropped(String),
}

/// One logged datagram.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestRecord {
    /// Virtual arrival time in microseconds.
    pub at_us: u64,
    /// Sequence number, if the frame had one.
    pub seq: Option<u16>,
    /// Vendor-log vector of the request, if it decoded.
    pub vector: Option<Vec<u32>>,
    /// Result.
    pub outcome: Outcome,
}

/// No reply is due: the request or reply was dropped, delayed or the card is silent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timeout;

impl std::fmt::Display for Timeout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "no reply (timeout)")
    }
}

impl std::error::Error for Timeout {}

fn s32(w: u32) -> i64 {
    w as i32 as i64
}

/// The simulated card.
#[derive(Debug)]
pub struct CardSimulator {
    cfg: SimConfig,
    now_us: u64,
    last_advance_us: u64,
    regs: BTreeMap<u32, u32>,
    axis_pos: [i64; AXIS_SLOTS],
    homed: [bool; AXIS_SLOTS],
    axis_faults: [u32; AXIS_SLOTS],
    axis_motion: BTreeMap<usize, (u64, u32, i64)>,
    moving_until_us: u64,
    fifo: VecDeque<FifoItem>,
    fifo_running: bool,
    last_frame_id: Option<u32>,
    commands: Vec<Vec<u32>>,
    forbidden: Vec<Forbidden>,
    laser_refusals: u64,
    frames_accepted: u64,
    ticks_consumed: u64,
    items_consumed: u64,
    underruns: u64,
    queue_low_water: Option<usize>,
    position: [i64; 2],
    consumed: Vec<FifoItem>,
    requests: VecDeque<RequestRecord>,
    // faults
    drop_requests: u32,
    drop_replies: u32,
    delay_replies_us: u64,
    delay_count: Option<u32>,
    override_next: Option<Vec<u8>>,
    silent_until_us: u64,
    not_ready_until_us: u64,
    outbox: Vec<(u64, u64, Vec<u8>)>,
    outbox_counter: u64,
    last_reply: Option<Vec<u8>>,
}

impl CardSimulator {
    /// A powered-on card.
    pub fn new(cfg: SimConfig) -> CardSimulator {
        let mut s = CardSimulator {
            cfg,
            now_us: 0,
            last_advance_us: 0,
            regs: BTreeMap::new(),
            axis_pos: [0; AXIS_SLOTS],
            homed: [false; AXIS_SLOTS],
            axis_faults: [0; AXIS_SLOTS],
            axis_motion: BTreeMap::new(),
            moving_until_us: 0,
            fifo: VecDeque::new(),
            fifo_running: false,
            last_frame_id: None,
            commands: Vec::new(),
            forbidden: Vec::new(),
            laser_refusals: 0,
            frames_accepted: 0,
            ticks_consumed: 0,
            items_consumed: 0,
            underruns: 0,
            queue_low_water: None,
            position: [0; 2],
            consumed: Vec::new(),
            requests: VecDeque::new(),
            drop_requests: 0,
            drop_replies: 0,
            delay_replies_us: 0,
            delay_count: None,
            override_next: None,
            silent_until_us: 0,
            not_ready_until_us: 0,
            outbox: Vec::new(),
            outbox_counter: 0,
            last_reply: None,
        };
        s.power_on();
        s
    }

    fn power_on(&mut self) {
        self.regs.clear();
        for &(start, count) in &self.cfg.blocks {
            for a in start..start + count {
                self.regs.insert(a, 0);
            }
        }
        self.regs.insert(Status::ProgramId.addr(), self.cfg.program_id);
        self.regs.insert(Status::ProgramVersion.addr(), self.cfg.program_version);
        self.regs.insert(50005, 250);
        self.regs.insert(50006, self.cfg.zf_type);
        self.regs.insert(50017, self.cfg.k);
        // follower status: not in e-stop (high bit set), run command "ready"
        self.regs.insert(10002, 0x8000_0000);
        self.regs.insert(10003, 105);
        self.axis_pos = [0; AXIS_SLOTS];
        self.homed = [false; AXIS_SLOTS];
        self.axis_faults = [0; AXIS_SLOTS];
        self.axis_motion.clear();
        self.moving_until_us = 0;
        self.fifo.clear();
        self.fifo_running = false;
        self.last_frame_id = None;
        self.last_advance_us = self.now_us;
        self.update_status();
    }

    // ---- time -----------------------------------------------------------------------------

    /// Virtual time in microseconds.
    pub fn now_us(&self) -> u64 {
        self.now_us
    }

    /// Virtual time in milliseconds.
    pub fn now_ms(&self) -> u64 {
        self.now_us / 1000
    }

    /// Move the clock forward: the FIFO consumes ticks, motions finish, status words update.
    pub fn advance(&mut self, dt: Duration) {
        let target = self.now_us + dt.as_micros() as u64;
        if self.fifo_running {
            let tick = self.cfg.tick_us.max(1);
            let mut budget = (target - self.last_advance_us) / tick;
            if budget > 0 {
                self.last_advance_us += budget * tick;
                while !self.fifo.is_empty() && (budget > 0 || self.fifo[0].opcode() != crate::fifo::OP_TICK) {
                    let item = self.fifo.pop_front().expect("non-empty");
                    self.items_consumed += 1;
                    let is_tick = item.opcode() == crate::fifo::OP_TICK;
                    self.execute(&item);
                    if is_tick {
                        budget -= 1;
                    }
                }
                let depth = self.fifo.len();
                self.queue_low_water = Some(self.queue_low_water.map_or(depth, |q| q.min(depth)));
                if self.fifo.is_empty() {
                    // The queue ran dry while running.
                    self.underruns += 1;
                    self.fifo_running = false;
                    if self.cfg.fifo_starvation_alarm {
                        *self.regs.entry(Status::Alarm2.addr()).or_insert(0) |= alarm2::FIFO_STARVATION;
                    }
                }
            }
        } else {
            self.last_advance_us = target;
        }
        self.now_us = target;
        self.update_status();
    }

    fn execute(&mut self, item: &FifoItem) {
        if self.cfg.consumed_log_limit > 0 && self.consumed.len() < self.cfg.consumed_log_limit {
            self.consumed.push(item.clone());
        }
        if let Some(t) = item.as_tick() {
            self.position[0] += t.dx as i64;
            self.position[1] += t.dy as i64;
            self.ticks_consumed += 1;
        } else if item.opcode() == OP_MISC && item.payload().len() >= 3 && item.payload()[0] == MISC_DO {
            self.set_outputs(item.payload()[1], item.payload()[2]);
        }
    }

    fn set_outputs(&mut self, mask: u32, value: u32) {
        let reg = self.regs.entry(Status::Do.addr()).or_insert(0);
        *reg = (*reg & !mask) | (value & mask);
    }

    // ---- fault injection and stimuli -------------------------------------------------------

    /// Ignore the next `n` requests entirely.
    pub fn drop_requests(&mut self, n: u32) {
        self.drop_requests = n;
    }

    /// Process but do not answer the next `n` requests.
    pub fn drop_replies(&mut self, n: u32) {
        self.drop_replies = n;
    }

    /// Delay replies by `d` (the next `count` replies, or all if `None`).
    pub fn delay_replies(&mut self, d: Duration, count: Option<u32>) {
        self.delay_replies_us = d.as_micros() as u64;
        self.delay_count = count;
    }

    /// Queue `n` copies of the previous reply (wrong sequence number for the next exchange).
    pub fn send_stale(&mut self, n: usize) {
        if let Some(r) = self.last_reply.clone() {
            for _ in 0..n {
                self.push_out(self.now_us, r.clone());
            }
        }
    }

    /// Replace the next reply with these bytes.
    pub fn override_next_reply(&mut self, bytes: Vec<u8>) {
        self.override_next = Some(bytes);
    }

    /// Reboot: all state is reset, nothing is answered for `silent`, then every request gets
    /// exception 2 for `not_ready`.
    pub fn reboot(&mut self, silent: Duration, not_ready: Duration) {
        self.outbox.clear();
        self.drop_requests = 0;
        self.drop_replies = 0;
        self.delay_replies_us = 0;
        self.delay_count = None;
        self.override_next = None;
        self.power_on();
        self.silent_until_us = self.now_us + silent.as_micros() as u64;
        self.not_ready_until_us = self.silent_until_us + not_ready.as_micros() as u64;
    }

    /// Set the digital input word (register 1004).
    pub fn set_inputs(&mut self, word: u32) {
        self.regs.insert(Status::Di.addr(), word);
    }

    /// Set alarm words (register 1006 / 1007). `None` leaves a word unchanged.
    pub fn inject_alarm(&mut self, alarm1: Option<u32>, alarm2: Option<u32>) {
        if let Some(a) = alarm1 {
            self.regs.insert(Status::Alarm1.addr(), a);
        }
        if let Some(a) = alarm2 {
            self.regs.insert(Status::Alarm2.addr(), a);
        }
    }

    /// Set the parameter-status word (register 1031).
    pub fn set_param_status(&mut self, v: u32) {
        self.regs.insert(Status::ParamStatus.addr(), v);
    }

    /// Set the fault bits (limits, servo, dual drive: the low six bits) in the status word of an
    /// axis slot. They stay until set again.
    pub fn inject_axis_fault(&mut self, slot: usize, bits: u32) {
        if let Some(f) = self.axis_faults.get_mut(slot) {
            *f = bits & crate::registers::axis::FAULT_MASK;
        }
        self.update_status();
    }

    /// Set a word the card itself would report (a configuration or follower word of a mapped
    /// block), as a stimulus for a test. Returns false, and changes nothing, for an address
    /// outside the mapped blocks and for the forbidden registers.
    pub fn set_card_word(&mut self, addr: u32, value: u32) -> bool {
        if !self.regs.contains_key(&addr) || crate::registers::check_read(addr, 1).is_err() {
            return false;
        }
        self.regs.insert(addr, value);
        true
    }

    // ---- observation ----------------------------------------------------------------------

    /// A register value (0 if unmapped).
    pub fn reg(&self, addr: u32) -> u32 {
        self.regs.get(&addr).copied().unwrap_or(0)
    }

    /// The status block as the gate sees it.
    pub fn status_block(&self) -> StatusBlock {
        StatusBlock { words: (0..36).map(|i| self.reg(1000 + i)).collect() }
    }

    /// Digital outputs (register 1005).
    pub fn outputs(&self) -> u32 {
        self.reg(Status::Do.addr())
    }

    /// Accepted 0x65 vectors, oldest first.
    pub fn commands(&self) -> &[Vec<u32>] {
        &self.commands
    }

    /// Requests that named a forbidden register or vector. Must stay empty for a safe backend.
    pub fn forbidden_attempts(&self) -> &[Forbidden] {
        &self.forbidden
    }

    /// Laser-class writes refused. Must stay zero for a backend that is not armed for the laser.
    pub fn laser_refusals(&self) -> u64 {
        self.laser_refusals
    }

    /// Frames queued so far.
    pub fn frames_accepted(&self) -> u64 {
        self.frames_accepted
    }

    /// Ticks the FIFO executed.
    pub fn ticks_consumed(&self) -> u64 {
        self.ticks_consumed
    }

    /// Items (of any kind) the FIFO executed.
    pub fn items_consumed(&self) -> u64 {
        self.items_consumed
    }

    /// Times the running FIFO drained.
    pub fn underruns(&self) -> u64 {
        self.underruns
    }

    /// Lowest queue depth (items) seen while the program ran.
    pub fn queue_low_water(&self) -> Option<usize> {
        self.queue_low_water
    }

    /// Accumulated `(dx, dy)` pulses of consumed ticks.
    pub fn pulse_position(&self) -> (i64, i64) {
        (self.position[0], self.position[1])
    }

    /// Axis position in card units (micrometres at K = 1000).
    pub fn axis_position(&self, slot: usize) -> i64 {
        self.axis_pos.get(slot).copied().unwrap_or(0)
    }

    /// Homed flag of a slot.
    pub fn is_homed(&self, slot: usize) -> bool {
        self.homed.get(slot).copied().unwrap_or(false)
    }

    /// Items waiting in the FIFO.
    pub fn fifo_depth(&self) -> usize {
        self.fifo.len()
    }

    /// True while the FIFO program runs.
    pub fn fifo_running(&self) -> bool {
        self.fifo_running
    }

    /// Items the FIFO executed (if [`SimConfig::consumed_log_limit`] is set).
    pub fn consumed_items(&self) -> &[FifoItem] {
        &self.consumed
    }

    /// The request log (a ring).
    pub fn requests(&self) -> impl Iterator<Item = &RequestRecord> {
        self.requests.iter()
    }

    // ---- wire -----------------------------------------------------------------------------

    fn log(&mut self, seq: Option<u16>, vector: Option<Vec<u32>>, outcome: Outcome) {
        if self.cfg.request_log_limit == 0 {
            return;
        }
        if self.requests.len() >= self.cfg.request_log_limit {
            self.requests.pop_front();
        }
        self.requests.push_back(RequestRecord { at_us: self.now_us, seq, vector, outcome });
    }

    fn push_out(&mut self, due: u64, bytes: Vec<u8>) {
        self.outbox_counter += 1;
        self.outbox.push((due, self.outbox_counter, bytes));
    }

    /// Hand one request datagram to the card. The reply, if any, is collected with [`recv`](Self::recv).
    pub fn send(&mut self, datagram: &[u8]) {
        self.update_status();
        let seq = if datagram.len() >= 2 { Some(u16::from_be_bytes([datagram[0], datagram[1]])) } else { None };
        if self.now_us < self.silent_until_us {
            self.log(seq, None, Outcome::Dropped("card silent after reboot".into()));
            return;
        }
        if self.drop_requests > 0 {
            self.drop_requests -= 1;
            self.log(seq, None, Outcome::Dropped("injected request drop".into()));
            return;
        }
        let do_word = Some(self.reg(Status::Do.addr()));
        let (seq, req) = match decode_request(datagram, &self.cfg.safety.clone(), do_word, true) {
            Ok(v) => v,
            Err(RequestError::Forbidden(f)) => {
                let Some(seq) = seq else { return };
                self.log(Some(seq), None, Outcome::Exception(EXC_NOT_READY));
                self.forbidden.push(f);
                self.reply(seq, Reply::Exception { func: datagram.get(7).copied().unwrap_or(0), code: EXC_NOT_READY });
                return;
            }
            Err(e) => {
                self.log(seq, None, Outcome::Dropped(e.to_string()));
                return;
            }
        };
        let vector = req.to_vector();
        let func = req.func();
        let (reply, outcome) = if self.now_us < self.not_ready_until_us {
            (Reply::Exception { func, code: EXC_NOT_READY }, Outcome::Exception(EXC_NOT_READY))
        } else {
            match self.handle(req) {
                Ok(r) => (r, Outcome::Served),
                Err(code) => (Reply::Exception { func, code }, Outcome::Exception(code)),
            }
        };
        self.log(Some(seq), Some(vector), outcome);
        self.reply(seq, reply);
    }

    fn reply(&mut self, seq: u16, reply: Reply) {
        let Ok(bytes) = reply.encode(seq) else { return };
        self.last_reply = Some(bytes.clone());
        if self.drop_replies > 0 {
            self.drop_replies -= 1;
            return;
        }
        let bytes = self.override_next.take().unwrap_or(bytes);
        let mut due = self.now_us;
        if self.delay_replies_us > 0 {
            due += self.delay_replies_us;
            if let Some(c) = &mut self.delay_count {
                *c -= 1;
                if *c == 0 {
                    self.delay_replies_us = 0;
                    self.delay_count = None;
                }
            }
        }
        self.push_out(due, bytes);
    }

    /// Next reply that is due at the current virtual time.
    pub fn recv(&mut self) -> Option<Vec<u8>> {
        let now = self.now_us;
        let idx = self.outbox.iter().enumerate().filter(|(_, (due, _, _))| *due <= now).min_by_key(|(_, (due, n, _))| (*due, *n)).map(|(i, _)| i)?;
        Some(self.outbox.remove(idx).2)
    }

    /// Send a request and take the reply that is due now. `Err(Timeout)` if there is none.
    pub fn exchange(&mut self, datagram: &[u8]) -> Result<Vec<u8>, Timeout> {
        self.send(datagram);
        self.recv().ok_or(Timeout)
    }

    // ---- request handling -----------------------------------------------------------------

    fn readable(&self, addr: u32, count: u32) -> bool {
        self.cfg.blocks.iter().any(|&(s, n)| s <= addr && addr + count <= s + n)
    }

    fn handle(&mut self, req: Request) -> Result<Reply, u8> {
        match req {
            Request::Read(r) => {
                if !self.readable(r.addr(), r.count() as u32) {
                    return Err(EXC_NOT_READY);
                }
                let words = (r.addr()..r.addr() + r.count() as u32).map(|a| self.reg_as_read(a)).collect();
                Ok(Reply::Read { addr: r.addr(), words })
            }
            Request::Write(w) => {
                if w.policy() == Policy::Laser {
                    self.laser_refusals += 1;
                    return Err(EXC_NOT_READY);
                }
                let (reg, words) = (w.register(), w.words().to_vec());
                let code = match reg {
                    REG_COMMAND => self.command(&words),
                    REG_FIFO_DATA => self.fifo_frame(&words),
                    REG_FIFO_CONTROL => self.fifo_control(words[0]),
                    _ => EXC_NOT_READY,
                };
                self.update_status();
                if code == 0 { Ok(Reply::Write { func: crate::frame::FUNC_WRITE, addr: reg, count: words.len() as u16 }) } else { Err(code) }
            }
        }
    }

    /// A register as a read sees it: the fast block is a view of the two axis blocks.
    fn reg_as_read(&self, addr: u32) -> u32 {
        const FAST: u32 = 60001;
        let ro_words = (AXIS_SLOTS * AXIS_RO_WORDS) as u32;
        let rw_words = (AXIS_SLOTS * AXIS_RW_WORDS_KEPT) as u32;
        match addr.checked_sub(FAST) {
            Some(off) if off < ro_words => self.reg(2000 + off),
            Some(off) if off < ro_words + rw_words => {
                let (slot, word) = ((off - ro_words) / AXIS_RW_WORDS_KEPT as u32, (off - ro_words) % AXIS_RW_WORDS_KEPT as u32);
                self.reg(50200 + slot * AXIS_RW_WORDS_ON_WIRE as u32 + word)
            }
            _ => self.reg(addr),
        }
    }

    fn fifo_control(&mut self, v: u32) -> u8 {
        match v {
            1 => {
                self.fifo.clear();
                self.fifo_running = false;
                self.last_frame_id = None;
                *self.regs.entry(Status::Alarm2.addr()).or_insert(0) &= !alarm2::FIFO_STARVATION;
            }
            2 => {
                self.fifo_running = true;
                self.last_advance_us = self.now_us;
            }
            3 => self.fifo_running = false,
            _ => return EXC_BUSY,
        }
        0
    }

    fn queued_bytes(&self) -> u32 {
        self.fifo.iter().map(|i| 4 * i.len_words() as u32).sum()
    }

    fn fifo_frame(&mut self, words: &[u32]) -> u8 {
        let Ok(frame) = parse_fifo_words(words) else { return EXC_BUSY };
        if self.last_frame_id == Some(frame.frame_id) {
            return 0; // a repeated frame id is acknowledged, not queued
        }
        let extra: u32 = frame.items.iter().map(|i| 4 * i.len_words() as u32).sum();
        if self.queued_bytes() + extra > self.cfg.fifo_margin_empty {
            return EXC_BUSY;
        }
        self.fifo.extend(frame.items);
        self.last_frame_id = Some(frame.frame_id);
        self.frames_accepted += 1;
        self.regs.insert(Status::FifoFrameId.addr(), frame.frame_id);
        0
    }

    fn start_motion(&mut self, slot: usize, cmd_type: u32, target: i64) {
        let end = self.now_us + self.cfg.motion_time_ms * 1000;
        self.axis_motion.insert(slot, (end, cmd_type, target));
        self.moving_until_us = self.moving_until_us.max(end);
    }

    fn busy(&self) -> bool {
        self.now_us < self.moving_until_us || (self.cfg.busy_while_streaming && self.fifo_running)
    }

    fn command(&mut self, d: &[u32]) -> u8 {
        let n = d.len();
        let code = match d[0] {
            CMD_STOP => {
                let mask = d[1];
                let moving: Vec<usize> = self.axis_motion.keys().copied().filter(|s| (mask >> s) & 1 == 1).collect();
                if moving.is_empty() && self.cfg.stop_while_idle_exception {
                    EXC_BUSY
                } else {
                    for s in moving {
                        self.axis_motion.remove(&s);
                    }
                    self.moving_until_us = self.axis_motion.values().map(|m| m.0).max().unwrap_or(self.now_us);
                    0
                }
            }
            CMD_HOME | CMD_JOG | CMD_GOTO => {
                if self.busy() {
                    EXC_BUSY
                } else if d[0] == CMD_HOME && n == 3 {
                    for s in 0..AXIS_SLOTS {
                        if (d[1] >> s) & 1 == 1 {
                            self.homed[s] = false;
                            self.start_motion(s, CMD_HOME, 0);
                        }
                    }
                    0
                } else if d[0] == CMD_JOG && n == 6 {
                    let slot = (d[1] & 0x7FFF_FFFF) as usize;
                    if slot >= AXIS_SLOTS {
                        EXC_NOT_READY
                    } else {
                        let dist = s32(d[5]);
                        let target = if d[1] & ABSOLUTE_BIT != 0 { dist } else { self.axis_pos[slot] + dist };
                        self.start_motion(slot, CMD_JOG, target);
                        0
                    }
                } else if d[0] == CMD_GOTO && n == 9 {
                    let vals: Vec<i64> = d[5..9].iter().map(|&w| s32(w)).collect();
                    let axes: Vec<usize> = (0..4).filter(|b| (d[1] >> b) & 1 == 1).collect();
                    for (i, s) in axes.into_iter().enumerate() {
                        let target = if d[1] & ABSOLUTE_BIT != 0 { vals[i] } else { self.axis_pos[s] + vals[i] };
                        self.start_motion(s, CMD_GOTO, target);
                    }
                    0
                } else {
                    EXC_NOT_READY
                }
            }
            CMD_9999 if n >= 2 => match d[1] {
                MISC_PROLOGUE => 0,
                MISC_DO if n == 4 => {
                    self.set_outputs(d[2] & 0xFFFF, d[3]);
                    0
                }
                MISC_DA if n == 4 && d[2] <= 1 => {
                    self.regs.insert(Status::Da1.addr() + d[2], d[3]);
                    0
                }
                MISC_PWM | MISC_PWM_5V if n == 4 || n == 5 => {
                    self.regs.insert(Status::PwmFreq.addr(), d[2]);
                    self.regs.insert(Status::PwmDuty.addr(), d[3]);
                    0
                }
                _ => EXC_NOT_READY,
            },
            _ => EXC_NOT_READY,
        };
        if code == 0 {
            self.commands.push(d.to_vec());
        }
        code
    }

    fn update_status(&mut self) {
        let now = self.now_us;
        let done: Vec<usize> = self.axis_motion.iter().filter(|(_, m)| now >= m.0).map(|(&s, _)| s).collect();
        for s in done {
            let (_, ty, target) = self.axis_motion.remove(&s).expect("present");
            self.axis_pos[s] = target;
            if ty == CMD_HOME {
                self.homed[s] = true;
            }
        }
        let space = self.cfg.fifo_margin_empty.saturating_sub(self.queued_bytes());
        self.regs.insert(Status::FifoSpaceMargin.addr(), space);
        let run = if self.fifo_running {
            2
        } else if now < self.moving_until_us {
            1
        } else {
            0
        };
        self.regs.insert(Status::RunStatus.addr(), run);
        self.regs.insert(Status::ProcessingStatus.addr(), u32::from(self.fifo_running));
        if self.readable(2000, 50) {
            for slot in 0..AXIS_SLOTS {
                let mut word = self.axis_faults[slot] | if self.homed[slot] { crate::registers::axis::HOMED } else { 0 };
                if let Some(&(_, ty, _)) = self.axis_motion.get(&slot) {
                    word |= (1 << 16) | (ty << 24);
                }
                let base = 2000 + (AXIS_RO_WORDS * slot) as u32;
                self.regs.insert(base, word);
                self.regs.insert(base + 2, self.axis_pos[slot] as i32 as u32);
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Loopback link: gate + codec + simulator, the shape a future backend will have.
// ---------------------------------------------------------------------------------------------

/// Why a [`SimLink`] call failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkError {
    /// The safety gate refused the access.
    Refused(GateRefusal),
    /// The request cannot be represented.
    Forbidden(Forbidden),
    /// No reply after all attempts.
    Timeout,
    /// The reply did not decode (bad sequence number, wrong length).
    Decode(DecodeError),
    /// The card answered with an exception.
    Card {
        /// Request function.
        func: u8,
        /// Exception byte.
        code: u8,
    },
    /// The reply had the wrong shape for the request.
    Unexpected,
    /// The card's program is older than the connect asked for.
    Version {
        /// Register 1001.
        found: u32,
        /// The minimum asked for.
        needed: u32,
    },
}

impl std::fmt::Display for LinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LinkError::Refused(r) => write!(f, "gate refused: {r}"),
            LinkError::Forbidden(r) => write!(f, "{r}"),
            LinkError::Timeout => write!(f, "timeout"),
            LinkError::Decode(e) => write!(f, "{e}"),
            LinkError::Card { func, code } => {
                write!(f, "card exception {code} for func {func:#04x} (ErrCode {})", crate::frame::exception_errcode(*code))
            }
            LinkError::Unexpected => write!(f, "unexpected reply"),
            LinkError::Version { found, needed } => write!(f, "card program version {found} is older than {needed}"),
        }
    }
}

impl std::error::Error for LinkError {}

/// Attempts every step of a stop sequence gets at least, whatever [`SimLink::attempts`] says.
pub const STOP_ATTEMPTS: u32 = 3;

/// How [`SimLink::connect`] behaves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConnectOptions {
    /// Lowest card program version to accept.
    pub min_version: u32,
    /// Also send the connect prologue `0x65 <- [9999, 5, 0, 0]`. The vendor always sends it and
    /// nobody knows what it does, so the default is a connect that only reads.
    pub send_prologue: bool,
}

impl Default for ConnectOptions {
    fn default() -> ConnectOptions {
        ConnectOptions { min_version: MIN_PROGRAM_VERSION, send_prologue: false }
    }
}

/// What a connect read from the card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectReport {
    /// Program id and version.
    pub version: VersionInfo,
    /// Status block.
    pub status: StatusBlock,
    /// System block: bus cycle, follower type, K.
    pub system: SystemBlock,
    /// Axis read-only block.
    pub axis_ro: AxisRoBlock,
    /// Axis read/write block.
    pub axis_rw: AxisRwBlock,
    /// Follower status, read only when the card reports a follower.
    pub follower: Option<ZfStatus>,
    /// True when the prologue was sent.
    pub prologue_sent: bool,
}

/// A loopback link: every call goes through the gate, the frame codec and the simulator.
#[derive(Debug)]
pub struct SimLink {
    /// The card.
    pub sim: CardSimulator,
    /// The PC-side gate.
    pub gate: SafetyGate,
    /// Attempts per request; a retry re-sends the same sequence number, like the vendor driver.
    pub attempts: u32,
    seq: u16,
    unanswered: bool,
}

impl SimLink {
    /// New link with a default gate and a single attempt per request.
    pub fn new(sim: CardSimulator) -> SimLink {
        SimLink { sim, gate: SafetyGate::default(), attempts: 1, seq: 0, unanswered: false }
    }

    fn exchange(&mut self, req: &Request) -> Result<Reply, LinkError> {
        let seq = self.seq;
        self.seq = self.seq.wrapping_add(1);
        let frame = req.encode(seq).map_err(LinkError::Decode)?;
        self.unanswered = false;
        for _ in 0..self.attempts.max(1) {
            self.sim.send(&frame);
            // An answer to an earlier request that arrives late carries that request's sequence
            // number. It is dropped here; taking it would put every later request one answer
            // out of step.
            let mine = std::iter::from_fn(|| self.sim.recv()).find(|bytes| crate::frame::RawFrame::parse(bytes).seq == Some(seq));
            if let Some(bytes) = mine {
                let rep = match Reply::decode(&bytes, Some(seq)) {
                    Ok(rep) => rep,
                    Err(e) => {
                        // An answer that cannot be read says nothing about what the card did.
                        self.unanswered = true;
                        return Err(LinkError::Decode(e));
                    }
                };
                return match rep {
                    Reply::Exception { func, code } => Err(LinkError::Card { func, code }),
                    other => Ok(other),
                };
            }
            // No answer to this attempt: the card may still have acted on it.
            self.unanswered = true;
        }
        Err(LinkError::Timeout)
    }

    /// Checked read.
    pub fn read(&mut self, addr: u32, count: u16) -> Result<Vec<u32>, LinkError> {
        self.gate.check_read(addr, count as u32).map_err(LinkError::Refused)?;
        let req = Request::read(addr, count).map_err(LinkError::Forbidden)?;
        // Like the vendor driver, an answer counts only when it names the request's function and
        // address; the block read must also be the length asked for.
        match self.exchange(&req)? {
            Reply::Read { addr: a, words } if a == addr && words.len() == count as usize => Ok(words),
            _ => Err(LinkError::Unexpected),
        }
    }

    /// Checked write.
    pub fn write(&mut self, w: &crate::command::SafeWrite) -> Result<(), LinkError> {
        let now = self.sim.now_ms();
        self.gate.check_write(w, now).map_err(LinkError::Refused)?;
        // An echo counts only when it is a write echo of this register (the vendor driver checks
        // function and address too): an answer to something else confirms nothing.
        match self.exchange(&Request::write(w.clone())) {
            Ok(Reply::Write { func: crate::frame::FUNC_WRITE, addr, .. }) if addr == w.register() => {
                self.gate.after_send(w, now);
                Ok(())
            }
            other => {
                // The card refused it, or did not answer every attempt. A write that may have
                // reached the card without an answer is tracked as if it had taken effect.
                if self.unanswered || other.is_ok() {
                    self.gate.after_unconfirmed(w, now);
                }
                other.and(Err(LinkError::Unexpected))
            }
        }
    }

    /// Connect the way the vendor software does, minus everything that is not understood: read
    /// the version and refuse an old card, optionally send the prologue, then read the status,
    /// system and axis blocks, and the follower status if the card reports a follower. Nothing
    /// else is written: not register 100, not the licence area, not the hardware parameters. The
    /// gate stays disarmed.
    pub fn connect(&mut self, options: ConnectOptions) -> Result<ConnectReport, LinkError> {
        let version = self.read_version()?;
        if !version.meets(options.min_version) {
            return Err(LinkError::Version { found: version.program_version, needed: options.min_version });
        }
        if options.send_prologue {
            self.write(&crate::command::connect_prologue().map_err(LinkError::Forbidden)?)?;
        }
        let status = self.poll_status()?;
        let system = self.read_system()?;
        let axis_ro = self.read_axis_ro()?;
        let axis_rw = self.read_axis_rw()?;
        let follower = if system.has_follower() { Some(self.read_follower()?) } else { None };
        Ok(ConnectReport { version, status, system, axis_ro, axis_rw, follower, prologue_sent: options.send_prologue })
    }

    /// Leave the card: send the stop sequence, disarm, and forget what was read. Returns the
    /// number of stop steps that reached the card. Nothing that needs a fresh status passes the
    /// gate afterwards until the next poll.
    pub fn disconnect(&mut self) -> usize {
        let steps = self.stop_sequence(false);
        self.gate.arming.disarm("disconnect");
        self.gate.forget_card_state();
        steps
    }

    /// `READ 1000/2`: program id and version.
    pub fn read_version(&mut self) -> Result<VersionInfo, LinkError> {
        VersionInfo::from_words(&self.read(1000, 2)?).ok_or(LinkError::Unexpected)
    }

    /// `READ 50000/26`: the system block.
    pub fn read_system(&mut self) -> Result<SystemBlock, LinkError> {
        SystemBlock::from_words(&self.read(50000, 26)?).ok_or(LinkError::Unexpected)
    }

    /// `READ 2000/50`: the axis read-only block.
    pub fn read_axis_ro(&mut self) -> Result<AxisRoBlock, LinkError> {
        AxisRoBlock::from_words(&self.read(2000, 50)?).ok_or(LinkError::Unexpected)
    }

    /// `READ 50200/100`: the axis read/write block.
    pub fn read_axis_rw(&mut self) -> Result<AxisRwBlock, LinkError> {
        AxisRwBlock::from_words(&self.read(50200, 100)?).ok_or(LinkError::Unexpected)
    }

    /// `READ 60001/120`: both axis blocks in one read.
    pub fn read_fast(&mut self) -> Result<FastBlock, LinkError> {
        FastBlock::from_words(&self.read(60001, 120)?).ok_or(LinkError::Unexpected)
    }

    /// `READ 10000/18`: the follower status block.
    pub fn read_follower(&mut self) -> Result<ZfStatus, LinkError> {
        ZfStatus::from_words(&self.read(10000, 18)?).ok_or(LinkError::Unexpected)
    }

    /// `READ 11000/39`: the follower properties, as raw words. They can be read and never written.
    pub fn read_follower_properties(&mut self) -> Result<Vec<u32>, LinkError> {
        self.read(11000, 39)
    }

    /// Read hardware-parameter block `index` (0..=31, 52 words), as raw words, to compare it with
    /// a copy on the PC. These blocks are never written.
    pub fn read_hard_params(&mut self, index: u32) -> Result<Vec<u32>, LinkError> {
        let addr = hard_param_block_addr(index).ok_or(LinkError::Forbidden(Forbidden::ReadNotAllowed(HARD_PARAM_BASE, 0)))?;
        self.read(addr, HARD_PARAM_BLOCK_WORDS as u16)
    }

    /// Operator emergency stop: latch the gate, then send the stop sequence with the laser-gate
    /// output (if the machine has one, else 0) switched off right after the motion stop. Returns
    /// the number of steps that reached the card.
    pub fn emergency_stop(&mut self, laser_gate_port: u32) -> usize {
        self.gate.arming.estop();
        self.gate.note_stop();
        let params = crate::command::MachineParams::default();
        let do_word = self.gate.last_do_word.unwrap_or(0);
        let laser_ports = self.gate.config.safety.laser_do_ports.clone();
        let seq = crate::command::estop_sequence(&params, self.gate.fifo_running, 0, do_word, &laser_ports, laser_gate_port);
        self.send_stop_steps(&seq)
    }

    /// Pause. It is the stop sequence: the vendor's pause and stop are the same, and how a paused
    /// job goes on is not known, so there is no resume and a start is refused until the queue is
    /// cleared.
    pub fn pause(&mut self) -> usize {
        self.stop_sequence(false)
    }

    /// Read block 1000/36, feed the gate, and send the stop sequence if the gate asks for it.
    ///
    /// A poll that fails runs the gate's comm-loss watchdog: once the last good poll of a running
    /// job is older than the timeout, the job is given up and the queue stopped (on every failed
    /// poll until the card acknowledges the stop).
    pub fn poll_status(&mut self) -> Result<StatusBlock, LinkError> {
        let words = match self.read(1000, 36) {
            Ok(words) => words,
            Err(e) => {
                if self.gate.watchdog(self.sim.now_ms())
                    && self.gate.fifo_running
                    && let Ok(w) = crate::command::fifo_stop()
                {
                    self.send_stop_steps(&[w]);
                }
                return Err(e);
            }
        };
        let status = StatusBlock::from_words(&words).ok_or(LinkError::Unexpected)?;
        let now = self.sim.now_ms();
        let actions = self.gate.observe_status(now, &status);
        if !actions.is_empty() {
            let estop = actions.contains(&GateAction::Estop);
            self.stop_sequence(estop);
        }
        Ok(status)
    }

    /// Stop sequence, every step attempted even if one fails. Returns the number of steps that
    /// reached the card.
    pub fn stop_sequence(&mut self, estop: bool) -> usize {
        if estop {
            self.gate.arming.estop();
        } else {
            self.gate.arming.on_stop();
        }
        self.gate.note_stop();
        let params = crate::command::MachineParams::default();
        let do_word = self.gate.last_do_word.unwrap_or(0);
        // The laser ports this gate is set up with; the stop adds the default ones to them.
        let laser_ports = self.gate.config.safety.laser_do_ports.clone();
        let seq = crate::command::stop_sequence(&params, self.gate.fifo_running, 0, do_word, &laser_ports);
        self.send_stop_steps(&seq)
    }

    /// Send the steps of a stop, each one tried at least [`STOP_ATTEMPTS`] times: a stop whose
    /// datagram got lost must not leave the machine running. Returns how many reached the card.
    fn send_stop_steps(&mut self, seq: &[crate::command::SafeWrite]) -> usize {
        let attempts = self.attempts;
        self.attempts = attempts.max(STOP_ATTEMPTS);
        let sent = seq.iter().filter(|w| self.write(w).is_ok()).count();
        self.attempts = attempts;
        sent
    }

    /// Move the virtual clock. A deadman release stop that does not reach the card is asked for
    /// again on the next call, until the card has answered it.
    pub fn advance(&mut self, dt: Duration) {
        self.sim.advance(dt);
        let now = self.sim.now_ms();
        for idx in self.gate.service(now) {
            if let Ok(w) = crate::command::jog_release_stop(&[idx], 200_000, &crate::command::MachineParams::default()) {
                let attempts = self.attempts;
                self.attempts = attempts.max(STOP_ATTEMPTS);
                let result = self.write(&w);
                self.attempts = attempts;
                if matches!(result, Err(LinkError::Timeout | LinkError::Decode(_) | LinkError::Unexpected)) {
                    self.gate.release_stop_not_confirmed(idx, now);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{self, MachineParams};
    use crate::fifo::{FifoFeeder, FramePacker};
    use crate::gate::ArmState;

    fn p() -> MachineParams {
        MachineParams::default()
    }

    fn link() -> SimLink {
        SimLink::new(CardSimulator::new(SimConfig::default()))
    }

    fn armed() -> SimLink {
        let mut l = link();
        l.gate.arming.arm_motion().unwrap();
        l.poll_status().unwrap();
        l
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn logged_read_and_exception_frames() {
        let mut sim = CardSimulator::new(SimConfig::default());
        // the logged sendto: READ 1000 x 2
        let sendto = [0x00, 0x00, 0x8c, 0x75, 0x00, 0x08, 0x00, 0x30, 0xe8, 0x03, 0x00, 0x00, 0x02, 0x00];
        let rep = sim.exchange(&sendto).unwrap();
        match Reply::decode(&rep, Some(0)).unwrap() {
            Reply::Read { addr, words } => {
                assert_eq!(addr, 1000);
                assert_eq!(words, vec![100, 20152]);
            }
            other => panic!("{other:?}"),
        }
        // replies use the card's CRC order
        assert_eq!(crate::frame::RawFrame::parse(&rep).crc_order, Some(crate::crc::CrcOrder::LoFirst));
        // an unmapped read answers with exception 2
        let r = Request::read(11000, 39).unwrap();
        let mut sim2 = CardSimulator::new(SimConfig { blocks: vec![(1000, 36)], ..SimConfig::default() });
        let rep = sim2.exchange(&r.encode(7).unwrap()).unwrap();
        assert_eq!(Reply::decode(&rep, Some(7)).unwrap(), Reply::Exception { func: 0x30, code: 2 });
    }

    #[test]
    fn connect_handshake_and_status_poll() {
        let mut l = link();
        l.write(&command::connect_prologue().unwrap()).unwrap();
        assert_eq!(l.sim.commands(), &[vec![9999, 5, 0, 0]]);
        let s = l.poll_status().unwrap();
        assert_eq!(s.fifo_space_margin(), 60000);
        assert!(!s.fifo_running());
        let sys = l.read(50000, 26).unwrap();
        assert_eq!((sys[5], sys[6], sys[17]), (250, 1, 1000));
        let zf = l.read(10000, 18).unwrap();
        assert_eq!(crate::registers::ZfStatus::from_words(&zf).unwrap().state(), crate::registers::ZfState::Ready);
    }

    #[test]
    fn gate_blocks_motion_until_armed_and_homed() {
        let mut l = link();
        l.poll_status().unwrap();
        let jog = command::jog_step(0, 5.0, &p()).unwrap();
        assert!(matches!(l.write(&jog), Err(LinkError::Refused(GateRefusal::NeedsArming { .. }))));
        assert!(l.sim.commands().is_empty(), "nothing reached the card");
        l.gate.arming.arm_motion().unwrap();
        let goto = command::go_to(10.0, 10.0, &p()).unwrap();
        assert!(matches!(l.write(&goto), Err(LinkError::Refused(GateRefusal::NotHomed))));
        l.write(&jog).unwrap();
        assert_eq!(l.sim.commands().len(), 1);
    }

    #[test]
    fn homing_jog_and_busy_exception() {
        let mut l = armed();
        l.write(&command::home_axis(0).unwrap()).unwrap();
        // a second motion while busy is refused by the card with exception 3
        let jog = command::jog_step(1, 1.0, &p()).unwrap();
        assert_eq!(l.write(&jog), Err(LinkError::Card { func: 0x40, code: EXC_BUSY }));
        let s = l.poll_status().unwrap();
        assert_eq!(s.word(Status::RunStatus), 1);
        let ro = l.read(2000, 50).unwrap();
        assert!(crate::registers::axis::busy(ro[0]));
        assert_eq!(crate::registers::axis::command_type(ro[0]), 2);
        l.advance(ms(400));
        let ro = l.read(2000, 50).unwrap();
        assert!(!crate::registers::axis::busy(ro[0]));
        assert!(ro[0] & crate::registers::axis::HOMED != 0);
        assert!(l.sim.is_homed(0) && !l.sim.is_homed(1));
        l.write(&jog).unwrap();
        l.advance(ms(400));
        assert_eq!(l.sim.axis_position(1), 1000);
    }

    #[test]
    fn absolute_move_and_go_to_after_homing() {
        let mut l = armed();
        l.write(&command::home_axis(0).unwrap()).unwrap();
        l.advance(ms(400));
        l.write(&command::home_axis(1).unwrap()).unwrap();
        l.advance(ms(400));
        l.gate.homed = true;
        // the gate wants a status no older than a second before anything moves
        l.poll_status().unwrap();
        l.write(&command::go_to(430.178, 174.097, &p()).unwrap()).unwrap();
        l.advance(ms(400));
        assert_eq!((l.sim.axis_position(0), l.sim.axis_position(1)), (430_178, 174_097));
        l.poll_status().unwrap();
        l.write(&command::move_axis_absolute(0, 100.0, &p()).unwrap()).unwrap();
        l.advance(ms(400));
        assert_eq!(l.sim.axis_position(0), 100_000);
    }

    #[test]
    fn stop_cancels_a_motion() {
        let mut l = armed();
        let held = command::jog_continuous(0, true, &p(), None).unwrap();
        l.write(&held).unwrap();
        assert!(l.poll_status().unwrap().word(Status::RunStatus) == 1);
        l.write(&command::jog_release_stop(&[0], 200_000, &p()).unwrap()).unwrap();
        l.advance(ms(1));
        assert_eq!(l.poll_status().unwrap().word(Status::RunStatus), 0);
        assert_eq!(l.sim.axis_position(0), 0, "stopped dead at the start (simulator choice)");
        // with the idle-stop option the card reports "nothing to stop"
        let mut sim = CardSimulator::new(SimConfig { stop_while_idle_exception: true, ..SimConfig::default() });
        let f = Request::write(command::stop_all_no_decel().unwrap()).encode(1).unwrap();
        let rep = sim.exchange(&f).unwrap();
        assert_eq!(Reply::decode(&rep, Some(1)).unwrap(), Reply::Exception { func: 0x40, code: 3 });
    }

    #[test]
    fn deadman_releases_a_held_jog() {
        let mut l = armed();
        l.write(&command::jog_continuous(0, true, &p(), None).unwrap()).unwrap();
        l.advance(ms(100));
        assert!(l.gate.jog_keepalive(0, l.sim.now_ms()));
        assert!(!l.sim.commands().iter().any(|c| c[0] == 1));
        l.advance(ms(250)); // no more key events
        assert!(l.sim.commands().iter().any(|c| c[0] == 1 && c[1] == 1), "release stop for axis 0 was sent");
    }

    fn stream_plan(l: &mut SimLink, ticks: usize) -> FifoFeeder {
        let mut pk = FramePacker::new();
        for _ in 0..ticks {
            pk.add_record(&[FifoItem::tick(2, -1)]);
        }
        let mut f = FifoFeeder::new(pk.finish());
        f.push(vec![], true);
        let _ = l;
        f
    }

    fn pump(l: &mut SimLink, f: &mut FifoFeeder) {
        let s = l.poll_status().unwrap();
        let r = f.fill(s.fifo_frame_id(), s.fifo_space_margin(), 50);
        for (id, frame) in r.frames {
            l.write(&command::fifo_packed_frame(id, &frame).unwrap()).unwrap();
        }
    }

    #[test]
    fn fifo_streams_fill_and_drain() {
        let mut l = armed();
        l.write(&command::fifo_clear().unwrap()).unwrap();
        let mut feeder = stream_plan(&mut l, 250); // 3 frames: 99 + 99 + 52
        pump(&mut l, &mut feeder);
        assert_eq!(feeder.pending(), 0);
        assert_eq!(l.sim.frames_accepted(), 3);
        assert_eq!(l.sim.fifo_depth(), 250);
        let s = l.poll_status().unwrap();
        assert_eq!(s.fifo_frame_id(), 3);
        assert_eq!(s.fifo_space_margin(), 60000 - 250 * 12);
        assert_eq!(crate::fifo::queued_ticks(s.fifo_space_margin()), 250);
        l.write(&command::fifo_start().unwrap()).unwrap();
        assert!(l.poll_status().unwrap().fifo_running());
        l.advance(Duration::from_micros(250 * 100));
        assert_eq!(l.sim.ticks_consumed(), 100);
        assert_eq!(l.sim.pulse_position(), (200, -100));
        assert_eq!(l.sim.fifo_depth(), 150);
        // the gate keeps a jog away from a running stream
        assert_eq!(l.write(&command::jog_step(0, 1.0, &p()).unwrap()), Err(LinkError::Refused(GateRefusal::StreamRunning)));
        // and the card's own busy rule while streaming, with the gate looking away
        l.gate.fifo_running = false;
        assert_eq!(l.write(&command::jog_step(0, 1.0, &p()).unwrap()), Err(LinkError::Card { func: 0x40, code: EXC_BUSY }));
        l.gate.fifo_running = true;
        l.advance(Duration::from_micros(250 * 200));
        assert_eq!(l.sim.ticks_consumed(), 250);
        assert_eq!(l.sim.pulse_position(), (500, -250));
        assert!(!l.sim.fifo_running());
        assert_eq!(l.sim.underruns(), 1);
        // the drained program raises starvation (conservative default)
        let s = l.poll_status().unwrap();
        assert!(s.alarm2() & alarm2::FIFO_STARVATION != 0);
        assert_eq!(l.gate.arming.state(), ArmState::Disarmed, "the gate reacted to the alarm");
    }

    #[test]
    fn clean_drain_without_starvation_alarm() {
        let mut l = SimLink::new(CardSimulator::new(SimConfig { fifo_starvation_alarm: false, ..SimConfig::default() }));
        l.gate.arming.arm_motion().unwrap();
        l.poll_status().unwrap();
        let mut f = stream_plan(&mut l, 10);
        pump(&mut l, &mut f);
        l.write(&command::fifo_start().unwrap()).unwrap();
        l.advance(ms(10));
        assert_eq!(l.sim.ticks_consumed(), 10);
        assert_eq!(l.poll_status().unwrap().alarm2(), 0);
        assert_eq!(l.gate.arming.state(), ArmState::MotionArmed);
        assert!(f.job_finished(60000, 1) || !l.sim.fifo_running());
    }

    #[test]
    fn full_fifo_returns_exception_and_repeated_frame_ids_are_acknowledged() {
        let mut l = armed();
        let mut pk = FramePacker::new();
        for _ in 0..99 {
            pk.add_record(&[FifoItem::tick(1, 1)]);
        }
        let frame = pk.finish().remove(0);
        l.write(&command::fifo_packed_frame(1, &frame).unwrap()).unwrap();
        // same id again: acknowledged, not queued
        l.write(&command::fifo_packed_frame(1, &frame).unwrap()).unwrap();
        assert_eq!(l.sim.fifo_depth(), 99);
        assert_eq!(l.sim.frames_accepted(), 1);
        // fill to capacity: 60000 bytes / 1188 bytes per frame (the frame id is not stored)
        let mut id = 2;
        let err = loop {
            match l.write(&command::fifo_packed_frame(id, &frame).unwrap()) {
                Ok(()) => id += 1,
                Err(e) => break e,
            }
            assert!(id < 100, "FIFO never filled");
        };
        assert_eq!(err, LinkError::Card { func: 0x40, code: EXC_BUSY });
        assert_eq!(l.sim.fifo_depth(), 99 * 50);
        // clear empties it
        l.write(&command::fifo_clear().unwrap()).unwrap();
        assert_eq!(l.sim.fifo_depth(), 0);
        assert_eq!(l.poll_status().unwrap().fifo_space_margin(), 60000);
    }

    #[test]
    fn consumed_log_matches_the_planned_stream() {
        let cfg = SimConfig { consumed_log_limit: 1000, fifo_starvation_alarm: false, ..SimConfig::default() };
        let mut l = SimLink::new(CardSimulator::new(cfg));
        l.gate.arming.arm_motion().unwrap();
        l.poll_status().unwrap();
        let plan = crate::planner::dry_run(&[crate::planner::PathSegment::cut(5.0, 0.0, 50.0)], &crate::planner::PlanConfig::default()).unwrap();
        let mut f = plan.feeder();
        pump(&mut l, &mut f);
        assert_eq!(f.pending(), 0);
        l.write(&command::fifo_start().unwrap()).unwrap();
        l.advance(Duration::from_micros(250 * plan.tick_count));
        assert_eq!(l.sim.ticks_consumed(), plan.tick_count);
        assert_eq!(l.sim.pulse_position(), (plan.total_pulses_x, plan.total_pulses_y));
        assert!(l.sim.consumed_items().iter().all(|i| i.as_tick().is_some_and(|t| t.duty == 0)));
    }

    #[test]
    fn timeouts_drops_and_retries() {
        let mut l = armed();
        // an injected request drop: a plain timeout
        l.sim.drop_requests(1);
        assert_eq!(l.read(1000, 2), Err(LinkError::Timeout));
        assert_eq!(l.read(1000, 2).unwrap().len(), 2);
        // retry: dropped request, second attempt works
        l.attempts = 2;
        l.sim.drop_requests(1);
        assert_eq!(l.read(1000, 2).unwrap().len(), 2);
        // dropped reply of a jog: the retry hits a busy card (the jog already ran)
        l.sim.drop_replies(1);
        let jog = command::jog_step(0, 1.0, &p()).unwrap();
        assert_eq!(l.write(&jog), Err(LinkError::Card { func: 0x40, code: EXC_BUSY }));
        assert_eq!(l.sim.commands().len(), 1, "executed once");
        let log: Vec<_> = l.sim.requests().map(|r| r.outcome.clone()).collect();
        assert!(log.iter().any(|o| matches!(o, Outcome::Dropped(_))));
    }

    #[test]
    fn delayed_replies_arrive_late() {
        let mut sim = CardSimulator::new(SimConfig::default());
        sim.delay_replies(ms(5), Some(1));
        let req = Request::read(1000, 2).unwrap().encode(1).unwrap();
        assert_eq!(sim.exchange(&req), Err(Timeout));
        assert!(sim.recv().is_none());
        sim.advance(ms(4));
        assert!(sim.recv().is_none());
        sim.advance(ms(1));
        let rep = sim.recv().unwrap();
        assert!(matches!(Reply::decode(&rep, Some(1)).unwrap(), Reply::Read { .. }));
        // only one reply was delayed
        assert!(sim.exchange(&Request::read(1000, 2).unwrap().encode(2).unwrap()).is_ok());
    }

    #[test]
    fn stale_and_overridden_replies_are_visible_to_the_decoder() {
        let mut sim = CardSimulator::new(SimConfig::default());
        let first = Request::read(1000, 2).unwrap().encode(1).unwrap();
        sim.exchange(&first).unwrap();
        sim.send_stale(1);
        let second = Request::read(1000, 2).unwrap().encode(2).unwrap();
        sim.send(&second);
        let stale = sim.recv().unwrap();
        let e = Reply::decode(&stale, Some(2)).unwrap_err();
        assert_eq!(e.rc, 4);
        let good = sim.recv().unwrap();
        assert!(Reply::decode(&good, Some(2)).is_ok());
        // override with the logged exception 503 frame
        let exc: Vec<u8> = vec![0x1d, 0x3f, 0x34, 0x45, 0x00, 0x03, 0x00, 0xc0, 0x03];
        sim.override_next_reply(exc.clone());
        assert_eq!(sim.exchange(&first).unwrap(), exc);
    }

    #[test]
    fn reboot_goes_silent_then_not_ready() {
        let mut l = armed();
        l.sim.reboot(ms(1000), ms(1000));
        assert_eq!(l.read(1000, 2), Err(LinkError::Timeout));
        l.advance(ms(1000));
        assert_eq!(l.read(1000, 2), Err(LinkError::Card { func: 0x30, code: EXC_NOT_READY }));
        l.advance(ms(1000));
        assert_eq!(l.read(1000, 2).unwrap().len(), 2);
        assert!(!l.sim.is_homed(0), "state is reset");
    }

    #[test]
    fn alarms_and_the_estop_latch_through_the_gate() {
        let mut l = armed();
        l.write(&command::fifo_clear().unwrap()).unwrap();
        l.sim.inject_alarm(Some(crate::registers::alarm1::ESTOP), None);
        l.poll_status().unwrap();
        assert!(l.gate.arming.estop_latched());
        assert_eq!(l.gate.arming.state(), ArmState::Disarmed);
        assert!(matches!(l.write(&command::jog_step(0, 1.0, &p()).unwrap()), Err(LinkError::Refused(_))));
        // stop primitives still go through
        assert!(l.write(&command::stop_all_no_decel().unwrap()).is_ok());
        // a parameter-restart flag refuses motion after re-arming
        l.sim.inject_alarm(Some(0), Some(0));
        l.poll_status().unwrap();
        l.gate.arming.acknowledge_estop();
        l.gate.arming.arm_motion().unwrap();
        l.sim.set_param_status(1);
        l.poll_status().unwrap();
        assert!(matches!(l.write(&command::jog_step(0, 1.0, &p()).unwrap()), Err(LinkError::Refused(GateRefusal::MachineFault(_)))));
    }

    #[test]
    fn outputs_and_stop_sequence() {
        let mut l = armed();
        // the simulator's outputs word starts at 0; turn a lamp on through the card side
        l.sim.set_inputs(0b101);
        assert!(l.poll_status().unwrap().di_level(1));
        l.write(&command::do_off(1).unwrap()).unwrap();
        let steps = l.stop_sequence(false);
        assert_eq!(steps, 6);
        assert!(l.sim.commands().iter().any(|c| c.as_slice() == [9999, 4, 0, 0]));
        assert!(!l.sim.commands().iter().any(|c| c.as_slice() == [101]));
        // pwm off reaches the status words
        assert_eq!(l.sim.reg(Status::PwmDuty.addr()), 0);
    }

    #[test]
    fn an_emergency_stop_with_the_gas_on_still_sends_every_stop_step() {
        let mut l = armed();
        // Gas valve on (port 2), as during a cut.
        l.write(&command::SafeWrite::try_from_raw(0x65, &[9999, 2, 0b10, 0b10]).unwrap()).unwrap();
        assert_eq!(l.poll_status().unwrap().word(Status::Do) & 0xFFFF, 0b10);
        // The card reports an E-stop: the gate latches, disarms and the stop sequence goes out.
        l.sim.inject_alarm(Some(1 << 30), None);
        let before = l.sim.commands().len();
        l.poll_status().unwrap();
        assert!(l.gate.arming.estop_latched());
        let sent: Vec<Vec<u32>> = l.sim.commands()[before..].to_vec();
        // Whatever form it takes, the laser ports 5, 6 and 9 are told to switch off.
        for port in [5u32, 6, 9] {
            let bit = 1 << (port - 1);
            assert!(
                sent.iter().any(|c| c.len() == 4 && c[0] == 9999 && c[1] == 2 && c[2] & bit != 0 && c[3] & bit == 0),
                "nothing switched laser port {port} off: {sent:?}"
            );
        }
        // And the gas stays as it was: a stop never switches anything on.
        assert_eq!(l.sim.outputs() & 0b10, 0b10);
    }

    #[test]
    fn forbidden_requests_are_refused_and_recorded() {
        let mut sim = CardSimulator::new(SimConfig::default());
        let mk = |addr: u32, words: &[u32]| {
            let mut p = Vec::new();
            p.extend_from_slice(&addr.to_le_bytes());
            p.extend_from_slice(&(words.len() as u16).to_le_bytes());
            for w in words {
                p.extend_from_slice(&w.to_le_bytes());
            }
            crate::frame::build_frame(9, 0x40, &p, crate::crc::CrcOrder::HiFirst).unwrap()
        };
        for (addr, words) in [(150u32, &[5555u32][..]), (151, &[1]), (59502, &[1]), (0x65, &[0x65]), (0x65, &[0x66]), (0x65, &[107])] {
            let rep = sim.exchange(&mk(addr, words)).unwrap();
            assert_eq!(Reply::decode(&rep, Some(9)).unwrap(), Reply::Exception { func: 0x40, code: 2 }, "{addr} {words:?}");
        }
        assert_eq!(sim.forbidden_attempts().len(), 6);
        assert!(sim.commands().is_empty());
        assert_eq!(sim.reg(150), 0);
        // reads of the licence area
        let rd = crate::frame::build_frame(3, 0x30, &[0x6c, 0xe8, 0, 0, 1, 0], crate::crc::CrcOrder::HiFirst).unwrap();
        let rep = sim.exchange(&rd).unwrap();
        assert_eq!(Reply::decode(&rep, Some(3)).unwrap(), Reply::Exception { func: 0x30, code: 2 });
        assert_eq!(sim.forbidden_attempts().len(), 7);
    }

    #[test]
    fn laser_class_data_never_runs() {
        let mut sim = CardSimulator::new(SimConfig::default());
        let mk = |reg: u32, words: &[u32]| {
            let mut p = Vec::new();
            p.extend_from_slice(&reg.to_le_bytes());
            p.extend_from_slice(&(words.len() as u16).to_le_bytes());
            for w in words {
                p.extend_from_slice(&w.to_le_bytes());
            }
            crate::frame::build_frame(1, 0x40, &p, crate::crc::CrcOrder::HiFirst).unwrap()
        };
        // DO9 on, PWM with duty, and a FIFO tick with duty
        for (reg, words) in
            [(0x65u32, vec![9999u32, 2, 0x100, 0x100]), (0x65, vec![9999, 3, 1234, 20]), (0x66, vec![1, 0x0008_0BB8, 0, (5000 << 16) | 4])]
        {
            let rep = sim.exchange(&mk(reg, &words)).unwrap();
            assert!(matches!(Reply::decode(&rep, Some(1)).unwrap(), Reply::Exception { .. }));
        }
        assert_eq!(sim.laser_refusals(), 3);
        assert_eq!(sim.outputs(), 0);
        assert_eq!(sim.fifo_depth(), 0);
        assert!(sim.forbidden_attempts().is_empty(), "laser data is a policy refusal, not a forbidden register");
    }

    #[test]
    fn damaged_frames_are_ignored() {
        let mut sim = CardSimulator::new(SimConfig::default());
        let mut f = Request::read(1000, 2).unwrap().encode(1).unwrap();
        f[9] ^= 0xff;
        assert_eq!(sim.exchange(&f), Err(Timeout));
        assert_eq!(sim.exchange(&f[..5]), Err(Timeout));
        assert_eq!(sim.exchange(&[]), Err(Timeout));
        assert!(sim.requests().all(|r| matches!(r.outcome, Outcome::Dropped(_))));
    }

    #[test]
    fn request_log_is_a_ring() {
        let mut sim = CardSimulator::new(SimConfig { request_log_limit: 3, ..SimConfig::default() });
        for i in 0..10u16 {
            sim.exchange(&Request::read(1000, 2).unwrap().encode(i).unwrap()).unwrap();
        }
        let seqs: Vec<_> = sim.requests().map(|r| r.seq.unwrap()).collect();
        assert_eq!(seqs, vec![7, 8, 9]);
    }

    #[test]
    fn digital_output_records_in_the_stream_take_effect() {
        // a gas record (DO3 high air on) inside a frame is accepted and executed in order
        let cfg = SimConfig { fifo_starvation_alarm: false, ..SimConfig::default() };
        let mut sim = CardSimulator::new(cfg);
        let mut words = vec![1u32];
        words.extend(FifoItem::tick(1, 0).words());
        words.extend([0x000C_270F, 2, 4, 4]);
        words.extend(FifoItem::tick(1, 0).words());
        let w = crate::command::SafeWrite::try_from_raw(0x66, &words).unwrap();
        sim.exchange(&Request::write(w).encode(1).unwrap()).unwrap();
        sim.exchange(&Request::write(command::fifo_start().unwrap()).encode(2).unwrap()).unwrap();
        sim.advance(Duration::from_micros(250));
        // one tick budget: the first tick runs, then the non-tick record that follows it
        assert_eq!(sim.ticks_consumed(), 1);
        assert_eq!(sim.outputs(), 4);
        assert_eq!(sim.items_consumed(), 2);
    }

    #[test]
    fn a_stop_switches_off_the_laser_ports_this_gate_is_set_up_with() {
        let mut safety = SafetyConfig::default();
        safety.motion_do_ports.push(8); // the simulated card accepts port 8
        let mut l = SimLink::new(CardSimulator::new(SimConfig { safety: safety.clone(), ..SimConfig::default() }));
        l.gate.config.safety.laser_do_ports = vec![5, 6, 8, 9];
        let on = command::SafeWrite::from_raw_for_simulator(0x65, &[9999, 2, 0x80, 0x80], &safety, None).unwrap();
        l.sim.exchange(&Request::write(on).encode(77).unwrap()).unwrap();
        assert_eq!(l.sim.outputs() & 0x80, 0x80);
        l.poll_status().unwrap();
        l.stop_sequence(false);
        assert_eq!(l.sim.outputs() & 0x80, 0, "port 8 is a laser port on this machine and stayed on");
    }

    #[test]
    fn a_held_jog_whose_answer_was_lost_still_gets_its_release_stop() {
        let mut l = armed();
        l.sim.drop_replies(1);
        assert_eq!(l.write(&command::jog_continuous(0, true, &p(), None).unwrap()), Err(LinkError::Timeout));
        assert_eq!(l.sim.commands().len(), 1, "the card did start the jog");
        l.advance(ms(250));
        assert!(l.sim.commands().iter().any(|c| c[0] == 1 && c[1] & 1 == 1), "no release stop went out: {:?}", l.sim.commands());
    }

    #[test]
    fn a_stop_right_after_a_start_whose_answer_was_lost_stops_the_stream() {
        let mut l = armed();
        l.write(&command::fifo_clear().unwrap()).unwrap();
        let mut f = stream_plan(&mut l, 250);
        pump(&mut l, &mut f);
        l.sim.drop_replies(1);
        assert_eq!(l.write(&command::fifo_start().unwrap()), Err(LinkError::Timeout));
        assert!(l.sim.fifo_running(), "the card did start");
        l.stop_sequence(false);
        assert!(!l.sim.fifo_running(), "the stream was left running");
    }

    #[test]
    fn a_job_stopped_by_an_alarm_is_not_run_on_by_a_bare_start() {
        let mut l = SimLink::new(CardSimulator::new(SimConfig { fifo_starvation_alarm: false, ..SimConfig::default() }));
        l.gate.arming.arm_motion().unwrap();
        l.poll_status().unwrap();
        l.write(&command::fifo_clear().unwrap()).unwrap();
        let mut f = stream_plan(&mut l, 250);
        pump(&mut l, &mut f);
        l.write(&command::fifo_start().unwrap()).unwrap();
        l.advance(Duration::from_micros(250 * 100));
        // an alarm that only the status block shows
        l.sim.inject_alarm(None, Some(1 << 3));
        l.poll_status().unwrap();
        assert!(!l.sim.fifo_running());
        assert_eq!(l.sim.fifo_depth(), 150, "the card still holds the rest of the job");
        // the alarm goes away and the operator arms again
        l.sim.inject_alarm(None, Some(0));
        l.poll_status().unwrap();
        l.gate.arming.arm_motion().unwrap();
        let before = l.sim.ticks_consumed();
        assert!(l.write(&command::fifo_start().unwrap()).is_err(), "a bare start ran the rest of a stopped job");
        l.advance(ms(50));
        assert_eq!(l.sim.ticks_consumed(), before);
        // clearing the queue makes room for a new job
        l.poll_status().unwrap();
        l.write(&command::fifo_clear().unwrap()).unwrap();
        let mut f = stream_plan(&mut l, 10);
        pump(&mut l, &mut f);
        l.write(&command::fifo_start().unwrap()).unwrap();
        l.advance(ms(10));
        assert_eq!(l.sim.ticks_consumed(), before + 10);
    }

    #[test]
    fn one_late_answer_does_not_put_every_later_request_out_of_step() {
        let mut l = armed();
        l.sim.delay_replies(ms(5), Some(1));
        assert_eq!(l.read(1000, 2), Err(LinkError::Timeout));
        l.advance(ms(10)); // the late answer is waiting now
        assert_eq!(l.read(1000, 2).unwrap(), vec![100, 20152], "the late answer was taken for this request's");
        assert!(l.poll_status().is_ok());
        assert!(l.write(&command::jog_step(0, 1.0, &p()).unwrap()).is_ok());
    }

    #[test]
    fn a_stop_whose_first_datagram_is_lost_still_stops_the_stream() {
        let mut l = armed();
        l.write(&command::fifo_clear().unwrap()).unwrap();
        let mut f = stream_plan(&mut l, 250);
        pump(&mut l, &mut f);
        l.write(&command::fifo_start().unwrap()).unwrap();
        l.advance(Duration::from_micros(250 * 10));
        l.sim.drop_requests(1);
        l.stop_sequence(false);
        assert!(!l.sim.fifo_running(), "the stream was left running");
        assert_eq!(l.attempts, 1, "the retry is the stop's own, not a changed setting");
    }
}
