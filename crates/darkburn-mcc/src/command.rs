//! Typed writes to the MCC100: the only way to describe a write in this crate.
//!
//! [`SafeWrite`] has private fields. Every value of the type is produced either by one of the
//! constructors in this module or by [`SafeWrite::try_from_raw`], and both go through one
//! classifier ([`classify_write`]) that re-derives from the raw words what the write is. The
//! classifier is an allow-list: anything it does not recognise is [`Forbidden`].
//!
//! Hard limits enforced here (see `docs/BUILD-RULES.md`):
//!
//! * No write to registers 150 or 151, the licence / clock area 59500..=59599, hardware
//!   parameters, follower properties, register 100 or block 5000..=5012.
//! * No `0x65 <- [0x65]` and no `0x65 <- [0x66]`. (The owner's analysis allows `[101]` as a
//!   follower stop; the project rule is stricter, so this crate does not represent it.)
//! * No follower moves, pierce records, gas pressure writes, non-zero analogue outputs, laser
//!   enable, PWM with duty or any other laser data. Those writes are the vendor interlocks' job.
//!   The classifier recognises laser data so a simulator can model the vendor gate, but
//!   [`SafeWrite::try_from_raw`] and [`crate::Request::decode`] refuse it.
//! * No byte-block (0x26) or stream (0x20, 0x21) request can be built.
//!
//! Units follow the owner's analysis (`11-static-findings.md` section 2): speeds are `mm/s * K`, distances
//! `mm * K` (K = register 50017 = 1000, so micrometres), acceleration is whole `mm/s^2`, jerk is ten
//! times the acceleration. Conversions truncate toward zero like the vendor's `_ftol2`.

use crate::fifo::{self, FifoItem};
use crate::registers::{Forbidden, REG_COMMAND, REG_FIFO_CONTROL, REG_FIFO_DATA, plain_write_reason};

/// Sub-command 1: stop.
pub const CMD_STOP: u32 = 1;
/// Sub-command 2: home.
pub const CMD_HOME: u32 = 2;
/// Sub-command 3: jog / relative move (bit 31 of the axis word = absolute).
pub const CMD_JOG: u32 = 3;
/// Sub-command 5: go to point.
pub const CMD_GOTO: u32 = 5;
/// Sub-command 9999: miscellaneous (connect prologue, DO, DA, PWM).
pub const CMD_9999: u32 = 9999;
/// 9999 sub-command: connect prologue `[9999, 5, 0, 0]`.
pub const MISC_PROLOGUE: u32 = 5;
/// 9999 sub-command: digital output.
pub const MISC_DO: u32 = 2;
/// 9999 sub-command: PWM.
pub const MISC_PWM: u32 = 3;
/// 9999 sub-command: analogue output.
pub const MISC_DA: u32 = 4;
/// 9999 sub-command: PWM, 5 V variant.
pub const MISC_PWM_5V: u32 = 0x11;
/// FIFO control: clear.
pub const FIFO_CLEAR: u32 = 1;
/// FIFO control: start.
pub const FIFO_START: u32 = 2;
/// FIFO control: stop.
pub const FIFO_STOP: u32 = 3;
/// Axis mask for all five slots.
pub const AXIS_MASK_ALL: u32 = 0x1F;
/// Bit 31 of the axis word: absolute target (inferred, medium-high).
pub const ABSOLUTE_BIT: u32 = 0x8000_0000;

/// Arming level a write needs in the safety gate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Policy {
    /// Safety primitives: stop, outputs off, FIFO stop. Always allowed.
    Always,
    /// Connect prologue, any arming state.
    Connect,
    /// Needs motion armed.
    Motion,
    /// Needs motion armed and the axes homed.
    MotionHomed,
    /// Needs laser armed. No public constructor produces such a write.
    Laser,
}

/// Machine limits the classifier uses. Defaults describe the owner's CF1390 (1371 x 950 mm bed).
#[derive(Clone, Debug, PartialEq)]
pub struct SafetyConfig {
    /// DO ports that are laser hazards: switching them on needs the laser armed.
    pub laser_do_ports: Vec<u32>,
    /// DO ports for lamp and gas: switching them on needs motion armed.
    pub motion_do_ports: Vec<u32>,
    /// Axis-list indices a jog may address (0 X, 1 Y).
    pub jog_axis_indices: Vec<u32>,
    /// Slots that may be homed, one at a time.
    pub home_slots: Vec<u32>,
    /// Axis mask of the go-to vector (X and Y).
    pub goto_axis_mask: u32,
    /// Speed ceiling as a card word: 3000 mm/s * K.
    pub max_speed_word: u32,
    /// Acceleration ceiling in mm/s^2.
    pub max_acc: u32,
    /// Longest relative jog as a card word: 20 * 200 mm/s * K.
    pub max_jog_distance_word: u32,
    /// Stop deceleration word bounds.
    pub stop_vd_range: (u32, u32),
    /// Soft limits per axis-list index (X, Y) as inclusive card words (mm * K).
    pub soft_limits_word: [(i64, i64); 2],
}

impl Default for SafetyConfig {
    fn default() -> SafetyConfig {
        SafetyConfig {
            laser_do_ports: vec![5, 6, 9],
            motion_do_ports: vec![1, 2, 3, 7],
            jog_axis_indices: vec![0, 1],
            home_slots: vec![0, 1],
            goto_axis_mask: 0x3,
            max_speed_word: 3000 * 1000,
            max_acc: 20000,
            max_jog_distance_word: 4_000_000,
            stop_vd_range: (2000, 100_000),
            soft_limits_word: [(0, 1_371_000), (0, 950_000)],
        }
    }
}

impl SafetyConfig {
    fn port_mask(ports: &[u32]) -> u32 {
        ports.iter().filter(|p| (1..=16).contains(*p)).fold(0, |m, p| m | (1 << (p - 1)))
    }
}

/// Classifier output: the arming level a write needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Decision {
    /// Required level.
    pub policy: Policy,
    /// The axes must be homed.
    pub needs_homed: bool,
}

fn s32(w: u32) -> i64 {
    w as i32 as i64
}

fn deny<T>(msg: impl Into<String>) -> Result<T, Forbidden> {
    Err(Forbidden::Command(msg.into()))
}

fn soft_limit_check(axis: u32, target: i64, cfg: &SafetyConfig) -> Result<(), Forbidden> {
    let Some(&(lo, hi)) = cfg.soft_limits_word.get(axis as usize) else {
        return deny(format!("no soft limit for axis index {axis}"));
    };
    if target < lo || target > hi {
        return deny(format!("target {target} on axis {axis} outside soft limit [{lo}, {hi}]"));
    }
    Ok(())
}

fn do_policy(on_bits: u32, cfg: &SafetyConfig) -> Result<Decision, Forbidden> {
    let d = |policy| Ok(Decision { policy, needs_homed: false });
    if on_bits == 0 {
        return d(Policy::Always);
    }
    let laser = SafetyConfig::port_mask(&cfg.laser_do_ports);
    let motion = SafetyConfig::port_mask(&cfg.motion_do_ports);
    if on_bits & !(laser | motion) != 0 {
        return deny("DO port not in the allow-list");
    }
    d(if on_bits & laser != 0 { Policy::Laser } else { Policy::Motion })
}

fn classify_command(w: &[u32], cfg: &SafetyConfig, do_word: Option<u32>) -> Result<Decision, Forbidden> {
    let d = |policy| Ok(Decision { policy, needs_homed: false });
    let n = w.len();
    let Some(&sub) = w.first() else { return deny("empty command vector") };
    match sub {
        CMD_STOP => {
            if n == 2 && w[1] == AXIS_MASK_ALL {
                return d(Policy::Always);
            }
            let (lo, hi) = cfg.stop_vd_range;
            if n == 5 && w[1] > 0 && w[1] <= AXIS_MASK_ALL && w[2] == 2 && (lo..=hi).contains(&w[3]) && w[4] == w[3].wrapping_mul(10) {
                return d(Policy::Always);
            }
            deny("malformed stop vector (only [1,0x1F] or [1,mask,2,vd,10vd])")
        }
        CMD_HOME => {
            if n == 3 && w[2] == 0 && cfg.home_slots.iter().any(|s| w[1] == 1 << s) {
                return d(Policy::Motion);
            }
            deny("system, multi-axis and lift-table home are not supported")
        }
        CMD_JOG => {
            if n != 6 {
                return deny("jog vector must have 6 words");
            }
            let idx = w[1] & 0x7FFF_FFFF;
            let absolute = w[1] & ABSOLUTE_BIT != 0;
            if !cfg.jog_axis_indices.contains(&idx) {
                return deny(format!("jog axis word {:#x} not allowed", w[1]));
            }
            if w[2] == 0 || w[2] > cfg.max_speed_word {
                return deny("jog speed word out of range");
            }
            if w[3] == 0 || w[3] > cfg.max_acc || w[4] != w[3].wrapping_mul(10) {
                return deny("jog acc/jerk out of range or jerk != 10*acc");
            }
            let dist = s32(w[5]);
            if !absolute && dist.unsigned_abs() > cfg.max_jog_distance_word as u64 {
                return deny("jog distance beyond the continuous-jog length");
            }
            if absolute {
                soft_limit_check(idx, dist, cfg)?;
                return Ok(Decision { policy: Policy::MotionHomed, needs_homed: true });
            }
            d(Policy::Motion)
        }
        CMD_GOTO => {
            if n == 9
                && w[1] == (cfg.goto_axis_mask | ABSOLUTE_BIT)
                && w[2] > 0
                && w[2] <= cfg.max_speed_word
                && w[3] > 0
                && w[3] <= cfg.max_acc
                && w[4] == w[3].wrapping_mul(10)
                && w[7] == 0
                && w[8] == 0
            {
                soft_limit_check(0, s32(w[5]), cfg)?;
                soft_limit_check(1, s32(w[6]), cfg)?;
                return Ok(Decision { policy: Policy::MotionHomed, needs_homed: true });
            }
            deny("go-to vector not of the form [5,0x80000003,v,a,10a,x,y,0,0]")
        }
        // 101 (0x65) and 102 (0x66) are the two single-word commands the project rule forbids.
        101 | 102 => deny(format!("0x65 <- [{sub}] is never written")),
        CMD_9999 if n >= 2 => match w[1] {
            MISC_PROLOGUE => {
                if w == [9999, 5, 0, 0] {
                    d(Policy::Connect)
                } else {
                    deny("connect prologue must be exactly [9999,5,0,0]")
                }
            }
            MISC_DO if n == 4 => {
                let (mask, value) = (w[2], w[3]);
                if value & !mask != 0 {
                    return deny("DO value has bits outside the mask");
                }
                if mask == 0xFFFF {
                    let known = do_word.unwrap_or(0);
                    let newly_on = value & !known & 0xFFFF;
                    let allow = SafetyConfig::port_mask(&cfg.laser_do_ports) | SafetyConfig::port_mask(&cfg.motion_do_ports);
                    let dec = do_policy(newly_on | (value & allow), cfg)?;
                    return Ok(dec);
                }
                if mask & (mask - 1) != 0 || mask == 0 || mask > 0x200 {
                    return deny("DO write must address one port 1..10");
                }
                do_policy(value, cfg)
            }
            MISC_DA if n == 4 => {
                if w[2] > 1 {
                    return deny("DA channel word must be 0 or 1");
                }
                if w[3] == 0 { d(Policy::Always) } else { deny("non-zero analogue output is not supported") }
            }
            MISC_PWM | MISC_PWM_5V if n == 4 || n == 5 => {
                if w[3..].iter().all(|&x| x == 0) {
                    d(Policy::Always)
                } else {
                    d(Policy::Laser)
                }
            }
            other => deny(format!("9999 sub-command {other} not allowed")),
        },
        _ => deny(format!("0x65 sub-command {sub} not allowed")),
    }
}

/// Classify `WRITE addr <- words`. `do_word` is the last digital-output word read from register
/// 1005, used to decide whether a bulk DO write switches anything on.
pub fn classify_write(addr: u32, words: &[u32], cfg: &SafetyConfig, do_word: Option<u32>) -> Result<Decision, Forbidden> {
    match addr {
        REG_COMMAND => classify_command(words, cfg, do_word),
        REG_FIFO_CONTROL => match words {
            [FIFO_STOP] => Ok(Decision { policy: Policy::Always, needs_homed: false }),
            [FIFO_CLEAR] | [FIFO_START] => Ok(Decision { policy: Policy::Motion, needs_homed: false }),
            _ => Err(Forbidden::FifoControl),
        },
        REG_FIFO_DATA => {
            if words.is_empty() {
                return Err(Forbidden::FifoFrame("empty frame".into()));
            }
            let frame = fifo::parse_fifo_words(words).map_err(|e| Forbidden::FifoFrame(e.0))?;
            let policy = fifo::classify_frame(&frame)?;
            Ok(Decision { policy, needs_homed: false })
        }
        other => Err(plain_write_reason(other)),
    }
}

/// A validated write to the card. Cannot be constructed for a forbidden register or vector.
///
/// Two writes are equal when register and words are equal; the constructor name is a label.
#[derive(Clone, Debug)]
pub struct SafeWrite {
    register: u32,
    words: Vec<u32>,
    name: &'static str,
    policy: Policy,
    needs_homed: bool,
}

impl PartialEq for SafeWrite {
    fn eq(&self, other: &SafeWrite) -> bool {
        self.register == other.register && self.words == other.words
    }
}

impl Eq for SafeWrite {}

impl SafeWrite {
    /// Classify a raw write and wrap it. Laser data is refused here too: this crate cannot
    /// represent it.
    pub fn try_from_raw(register: u32, words: &[u32]) -> Result<SafeWrite, Forbidden> {
        let w = Self::from_raw_for_simulator(register, words, &SafetyConfig::default(), None)?;
        if w.policy == Policy::Laser {
            return Err(Forbidden::LaserData(format!("write to {register:#x} would enable or modulate the laser")));
        }
        Ok(w)
    }

    /// Classify a raw write including laser-class data. Used by the simulator so it can model the
    /// vendor gate; not exported.
    pub(crate) fn from_raw_for_simulator(register: u32, words: &[u32], cfg: &SafetyConfig, do_word: Option<u32>) -> Result<SafeWrite, Forbidden> {
        let d = classify_write(register, words, cfg, do_word)?;
        Ok(SafeWrite { register, words: words.to_vec(), name: "raw", policy: d.policy, needs_homed: d.needs_homed })
    }

    fn build(register: u32, words: Vec<u32>, name: &'static str) -> Result<SafeWrite, Forbidden> {
        let d = classify_write(register, &words, &SafetyConfig::default(), None)?;
        if d.policy == Policy::Laser {
            return Err(Forbidden::LaserData(name.into()));
        }
        Ok(SafeWrite { register, words, name, policy: d.policy, needs_homed: d.needs_homed })
    }

    /// Target register (0x65, 0x66 or 0x67).
    pub fn register(&self) -> u32 {
        self.register
    }

    /// The u32 words that follow `[0x40, register, n]` on the wire.
    pub fn words(&self) -> &[u32] {
        &self.words
    }

    /// Constructor name.
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// Arming level this write needs.
    pub fn policy(&self) -> Policy {
        self.policy
    }

    /// True if the axes must be homed first.
    pub fn needs_homed(&self) -> bool {
        self.needs_homed
    }

    /// The vendor log form `[func, addr, count, data...]`.
    pub fn request_vector(&self) -> Vec<u32> {
        let mut v = vec![0x40, self.register, self.words.len() as u32];
        v.extend_from_slice(&self.words);
        v
    }
}

/// Truncate toward zero like the vendor's `_ftol2`.
pub fn trunc_int(x: f64) -> Result<i64, Forbidden> {
    if !x.is_finite() || x.abs() > 2.0e9 {
        return deny(format!("cannot convert {x}"));
    }
    Ok(x.trunc() as i64)
}

fn word(v: i64) -> Result<u32, Forbidden> {
    if v < i32::MIN as i64 || v > i32::MAX as i64 {
        return deny(format!("value {v} does not fit a card word"));
    }
    Ok(v as i32 as u32)
}

/// Parameters the builders read. Defaults are the owner's machine (`BkManuPara.xml`, `BkHardPara.xml`).
/// `k` must be refreshed from register 50017 after connect; the card is authoritative.
#[derive(Clone, Debug, PartialEq)]
pub struct MachineParams {
    /// Card units per mm.
    pub k: i64,
    /// Fast jog speed, mm/s.
    pub jog_fast_speed: f64,
    /// Slow jog speed, mm/s.
    pub jog_slow_speed: f64,
    /// Use the fast speed.
    pub is_fast_mode: bool,
    /// Fast move acceleration, mm/s^2.
    pub x_fast_move_acc: f64,
    /// Fast move speed, mm/s.
    pub x_fast_move_speed: f64,
    /// Speed ceiling, mm/s.
    pub fcp_max_speed: f64,
    /// Acceleration ceiling, mm/s^2.
    pub fcp_max_acc: i64,
    /// Jog stop deceleration factor.
    pub jog_stop_dcc_factor: i64,
    /// Empty-move speed factor.
    pub empty_move_speed_factor: f64,
    /// Empty-move acceleration factor.
    pub empty_move_acc_factor: f64,
}

impl Default for MachineParams {
    fn default() -> MachineParams {
        MachineParams {
            k: 1000,
            jog_fast_speed: 200.0,
            jog_slow_speed: 50.0,
            is_fast_mode: true,
            x_fast_move_acc: 5999.99,
            x_fast_move_speed: 500.0,
            fcp_max_speed: 3000.0,
            fcp_max_acc: 20000,
            jog_stop_dcc_factor: 1,
            empty_move_speed_factor: 1.1,
            empty_move_acc_factor: 1.5,
        }
    }
}

impl MachineParams {
    fn jog_speed_acc(&self) -> Result<(u32, u32), Forbidden> {
        let speed = if self.is_fast_mode { self.jog_fast_speed } else { self.jog_slow_speed };
        let v = trunc_int(speed * self.k as f64)?.min(trunc_int(self.fcp_max_speed * self.k as f64)?);
        let a = trunc_int(self.x_fast_move_acc)?.min(self.fcp_max_acc);
        Ok((word(v)?, word(a)?))
    }
}

/// Connect prologue `0x65 <- [9999, 5, 0, 0]` (meaning unknown; the vendor always sends it).
pub fn connect_prologue() -> Result<SafeWrite, Forbidden> {
    SafeWrite::build(REG_COMMAND, vec![CMD_9999, MISC_PROLOGUE, 0, 0], "connect_prologue")
}

/// Held jog `[3, i, v, a, 10a, +-d]`, relative, `d = trunc(20 * JogFastSpeed * K)`. When
/// `soft_limit_remaining_mm` is given the distance is clipped to it.
pub fn jog_continuous(axis_index: u32, positive: bool, params: &MachineParams, soft_limit_remaining_mm: Option<f64>) -> Result<SafeWrite, Forbidden> {
    let (v, a) = params.jog_speed_acc()?;
    let mut dist = 20.0 * params.jog_fast_speed;
    if let Some(rem) = soft_limit_remaining_mm {
        dist = dist.min(rem).max(0.0);
    }
    let d = trunc_int(dist * params.k as f64)?;
    let d = if positive { d } else { -d };
    SafeWrite::build(REG_COMMAND, vec![CMD_JOG, axis_index, v, a, a.wrapping_mul(10), word(d)?], "jog_continuous")
}

/// Step jog `[3, i, v, a, 10a, trunc(dist * K)]`, relative, signed distance in mm.
pub fn jog_step(axis_index: u32, distance_mm: f64, params: &MachineParams) -> Result<SafeWrite, Forbidden> {
    let (v, a) = params.jog_speed_acc()?;
    let d = trunc_int(distance_mm * params.k as f64)?;
    SafeWrite::build(REG_COMMAND, vec![CMD_JOG, axis_index, v, a, a.wrapping_mul(10), word(d)?], "jog_step")
}

/// Single-axis absolute move `[3, i | 0x80000000, v, a, 10a, target]`. Needs homed axes.
pub fn move_axis_absolute(axis_index: u32, target_mm: f64, params: &MachineParams) -> Result<SafeWrite, Forbidden> {
    let (v, a) = params.jog_speed_acc()?;
    let t = trunc_int(target_mm * params.k as f64)?;
    SafeWrite::build(REG_COMMAND, vec![CMD_JOG, axis_index | ABSOLUTE_BIT, v, a, a.wrapping_mul(10), word(t)?], "move_axis_absolute")
}

/// Go-to `[5, 0x80000003, v, a, 10a, x*K, y*K, 0, 0]`. Needs homed axes.
pub fn go_to(x_mm: f64, y_mm: f64, params: &MachineParams) -> Result<SafeWrite, Forbidden> {
    let v = trunc_int(params.x_fast_move_speed * params.empty_move_speed_factor * params.k as f64)?
        .min(trunc_int(params.fcp_max_speed * params.k as f64)?);
    let a = trunc_int(params.x_fast_move_acc * params.empty_move_acc_factor)?.min(params.fcp_max_acc);
    let (v, a) = (word(v)?, word(a)?);
    SafeWrite::build(
        REG_COMMAND,
        vec![
            CMD_GOTO,
            0x3 | ABSOLUTE_BIT,
            v,
            a,
            a.wrapping_mul(10),
            word(trunc_int(x_mm * params.k as f64)?)?,
            word(trunc_int(y_mm * params.k as f64)?)?,
            0,
            0,
        ],
        "go_to",
    )
}

/// Home one axis `[2, 1 << slot, 0]`. Only slots 0 (X) and 1 (Y) are accepted.
pub fn home_axis(slot: u32) -> Result<SafeWrite, Forbidden> {
    SafeWrite::build(REG_COMMAND, vec![CMD_HOME, 1u32.checked_shl(slot).unwrap_or(0), 0], "home_axis")
}

/// Deceleration word of the stop vectors: `clamp(F * 100000 / (v_last / K), 2000, 100000)`.
/// `v_last_word <= 0` means 100000. Deviation from the vendor: a zero quotient uses the upper
/// clamp instead of dividing by zero.
pub fn stop_decel_word(v_last_word: i64, params: &MachineParams) -> u32 {
    let (lo, hi) = (2000i64, 100_000i64);
    let v = if v_last_word <= 0 { 100_000 } else { v_last_word };
    let q = v / params.k.max(1);
    let vd = if q == 0 { hi } else { (params.jog_stop_dcc_factor * 100_000) / q };
    vd.clamp(lo, hi) as u32
}

/// Jog key released: `[1, OR(1 << slot), 2, vd, 10 * vd]`.
pub fn jog_release_stop(slots: &[u32], v_last_word: i64, params: &MachineParams) -> Result<SafeWrite, Forbidden> {
    let mut mask = 0u32;
    for &s in slots {
        if s >= 5 {
            return deny(format!("slot {s} outside 0..4"));
        }
        mask |= 1 << s;
    }
    if mask == 0 {
        return deny("no axis to stop");
    }
    let vd = stop_decel_word(v_last_word, params);
    SafeWrite::build(REG_COMMAND, vec![CMD_STOP, mask, 2, vd, vd * 10], "jog_release_stop")
}

/// Stop all axes `[1, 0x1F, 2, vd, 10 * vd]`.
pub fn stop_all(v_last_word: i64, params: &MachineParams) -> Result<SafeWrite, Forbidden> {
    let vd = stop_decel_word(v_last_word, params);
    SafeWrite::build(REG_COMMAND, vec![CMD_STOP, AXIS_MASK_ALL, 2, vd, vd * 10], "stop_all")
}

/// Stop all axes with the card's default deceleration `[1, 0x1F]`.
pub fn stop_all_no_decel() -> Result<SafeWrite, Forbidden> {
    SafeWrite::build(REG_COMMAND, vec![CMD_STOP, AXIS_MASK_ALL], "stop_all_no_decel")
}

/// `0x67 <- [1]`.
pub fn fifo_clear() -> Result<SafeWrite, Forbidden> {
    SafeWrite::build(REG_FIFO_CONTROL, vec![FIFO_CLEAR], "fifo_clear")
}

/// `0x67 <- [2]`. Starting needs motion armed.
pub fn fifo_start() -> Result<SafeWrite, Forbidden> {
    SafeWrite::build(REG_FIFO_CONTROL, vec![FIFO_START], "fifo_start")
}

/// `0x67 <- [3]`. Always allowed.
pub fn fifo_stop() -> Result<SafeWrite, Forbidden> {
    SafeWrite::build(REG_FIFO_CONTROL, vec![FIFO_STOP], "fifo_stop")
}

/// One digital output off: `[9999, 2, 1 << (p-1), 0]` for the lamp, gas and laser ports.
pub fn do_off(port: u32) -> Result<SafeWrite, Forbidden> {
    if !(1..=10).contains(&port) {
        return deny(format!("DO port {port} outside 1..10"));
    }
    SafeWrite::build(REG_COMMAND, vec![CMD_9999, MISC_DO, 1 << (port - 1), 0], "do_off")
}

/// Bulk output write that only switches the listed ports off and keeps the rest of the word the
/// caller read from register 1005: `[9999, 2, 0xFFFF, keep]`.
pub fn do_bulk_off(current_do_word: u32, ports_off: &[u32]) -> Result<SafeWrite, Forbidden> {
    let mask = ports_off.iter().filter(|p| (1..=16).contains(*p)).fold(0u32, |m, p| m | (1 << (p - 1)));
    let keep = current_do_word & !mask & 0xFFFF;
    let d = classify_write(REG_COMMAND, &[CMD_9999, MISC_DO, 0xFFFF, keep], &SafetyConfig::default(), Some(current_do_word))?;
    if d.policy == Policy::Laser {
        return Err(Forbidden::LaserData("bulk output write keeps a laser port on".into()));
    }
    Ok(SafeWrite { register: REG_COMMAND, words: vec![CMD_9999, MISC_DO, 0xFFFF, keep], name: "do_bulk_off", policy: d.policy, needs_homed: false })
}

/// Analogue output to zero: `[9999, 4, channel - 1, 0]`.
pub fn da_zero(channel: u32) -> Result<SafeWrite, Forbidden> {
    if channel != 1 && channel != 2 {
        return deny("DA channel must be 1 or 2");
    }
    SafeWrite::build(REG_COMMAND, vec![CMD_9999, MISC_DA, channel - 1, 0], "da_zero")
}

/// PWM off: `[9999, 3 or 0x11, freq, 0, 0]`.
pub fn pwm_off(freq: u32, sub: u32) -> Result<SafeWrite, Forbidden> {
    if sub != MISC_PWM && sub != MISC_PWM_5V {
        return deny("PWM sub-command must be 3 or 0x11");
    }
    SafeWrite::build(REG_COMMAND, vec![CMD_9999, sub, freq, 0, 0], "pwm_off")
}

/// A FIFO frame `0x66 <- [frame_id, items...]` made of dry items. Items that carry laser data are
/// refused (none can be built through the public API anyway).
pub fn fifo_frame(frame_id: u32, items: &[FifoItem]) -> Result<SafeWrite, Forbidden> {
    let mut words = vec![frame_id];
    for i in items {
        words.extend(i.words());
    }
    SafeWrite::build(REG_FIFO_DATA, words, "fifo_frame")
}

/// A packed frame as a write.
pub fn fifo_packed_frame(frame_id: u32, frame: &fifo::PackedFrame) -> Result<SafeWrite, Forbidden> {
    SafeWrite::build(REG_FIFO_DATA, frame.payload(frame_id), "fifo_frame")
}

/// Stop sequence: FIFO stop if the FIFO runs, else stop-all; then outputs off. The follower stop
/// `[101]` of the vendor sequence is deliberately absent (project rule).
pub fn stop_sequence(
    params: &MachineParams,
    fifo_running: bool,
    v_last_word: i64,
    current_do_word: u32,
    laser_ports: &[u32],
) -> Result<Vec<SafeWrite>, Forbidden> {
    let mut seq = vec![if fifo_running { fifo_stop()? } else { stop_all(v_last_word, params)? }];
    seq.push(pwm_off(1234, MISC_PWM)?);
    seq.push(pwm_off(1234, MISC_PWM_5V)?);
    seq.push(da_zero(1)?);
    seq.push(da_zero(2)?);
    seq.push(do_bulk_off(current_do_word, laser_ports)?);
    Ok(seq)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p() -> MachineParams {
        MachineParams::default()
    }

    #[test]
    fn jog_vectors_match_the_logs() {
        // logged: 03 01 c350 176f ea56 3d0900 and the documented X+ / X- jogs
        let j = jog_continuous(0, true, &p(), None).unwrap();
        assert_eq!(j.words(), &[3, 0, 200_000, 5999, 59_990, 4_000_000]);
        let j = jog_continuous(1, false, &p(), None).unwrap();
        assert_eq!(j.words(), &[3, 1, 200_000, 5999, 59_990, (-4_000_000i32) as u32]);
        assert_eq!(j.policy(), Policy::Motion);
        let slow = MachineParams { is_fast_mode: false, ..p() };
        assert_eq!(jog_continuous(0, true, &slow, None).unwrap().words()[2], 50_000);
        let s = jog_step(0, 5.0, &MachineParams { is_fast_mode: false, ..p() }).unwrap();
        assert_eq!(s.words(), &[3, 0, 50_000, 5999, 59_990, 5000]);
        // soft-limit clip
        let c = jog_continuous(0, true, &p(), Some(12.5)).unwrap();
        assert_eq!(c.words()[5], 12_500);
    }

    #[test]
    fn go_to_reproduces_the_logged_vector() {
        // 05 80000003 86470 2327 15f86 69062 2a811 0 0
        let g = go_to(430.178, 174.097, &p()).unwrap();
        assert_eq!(g.words(), &[5, 0x8000_0003, 0x86470, 0x2327, 0x15f86, 0x69062, 0x2a811, 0, 0]);
        assert_eq!(g.policy(), Policy::MotionHomed);
        assert!(g.needs_homed());
        // outside the bed
        assert!(go_to(2000.0, 10.0, &p()).is_err());
        assert!(go_to(-1.0, 10.0, &p()).is_err());
        assert!(go_to(f64::NAN, 0.0, &p()).is_err());
    }

    #[test]
    fn stop_vectors() {
        // logs: 01 1f 02 7d0 4e20 and 01 01 02 7d0 4e20
        let s = stop_all(0, &p()).unwrap();
        assert_eq!(s.words(), &[1, 0x1f, 2, 2000, 20_000]); // logged: 01 1f 02 7d0 4e20
        let s = stop_all(200_000, &p()).unwrap();
        // 100000 / 200 = 500, clamped up to 2000
        assert_eq!(s.words(), &[1, 0x1f, 2, 2000, 20_000]);
        let r = jog_release_stop(&[0], 200_000, &p()).unwrap();
        assert_eq!(r.words(), &[1, 1, 2, 2000, 20_000]);
        assert_eq!(r.policy(), Policy::Always);
        assert_eq!(stop_all_no_decel().unwrap().words(), &[1, 0x1f]);
        assert!(jog_release_stop(&[], 0, &p()).is_err());
        assert!(jog_release_stop(&[5], 0, &p()).is_err());
        assert_eq!(stop_decel_word(50_000, &p()), 2000);
        assert_eq!(stop_decel_word(500, &p()), 100_000);
        assert_eq!(stop_decel_word(0, &p()), 2000);
        assert_eq!(stop_decel_word(-5, &p()), 2000);
    }

    #[test]
    fn homing_is_one_axis_x_or_y() {
        assert_eq!(home_axis(0).unwrap().words(), &[2, 1, 0]);
        assert_eq!(home_axis(1).unwrap().words(), &[2, 2, 0]);
        assert!(home_axis(2).is_err());
        assert!(home_axis(4).is_err());
        assert!(home_axis(40).is_err());
    }

    #[test]
    fn connect_and_fifo_control() {
        assert_eq!(connect_prologue().unwrap().words(), &[9999, 5, 0, 0]);
        assert_eq!(connect_prologue().unwrap().policy(), Policy::Connect);
        assert_eq!(fifo_stop().unwrap().policy(), Policy::Always);
        assert_eq!(fifo_start().unwrap().policy(), Policy::Motion);
        assert_eq!(fifo_clear().unwrap().register(), 0x67);
        assert_eq!(fifo_stop().unwrap().request_vector(), vec![0x40, 0x67, 1, 3]);
    }

    #[test]
    fn output_off_writes() {
        assert_eq!(do_off(9).unwrap().words(), &[9999, 2, 0x100, 0]);
        assert_eq!(do_off(1).unwrap().words(), &[9999, 2, 1, 0]);
        assert!(do_off(0).is_err());
        assert!(do_off(11).is_err());
        assert_eq!(da_zero(1).unwrap().words(), &[9999, 4, 0, 0]);
        assert!(da_zero(3).is_err());
        assert_eq!(pwm_off(1234, 0x11).unwrap().words(), &[9999, 0x11, 1234, 0, 0]);
        assert!(pwm_off(1234, 7).is_err());
        let b = do_bulk_off(0b1_0001_0101, &[5, 9]).unwrap();
        assert_eq!(b.words(), &[9999, 2, 0xFFFF, 0b0_0000_0101]);
        // keeping the laser gate on in a bulk write is not representable
        assert!(classify_write(0x65, &[9999, 2, 0xFFFF, 0x100], &SafetyConfig::default(), Some(0)).is_ok_and(|d| d.policy == Policy::Laser));
    }

    #[test]
    fn stop_sequence_has_no_follower_stop() {
        let seq = stop_sequence(&p(), false, 0, 0b1_0001_0000, &[5, 9]).unwrap();
        assert_eq!(seq[0].name(), "stop_all");
        assert!(seq.iter().all(|w| w.policy() == Policy::Always));
        assert!(seq.iter().all(|w| w.words() != [101]));
        let seq = stop_sequence(&p(), true, 0, 0, &[5, 9]).unwrap();
        assert_eq!(seq[0].name(), "fifo_stop");
    }

    #[test]
    fn forbidden_registers_cannot_be_constructed() {
        for reg in [
            150u32, 151, 59500, 59501, 59502, 59503, 59510, 59511, 59599, 59600, 59800, 11000, 11002, 100, 5000, 5001, 5008, 5012, 200, 201, 0, 1000,
            50000,
        ] {
            for words in [&[5555u32][..], &[9999], &[1, 2, 3], &[]] {
                assert!(SafeWrite::try_from_raw(reg, words).is_err(), "reg {reg} words {words:?}");
            }
        }
        // the specific forbidden command vectors
        for v in [&[0x65u32][..], &[0x66], &[101], &[102]] {
            let e = SafeWrite::try_from_raw(0x65, v).unwrap_err();
            assert!(matches!(e, Forbidden::Command(_)), "{v:?}: {e}");
        }
        assert!(matches!(SafeWrite::try_from_raw(150, &[5555]), Err(Forbidden::Register150Or151(150))));
        assert!(matches!(SafeWrite::try_from_raw(59502, &[1]), Err(Forbidden::LicenceArea(_))));
    }

    #[test]
    fn other_dangerous_vectors_are_refused() {
        let cases: &[&[u32]] = &[
            &[107],                                   // head calibration
            &[103, 1000, 0],                          // follower move
            &[104, 1, 2],                             // follower follow
            &[117, 1],                                // follower bookkeeping
            &[118, 5, 0],                             // pierce / bookkeeping
            &[9999, 13, 0xFFFF, 0],                   // extended DO
            &[9999, 16],                              // unexplained
            &[9999, 1, 0],                            // legacy clear
            &[9999, 4, 1, 5000],                      // non-zero DA
            &[9999, 2, 0x8000, 0x8000],               // DO outside the allow-list
            &[9999, 2, 0x300, 0x100],                 // two ports in one write
            &[9999, 2, 0x100, 0x200],                 // value outside mask
            &[2, 3, 0],                               // home X and Y together
            &[2, 0x10, 0],                            // lift table home
            &[3, 4, 50000, 4000, 40000, 1000000],     // lift table jog
            &[3, 0x8000_0010, 50000, 4000, 40000, 1], // roll feeder
            &[3, 0, 50000, 5999, 1, 1000],            // jerk != 10 acc
            &[3, 0, 5_000_000, 5999, 59990, 1000],    // speed above ceiling
            &[3, 0, 50000, 5999, 59990, 5_000_000],   // jog too long
            &[5, 0x3, 1, 1, 10, 0, 0, 0, 0],          // go-to without the absolute bit
            &[1, 0x1f, 2, 1, 10],                     // stop decel below range
            &[1, 0x1f, 2, 2000, 20001],               // jerk mismatch
            &[],
        ];
        for c in cases {
            assert!(SafeWrite::try_from_raw(0x65, c).is_err(), "{c:?} must be refused");
        }
        // absolute move outside the soft limits
        assert!(SafeWrite::try_from_raw(0x65, &[3, 0x8000_0000, 50000, 5999, 59990, 2_000_000]).is_err());
        assert!(SafeWrite::try_from_raw(0x65, &[3, 0x8000_0000, 50000, 5999, 59990, 100_000]).is_ok());
        assert!(SafeWrite::try_from_raw(0x67, &[4]).is_err());
        assert!(SafeWrite::try_from_raw(0x67, &[1, 2]).is_err());
    }

    #[test]
    fn laser_data_is_not_representable() {
        for v in [&[9999u32, 2, 0x100, 0x100][..], &[9999, 2, 0x10, 0x10], &[9999, 3, 1234, 10], &[9999, 0x11, 1234, 1, 0]] {
            let e = SafeWrite::try_from_raw(0x65, v).unwrap_err();
            assert!(matches!(e, Forbidden::LaserData(_)), "{v:?}: {e}");
            // but the simulator's classifier sees them and says they need the laser armed
            let w = SafeWrite::from_raw_for_simulator(0x65, v, &SafetyConfig::default(), None).unwrap();
            assert_eq!(w.policy(), Policy::Laser);
        }
        // a FIFO frame with a duty tick
        let frame = [1u32, 0x0008_0BB8, 0, (5000 << 16) | 4];
        assert!(matches!(SafeWrite::try_from_raw(0x66, &frame), Err(Forbidden::LaserData(_))));
        // zero duty is fine
        let frame = [1u32, 0x0008_0BB8, 0, 5000 << 16];
        assert!(SafeWrite::try_from_raw(0x66, &frame).is_ok());
    }

    #[test]
    fn fifo_frames_go_through_the_same_gate() {
        let w = fifo_frame(7, &[FifoItem::boundary(), FifoItem::tick(1, 2)]).unwrap();
        assert_eq!(w.register(), 0x66);
        assert_eq!(w.words()[0], 7);
        assert_eq!(w.policy(), Policy::Motion);
        assert_eq!(w.request_vector()[..3], [0x40, 0x66, 5]);
        assert!(SafeWrite::try_from_raw(0x66, &[]).is_err());
        assert!(SafeWrite::try_from_raw(0x66, &[1, 0xdead]).is_err());
    }

    #[test]
    fn every_constructor_reclassifies_to_itself() {
        let ws = vec![
            connect_prologue().unwrap(),
            jog_continuous(0, true, &p(), None).unwrap(),
            jog_step(1, -2.5, &p()).unwrap(),
            move_axis_absolute(0, 100.0, &p()).unwrap(),
            go_to(10.0, 20.0, &p()).unwrap(),
            home_axis(0).unwrap(),
            jog_release_stop(&[0, 1], 200_000, &p()).unwrap(),
            stop_all(0, &p()).unwrap(),
            stop_all_no_decel().unwrap(),
            fifo_clear().unwrap(),
            fifo_start().unwrap(),
            fifo_stop().unwrap(),
            do_off(5).unwrap(),
            do_bulk_off(0b1_0111_0101, &[5, 6, 9]).unwrap(),
            da_zero(2).unwrap(),
            pwm_off(1234, 3).unwrap(),
            fifo_frame(1, &[FifoItem::tick(0, 0)]).unwrap(),
        ];
        for w in ws {
            let again = SafeWrite::try_from_raw(w.register(), w.words()).unwrap_or_else(|e| panic!("{}: {e}", w.name()));
            assert_eq!(again.policy(), w.policy(), "{}", w.name());
            assert_eq!(again.needs_homed(), w.needs_homed(), "{}", w.name());
        }
    }

    #[test]
    fn conversions_are_checked() {
        assert!(jog_step(0, 1.0e12, &p()).is_err());
        assert!(jog_step(0, f64::INFINITY, &p()).is_err());
        assert_eq!(trunc_int(-2.9).unwrap(), -2);
        assert_eq!(trunc_int(2.009 * 1000.0).unwrap(), 2009);
        assert!(jog_step(2, 1.0, &p()).is_err(), "axis index 2 is not a jog axis");
    }
}
