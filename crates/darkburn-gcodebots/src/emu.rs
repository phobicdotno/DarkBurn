//! In-memory controllers for the tests: Smoothieware, Marlin, Snapmaker and a plain `ok`
//! device. They are written from the public protocol descriptions (see `ATTRIBUTION.md`) and
//! are only as exact as those descriptions; the uncertain details are listed in
//! `docs/hardware-checks-gcodebots.md`.
//!
//! Model: lines go into a planner of limited size. `ok` is sent when a line has been accepted
//! into the planner, so a full planner holds the `ok` back (that is the flow control). Motion
//! runs on a simulated clock that advances while the driver reads.

use std::collections::{BTreeMap, VecDeque};
use std::io;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use darkburn_grbl::Transport;

/// Which firmware an emulator imitates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmuFlavor {
    Smoothie,
    Marlin,
    Snapmaker,
    /// Answers `ok` to everything and moves; no status, no banner (a custom device).
    Plain,
}

/// How a Smoothieware `?` report is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SmoothieStatusStyle {
    /// `<Idle,MPos:0.0000,0.0000,0.0000,WPos:0.0000,0.0000,0.0000>`
    #[default]
    Comma,
    /// `<Idle|MPos:0.0000,0.0000,0.0000|WPos:0.0000,0.0000,0.0000|F:6000.0>`
    Pipe,
}

#[derive(Clone, Debug)]
pub struct EmuConfig {
    pub flavor: EmuFlavor,
    /// Lines the planner holds before `ok` is held back.
    pub planner_capacity: usize,
    /// Simulated seconds per real second.
    pub time_scale: f64,
    /// Travel X, Y, Z in mm (soft endstops on Marlin).
    pub travel: [f64; 3],
    /// Rapid speed, mm/min.
    pub rapid_feed: f64,
    pub smoothie_status: SmoothieStatusStyle,
    /// Fan index that drives the laser (Marlin/Snapmaker fan mode).
    pub laser_fan: Option<u8>,
    /// Print the start-up banner when the port opens.
    pub banner: bool,
}

impl EmuConfig {
    pub fn new(flavor: EmuFlavor) -> Self {
        Self {
            flavor,
            planner_capacity: 16,
            time_scale: 20.0,
            travel: [300.0, 200.0, 50.0],
            rapid_feed: 12_000.0,
            smoothie_status: SmoothieStatusStyle::Comma,
            laser_fan: None,
            banner: true,
        }
    }
    pub fn with_time_scale(mut self, s: f64) -> Self {
        self.time_scale = s;
        self
    }
    pub fn with_planner(mut self, n: usize) -> Self {
        self.planner_capacity = n;
        self
    }
    pub fn with_laser_fan(mut self, tool: u8) -> Self {
        self.laser_fan = Some(tool);
        self
    }
    pub fn with_status_style(mut self, s: SmoothieStatusStyle) -> Self {
        self.smoothie_status = s;
        self
    }
}

/// What a test can inspect.
#[derive(Clone, Debug, Default)]
pub struct EmuStats {
    /// Every command line received (comments stripped, as text).
    pub lines: Vec<String>,
    /// Distance travelled with the laser powered during `G1` moves, mm.
    pub burned_mm: f64,
    /// Distance travelled with the laser powered during `G0` moves, mm (a driver or translator bug).
    pub rapid_burn_mm: f64,
    pub max_planner_len: usize,
    pub status_requests: usize,
    pub resets: usize,
    pub homes: usize,
    pub max_power: f64,
}

#[derive(Clone, Debug)]
enum Item {
    Move { to: [f64; 3], feed: f64, rapid: bool, power: f64 },
    Home,
    Dwell(f64),
    Sync,
}

#[derive(Clone, Debug)]
struct Queued {
    item: Item,
    ack: bool,
}

struct Exec {
    item: Item,
    from: [f64; 3],
    total: f64,
    left: f64,
    ack: bool,
}

/// An in-memory controller. See the module docs.
pub struct LineEmulator {
    cfg: EmuConfig,
    out: Vec<u8>,
    rx: Vec<u8>,
    inbox: VecDeque<String>,
    planner: VecDeque<Queued>,
    exec: Option<Exec>,
    mpos: [f64; 3],
    lpos: [f64; 3],
    offset: [f64; 3],
    abs: bool,
    motion: u8,
    feed: f64,
    s: f64,
    /// M3/M4 active.
    laser_enabled: bool,
    inline: bool,
    fan: [f64; 4],
    air: bool,
    fire_pct: Option<f64>,
    paused: bool,
    halted: bool,
    homed: bool,
    soft_endstops: bool,
    unplugged: bool,
    dtr: bool,
    config: Vec<(String, String)>,
    marlin: BTreeMap<String, [f64; 3]>,
    stats: EmuStats,
    firing_now: bool,
}

fn strip(line: &str) -> String {
    let mut out = String::new();
    let mut depth = 0;
    for c in line.chars() {
        match c {
            '(' => depth += 1,
            ')' if depth > 0 => depth -= 1,
            ';' | '*' if depth == 0 => break,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out.trim().to_owned()
}

fn words(line: &str) -> Vec<(char, f64, bool)> {
    // (letter, value, had_value)
    let mut out = Vec::new();
    let mut it = line.chars().peekable();
    while let Some(&c) = it.peek() {
        if c.is_ascii_alphabetic() {
            it.next();
            let mut v = String::new();
            while let Some(&d) = it.peek() {
                if d.is_ascii_digit() || matches!(d, '.' | '-' | '+') {
                    v.push(d);
                    it.next();
                } else {
                    break;
                }
            }
            out.push((c.to_ascii_uppercase(), v.parse().unwrap_or(0.0), !v.is_empty()));
        } else {
            it.next();
        }
    }
    out
}

impl LineEmulator {
    pub fn new(cfg: EmuConfig) -> Self {
        let mut marlin = BTreeMap::new();
        marlin.insert("M92".into(), [80.0, 80.0, 400.0]);
        marlin.insert("M203".into(), [300.0, 300.0, 5.0]);
        marlin.insert("M201".into(), [1000.0, 1000.0, 100.0]);
        marlin.insert("M206".into(), [0.0, 0.0, 0.0]);
        let config = [
            ("alpha_steps_per_mm", "80"),
            ("beta_steps_per_mm", "80"),
            ("gamma_steps_per_mm", "1600"),
            ("alpha_max_travel", "300"),
            ("beta_max_travel", "200"),
            ("laser_module_enable", "true"),
            ("laser_module_pwm_pin", "2.5"),
            ("laser_module_max_power", "1.0"),
            ("laser_module_minimum_power", "0.0"),
        ]
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
        Self {
            cfg,
            out: Vec::new(),
            rx: Vec::new(),
            inbox: VecDeque::new(),
            planner: VecDeque::new(),
            exec: None,
            mpos: [0.0; 3],
            lpos: [0.0; 3],
            offset: [0.0; 3],
            abs: true,
            motion: 0,
            feed: 1000.0,
            s: 0.0,
            laser_enabled: false,
            inline: false,
            fan: [0.0; 4],
            air: false,
            fire_pct: None,
            paused: false,
            halted: false,
            homed: false,
            soft_endstops: true,
            unplugged: false,
            dtr: true,
            config,
            marlin,
            stats: EmuStats::default(),
            firing_now: false,
        }
    }

    // ----- inspection -----

    pub fn stats(&self) -> &EmuStats {
        &self.stats
    }
    pub fn machine_position(&self) -> [f64; 3] {
        self.mpos
    }
    pub fn work_position(&self) -> [f64; 3] {
        [self.mpos[0] - self.offset[0], self.mpos[1] - self.offset[1], self.mpos[2] - self.offset[2]]
    }
    pub fn is_halted(&self) -> bool {
        self.halted
    }
    pub fn is_paused(&self) -> bool {
        self.paused
    }
    pub fn air_on(&self) -> bool {
        self.air
    }
    pub fn fan_value(&self, tool: usize) -> f64 {
        self.fan.get(tool).copied().unwrap_or(0.0)
    }
    pub fn planner_len(&self) -> usize {
        self.planner.len() + usize::from(self.exec.is_some())
    }
    pub fn homed(&self) -> bool {
        self.homed
    }
    pub fn config_value(&self, key: &str) -> Option<&str> {
        self.config.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }
    pub fn marlin_setting(&self, code: &str) -> Option<[f64; 3]> {
        self.marlin.get(code).copied()
    }
    pub fn soft_endstops(&self) -> bool {
        self.soft_endstops
    }
    pub fn dtr(&self) -> bool {
        self.dtr
    }
    /// True when the laser is emitting right now (manual fire or a powered move running).
    pub fn laser_on(&self) -> bool {
        self.fire_pct.is_some_and(|p| p > 0.0) || self.firing_now
    }
    /// True when the laser would fire on the next powered move (`M3`/`M4` active with power).
    pub fn laser_armed(&self) -> bool {
        self.power_now(true) > 0.0
    }
    pub fn is_unplugged(&self) -> bool {
        self.unplugged
    }
    pub fn unplug(&mut self) {
        self.unplugged = true;
    }
    pub fn set_dtr(&mut self, level: bool) {
        self.dtr = level;
    }
    /// Power-cycles the controller (the only way out of a Marlin `M112`).
    pub fn power_cycle(&mut self) {
        let cfg = self.cfg.clone();
        let stats = std::mem::take(&mut self.stats);
        *self = Self::new(cfg);
        self.stats = stats;
        self.stats.resets += 1;
        self.banner();
    }
    pub fn port_opened(&mut self) {
        self.banner();
    }

    fn banner(&mut self) {
        if !self.cfg.banner {
            return;
        }
        match self.cfg.flavor {
            EmuFlavor::Smoothie => self.say("Smoothie\nok"),
            EmuFlavor::Marlin => self.say("start\necho:Marlin 2.1.2\necho: Last Updated: 2023-11-01 | Author: (none, default config)\necho:Compiled: Nov  1 2023\necho:Free Memory: 5000  PlannerBufferBytes: 1200"),
            EmuFlavor::Snapmaker => self.say("start\necho:Snapmaker 2.0"),
            EmuFlavor::Plain => {}
        }
    }

    pub fn take_output(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.out)
    }

    fn say(&mut self, text: &str) {
        let eol = if self.cfg.flavor == EmuFlavor::Smoothie { "\r\n" } else { "\n" };
        for l in text.split('\n') {
            self.out.extend_from_slice(l.as_bytes());
            self.out.extend_from_slice(eol.as_bytes());
        }
    }

    fn ok(&mut self) {
        self.say("ok");
    }

    // ----- input -----

    pub fn receive(&mut self, data: &[u8]) {
        for &b in data {
            match b {
                b'?' if self.cfg.flavor == EmuFlavor::Smoothie => self.status_report(),
                0x18 if self.cfg.flavor == EmuFlavor::Smoothie => self.smoothie_reset(),
                b'\n' | b'\r' => {
                    let line = String::from_utf8_lossy(&self.rx).into_owned();
                    self.rx.clear();
                    let line = line.trim();
                    if !line.is_empty() {
                        self.inbox.push_back(line.to_owned());
                    }
                }
                _ => self.rx.push(b),
            }
        }
        self.process_inbox();
    }

    fn smoothie_reset(&mut self) {
        self.planner.clear();
        self.exec = None;
        self.inbox.clear();
        self.fire_pct = None;
        self.laser_enabled = false;
        self.firing_now = false;
        self.paused = false;
        self.halted = true;
        self.lpos = self.mpos;
        self.stats.resets += 1;
        self.say("ALARM: Kill button pressed - reset or M999 to continue");
    }

    fn status_report(&mut self) {
        self.stats.status_requests += 1;
        let state = if self.halted {
            "Alarm"
        } else if self.paused {
            "Hold"
        } else if self.exec.as_ref().is_some_and(|e| matches!(e.item, Item::Home)) {
            "Home"
        } else if self.exec.is_some() {
            "Run"
        } else {
            "Idle"
        };
        let m = self.mpos;
        let w = self.work_position();
        let text = match self.cfg.smoothie_status {
            SmoothieStatusStyle::Comma => format!("<{state},MPos:{:.4},{:.4},{:.4},WPos:{:.4},{:.4},{:.4}>", m[0], m[1], m[2], w[0], w[1], w[2]),
            SmoothieStatusStyle::Pipe => {
                format!("<{state}|MPos:{:.4},{:.4},{:.4}|WPos:{:.4},{:.4},{:.4}|F:{:.1}>", m[0], m[1], m[2], w[0], w[1], w[2], self.feed)
            }
        };
        self.say(&text);
    }

    fn process_inbox(&mut self) {
        while let Some(line) = self.inbox.front().cloned() {
            if self.try_line(&line) {
                self.inbox.pop_front();
            } else {
                break;
            }
        }
    }

    /// Runs one line. Returns false when the planner is full and the line must wait.
    fn try_line(&mut self, raw: &str) -> bool {
        let line = strip(raw);
        if line.is_empty() {
            self.ok();
            return true;
        }
        let first = line.split_whitespace().next().unwrap_or("").to_ascii_lowercase();
        if self.cfg.flavor == EmuFlavor::Smoothie {
            // Console commands are handled even while halted.
            if self.smoothie_console(&line, &first) {
                self.stats.lines.push(line);
                return true;
            }
        }
        let up = line.to_ascii_uppercase();
        if self.halted {
            match self.cfg.flavor {
                EmuFlavor::Smoothie => {
                    if up.starts_with("$X") || up.starts_with("M999") {
                        self.halted = false;
                        self.stats.lines.push(line);
                        self.ok();
                    } else {
                        self.say("!!");
                    }
                }
                // A halted Marlin does not answer at all until it is reset.
                _ => {}
            }
            return true;
        }
        if up.starts_with('$') && self.cfg.flavor == EmuFlavor::Smoothie {
            self.stats.lines.push(line.clone());
            if up.starts_with("$H") {
                return self.enqueue(Item::Home, true, true);
            }
            self.ok();
            return true;
        }
        let w = words(&up);
        if w.is_empty() {
            self.stats.lines.push(line);
            self.ok();
            return true;
        }
        // Does this line need a planner slot?
        let has_axis = w.iter().any(|(l, _, _)| matches!(l, 'X' | 'Y' | 'Z'));
        let mcode = w.iter().find(|(l, _, _)| *l == 'M').map(|(_, v, _)| *v as u32);
        let non_move_g = w.iter().any(|(l, v, _)| *l == 'G' && *v == 92.0);
        let needs_slot = (has_axis && !non_move_g)
            || w.iter().any(|(l, v, _)| *l == 'G' && (*v == 4.0 || *v == 28.0 || (*v - 28.2).abs() < 1e-6))
            || mcode == Some(400);
        if needs_slot && self.planner.len() >= self.cfg.planner_capacity {
            return false;
        }
        self.stats.lines.push(line.clone());
        self.exec_words(&up, &w);
        true
    }

    fn enqueue(&mut self, item: Item, ack_on_done: bool, _from_line: bool) -> bool {
        if self.planner.len() >= self.cfg.planner_capacity {
            return false;
        }
        self.planner.push_back(Queued { item, ack: ack_on_done });
        self.stats.max_planner_len = self.stats.max_planner_len.max(self.planner.len());
        true
    }

    /// Effective laser power (0..1) for the next move, by flavor and mode.
    fn power_now(&self, g1: bool) -> f64 {
        if let Some(t) = self.cfg.laser_fan {
            let v = self.fan[usize::from(t).min(3)];
            return (v / 255.0).clamp(0.0, 1.0);
        }
        match self.cfg.flavor {
            EmuFlavor::Smoothie => {
                if g1 && self.laser_enabled {
                    self.s.clamp(0.0, 1.0)
                } else {
                    0.0
                }
            }
            _ => {
                if !self.laser_enabled {
                    0.0
                } else if self.inline && !g1 {
                    0.0
                } else {
                    (self.s / 255.0).clamp(0.0, 1.0)
                }
            }
        }
    }

    fn exec_words(&mut self, up: &str, w: &[(char, f64, bool)]) {
        let get = |l: char| w.iter().find(|(c, _, h)| *c == l && *h).map(|(_, v, _)| *v);
        let mut handled_reply = false;
        for (l, v, _) in w {
            if *l == 'G' {
                match *v {
                    g if g == 20.0 || g == 21.0 => {}
                    g if g == 90.0 => self.abs = true,
                    g if g == 91.0 => self.abs = false,
                    g if g == 0.0 => self.motion = 0,
                    g if g == 1.0 => self.motion = 1,
                    g if g == 92.0 => {
                        for (i, a) in ['X', 'Y', 'Z'].iter().enumerate() {
                            if let Some(val) = get(*a) {
                                self.offset[i] = self.lpos[i] - val;
                            }
                        }
                        if !w.iter().any(|(c, _, _)| matches!(c, 'X' | 'Y' | 'Z')) {
                            self.offset = self.lpos;
                        }
                    }
                    g if (g - 92.1).abs() < 1e-6 => self.offset = [0.0; 3],
                    g if g == 4.0 => {
                        let secs = get('P').map_or(0.0, |ms| ms / 1000.0) + get('S').unwrap_or(0.0);
                        self.enqueue(Item::Dwell(secs), true, true);
                        handled_reply = true;
                    }
                    g if g == 28.0 || (g - 28.2).abs() < 1e-6 => {
                        self.enqueue(Item::Home, true, true);
                        handled_reply = true;
                    }
                    _ => {}
                }
            }
        }
        if let Some(f) = get('F') {
            if f > 0.0 {
                self.feed = f;
            }
        }
        if let Some(s) = get('S') {
            // S on M-codes is handled by the M-code itself; modal S for moves.
            if !up.contains("M106") && !up.contains("M67") {
                self.s = s;
            }
        }
        for (l, v, _) in w {
            if *l == 'M' {
                handled_reply |= self.m_code(*v as u32, up, w);
            }
        }
        let has_axis = w.iter().any(|(l, _, h)| matches!(l, 'X' | 'Y' | 'Z') && *h);
        let non_move_g = w.iter().any(|(l, v, _)| *l == 'G' && (*v == 92.0 || *v == 28.0 || (*v - 28.2).abs() < 1e-6 || *v == 4.0));
        if has_axis && !non_move_g {
            let g1 = self.motion == 1;
            let mut target = self.lpos;
            for (i, a) in ['X', 'Y', 'Z'].iter().enumerate() {
                if let Some(v) = get(*a) {
                    target[i] = if self.abs { v + self.offset[i] } else { self.lpos[i] + v };
                }
            }
            if self.soft_endstops && matches!(self.cfg.flavor, EmuFlavor::Marlin | EmuFlavor::Snapmaker) {
                for i in 0..3 {
                    target[i] = target[i].clamp(0.0, self.cfg.travel[i]);
                }
            }
            let power = self.power_now(g1);
            self.stats.max_power = self.stats.max_power.max(power);
            let feed = if g1 { self.feed } else { self.cfg.rapid_feed };
            self.lpos = target;
            self.enqueue(Item::Move { to: target, feed, rapid: !g1, power }, false, true);
            self.ok();
            return;
        }
        if !handled_reply {
            self.ok();
        }
    }

    /// Handles one M code. Returns true when it sent (or queued) its own reply.
    fn m_code(&mut self, m: u32, up: &str, w: &[(char, f64, bool)]) -> bool {
        if m == 400 {
            self.enqueue(Item::Sync, true, true);
            return true;
        }
        self.m_code_inner(m, up, w)
    }

    fn m_code_inner(&mut self, m: u32, up: &str, w: &[(char, f64, bool)]) -> bool {
        let get = |l: char| w.iter().find(|(c, _, h)| *c == l && *h).map(|(_, v, _)| *v);
        let has = |l: char| w.iter().any(|(c, _, _)| *c == l);
        let flavor = self.cfg.flavor;
        match m {
            3 | 4 => {
                self.laser_enabled = true;
                self.inline = has('I');
                if let Some(p) = get('P') {
                    // Snapmaker style percent power.
                    self.s = p / 100.0 * 255.0;
                }
                false
            }
            5 => {
                self.laser_enabled = false;
                if has('I') {
                    self.inline = false;
                }
                self.s = 0.0;
                false
            }
            7 | 8 => {
                self.air = true;
                false
            }
            9 => {
                self.air = false;
                false
            }
            106 => {
                let tool = get('P').unwrap_or(0.0) as usize;
                if let Some(slot) = self.fan.get_mut(tool) {
                    *slot = get('S').unwrap_or(255.0).clamp(0.0, 255.0);
                }
                if tool == 1 {
                    self.air = self.fan[1] > 0.0;
                } else if flavor == EmuFlavor::Smoothie && !has('P') {
                    self.air = true;
                }
                false
            }
            107 => {
                let tool = get('P').unwrap_or(0.0) as usize;
                if let Some(slot) = self.fan.get_mut(tool) {
                    *slot = 0.0;
                }
                if tool == 1 || flavor == EmuFlavor::Smoothie {
                    self.air = false;
                }
                false
            }
            114 => {
                let w = self.work_position();
                match flavor {
                    EmuFlavor::Smoothie => {
                        let m = self.mpos;
                        self.say(&format!("ok C: X:{:.4} Y:{:.4} Z:{:.4}", m[0], m[1], m[2]));
                        let _ = w;
                    }
                    _ => {
                        let m = self.mpos;
                        self.say(&format!(
                            "X:{:.2} Y:{:.2} Z:{:.2} E:0.00 Count X:{} Y:{} Z:{}\nok",
                            w[0],
                            w[1],
                            w[2],
                            (m[0] * 80.0) as i64,
                            (m[1] * 80.0) as i64,
                            (m[2] * 400.0) as i64
                        ));
                    }
                }
                true
            }
            115 if flavor != EmuFlavor::Smoothie => {
                let name = if flavor == EmuFlavor::Snapmaker { "Snapmaker 2.0" } else { "Marlin 2.1.2 (Github)" };
                self.say(&format!(
                    "FIRMWARE_NAME:{name} SOURCE_CODE_URL:github.com/MarlinFirmware/Marlin PROTOCOL_VERSION:1.0 MACHINE_TYPE:Laser EXTRUDER_COUNT:0\nCap:EEPROM:1\nCap:AUTOREPORT_POS:0\nCap:SERIAL_XON_XOFF:0\nok"
                ));
                true
            }
            105 if flavor != EmuFlavor::Smoothie => {
                self.say("ok T:25.0 /0.0 B:25.0 /0.0");
                true
            }
            410 => {
                self.planner.clear();
                self.exec = None;
                self.firing_now = false;
                self.lpos = self.mpos;
                self.ok();
                true
            }
            112 => {
                self.planner.clear();
                self.exec = None;
                self.firing_now = false;
                self.fire_pct = None;
                self.laser_enabled = false;
                self.halted = true;
                if flavor == EmuFlavor::Smoothie {
                    self.say("ALARM: Kill button pressed - reset or M999 to continue\nok");
                } else {
                    self.say("Error:Printer halted. kill() called!");
                }
                true
            }
            999 => {
                self.halted = false;
                self.ok();
                true
            }
            211 => {
                if let Some(s) = get('S') {
                    self.soft_endstops = s != 0.0;
                    self.ok();
                } else {
                    let t = self.cfg.travel;
                    self.say(&format!(
                        "Soft endstops: {}  Min: X0.00 Y0.00 Z0.00  Max: X{:.2} Y{:.2} Z{:.2}\nok",
                        if self.soft_endstops { "ON" } else { "OFF" },
                        t[0],
                        t[1],
                        t[2]
                    ));
                }
                true
            }
            92 | 203 | 201 | 206 if flavor != EmuFlavor::Smoothie => {
                let key = format!("M{m}");
                let mut cur = self.marlin.get(&key).copied().unwrap_or([0.0; 3]);
                for (i, a) in ['X', 'Y', 'Z'].iter().enumerate() {
                    if let Some(v) = get(*a) {
                        cur[i] = v;
                    }
                }
                self.marlin.insert(key, cur);
                self.ok();
                true
            }
            503 if flavor != EmuFlavor::Smoothie => {
                let t = self.cfg.travel;
                let g = |k: &str| self.marlin.get(k).copied().unwrap_or([0.0; 3]);
                let (a, b, c, d) = (g("M92"), g("M203"), g("M201"), g("M206"));
                self.say(&format!(
                    "echo:; Linear Units:\necho:  G21 ; (mm)\necho:; Steps per unit:\necho:  M92 X{:.2} Y{:.2} Z{:.2}\necho:; Max feedrates (units/s):\necho:  M203 X{:.2} Y{:.2} Z{:.2}\necho:; Max Acceleration (units/s2):\necho:  M201 X{:.2} Y{:.2} Z{:.2}\necho:; Home offset:\necho:  M206 X{:.2} Y{:.2} Z{:.2}\necho:; Soft endstops\necho:  M211 S{} ; Min X0.00 Y0.00 Z0.00 ; Max X{:.2} Y{:.2} Z{:.2}\nok",
                    a[0], a[1], a[2], b[0], b[1], b[2], c[0], c[1], c[2], d[0], d[1], d[2],
                    u8::from(self.soft_endstops), t[0], t[1], t[2]
                ));
                true
            }
            500 if flavor != EmuFlavor::Smoothie => {
                self.say("echo:Settings Stored (312 bytes; crc 41233)\nok");
                true
            }
            501 if flavor != EmuFlavor::Smoothie => {
                self.say("echo:Settings Loaded\nok");
                true
            }
            17 | 18 | 84 | 82 | 83 | 104 | 140 | 109 | 190 | 300 => {
                self.ok();
                true
            }
            _ => {
                if flavor == EmuFlavor::Smoothie {
                    // Smoothieware ignores what it does not know and still answers ok.
                    return false;
                }
                if !matches!(m, 3 | 4 | 5 | 7 | 8 | 9 | 106 | 107) {
                    self.say(&format!("echo:Unknown command: \"{}\"", up.split_whitespace().next().unwrap_or("")));
                }
                false
            }
        }
    }

    /// Smoothieware console commands. Returns true when the line was one.
    fn smoothie_console(&mut self, line: &str, first: &str) -> bool {
        match first {
            "version" => {
                self.say("Build version: edge-94de3d9, Build date: Oct 28 2014 13:24:47, MCU: LPC1769, System Clock: 120MHz\nok");
                true
            }
            "cat" => {
                let path = line.split_whitespace().nth(1).unwrap_or("");
                if path.ends_with("config.txt") {
                    let mut text = String::from("# Smoothieware Configuration file");
                    for (k, v) in &self.config {
                        text.push_str(&format!("\n{k} {v}"));
                    }
                    text.push_str("\nok");
                    self.say(&text);
                } else {
                    self.say(&format!("File not found: {path}\nok"));
                }
                true
            }
            "config-set" => {
                // config-set sd <key> <value>
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 4 {
                    let (k, v) = (parts[2].to_owned(), parts[3..].join(" "));
                    match self.config.iter_mut().find(|(ck, _)| *ck == k) {
                        Some(e) => e.1 = v.clone(),
                        None => self.config.push((k.clone(), v.clone())),
                    }
                    self.say(&format!("Setting {k} to {v} in config-override\nok"));
                } else {
                    self.say("error: usage: config-set [sd|flash] key value\nok");
                }
                true
            }
            "config-get" => {
                let parts: Vec<&str> = line.split_whitespace().collect();
                let key = parts.get(2).copied().unwrap_or("");
                match self.config_value(key).map(str::to_owned) {
                    Some(v) => self.say(&format!("sd: {key} is set to {v}\nok")),
                    None => self.say(&format!("sd: {key} is not in config\nok")),
                }
                true
            }
            "suspend" => {
                self.paused = true;
                self.say("Suspending, waiting for queue to empty\nok");
                true
            }
            "resume" => {
                self.paused = false;
                self.say("Resuming\nok");
                true
            }
            "fire" => {
                let arg = line.split_whitespace().nth(1).unwrap_or("").to_ascii_lowercase();
                if arg == "off" {
                    self.fire_pct = None;
                    self.say("turning laser off and returning to auto mode\nok");
                } else if let Ok(p) = arg.parse::<f64>() {
                    self.fire_pct = Some(p.clamp(0.0, 100.0));
                    self.say(&format!("Firing laser at {p:.0}% power, entering manual mode use fire off to return to auto mode\nok"));
                } else {
                    self.say("error: usage: fire power%|off\nok");
                }
                true
            }
            "help" | "ls" | "mem" => {
                self.say("ok");
                true
            }
            _ => false,
        }
    }

    // ----- time -----

    pub fn advance(&mut self, real: Duration) {
        let mut dt = real.as_secs_f64() * self.cfg.time_scale;
        let mut guard = 0;
        while dt > 0.0 && guard < 10_000 {
            guard += 1;
            self.process_inbox();
            if self.halted || self.paused {
                break;
            }
            if self.exec.is_none() {
                match self.planner.pop_front() {
                    Some(q) => {
                        let (total, from) = match &q.item {
                            Item::Move { to, feed, .. } => {
                                let d = ((to[0] - self.mpos[0]).powi(2) + (to[1] - self.mpos[1]).powi(2) + (to[2] - self.mpos[2]).powi(2)).sqrt();
                                (d / (feed / 60.0).max(0.001), self.mpos)
                            }
                            Item::Home => (0.4, self.mpos),
                            Item::Dwell(s) => (*s, self.mpos),
                            Item::Sync => (0.0, self.mpos),
                        };
                        self.exec = Some(Exec { item: q.item, from, total, left: total, ack: q.ack });
                    }
                    None => break,
                }
            }
            let Some(e) = self.exec.as_mut() else { break };
            let step = dt.min(e.left);
            e.left -= step;
            dt -= step;
            let frac = if e.total > 0.0 { 1.0 - e.left / e.total } else { 1.0 };
            let mut burned = 0.0;
            let mut rapid = false;
            let mut power = 0.0;
            if let Item::Move { to, feed, power: p, rapid: r } = &e.item {
                let prev = self.mpos;
                for i in 0..3 {
                    self.mpos[i] = e.from[i] + (to[i] - e.from[i]) * frac;
                }
                let d = ((self.mpos[0] - prev[0]).powi(2) + (self.mpos[1] - prev[1]).powi(2) + (self.mpos[2] - prev[2]).powi(2)).sqrt();
                let _ = feed;
                if *p > 0.0 {
                    burned = d;
                    power = *p;
                }
                rapid = *r;
            }
            if burned > 0.0 {
                if rapid {
                    self.stats.rapid_burn_mm += burned;
                } else {
                    self.stats.burned_mm += burned;
                }
            }
            self.firing_now = power > 0.0 && e.left > 0.0;
            if e.left <= 1e-12 {
                let done = self.exec.take();
                self.firing_now = false;
                if let Some(d) = done {
                    if matches!(d.item, Item::Home) {
                        self.mpos = [0.0; 3];
                        self.lpos = [0.0; 3];
                        self.homed = true;
                        self.stats.homes += 1;
                    }
                    if d.ack {
                        self.ok();
                    }
                }
            }
        }
        self.process_inbox();
    }
}

/// A shared handle to an emulator, like `darkburn_grbl::EmulatorHandle`.
#[derive(Clone)]
pub struct LineEmuHandle(Arc<Mutex<LineEmulator>>);

impl LineEmuHandle {
    pub fn new(emulator: LineEmulator) -> Self {
        Self(Arc::new(Mutex::new(emulator)))
    }
    pub fn with_config(cfg: EmuConfig) -> Self {
        Self::new(LineEmulator::new(cfg))
    }
    /// Opens a new in-memory link, like plugging in the cable.
    pub fn transport(&self) -> LineTransport {
        self.with(LineEmulator::port_opened);
        LineTransport { emu: self.clone(), last: Instant::now(), pending: Vec::new() }
    }
    pub fn with<R>(&self, f: impl FnOnce(&mut LineEmulator) -> R) -> R {
        let mut g = self.0.lock().unwrap_or_else(|p| p.into_inner());
        f(&mut g)
    }
}

/// In-memory link to a [`LineEmulator`].
pub struct LineTransport {
    emu: LineEmuHandle,
    last: Instant,
    pending: Vec<u8>,
}

impl LineTransport {
    fn pump(&mut self) -> io::Result<()> {
        let now = Instant::now();
        let dt = now - self.last;
        self.last = now;
        self.emu.with(|e| {
            if e.is_unplugged() {
                return Err(io::Error::new(io::ErrorKind::BrokenPipe, "emulator unplugged"));
            }
            e.advance(dt);
            self.pending.extend(e.take_output());
            Ok(())
        })
    }
}

impl Transport for LineTransport {
    fn write_all(&mut self, data: &[u8]) -> io::Result<()> {
        self.pump()?;
        self.emu.with(|e| e.receive(data));
        Ok(())
    }
    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> io::Result<usize> {
        let start = Instant::now();
        loop {
            self.pump()?;
            if !self.pending.is_empty() {
                let n = buf.len().min(self.pending.len());
                buf[..n].copy_from_slice(&self.pending[..n]);
                self.pending.drain(..n);
                return Ok(n);
            }
            let waited = start.elapsed();
            if waited >= timeout {
                return Ok(0);
            }
            std::thread::sleep((timeout - waited).min(Duration::from_millis(1)));
        }
    }
    fn set_dtr(&mut self, level: bool) -> io::Result<()> {
        self.emu.with(|e| e.set_dtr(level));
        Ok(())
    }
    fn name(&self) -> String {
        "line-emulator".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drain(e: &mut LineEmulator) -> String {
        String::from_utf8_lossy(&e.take_output()).into_owned()
    }

    #[test]
    fn smoothie_acks_and_status() {
        let mut e = LineEmulator::new(EmuConfig::new(EmuFlavor::Smoothie));
        e.port_opened();
        assert!(drain(&mut e).contains("Smoothie"));
        e.receive(b"G21\nG90\n");
        assert_eq!(drain(&mut e).matches("ok").count(), 2);
        e.receive(b"?");
        assert!(drain(&mut e).starts_with("<Idle,MPos:0.0000"));
        e.receive(b"version\n");
        assert!(drain(&mut e).contains("MCU: LPC1769"));
    }

    #[test]
    fn full_planner_holds_ok_back() {
        let mut e = LineEmulator::new(EmuConfig::new(EmuFlavor::Marlin).with_planner(2).with_time_scale(1.0));
        e.port_opened();
        drain(&mut e);
        for i in 1..=5 {
            e.receive(format!("G1 X{} F600\n", i * 10).as_bytes());
        }
        // Planner holds 2 and one is executing: not all five are acknowledged yet.
        let n = drain(&mut e).matches("ok").count();
        assert!(n < 5, "{n}");
        e.advance(Duration::from_secs(60));
        let n2 = drain(&mut e).matches("ok").count();
        assert_eq!(n + n2, 5);
    }

    #[test]
    fn marlin_m114_and_m503() {
        let mut e = LineEmulator::new(EmuConfig::new(EmuFlavor::Marlin));
        e.receive(b"M114\n");
        let r = drain(&mut e);
        assert!(r.starts_with("X:0.00 Y:0.00 Z:0.00 E:0.00 Count"), "{r}");
        e.receive(b"M92 X100\nM503\n");
        let r = drain(&mut e);
        assert!(r.contains("M92 X100.00 Y80.00"), "{r}");
        assert!(r.contains("M211 S1"));
    }

    #[test]
    fn marlin_m112_goes_silent_until_power_cycle() {
        let mut e = LineEmulator::new(EmuConfig::new(EmuFlavor::Marlin));
        e.receive(b"M112\n");
        assert!(drain(&mut e).contains("Printer halted"));
        e.receive(b"G1 X5\n");
        assert_eq!(drain(&mut e), "");
        e.power_cycle();
        e.receive(b"G1 X5\n");
        assert!(drain(&mut e).contains("ok"));
    }

    #[test]
    fn smoothie_fire_and_suspend() {
        let mut e = LineEmulator::new(EmuConfig::new(EmuFlavor::Smoothie));
        e.receive(b"fire 5\n");
        assert!(e.laser_on());
        e.receive(b"fire off\n");
        assert!(!e.laser_on());
        e.receive(b"suspend\n");
        assert!(e.is_paused());
        e.receive(b"resume\n");
        assert!(!e.is_paused());
    }

    #[test]
    fn rapid_with_fan_laser_on_is_counted_as_a_bug() {
        let mut e = LineEmulator::new(EmuConfig::new(EmuFlavor::Marlin).with_laser_fan(0));
        e.receive(b"M106 P0 S255\nG0 X10\n");
        e.advance(Duration::from_secs(5));
        assert!(e.stats().rapid_burn_mm > 9.0);
    }
}
