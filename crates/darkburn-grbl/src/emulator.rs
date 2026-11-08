//! An in-memory GRBL 1.1 controller for tests and demos.
//!
//! Behaviour follows the GRBL v1.1 interface documentation: a 128-byte serial RX buffer
//! (127 usable), a 15-block planner, `ok` / `error:N` per line, real-time bytes, `?` status
//! reports, `$` system commands, `$J=` jogging, alarms, check mode and laser mode. Motion is
//! timed from distance and feed rate (no acceleration) on a clock that runs `time_scale`
//! times faster than real time, so state goes Idle -> Run -> Idle like a real machine.
//!
//! The emulator is passive: [`MemoryTransport`](crate::MemoryTransport) feeds it bytes and
//! advances its clock. Test hooks (`trigger_hard_limit`, `unplug`, ...) are plain methods.

#![allow(clippy::needless_range_loop)] // per-axis arithmetic reads clearer with indices

use std::collections::{BTreeMap, VecDeque};
use std::fmt::Write as _;
use std::time::Duration;

use crate::protocol::{clean_line, rt, words};

/// Which firmware the emulator pretends to be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmulatorFlavor {
    /// GRBL 1.1h, the common diode-laser firmware.
    Grbl11h,
    /// GRBL 1.1e: same protocol, but an "M3 only" version for laser software.
    Grbl11e,
    /// GRBL 0.9j: older status grammar, text errors, no `$J=` and no overrides.
    Grbl09,
    /// grblHAL with a 1024-byte RX buffer and 35 planner blocks.
    GrblHal,
    /// FluidNC on an ESP32.
    FluidNc,
}

/// Emulator set-up.
#[derive(Clone, Debug)]
pub struct EmulatorConfig {
    pub flavor: EmulatorFlavor,
    /// Simulated seconds per real second. 1.0 is real time; tests use 50-1000.
    pub time_scale: f64,
    /// Serial RX buffer size; one byte is always kept free, as in GRBL's ring buffer.
    pub rx_buffer_size: usize,
    pub planner_blocks: usize,
    /// Real-time delay between a power-on reset and the banner.
    pub boot_delay: Duration,
    /// Opening the port resets the board (Arduino boards reset through DTR).
    pub reset_on_open: bool,
    /// Homing sets the machine origin to 0 and soft limits use 0..max travel (common on diode
    /// lasers). When false, soft limits use -max travel..0 like stock GRBL.
    pub force_set_origin: bool,
    /// Simulated seconds a homing cycle takes.
    pub homing_seconds: f64,
    /// Make `$H` fail with ALARM:9.
    pub homing_fails: bool,
    /// Keep every processed line in [`EmulatorStats::lines`].
    pub record_lines: bool,
    /// Settings that differ from the built-in defaults.
    pub settings: Vec<(u16, f64)>,
}

impl Default for EmulatorConfig {
    fn default() -> Self {
        Self::atomstack()
    }
}

impl EmulatorConfig {
    /// The owner's machine: Atomstack 400 × 400 mm, GRBL 1.1h, 115200 baud, S max 1000,
    /// laser mode on.
    pub fn atomstack() -> Self {
        Self {
            flavor: EmulatorFlavor::Grbl11h,
            time_scale: 1.0,
            rx_buffer_size: 128,
            planner_blocks: 15,
            boot_delay: Duration::from_millis(30),
            reset_on_open: true,
            force_set_origin: true,
            homing_seconds: 2.0,
            homing_fails: false,
            record_lines: true,
            settings: Vec::new(),
        }
    }
    /// Another firmware family with matching buffer sizes.
    pub fn flavor(flavor: EmulatorFlavor) -> Self {
        let mut c = Self::atomstack();
        c.flavor = flavor;
        if flavor == EmulatorFlavor::GrblHal {
            c.rx_buffer_size = 1024;
            c.planner_blocks = 35;
        }
        c
    }
    pub fn with_time_scale(mut self, scale: f64) -> Self {
        self.time_scale = scale;
        self
    }
    pub fn with_setting(mut self, n: u16, v: f64) -> Self {
        self.settings.push((n, v));
        self
    }
}

/// Counters the tests look at.
#[derive(Clone, Debug, Default)]
pub struct EmulatorStats {
    /// Non-empty lines taken out of the RX buffer.
    pub lines_processed: usize,
    pub oks: usize,
    pub errors: usize,
    /// Most bytes ever waiting in the RX buffer.
    pub rx_peak: usize,
    /// Bytes dropped because the RX buffer was full (must stay 0 for a correct sender).
    pub rx_overflow: usize,
    pub motion_blocks: usize,
    /// Every line processed (when `record_lines`).
    pub lines: Vec<String>,
    /// Every real-time byte received except `?`.
    pub realtime: Vec<u8>,
    pub status_queries: usize,
    pub soft_resets: usize,
    pub power_on_resets: usize,
    /// Simulated seconds the laser was emitting.
    pub laser_on_seconds: f64,
    /// Times the laser went from off to on.
    pub laser_switch_ons: usize,
    /// Highest S value (after override) the laser fired at.
    pub max_laser_s: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum State {
    Idle,
    Run,
    /// `complete`: Hold:0; otherwise decelerating (Hold:1).
    Hold {
        complete: bool,
    },
    Jog,
    Alarm,
    /// 0 closed/ready, 1 ajar, 2 holding, 3 resuming.
    Door(u8),
    Check,
    Home,
    Sleep,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Spindle {
    Off,
    Cw,
    Ccw,
}

#[derive(Clone, Copy, Debug)]
struct Block {
    target: [f64; 3],
    feed: f64,
    rapid: bool,
    jog: bool,
    /// S value while this block runs (0 = laser off).
    power: f64,
    /// Seconds to wait (G4). Not a move.
    dwell: f64,
}

#[derive(Clone, Copy, Debug)]
struct Active {
    block: Block,
    start: [f64; 3],
    length: f64,
    done: f64,
    dwell_left: f64,
}

const DECEL_SECONDS: f64 = 0.05;
const LINE_LIMIT: usize = 80;

/// The emulated controller.
pub struct GrblEmulator {
    cfg: EmulatorConfig,
    now: f64,
    out: Vec<u8>,
    rx: VecDeque<u8>,
    last_was_cr: bool,
    boot_until: Option<f64>,
    timer_until: Option<f64>,
    state: State,
    resume_after_hold: bool,
    /// Critical alarm: ignore everything except real-time bytes until a reset.
    wait_reset: bool,
    alarm_code: Option<u16>,
    mpos: [f64; 3],
    gc_pos: [f64; 3],
    wcs: [[f64; 3]; 6],
    wcs_index: usize,
    g92: [f64; 3],
    motion_mode: u8,
    absolute: bool,
    inches: bool,
    feed: f64,
    s: f64,
    spindle: Spindle,
    flood: bool,
    mist: bool,
    planner: VecDeque<Block>,
    current: Option<Active>,
    ov: (u16, u16, u16),
    settings: BTreeMap<u16, f64>,
    reports: u32,
    wco_dirty: bool,
    ov_dirty: bool,
    hard_limit_pins: String,
    door_switch_open: bool,
    dtr: bool,
    unplugged: bool,
    laser_was_on: bool,
    stats: EmulatorStats,
}

fn default_settings(flavor: EmulatorFlavor) -> BTreeMap<u16, f64> {
    let mut m: BTreeMap<u16, f64> = [
        (0, 10.0),
        (1, 25.0),
        (2, 0.0),
        (3, 0.0),
        (4, 0.0),
        (5, 0.0),
        (6, 0.0),
        (10, 1.0),
        (11, 0.010),
        (12, 0.002),
        (13, 0.0),
        (20, 0.0),
        (21, 0.0),
        (22, 0.0),
        (23, 0.0),
        (24, 100.0),
        (25, 1000.0),
        (26, 250.0),
        (27, 2.0),
        (30, 1000.0),
        (31, 0.0),
        (32, 1.0),
        (100, 80.0),
        (101, 80.0),
        (102, 800.0),
        (110, 6000.0),
        (111, 6000.0),
        (112, 600.0),
        (120, 1000.0),
        (121, 1000.0),
        (122, 100.0),
        (130, 400.0),
        (131, 400.0),
        (132, 50.0),
    ]
    .into_iter()
    .collect();
    if flavor == EmulatorFlavor::GrblHal {
        m.extend([(33, 1000.0), (34, 0.0), (35, 0.0), (36, 100.0)]);
    }
    if flavor == EmulatorFlavor::Grbl09 {
        m.remove(&30);
        m.remove(&31);
        m.remove(&32);
    }
    m
}

fn is_float_setting(n: u16) -> bool {
    matches!(n, 11 | 12 | 24 | 25 | 27 | 100..=132)
}

impl GrblEmulator {
    pub fn new(cfg: EmulatorConfig) -> Self {
        let mut settings = default_settings(cfg.flavor);
        for (n, v) in &cfg.settings {
            settings.insert(*n, *v);
        }
        let mut e = Self {
            cfg,
            now: 0.0,
            out: Vec::new(),
            rx: VecDeque::new(),
            last_was_cr: false,
            boot_until: None,
            timer_until: None,
            state: State::Idle,
            resume_after_hold: false,
            wait_reset: false,
            alarm_code: None,
            mpos: [0.0; 3],
            gc_pos: [0.0; 3],
            wcs: [[0.0; 3]; 6],
            wcs_index: 0,
            g92: [0.0; 3],
            motion_mode: 0,
            absolute: true,
            inches: false,
            feed: 0.0,
            s: 0.0,
            spindle: Spindle::Off,
            flood: false,
            mist: false,
            planner: VecDeque::new(),
            current: None,
            ov: (100, 100, 100),
            settings,
            reports: 0,
            wco_dirty: true,
            ov_dirty: true,
            hard_limit_pins: String::new(),
            door_switch_open: false,
            dtr: true,
            unplugged: false,
            laser_was_on: false,
            stats: EmulatorStats::default(),
        };
        e.power_on_reset();
        e
    }

    // ----- inspection and test hooks -----

    pub fn stats(&self) -> &EmulatorStats {
        &self.stats
    }
    pub fn stats_mut(&mut self) -> &mut EmulatorStats {
        &mut self.stats
    }
    pub fn setting(&self, n: u16) -> Option<f64> {
        self.settings.get(&n).copied()
    }
    pub fn machine_position(&self) -> [f64; 3] {
        self.mpos
    }
    pub fn work_position(&self) -> [f64; 3] {
        let w = self.wco();
        [self.mpos[0] - w[0], self.mpos[1] - w[1], self.mpos[2] - w[2]]
    }
    pub fn work_offset(&self) -> [f64; 3] {
        self.wco()
    }
    /// Status state name as it would appear in a report, e.g. `Run` or `Hold:0`.
    pub fn state_name(&self) -> String {
        self.state_text()
    }
    pub fn rx_len(&self) -> usize {
        self.rx.len()
    }
    pub fn planner_len(&self) -> usize {
        self.planner.len() + usize::from(self.current.is_some())
    }
    pub fn overrides(&self) -> (u16, u16, u16) {
        self.ov
    }
    /// True while the laser is emitting.
    pub fn laser_on(&self) -> bool {
        self.laser_power() > 0.0
    }
    pub fn spindle_enabled(&self) -> bool {
        self.spindle != Spindle::Off
    }
    pub fn is_unplugged(&self) -> bool {
        self.unplugged
    }
    /// Pull the cable: the transport fails until [`replug`](Self::replug).
    pub fn unplug(&mut self) {
        self.unplugged = true;
    }
    pub fn replug(&mut self) {
        self.unplugged = false;
    }
    /// A limit switch closes while running: ALARM:1, everything stops.
    pub fn trigger_hard_limit(&mut self, axis: char) {
        self.hard_limit_pins = axis.to_ascii_uppercase().to_string();
        self.raise_alarm(1, true);
    }
    pub fn release_limit_switch(&mut self) {
        self.hard_limit_pins.clear();
    }
    /// Physically opens or closes the safety door switch.
    pub fn set_door_switch(&mut self, open: bool) {
        self.door_switch_open = open;
        if open {
            self.enter_door();
        } else if self.state == State::Door(1) {
            self.state = State::Door(0);
        }
    }

    // ----- transport side -----

    /// Called when a transport is created (cable plugged in / port opened).
    pub fn port_opened(&mut self) {
        self.unplugged = false;
        self.out.clear();
        if self.cfg.reset_on_open {
            self.power_on_reset();
        }
    }
    pub fn port_closed(&mut self) {}

    /// DTR going high after being low resets Arduino-based boards.
    pub fn set_dtr(&mut self, level: bool) {
        if level && !self.dtr {
            self.power_on_reset();
        }
        self.dtr = level;
    }

    /// Takes everything the controller has written since the last call.
    pub fn take_output(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.out)
    }

    /// Bytes arriving from the host.
    pub fn receive(&mut self, data: &[u8]) {
        for &b in data {
            if self.boot_until.is_some() {
                continue;
            }
            if self.is_rt(b) {
                self.realtime(b);
                continue;
            }
            if self.wait_reset {
                continue;
            }
            if self.cfg.flavor == EmulatorFlavor::GrblHal {
                // grblHAL counts CR LF as a single line end.
                if b == b'\n' && self.last_was_cr {
                    self.last_was_cr = false;
                    continue;
                }
                self.last_was_cr = b == b'\r';
            }
            if self.rx.len() >= self.cfg.rx_buffer_size - 1 {
                self.stats.rx_overflow += 1;
                continue;
            }
            self.rx.push_back(b);
            self.stats.rx_peak = self.stats.rx_peak.max(self.rx.len());
        }
        self.process_rx();
    }

    fn is_rt(&self, b: u8) -> bool {
        if self.cfg.flavor == EmulatorFlavor::Grbl09 {
            matches!(b, rt::STATUS | rt::CYCLE_START | rt::FEED_HOLD | rt::SOFT_RESET)
        } else {
            rt::is_realtime(b)
        }
    }

    /// Advances the simulated clock by `real` real time.
    pub fn advance(&mut self, real: Duration) {
        let mut remaining = real.as_secs_f64() * self.cfg.time_scale;
        let mut guard = 0u32;
        loop {
            self.process_rx();
            if remaining <= 0.0 || guard > 2_000_000 {
                break;
            }
            guard += 1;
            let used = self.step(remaining);
            remaining -= used;
        }
        self.process_rx();
    }

    // ----- clock -----

    fn step(&mut self, max: f64) -> f64 {
        let power_before = self.laser_power();
        let used = self.step_inner(max);
        if used > 0.0 && power_before > 0.0 {
            self.stats.laser_on_seconds += used;
        }
        self.track_laser();
        used
    }

    fn step_inner(&mut self, max: f64) -> f64 {
        if let Some(t) = self.boot_until {
            let d = (t - self.now).max(0.0);
            if d <= max {
                self.now = t;
                self.boot_until = None;
                self.print_banner();
                return d;
            }
            self.now += max;
            return max;
        }
        if let Some(t) = self.timer_until {
            let d = (t - self.now).max(0.0);
            if d > max {
                self.now += max;
                return max;
            }
            self.now = t;
            self.timer_until = None;
            self.timer_done();
            return d;
        }
        match self.state {
            State::Run | State::Jog => self.step_motion(max),
            State::Idle if !self.planner.is_empty() => {
                self.state = if self.planner[0].jog { State::Jog } else { State::Run };
                0.0
            }
            _ => {
                self.now += max;
                max
            }
        }
    }

    fn timer_done(&mut self) {
        match self.state {
            State::Hold { complete: false } => {
                self.state = State::Hold { complete: true };
                if self.resume_after_hold {
                    self.resume_after_hold = false;
                    self.resume();
                }
            }
            State::Door(2) => self.state = State::Door(if self.door_switch_open { 1 } else { 0 }),
            State::Door(3) => self.state = if self.cycle_pending() { State::Run } else { State::Idle },
            State::Home => {
                if self.cfg.homing_fails {
                    // GRBL aborts through its reset path: alarm, the `$H` line's `ok`, banner.
                    self.raise_alarm(9, false);
                    self.reply_ok();
                    self.reset_modal();
                    self.print_banner();
                    return;
                }
                self.mpos = [0.0; 3];
                if !self.cfg.force_set_origin {
                    let pull = self.settings.get(&27).copied().unwrap_or(1.0);
                    self.mpos = [-pull, -pull, -pull];
                }
                self.gc_pos = self.mpos;
                self.state = State::Idle;
                self.alarm_code = None;
                self.reply_ok();
            }
            _ => {}
        }
    }

    fn cycle_pending(&self) -> bool {
        self.current.is_some() || !self.planner.is_empty()
    }

    fn effective_rate(&self, b: &Block) -> f64 {
        let max_xy = self.settings[&110].min(self.settings[&111]);
        if b.jog {
            return b.feed.min(max_xy).max(1.0);
        }
        if b.rapid {
            return (max_xy * f64::from(self.ov.1) / 100.0).max(1.0);
        }
        (b.feed * f64::from(self.ov.0) / 100.0).min(max_xy).max(1.0)
    }

    fn step_motion(&mut self, max: f64) -> f64 {
        if self.current.is_none() {
            match self.planner.pop_front() {
                Some(block) => {
                    let start = self.mpos;
                    let length = dist(start, block.target);
                    self.current = Some(Active { block, start, length, done: 0.0, dwell_left: block.dwell });
                    if block.jog {
                        self.state = State::Jog;
                    } else if self.state == State::Jog {
                        self.state = State::Run;
                    }
                }
                None => {
                    self.state = State::Idle;
                    self.gc_pos = self.mpos;
                    return 0.0;
                }
            }
        }
        let mut cur = self.current.expect("active block");
        let used;
        if cur.dwell_left > 0.0 {
            used = cur.dwell_left.min(max);
            cur.dwell_left -= used;
        } else {
            let rate = self.effective_rate(&cur.block) / 60.0;
            let left = cur.length - cur.done;
            let t_left = left / rate;
            if t_left <= max {
                used = t_left;
                cur.done = cur.length;
            } else {
                used = max;
                cur.done += rate * max;
            }
            let f = if cur.length > 0.0 { cur.done / cur.length } else { 1.0 };
            for i in 0..3 {
                self.mpos[i] = cur.start[i] + (cur.block.target[i] - cur.start[i]) * f;
            }
        }
        self.now += used;
        if cur.dwell_left <= 0.0 && cur.done >= cur.length {
            self.mpos = cur.block.target;
            self.current = None;
            self.stats.motion_blocks += 1;
        } else {
            self.current = Some(cur);
        }
        used
    }

    // ----- laser -----

    fn laser_mode(&self) -> bool {
        self.settings.get(&32).copied().unwrap_or(0.0) >= 1.0
    }

    fn laser_power(&self) -> f64 {
        let scale = f64::from(self.ov.2) / 100.0;
        match self.state {
            State::Run | State::Jog => {
                if let Some(c) = &self.current {
                    if c.dwell_left > 0.0 { self.static_power() } else { c.block.power * scale }
                } else {
                    0.0
                }
            }
            State::Idle | State::Check => self.static_power(),
            _ => 0.0,
        }
    }

    /// Power while not moving: M3 in G1/G2/G3 mode fires in laser mode; any spindle-on state
    /// fires without laser mode.
    fn static_power(&self) -> f64 {
        if self.state == State::Check || self.spindle == Spindle::Off || self.s <= 0.0 {
            return 0.0;
        }
        let on = if self.laser_mode() { self.spindle == Spindle::Cw && self.motion_mode != 0 } else { true };
        if on { self.s * f64::from(self.ov.2) / 100.0 } else { 0.0 }
    }

    fn track_laser(&mut self) {
        let p = self.laser_power();
        if p > 0.0 {
            self.stats.max_laser_s = self.stats.max_laser_s.max(p);
            if !self.laser_was_on {
                self.stats.laser_switch_ons += 1;
            }
        }
        self.laser_was_on = p > 0.0;
    }

    // ----- output -----

    fn print(&mut self, line: &str) {
        self.out.extend_from_slice(line.as_bytes());
        self.out.extend_from_slice(b"\r\n");
    }

    fn reply_ok(&mut self) {
        self.stats.oks += 1;
        self.print("ok");
    }

    fn reply_error(&mut self, code: u16) {
        self.stats.errors += 1;
        if self.cfg.flavor == EmulatorFlavor::Grbl09 {
            let text = crate::codes::error_message(code);
            self.print(&format!("error: {}", text.trim_end_matches('.')));
        } else {
            self.print(&format!("error:{code}"));
        }
    }

    fn banner(&self) -> &'static str {
        match self.cfg.flavor {
            EmulatorFlavor::Grbl11h => "Grbl 1.1h ['$' for help]",
            EmulatorFlavor::Grbl11e => "Grbl 1.1e ['$' for help]",
            EmulatorFlavor::Grbl09 => "Grbl 0.9j ['$' for help]",
            EmulatorFlavor::GrblHal => "GrblHAL 1.1f ['$' or '$HELP' for help]",
            EmulatorFlavor::FluidNc => "Grbl 3.9 [FluidNC v3.9.7 (noradio) '$' for help]",
        }
    }

    fn print_banner(&mut self) {
        self.print("");
        let b = self.banner();
        self.print(b);
        if self.state == State::Alarm {
            self.print("[MSG:'$H'|'$X' to unlock]");
        }
    }

    // ----- resets and alarms -----

    fn clear_motion(&mut self) {
        self.rx.clear();
        self.planner.clear();
        self.current = None;
        self.timer_until = None;
        self.resume_after_hold = false;
        self.gc_pos = self.mpos;
    }

    fn power_on_reset(&mut self) {
        self.stats.power_on_resets += 1;
        self.clear_motion();
        self.mpos = [0.0; 3];
        self.gc_pos = self.mpos;
        self.g92 = [0.0; 3];
        self.reset_modal();
        self.wait_reset = false;
        self.alarm_code = None;
        self.state = if self.settings.get(&22).copied().unwrap_or(0.0) >= 1.0 { State::Alarm } else { State::Idle };
        self.boot_until = Some(self.now + self.cfg.boot_delay.as_secs_f64() * self.cfg.time_scale);
        self.reports = 0;
        self.wco_dirty = true;
        self.ov_dirty = true;
        self.track_laser();
    }

    fn reset_modal(&mut self) {
        self.motion_mode = 0;
        self.absolute = true;
        self.inches = false;
        self.feed = 0.0;
        self.s = 0.0;
        self.spindle = Spindle::Off;
        self.flood = false;
        self.mist = false;
        self.wcs_index = 0;
        self.ov = (100, 100, 100);
    }

    fn soft_reset(&mut self) {
        self.stats.soft_resets += 1;
        let moving = matches!(self.state, State::Run | State::Jog | State::Home | State::Hold { complete: false })
            || matches!(self.state, State::Door(2) | State::Door(3));
        if moving && !self.wait_reset {
            self.raise_alarm_quiet(if self.state == State::Home { 6 } else { 3 });
            let code = self.alarm_code.unwrap_or(3);
            self.print_alarm(code);
        }
        let in_alarm = self.state == State::Alarm || self.wait_reset;
        self.clear_motion();
        self.reset_modal();
        self.wait_reset = false;
        self.state = if in_alarm { State::Alarm } else { State::Idle };
        self.reports = 0;
        self.wco_dirty = true;
        self.ov_dirty = true;
        self.print_banner();
        self.track_laser();
    }

    fn print_alarm(&mut self, code: u16) {
        if self.cfg.flavor == EmulatorFlavor::Grbl09 {
            let text = match code {
                1 | 2 => "Hard/soft limit",
                3 => "Abort during cycle",
                _ => "Homing fail",
            };
            self.print(&format!("ALARM: {text}"));
        } else {
            self.print(&format!("ALARM:{code}"));
        }
    }

    fn raise_alarm_quiet(&mut self, code: u16) {
        self.alarm_code = Some(code);
        self.state = State::Alarm;
    }

    fn raise_alarm(&mut self, code: u16, critical: bool) {
        self.clear_motion();
        self.raise_alarm_quiet(code);
        self.print_alarm(code);
        if critical {
            self.wait_reset = true;
            self.print("[MSG:Reset to continue]");
        }
        self.track_laser();
    }

    // ----- real-time commands -----

    fn realtime(&mut self, b: u8) {
        if b != rt::STATUS {
            self.stats.realtime.push(b);
        }
        match b {
            rt::STATUS => {
                self.stats.status_queries += 1;
                if self.state != State::Home {
                    let r = self.status_report();
                    self.print(&r);
                }
            }
            rt::SOFT_RESET => self.soft_reset(),
            rt::FEED_HOLD => self.feed_hold(),
            rt::CYCLE_START => self.cycle_start(),
            rt::SAFETY_DOOR => self.enter_door(),
            rt::JOG_CANCEL => {
                if self.state == State::Jog {
                    self.cancel_jog();
                }
            }
            0x90..=0x94 => {
                let f = &mut self.ov.0;
                *f = match b {
                    0x90 => 100,
                    0x91 => (*f + 10).min(200),
                    0x92 => f.saturating_sub(10).max(10),
                    0x93 => (*f + 1).min(200),
                    _ => f.saturating_sub(1).max(10),
                };
                self.ov_dirty = true;
            }
            0x95 => self.set_rapid(100),
            0x96 => self.set_rapid(50),
            0x97 => self.set_rapid(25),
            0x99..=0x9D => {
                let s = &mut self.ov.2;
                *s = match b {
                    0x99 => 100,
                    0x9A => (*s + 10).min(200),
                    0x9B => s.saturating_sub(10).max(10),
                    0x9C => (*s + 1).min(200),
                    _ => s.saturating_sub(1).max(10),
                };
                self.ov_dirty = true;
            }
            rt::FLOOD_TOGGLE => self.flood = !self.flood,
            rt::MIST_TOGGLE => self.mist = !self.mist,
            _ => {}
        }
        self.track_laser();
    }

    fn set_rapid(&mut self, v: u16) {
        self.ov.1 = v;
        self.ov_dirty = true;
    }

    fn feed_hold(&mut self) {
        match self.state {
            State::Run => {
                self.state = State::Hold { complete: false };
                self.timer_until = Some(self.now + DECEL_SECONDS);
            }
            State::Jog => self.cancel_jog(),
            State::Idle if self.cycle_pending() => {
                self.state = State::Hold { complete: true };
            }
            _ => {}
        }
    }

    fn cycle_start(&mut self) {
        match self.state {
            State::Hold { complete: true } => self.resume(),
            State::Hold { complete: false } => self.resume_after_hold = true,
            State::Door(0) => {
                self.state = State::Door(3);
                self.timer_until = Some(self.now + DECEL_SECONDS);
            }
            _ => {}
        }
    }

    fn resume(&mut self) {
        self.state = if self.cycle_pending() { State::Run } else { State::Idle };
    }

    fn enter_door(&mut self) {
        if self.cfg.flavor == EmulatorFlavor::Grbl09 {
            return;
        }
        match self.state {
            State::Run | State::Hold { .. } | State::Jog => {
                if self.state == State::Jog {
                    self.cancel_jog();
                    self.state = State::Door(if self.door_switch_open { 1 } else { 0 });
                } else {
                    self.state = State::Door(2);
                    self.timer_until = Some(self.now + DECEL_SECONDS);
                }
            }
            State::Idle => self.state = State::Door(if self.door_switch_open { 1 } else { 0 }),
            _ => {}
        }
    }

    fn cancel_jog(&mut self) {
        self.planner.retain(|b| !b.jog);
        if self.current.is_some_and(|c| c.block.jog) {
            self.current = None;
        }
        self.gc_pos = self.mpos;
        self.state = if self.cycle_pending() { State::Run } else { State::Idle };
    }

    // ----- status report -----

    fn wco(&self) -> [f64; 3] {
        let w = self.wcs[self.wcs_index];
        [w[0] + self.g92[0], w[1] + self.g92[1], w[2] + self.g92[2]]
    }

    fn state_text(&self) -> String {
        match self.state {
            State::Idle => "Idle".into(),
            State::Run => "Run".into(),
            State::Hold { complete } => format!("Hold:{}", if complete { 0 } else { 1 }),
            State::Jog => "Jog".into(),
            State::Alarm => "Alarm".into(),
            State::Door(n) => format!("Door:{n}"),
            State::Check => "Check".into(),
            State::Home => "Home".into(),
            State::Sleep => "Sleep".into(),
        }
    }

    fn report_unit(&self, v: f64) -> String {
        let inch = self.settings.get(&13).copied().unwrap_or(0.0) >= 1.0;
        if inch { format!("{:.4}", v / 25.4) } else { format!("{v:.3}") }
    }

    fn axes_text(&self, a: [f64; 3]) -> String {
        format!("{},{},{}", self.report_unit(a[0]), self.report_unit(a[1]), self.report_unit(a[2]))
    }

    fn status_report(&mut self) -> String {
        let wco = self.wco();
        let wpos = self.work_position();
        let state = self.state_text();
        if self.cfg.flavor == EmulatorFlavor::Grbl09 {
            let state = state.split(':').next().unwrap_or("Idle").to_owned();
            return format!("<{state},MPos:{},WPos:{}>", self.axes_text(self.mpos), self.axes_text(wpos));
        }
        let mask = self.settings.get(&10).copied().unwrap_or(1.0) as u32;
        let mut r = format!("<{state}");
        if mask & 1 == 1 {
            let _ = write!(r, "|MPos:{}", self.axes_text(self.mpos));
        } else {
            let _ = write!(r, "|WPos:{}", self.axes_text(wpos));
        }
        if mask & 2 == 2 {
            let free_blocks = self.cfg.planner_blocks.saturating_sub(self.planner_len());
            let free_rx = self.cfg.rx_buffer_size - 1 - self.rx.len();
            let _ = write!(r, "|Bf:{free_blocks},{free_rx}");
        }
        let feed = match &self.current {
            Some(c) if matches!(self.state, State::Run | State::Jog) => self.effective_rate(&c.block),
            _ => 0.0,
        };
        let _ = write!(r, "|FS:{},{}", feed.round(), self.laser_power().round());
        let mut pins = self.hard_limit_pins.clone();
        if self.door_switch_open {
            pins.push('D');
        }
        if !pins.is_empty() {
            let _ = write!(r, "|Pn:{pins}");
        }
        if self.wco_dirty || self.reports.is_multiple_of(10) {
            let _ = write!(r, "|WCO:{}", self.axes_text(wco));
            self.wco_dirty = false;
        } else if self.ov_dirty || self.reports % 10 == 5 {
            let _ = write!(r, "|Ov:{},{},{}", self.ov.0, self.ov.1, self.ov.2);
            self.ov_dirty = false;
            let mut acc = String::new();
            match self.spindle {
                Spindle::Cw => acc.push('S'),
                Spindle::Ccw => acc.push('C'),
                Spindle::Off => {}
            }
            if self.flood {
                acc.push('F');
            }
            if self.mist {
                acc.push('M');
            }
            if !acc.is_empty() {
                let _ = write!(r, "|A:{acc}");
            }
        }
        self.reports += 1;
        r.push('>');
        r
    }

    // ----- line processing -----

    fn process_rx(&mut self) {
        loop {
            if self.boot_until.is_some() || self.wait_reset || self.state == State::Home || self.state == State::Sleep {
                return;
            }
            let Some(end) = self.rx.iter().position(|&b| b == b'\n' || b == b'\r') else {
                return;
            };
            let raw: String = self.rx.iter().take(end).map(|&b| b as char).collect();
            match self.try_line(&raw) {
                LineOutcome::Wait => return,
                LineOutcome::Done => {
                    // The line may itself have flushed the buffer (alarm, reset).
                    let n = (end + 1).min(self.rx.len());
                    self.rx.drain(..n);
                    self.track_laser();
                }
            }
        }
    }

    fn try_line(&mut self, raw: &str) -> LineOutcome {
        let Some(line) = clean_line(raw) else {
            self.reply_ok();
            return LineOutcome::Done;
        };
        let compact: String = line.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_ascii_uppercase();
        if compact.len() >= LINE_LIMIT {
            self.record(&line);
            self.reply_error(11);
            return LineOutcome::Done;
        }
        if compact.starts_with('$') {
            return self.system_command(&line, &compact);
        }
        match self.parse_gcode(&compact) {
            Err(code) => {
                self.record(&line);
                self.reply_error(code);
                LineOutcome::Done
            }
            Ok(g) => {
                if matches!(self.state, State::Alarm | State::Jog) {
                    self.record(&line);
                    self.reply_error(9);
                    return LineOutcome::Done;
                }
                if g.sync && self.cycle_pending() {
                    return LineOutcome::Wait;
                }
                if g.has_motion && self.planner_len() >= self.cfg.planner_blocks {
                    return LineOutcome::Wait;
                }
                self.record(&line);
                if self.apply_gcode(g) {
                    self.reply_ok();
                }
                LineOutcome::Done
            }
        }
    }

    fn record(&mut self, line: &str) {
        self.stats.lines_processed += 1;
        if self.cfg.record_lines {
            self.stats.lines.push(line.to_owned());
        }
    }

    fn idle_like(&self) -> bool {
        matches!(self.state, State::Idle | State::Alarm | State::Check) && !self.cycle_pending()
    }

    fn system_command(&mut self, line: &str, cmd: &str) -> LineOutcome {
        if let Some(jog) = cmd.strip_prefix("$J=") {
            if self.cfg.flavor == EmulatorFlavor::Grbl09 {
                self.record(line);
                self.reply_error(3);
                return LineOutcome::Done;
            }
            let res = self.parse_jog(jog);
            if let Ok(b) = &res {
                if self.state == State::Jog && self.planner_len() >= self.cfg.planner_blocks {
                    return LineOutcome::Wait;
                }
                if self.state == State::Idle && self.cycle_pending() {
                    return LineOutcome::Wait;
                }
                self.record(line);
                self.planner.push_back(*b);
                self.gc_pos = b.target;
                if self.state == State::Idle {
                    self.state = State::Jog;
                }
                self.reply_ok();
                return LineOutcome::Done;
            }
            self.record(line);
            self.reply_error(res.err().unwrap_or(16));
            return LineOutcome::Done;
        }
        // Everything else needs an empty planner.
        let anytime = matches!(cmd, "$" | "$G" | "$I" | "$#");
        if !anytime && !self.idle_like() {
            if self.cycle_pending() && matches!(self.state, State::Idle) {
                return LineOutcome::Wait;
            }
            self.record(line);
            self.reply_error(8);
            return LineOutcome::Done;
        }
        self.record(line);
        if let Some(rest) = cmd.strip_prefix('$')
            && let Some((n, v)) = rest.split_once('=')
            && let Ok(n) = n.parse::<u16>()
        {
            match self.write_setting(n, v) {
                Ok(()) => self.reply_ok(),
                Err(code) => self.reply_error(code),
            }
            return LineOutcome::Done;
        }
        let alarm_ok = matches!(cmd, "$$" | "$#" | "$G" | "$I" | "$" | "$X" | "$H" | "$N" | "$C" | "$SLP") || cmd.starts_with("$RST=");
        if self.state == State::Alarm && !alarm_ok {
            self.reply_error(9);
            return LineOutcome::Done;
        }
        match cmd {
            "$" => {
                self.print("[HLP:$$ $# $G $I $N $x=val $Nx=line $J=line $SLP $C $X $H ~ ! ? ctrl-x]");
                self.reply_ok();
            }
            "$$" => {
                let lines: Vec<String> = self
                    .settings
                    .iter()
                    .map(|(n, v)| if is_float_setting(*n) { format!("${n}={v:.3}") } else { format!("${n}={}", *v as i64) })
                    .collect();
                for l in lines {
                    self.print(&l);
                }
                self.reply_ok();
            }
            "$I" => {
                match self.cfg.flavor {
                    EmulatorFlavor::Grbl09 => self.print("[0.9j.20160303:]"),
                    EmulatorFlavor::Grbl11h => {
                        self.print("[VER:1.1h.20190825:]");
                        self.print(&format!("[OPT:V,{},{}]", self.cfg.planner_blocks, self.cfg.rx_buffer_size));
                    }
                    EmulatorFlavor::Grbl11e => {
                        self.print("[VER:1.1e.20161128:]");
                        self.print(&format!("[OPT:V,{},{}]", self.cfg.planner_blocks, self.cfg.rx_buffer_size));
                    }
                    EmulatorFlavor::GrblHal => {
                        self.print("[VER:1.1f.20240101:]");
                        self.print(&format!("[OPT:VNMSL,{},{},3,0]", self.cfg.planner_blocks, self.cfg.rx_buffer_size));
                        self.print("[FIRMWARE:grblHAL]");
                    }
                    EmulatorFlavor::FluidNc => {
                        self.print("[VER:3.9 FluidNC v3.9.7:]");
                        self.print("[OPT:PHS]");
                    }
                }
                self.reply_ok();
            }
            "$G" => {
                let m = match self.spindle {
                    Spindle::Off => 5,
                    Spindle::Cw => 3,
                    Spindle::Ccw => 4,
                };
                let line = format!(
                    "[GC:G{} G{} G17 G{} G{} G94 M{m} M9 T0 F{} S{}]",
                    self.motion_mode,
                    54 + self.wcs_index,
                    if self.inches { 20 } else { 21 },
                    if self.absolute { 90 } else { 91 },
                    self.feed,
                    self.s
                );
                self.print(&line);
                self.reply_ok();
            }
            "$#" => {
                for i in 0..6 {
                    let l = format!("[G{}:{}]", 54 + i, self.axes_text(self.wcs[i]));
                    self.print(&l);
                }
                self.print("[G28:0.000,0.000,0.000]");
                self.print("[G30:0.000,0.000,0.000]");
                let l = format!("[G92:{}]", self.axes_text(self.g92));
                self.print(&l);
                self.print("[TLO:0.000]");
                self.print("[PRB:0.000,0.000,0.000:0]");
                self.reply_ok();
            }
            "$N" => {
                self.print("$N0=");
                self.print("$N1=");
                self.reply_ok();
            }
            "$X" => {
                if self.state == State::Alarm {
                    self.state = State::Idle;
                    self.alarm_code = None;
                    self.print("[MSG:Caution: Unlocked]");
                }
                self.reply_ok();
            }
            "$H" => {
                if self.settings.get(&22).copied().unwrap_or(0.0) < 1.0 {
                    self.reply_error(5);
                } else {
                    self.state = State::Home;
                    self.timer_until = Some(self.now + self.cfg.homing_seconds);
                    // `ok` is sent when homing completes.
                }
            }
            "$C" => {
                if self.state == State::Check {
                    self.print("[MSG:Disabled]");
                    self.reply_ok();
                    self.soft_reset();
                } else if self.state == State::Idle {
                    self.state = State::Check;
                    self.print("[MSG:Enabled]");
                    self.reply_ok();
                } else {
                    self.reply_error(8);
                }
            }
            "$SLP" => {
                self.state = State::Sleep;
                self.print("[MSG:Sleeping]");
                self.reply_ok();
            }
            _ if cmd.starts_with("$RST=") => {
                let what = &cmd[5..];
                if matches!(what, "$" | "*") {
                    self.settings = default_settings(self.cfg.flavor);
                    self.print("[MSG:Restoring defaults]");
                }
                if matches!(what, "#" | "*") {
                    self.wcs = [[0.0; 3]; 6];
                    self.g92 = [0.0; 3];
                    self.wco_dirty = true;
                }
                if matches!(what, "$" | "#" | "*") {
                    self.reply_ok();
                } else {
                    self.reply_error(3);
                }
            }
            _ if cmd.starts_with("$N") && cmd.contains('=') => self.reply_ok(),
            _ => self.reply_error(3),
        }
        LineOutcome::Done
    }

    fn write_setting(&mut self, n: u16, v: &str) -> Result<(), u16> {
        if !self.settings.contains_key(&n) {
            return Err(3);
        }
        let v: f64 = v.trim().parse().map_err(|_| 2u16)?;
        if v < 0.0 {
            return Err(4);
        }
        if n == 0 && v < 3.0 {
            return Err(6);
        }
        if n == 20 && v >= 1.0 && self.settings.get(&22).copied().unwrap_or(0.0) < 1.0 {
            return Err(10);
        }
        self.settings.insert(n, v);
        Ok(())
    }

    fn workspace(&self, axis: usize) -> (f64, f64) {
        let travel = self.settings.get(&(130 + axis as u16)).copied().unwrap_or(0.0);
        if self.cfg.force_set_origin { (0.0, travel) } else { (-travel, 0.0) }
    }

    fn soft_limits_on(&self) -> bool {
        self.settings.get(&20).copied().unwrap_or(0.0) >= 1.0
    }

    fn outside_travel(&self, t: [f64; 3]) -> bool {
        (0..3).any(|i| {
            let (lo, hi) = self.workspace(i);
            t[i] < lo - 1e-6 || t[i] > hi + 1e-6
        })
    }

    fn parse_jog(&self, body: &str) -> Result<Block, u16> {
        if !matches!(self.state, State::Idle | State::Jog) {
            return Err(if self.state == State::Alarm { 9 } else { 8 });
        }
        let mut target: [Option<f64>; 3] = [None; 3];
        let mut feed = None;
        let mut relative = !self.absolute;
        let mut inches = self.inches;
        let mut machine = false;
        for (l, v) in words(body) {
            let num: f64 = v.parse().map_err(|_| 2u16)?;
            match l {
                'X' | 'Y' | 'Z' => {
                    let i = (l as u8 - b'X') as usize;
                    if target[i].is_some() {
                        return Err(25);
                    }
                    target[i] = Some(num);
                }
                'F' => feed = Some(num),
                'G' => match v.as_str() {
                    "20" => inches = true,
                    "21" => inches = false,
                    "90" => relative = false,
                    "91" => relative = true,
                    "53" => machine = true,
                    _ => return Err(16),
                },
                'N' => {}
                _ => return Err(16),
            }
        }
        let feed = feed.ok_or(22u16)?;
        if target.iter().all(Option::is_none) {
            return Err(16);
        }
        let k = if inches { 25.4 } else { 1.0 };
        let base = self.gc_pos;
        let wco = self.wco();
        let mut t = base;
        for i in 0..3 {
            if let Some(v) = target[i] {
                t[i] = if relative {
                    base[i] + v * k
                } else if machine {
                    v * k
                } else {
                    v * k + wco[i]
                };
            }
        }
        if self.soft_limits_on() && self.outside_travel(t) {
            return Err(15);
        }
        let power = self.jog_power();
        Ok(Block { target: t, feed: feed * k, rapid: false, jog: true, power, dwell: 0.0 })
    }

    /// Jogs behave like G1, so an enabled laser fires while jogging (documented GRBL caution).
    fn jog_power(&self) -> f64 {
        if self.spindle == Spindle::Off { 0.0 } else { self.s }
    }

    fn parse_gcode(&self, line: &str) -> Result<GLine, u16> {
        let mut g = GLine::default();
        let mut seen = String::new();
        for (l, v) in words(line) {
            if !l.is_ascii_alphabetic() {
                return Err(1);
            }
            if v.is_empty() {
                return Err(2);
            }
            let num: f64 = v.parse().map_err(|_| 2u16)?;
            match l {
                'G' => {
                    let code = (num * 10.0).round() as i32;
                    match code {
                        0 | 10 | 20 | 30 => {
                            if g.motion.is_some() {
                                return Err(21);
                            }
                            g.motion = Some((code / 10) as u8);
                        }
                        40 | 100 | 280 | 300 | 920 | 921 | 281 | 301 => {
                            if g.non_modal.is_some() {
                                return Err(21);
                            }
                            g.non_modal = Some(code);
                        }
                        170 | 180 | 190 | 940 | 400 | 490 | 800 | 610 | 640 => {}
                        200 => g.units = Some(true),
                        210 => g.units = Some(false),
                        900 => g.absolute = Some(true),
                        910 => g.absolute = Some(false),
                        530 => g.g53 = true,
                        540..=590 if code % 10 == 0 => g.wcs = Some(((code - 540) / 10) as usize),
                        _ => return Err(20),
                    }
                }
                'M' => match num as i32 {
                    3 => g.spindle = Some(Spindle::Cw),
                    4 => g.spindle = Some(Spindle::Ccw),
                    5 => g.spindle = Some(Spindle::Off),
                    7 => g.coolant.push(7),
                    8 => g.coolant.push(8),
                    9 => g.coolant.push(9),
                    0 | 1 => {}
                    2 | 30 => g.program_end = true,
                    _ => return Err(20),
                },
                'X' | 'Y' | 'Z' | 'F' | 'S' | 'P' | 'L' | 'I' | 'J' | 'K' | 'R' | 'A' | 'T' => {
                    if seen.contains(l) {
                        return Err(25);
                    }
                    seen.push(l);
                    match l {
                        'X' => g.axes[0] = Some(num),
                        'Y' => g.axes[1] = Some(num),
                        'Z' => g.axes[2] = Some(num),
                        'F' => {
                            if num < 0.0 {
                                return Err(4);
                            }
                            g.f = Some(num)
                        }
                        'S' => {
                            if num < 0.0 {
                                return Err(4);
                            }
                            g.s = Some(num)
                        }
                        'P' => g.p = Some(num),
                        'L' => g.l = Some(num),
                        'I' | 'J' | 'K' | 'R' => g.arc_words = true,
                        'T' if num > 255.0 => return Err(38),
                        _ => {}
                    }
                }
                'N' => {
                    if !(0.0..=10_000_000.0).contains(&num) {
                        return Err(27);
                    }
                }
                _ => return Err(20),
            }
        }
        let has_axes = g.axes.iter().any(Option::is_some);
        let axes_used_otherwise = matches!(g.non_modal, Some(100 | 920 | 281 | 301));
        let modal = g.motion.unwrap_or(self.motion_mode);
        if matches!(g.non_modal, Some(280 | 300)) {
            g.has_motion = true;
        } else if has_axes && !axes_used_otherwise {
            g.has_motion = true;
            if modal >= 1 && g.f.is_none() && self.feed <= 0.0 {
                return Err(22);
            }
            if modal >= 2 && !g.arc_words {
                return Err(35);
            }
        } else if g.motion.is_some() && !has_axes && g.motion != Some(0) && g.motion != Some(1) {
            return Err(26);
        }
        if g.g53 && !(g.has_motion && modal <= 1) {
            return Err(30);
        }
        if g.non_modal == Some(40) && g.p.is_none() {
            return Err(28);
        }
        if g.non_modal == Some(100) && (g.l.is_none() || g.p.is_none()) {
            return Err(28);
        }
        if g.non_modal == Some(920) && !has_axes {
            return Err(26);
        }
        let spindle_change = g.spindle.is_some_and(|s| s != self.spindle);
        let s_change_no_laser = !self.laser_mode() && g.s.is_some_and(|s| s != self.s);
        g.sync = spindle_change || s_change_no_laser || !g.coolant.is_empty() || g.program_end || matches!(g.non_modal, Some(40 | 100 | 920 | 921));
        Ok(g)
    }

    /// Applies a parsed line. Returns false when the line raised an alarm (no `ok`).
    fn apply_gcode(&mut self, g: GLine) -> bool {
        if let Some(u) = g.units {
            self.inches = u;
        }
        if let Some(a) = g.absolute {
            self.absolute = a;
        }
        if let Some(w) = g.wcs {
            self.wcs_index = w;
            self.wco_dirty = true;
        }
        if let Some(m) = g.motion {
            self.motion_mode = m;
        }
        let k = if self.inches { 25.4 } else { 1.0 };
        if let Some(f) = g.f {
            self.feed = f * k;
        }
        if let Some(s) = g.s {
            self.s = s;
        }
        if let Some(sp) = g.spindle {
            self.spindle = sp;
        }
        for c in &g.coolant {
            match c {
                7 => self.mist = true,
                8 => self.flood = true,
                _ => {
                    self.mist = false;
                    self.flood = false;
                }
            }
        }
        let check = self.state == State::Check;
        match g.non_modal {
            Some(40) => {
                let secs = g.p.unwrap_or(0.0);
                if !check && secs > 0.0 {
                    self.planner.push_back(Block { target: self.gc_pos, feed: 1.0, rapid: false, jog: false, power: 0.0, dwell: secs });
                }
            }
            Some(100) => {
                let l = g.l.unwrap_or(0.0) as i32;
                let p = g.p.unwrap_or(0.0) as usize;
                let idx = if p == 0 { self.wcs_index } else { (p - 1).min(5) };
                for i in 0..3 {
                    if let Some(v) = g.axes[i] {
                        self.wcs[idx][i] = if l == 20 { self.gc_pos[i] - self.g92[i] - v * k } else { v * k };
                    }
                }
                self.wco_dirty = true;
            }
            Some(920) => {
                let w = self.wcs[self.wcs_index];
                for i in 0..3 {
                    if let Some(v) = g.axes[i] {
                        self.g92[i] = self.gc_pos[i] - w[i] - v * k;
                    }
                }
                self.wco_dirty = true;
            }
            Some(921) => {
                self.g92 = [0.0; 3];
                self.wco_dirty = true;
            }
            _ => {}
        }
        if g.has_motion {
            let mut t = self.gc_pos;
            if matches!(g.non_modal, Some(280 | 300)) {
                t = [0.0; 3];
            } else {
                let wco = self.wco();
                for i in 0..3 {
                    if let Some(v) = g.axes[i] {
                        t[i] = if g.g53 {
                            v * k
                        } else if self.absolute {
                            v * k + wco[i]
                        } else {
                            self.gc_pos[i] + v * k
                        };
                    }
                }
            }
            if self.soft_limits_on() && !check && self.outside_travel(t) {
                self.raise_alarm(2, true);
                return false;
            }
            let rapid = self.motion_mode == 0 || matches!(g.non_modal, Some(280 | 300));
            let fires = self.spindle != Spindle::Off && !(self.laser_mode() && rapid);
            if !check {
                self.planner.push_back(Block { target: t, feed: self.feed, rapid, jog: false, power: if fires { self.s } else { 0.0 }, dwell: 0.0 });
                if self.state == State::Idle {
                    self.state = State::Run;
                }
            }
            self.gc_pos = t;
        }
        if g.program_end {
            self.spindle = Spindle::Off;
            self.flood = false;
            self.mist = false;
            self.absolute = true;
            self.motion_mode = 1;
            self.print("[MSG:Pgm End]");
        }
        true
    }
}

enum LineOutcome {
    Wait,
    Done,
}

#[derive(Default)]
struct GLine {
    motion: Option<u8>,
    non_modal: Option<i32>,
    units: Option<bool>,
    absolute: Option<bool>,
    wcs: Option<usize>,
    g53: bool,
    axes: [Option<f64>; 3],
    f: Option<f64>,
    s: Option<f64>,
    p: Option<f64>,
    l: Option<f64>,
    arc_words: bool,
    spindle: Option<Spindle>,
    coolant: Vec<u8>,
    program_end: bool,
    has_motion: bool,
    sync: bool,
}

fn dist(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn emu() -> GrblEmulator {
        let mut e = GrblEmulator::new(EmulatorConfig::atomstack().with_time_scale(1.0));
        e.advance(Duration::from_millis(100));
        e.take_output();
        e
    }

    fn send(e: &mut GrblEmulator, s: &str) -> String {
        e.receive(s.as_bytes());
        String::from_utf8(e.take_output()).unwrap()
    }

    #[test]
    fn boots_with_banner() {
        let mut e = GrblEmulator::new(EmulatorConfig::atomstack());
        e.advance(Duration::from_millis(100));
        let out = String::from_utf8(e.take_output()).unwrap();
        assert!(out.contains("Grbl 1.1h ['$' for help]"), "{out}");
    }

    #[test]
    fn ok_and_errors() {
        let mut e = emu();
        assert_eq!(send(&mut e, "G21\n"), "ok\r\n");
        assert_eq!(send(&mut e, "G1 X1\n"), "error:22\r\n");
        assert_eq!(send(&mut e, "G99\n"), "error:20\r\n");
        assert_eq!(send(&mut e, "X1 X2\n"), "error:25\r\n");
        assert_eq!(send(&mut e, "$999=1\n"), "error:3\r\n");
        assert_eq!(send(&mut e, "$110=-5\n"), "error:4\r\n");
        assert_eq!(send(&mut e, "$H\n"), "error:5\r\n");
        assert_eq!(send(&mut e, "\n"), "ok\r\n");
    }

    #[test]
    fn motion_takes_time_and_reports_run() {
        let mut e = emu();
        assert_eq!(send(&mut e, "G1 X100 F6000\n"), "ok\r\n");
        let s = send(&mut e, "?");
        assert!(s.starts_with("<Run|MPos:"), "{s}");
        e.advance(Duration::from_millis(500)); // 50 mm at 100 mm/s
        let p = e.machine_position();
        assert!((p[0] - 50.0).abs() < 2.0, "{p:?}");
        e.advance(Duration::from_millis(600));
        assert!(send(&mut e, "?").starts_with("<Idle|MPos:100.000,0.000,0.000"));
    }

    #[test]
    fn hold_resume_and_reset_alarm() {
        let mut e = emu();
        send(&mut e, "G1 X100 F600\n");
        e.receive(b"!");
        assert!(send(&mut e, "?").starts_with("<Hold:1"));
        e.advance(Duration::from_millis(100));
        assert!(send(&mut e, "?").starts_with("<Hold:0"));
        e.receive(b"~");
        assert!(send(&mut e, "?").starts_with("<Run"));
        let out = send(&mut e, "\x18");
        assert!(out.contains("ALARM:3"), "{out}");
        assert!(out.contains("'$H'|'$X' to unlock"));
        assert_eq!(send(&mut e, "G0 X1\n"), "error:9\r\n");
        assert!(send(&mut e, "$X\n").contains("Unlocked"));
        assert_eq!(e.state_name(), "Idle");
    }

    #[test]
    fn rx_buffer_overflow_is_counted() {
        let mut e = emu();
        e.receive(b"G1 X400 F100\n"); // long move fills nothing but keeps Run
        let line = b"G1 X1 Y1\n";
        for _ in 0..60 {
            e.receive(line);
        }
        assert!(e.stats().rx_overflow > 0);
        assert!(e.stats().rx_peak <= 127);
    }

    #[test]
    fn laser_mode_fires_only_in_g1() {
        let mut e = emu();
        send(&mut e, "G0 X10 M3 S500\n");
        assert!(!e.laser_on());
        e.advance(Duration::from_secs(1));
        send(&mut e, "G1 X20 F600\n");
        e.advance(Duration::from_millis(100));
        assert!(e.laser_on());
        e.advance(Duration::from_secs(2));
        // M3 + G1 modal + idle = static fire (the fire button)
        assert!(e.laser_on());
        send(&mut e, "M5 S0\n");
        assert!(!e.laser_on());
    }

    #[test]
    fn settings_round_trip() {
        let mut e = emu();
        assert_eq!(send(&mut e, "$110=5000\n"), "ok\r\n");
        let out = send(&mut e, "$$\n");
        assert!(out.contains("$110=5000.000"));
        assert!(out.contains("$32=1"));
        assert!(out.ends_with("ok\r\n"));
    }

    #[test]
    fn jog_and_cancel() {
        let mut e = emu();
        assert_eq!(send(&mut e, "$J=G91 X100 F600\n"), "ok\r\n");
        assert!(send(&mut e, "?").starts_with("<Jog"));
        e.advance(Duration::from_millis(500));
        e.receive(&[rt::JOG_CANCEL]);
        assert!(send(&mut e, "?").starts_with("<Idle"));
        let x = e.machine_position()[0];
        assert!(x > 1.0 && x < 10.0, "{x}");
        assert_eq!(send(&mut e, "$J=G91 X1 S100 F100\n"), "error:16\r\n");
        assert_eq!(send(&mut e, "$J=G91 X1\n"), "error:22\r\n");
    }

    #[test]
    fn soft_limit_alarm_needs_reset() {
        let mut e = GrblEmulator::new(EmulatorConfig::atomstack().with_setting(22, 1.0).with_setting(20, 1.0).with_time_scale(1000.0));
        e.advance(Duration::from_millis(50));
        let boot = String::from_utf8(e.take_output()).unwrap();
        assert!(boot.contains("to unlock"));
        send(&mut e, "$H\n");
        e.advance(Duration::from_millis(10));
        assert!(String::from_utf8(e.take_output()).unwrap().contains("ok"));
        assert_eq!(send(&mut e, "$J=G90 X500 F1000\n"), "error:15\r\n");
        let out = send(&mut e, "G0 X500\n");
        assert!(out.contains("ALARM:2") && out.contains("Reset to continue"), "{out}");
        assert_eq!(send(&mut e, "G0 X1\n"), "", "ignored until reset");
        let out = send(&mut e, "\x18");
        assert!(out.contains("Grbl 1.1h"));
        assert_eq!(e.state_name(), "Alarm");
    }

    #[test]
    fn work_offsets() {
        let mut e = emu();
        send(&mut e, "G0 X10 Y20\n");
        e.advance(Duration::from_secs(1));
        send(&mut e, "G10 L20 P1 X0 Y0\n");
        assert_eq!(e.work_position()[..2], [0.0, 0.0]);
        assert_eq!(e.work_offset()[..2], [10.0, 20.0]);
        send(&mut e, "G10 L2 P1 X0 Y0\n");
        assert_eq!(e.work_offset()[..2], [0.0, 0.0]);
        send(&mut e, "G92 X5\n");
        assert_eq!(e.work_position()[0], 5.0);
        send(&mut e, "G92.1\n");
        assert_eq!(e.work_position()[0], 10.0);
    }

    #[test]
    fn check_mode() {
        let mut e = emu();
        assert!(send(&mut e, "$C\n").contains("[MSG:Enabled]"));
        assert_eq!(send(&mut e, "G0 X100\n"), "ok\r\n");
        e.advance(Duration::from_secs(1));
        assert_eq!(e.machine_position()[0], 0.0);
        let out = send(&mut e, "$C\n");
        assert!(out.contains("[MSG:Disabled]") && out.contains("Grbl"));
    }

    #[test]
    fn flavors() {
        for (f, banner) in
            [(EmulatorFlavor::GrblHal, "GrblHAL 1.1f"), (EmulatorFlavor::FluidNc, "FluidNC v3.9.7"), (EmulatorFlavor::Grbl09, "Grbl 0.9j")]
        {
            let mut e = GrblEmulator::new(EmulatorConfig::flavor(f));
            e.advance(Duration::from_millis(100));
            let out = String::from_utf8(e.take_output()).unwrap();
            assert!(out.contains(banner), "{out}");
        }
        let mut e = GrblEmulator::new(EmulatorConfig::flavor(EmulatorFlavor::Grbl09));
        e.advance(Duration::from_millis(100));
        e.take_output();
        assert!(send(&mut e, "?").starts_with("<Idle,MPos:"));
        assert!(send(&mut e, "G1 X1\n").starts_with("error: "));
    }
}
