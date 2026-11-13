//! Safety-gate model: arming state machine and PC-side checks in front of the card.
//!
//! This follows the owner's `safety.py` (arming levels, E-stop latch, alarm reactions, comm-loss
//! watchdog, soft limits on relative jogs, deadman lease) as a pure state model. It does no I/O
//! and reads no clock: callers pass `now_ms`. It models a gate *in front of the vendor's own
//! interlocks*; it does not replace and cannot bypass the height follower, pierce or gas logic,
//! which live inside the card and the follower unit.
//!
//! The arming levels are: [`ArmState::Disarmed`], [`ArmState::MotionArmed`], [`ArmState::LaserArmed`].
//! No write that this crate can build publicly needs the laser level; the level exists so the model
//! matches the owner's gate and so a simulator test can show that laser-class data is refused.

use crate::command::{CMD_STOP, FIFO_STOP, Policy, SafeWrite, SafetyConfig, classify_write};
use crate::registers::{REG_COMMAND, REG_FIFO_CONTROL, REG_FIFO_DATA, Status, StatusBlock, check_read};
use std::collections::BTreeMap;

/// Arming level. Each level includes the rights of the lower ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ArmState {
    /// Only safety primitives and the connect prologue.
    Disarmed,
    /// Motion and FIFO streaming.
    MotionArmed,
    /// Laser-class writes. Needs operator confirmation and the running job's token.
    LaserArmed,
}

/// An arming transition that the rules do not permit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArmingError(pub &'static str);

impl std::fmt::Display for ArmingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "arming refused: {}", self.0)
    }
}

impl std::error::Error for ArmingError {}

/// One recorded state change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transition {
    /// Old state.
    pub old: ArmState,
    /// New state.
    pub new: ArmState,
    /// Event name.
    pub event: String,
}

/// DISARMED -> MOTION_ARMED -> LASER_ARMED.
#[derive(Clone, Debug)]
pub struct ArmingStateMachine {
    state: ArmState,
    job_token: Option<String>,
    estop_latched: bool,
    jobs_started: u64,
    /// Every transition, oldest first.
    pub history: Vec<Transition>,
}

impl Default for ArmingStateMachine {
    fn default() -> Self {
        ArmingStateMachine::new()
    }
}

impl ArmingStateMachine {
    /// A disarmed machine.
    pub fn new() -> ArmingStateMachine {
        ArmingStateMachine { state: ArmState::Disarmed, job_token: None, estop_latched: false, jobs_started: 0, history: Vec::new() }
    }

    /// Current state.
    pub fn state(&self) -> ArmState {
        self.state
    }

    /// True from [`estop`](Self::estop) until [`acknowledge_estop`](Self::acknowledge_estop).
    pub fn estop_latched(&self) -> bool {
        self.estop_latched
    }

    /// Token of the running job.
    pub fn job_token(&self) -> Option<&str> {
        self.job_token.as_deref()
    }

    /// Whether `policy` (ignoring the homed condition) is permitted now.
    pub fn allows(&self, policy: Policy) -> bool {
        match policy {
            Policy::Always | Policy::Connect => true,
            _ if self.estop_latched => false,
            Policy::Laser => self.state >= ArmState::LaserArmed,
            Policy::Motion | Policy::MotionHomed => self.state >= ArmState::MotionArmed,
        }
    }

    fn set(&mut self, new: ArmState, event: &str) {
        self.history.push(Transition { old: self.state, new, event: event.to_string() });
        self.state = new;
    }

    /// DISARMED -> MOTION_ARMED. Refused while the E-stop latch is set.
    pub fn arm_motion(&mut self) -> Result<(), ArmingError> {
        if self.estop_latched {
            return Err(ArmingError("E-stop latched: acknowledge before arming"));
        }
        if self.state == ArmState::Disarmed {
            self.set(ArmState::MotionArmed, "arm_motion");
        }
        Ok(())
    }

    /// Create the "job running" token needed for laser arming. Needs MOTION_ARMED.
    pub fn begin_job(&mut self) -> Result<String, ArmingError> {
        if self.estop_latched || self.state == ArmState::Disarmed {
            return Err(ArmingError("begin_job needs MOTION_ARMED"));
        }
        if self.job_token.is_some() {
            return Err(ArmingError("a job is already running"));
        }
        self.jobs_started += 1;
        let token = format!("job-{}", self.jobs_started);
        self.job_token = Some(token.clone());
        Ok(token)
    }

    /// MOTION_ARMED -> LASER_ARMED: only with explicit confirmation and the current job token.
    pub fn arm_laser(&mut self, confirmed: bool, job_token: Option<&str>) -> Result<(), ArmingError> {
        if self.estop_latched {
            return Err(ArmingError("E-stop latched"));
        }
        if self.state == ArmState::Disarmed {
            return Err(ArmingError("arm_laser needs MOTION_ARMED"));
        }
        if !confirmed {
            return Err(ArmingError("laser arming needs an explicit operator confirmation"));
        }
        if self.job_token.is_none() || job_token != self.job_token.as_deref() {
            return Err(ArmingError("laser arming needs the running job's token"));
        }
        self.set(ArmState::LaserArmed, "arm_laser");
        Ok(())
    }

    /// Job finished: drop the token; LASER_ARMED -> MOTION_ARMED.
    pub fn end_job(&mut self, job_token: Option<&str>) -> Result<(), ArmingError> {
        if let Some(t) = job_token
            && Some(t) != self.job_token.as_deref()
        {
            return Err(ArmingError("unknown job token"));
        }
        self.job_token = None;
        if self.state == ArmState::LaserArmed {
            self.set(ArmState::MotionArmed, "end_job");
        }
        Ok(())
    }

    /// Stop / pause: the laser disarms, motion stays armed.
    pub fn on_stop(&mut self) {
        self.job_token = None;
        if self.state == ArmState::LaserArmed {
            self.set(ArmState::MotionArmed, "stop");
        }
    }

    /// Any state -> DISARMED (alarm, comm loss, operator).
    pub fn disarm(&mut self, event: &str) {
        self.job_token = None;
        if self.state != ArmState::Disarmed {
            self.set(ArmState::Disarmed, event);
        }
    }

    /// E-stop: disarm and latch.
    pub fn estop(&mut self) {
        self.estop_latched = true;
        self.disarm("estop");
    }

    /// Operator acknowledged the E-stop; the machine stays DISARMED.
    pub fn acknowledge_estop(&mut self) {
        self.estop_latched = false;
        self.history.push(Transition { old: self.state, new: self.state, event: "estop acknowledged".into() });
    }
}

/// Why the gate refused a write or read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GateRefusal {
    /// The arming level is too low (or the E-stop is latched).
    NeedsArming {
        /// Policy of the write.
        policy: Policy,
        /// State at the time.
        state: ArmState,
        /// E-stop latch.
        estop_latched: bool,
    },
    /// The write needs homed axes.
    NotHomed,
    /// A card alarm or restart-required condition is active.
    MachineFault(String),
    /// A relative jog would leave the soft limits.
    SoftLimit(String),
    /// Streaming refused: the status poll is stale.
    CommLoss,
    /// A jog, home, go-to or FIFO clear refused: the FIFO stream is running.
    StreamRunning,
    /// `0x67 <- [2]` refused: frames queued while the laser was armed.
    LaserFramesQueued,
    /// The read is not allowed.
    ReadNotAllowed(String),
    /// The write breaks a limit of this gate's own configuration (bed size, ceilings, ports).
    Limit(String),
    /// `0x67 <- [2]` refused: the card still holds frames of a job that was stopped. How a stopped
    /// job is continued is not known, so the queue has to be cleared first.
    QueueHeld,
}

impl std::fmt::Display for GateRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GateRefusal::NeedsArming { policy, state, estop_latched } => {
                write!(f, "needs {policy:?}, state {state:?}{}", if *estop_latched { ", E-stop latched" } else { "" })
            }
            GateRefusal::NotHomed => write!(f, "axes not homed"),
            GateRefusal::MachineFault(s) => write!(f, "refused while {s}"),
            GateRefusal::SoftLimit(s) => write!(f, "{s}"),
            GateRefusal::CommLoss => write!(f, "status poll stale: no streaming"),
            GateRefusal::StreamRunning => write!(f, "a job is streaming: stop it first"),
            GateRefusal::LaserFramesQueued => write!(f, "card FIFO holds frames queued while laser armed: clear it first"),
            GateRefusal::ReadNotAllowed(s) => write!(f, "{s}"),
            GateRefusal::Limit(s) => write!(f, "outside this machine's limits: {s}"),
            GateRefusal::QueueHeld => write!(f, "the card still holds a stopped job: clear the queue first"),
        }
    }
}

impl std::error::Error for GateRefusal {}

/// What the caller must do after the gate observed a status poll.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GateAction {
    /// Send the stop sequence now.
    Stop(String),
    /// The card reports an E-stop: the latch is set and the stop sequence should go out.
    Estop,
}

/// Gate limits and timings.
#[derive(Clone, Debug, PartialEq)]
pub struct GateConfig {
    /// Classifier limits (soft limits, ports, ceilings).
    pub safety: SafetyConfig,
    /// A relative jog still running after this long needs a keep-alive.
    pub deadman_timeout_ms: u64,
    /// A status poll older than this counts as comm loss.
    pub comm_loss_timeout_ms: u64,
}

impl Default for GateConfig {
    fn default() -> GateConfig {
        GateConfig { safety: SafetyConfig::default(), deadman_timeout_ms: 200, comm_loss_timeout_ms: 1000 }
    }
}

/// PC-side gate. Call [`check_write`](Self::check_write) before sending and
/// [`after_send`](Self::after_send) once the card acknowledged.
#[derive(Clone, Debug)]
pub struct SafetyGate {
    /// Configuration.
    pub config: GateConfig,
    /// Arming state machine.
    pub arming: ArmingStateMachine,
    /// Set from the axis homed bits.
    pub homed: bool,
    /// Axis-list index to position in card words, used for soft-limit checks of relative jogs.
    pub positions_word: BTreeMap<u32, i64>,
    /// Tracked from register 1019 and from `0x67` writes.
    pub fifo_running: bool,
    /// Frames with laser data went into the FIFO and it was not cleared since.
    pub card_fifo_has_laser: bool,
    /// Register 1005 from the last status read.
    pub last_do_word: Option<u32>,
    /// Time of the last good status poll.
    pub last_status_ms: Option<u64>,
    /// Active card alarm or restart-required condition.
    pub machine_fault: Option<String>,
    last_alarm: (u32, u32),
    leases: BTreeMap<u32, u64>,
    frames_since_clear: bool,
    queue_held: bool,
}

impl Default for SafetyGate {
    fn default() -> SafetyGate {
        SafetyGate::new(GateConfig::default())
    }
}

impl SafetyGate {
    /// A new, disarmed gate.
    pub fn new(config: GateConfig) -> SafetyGate {
        SafetyGate {
            config,
            arming: ArmingStateMachine::new(),
            homed: false,
            positions_word: BTreeMap::new(),
            fifo_running: false,
            card_fifo_has_laser: false,
            last_do_word: None,
            last_status_ms: None,
            machine_fault: None,
            last_alarm: (0, 0),
            leases: BTreeMap::new(),
            frames_since_clear: false,
            queue_held: false,
        }
    }

    /// True while the card holds frames of a job that was stopped (by the operator, an alarm or
    /// an emergency stop) and the queue was not cleared since. A start is refused until then.
    pub fn queue_held(&self) -> bool {
        self.queue_held
    }

    /// A stop was asked for or is going out. Frames queued since the last clear now belong to a
    /// stopped job.
    pub fn note_stop(&mut self) {
        if self.frames_since_clear {
            self.queue_held = true;
        }
    }

    /// Check a read against the allow-list.
    pub fn check_read(&self, addr: u32, count: u32) -> Result<(), GateRefusal> {
        check_read(addr, count).map_err(|e| GateRefusal::ReadNotAllowed(e.to_string()))
    }

    /// Feed a status block (a read of 1000/36 or longer) and react to alarms.
    pub fn observe_status(&mut self, now_ms: u64, status: &StatusBlock) -> Vec<GateAction> {
        self.last_status_ms = Some(now_ms);
        self.last_do_word = Some(status.word(Status::Do) & 0xFFFF);
        self.fifo_running = status.fifo_running();
        let cutting = self.fifo_running || self.arming.state() == ArmState::LaserArmed;
        let (a1_raw, a2) = (status.alarm1(), status.alarm2());
        // alarm1 == 0x01000000 alone is not an alarm in the vendor software; treated as one while
        // cutting until a capture settles it (unverified).
        let a1 = if a1_raw == 0x0100_0000 && !cutting { 0 } else { a1_raw };
        let mut faults = Vec::new();
        if a1 != 0 {
            faults.push(format!("card alarm_1 {a1_raw:#010x}"));
        }
        if a2 != 0 {
            faults.push(format!("card alarm_2 {a2:#010x}"));
        }
        if status.restart_required() {
            faults.push("reg 1031 == 1: card restart required".to_string());
        }
        self.machine_fault = if faults.is_empty() { None } else { Some(faults.join("; ")) };
        let alarm = (a1, a2);
        let mut actions = Vec::new();
        if a1 & (1 << 30) != 0 {
            if !self.arming.estop_latched() {
                self.arming.estop();
                actions.push(GateAction::Estop);
            }
        } else if (a1 != 0 || a2 != 0) && (alarm != self.last_alarm || self.arming.state() != ArmState::Disarmed || self.fifo_running) {
            let why = self.machine_fault.clone().unwrap_or_else(|| "alarm".into());
            self.arming.disarm(&format!("alarm: {why}"));
            actions.push(GateAction::Stop(why));
        } else if status.restart_required() && (self.fifo_running || !self.leases.is_empty()) {
            // A card that wants a restart is not one to keep streaming to, nor one to keep a
            // held jog running on: stop like an alarm. The jog's lease ends here, so a key that
            // is still down does not keep it alive.
            let why = self.machine_fault.clone().unwrap_or_else(|| "card restart required".into());
            self.arming.disarm(&format!("restart required: {why}"));
            self.leases.clear();
            actions.push(GateAction::Stop(why));
        }
        self.last_alarm = alarm;
        if !actions.is_empty() {
            self.note_stop();
        }
        actions
    }

    fn comm_ok(&self, now_ms: u64) -> bool {
        self.last_status_ms.is_some_and(|t| now_ms.saturating_sub(t) <= self.config.comm_loss_timeout_ms)
    }

    /// Decide whether `w` may be sent now.
    pub fn check_write(&mut self, w: &SafeWrite, now_ms: u64) -> Result<(), GateRefusal> {
        // A write is classified against the default limits when it is built. This gate may be
        // set up for a smaller bed, lower ceilings or other ports, so it classifies the write
        // again with its own limits and the output word it last read, and the stricter answer
        // counts. Stopping is the exception: no limit of this gate holds a stop back.
        let (mut policy, mut needs_homed) = (w.policy(), w.needs_homed());
        match classify_write(w.register(), w.words(), &self.config.safety, self.last_do_word) {
            Ok(d) => {
                policy = policy.max(d.policy);
                needs_homed |= d.needs_homed;
            }
            Err(e) => {
                let stops = (w.register() == REG_COMMAND && w.words().first() == Some(&CMD_STOP))
                    || (w.register() == REG_FIFO_CONTROL && w.words() == [FIFO_STOP]);
                if !stops {
                    return Err(GateRefusal::Limit(e.to_string()));
                }
            }
        }
        if !self.arming.allows(policy) {
            return Err(GateRefusal::NeedsArming { policy, state: self.arming.state(), estop_latched: self.arming.estop_latched() });
        }
        if needs_homed && !self.homed {
            return Err(GateRefusal::NotHomed);
        }
        if !matches!(policy, Policy::Always | Policy::Connect)
            && let Some(f) = &self.machine_fault
        {
            return Err(GateRefusal::MachineFault(f.clone()));
        }
        let words = w.words();
        // While the card runs the FIFO, the head belongs to the stream: no jog, home or go-to,
        // and the queue is not thrown away under it. Stops stay `Always` and pass.
        let moves_the_head = w.register() == REG_COMMAND
            && matches!(words.first(), Some(&(crate::command::CMD_HOME | crate::command::CMD_JOG | crate::command::CMD_GOTO)));
        let clears_the_queue = w.register() == REG_FIFO_CONTROL && words == [crate::command::FIFO_CLEAR];
        if self.fifo_running && (moves_the_head || clears_the_queue) {
            return Err(GateRefusal::StreamRunning);
        }
        if w.register() == REG_COMMAND && words.len() == 6 && words[0] == 3 && words[1] & 0x8000_0000 == 0 {
            let idx = words[1];
            if self.homed {
                // Homed means the soft limits apply. A relative jog from a place the gate does
                // not know cannot be checked against them, so it does not go out.
                let Some(&pos) = self.positions_word.get(&idx) else {
                    return Err(GateRefusal::SoftLimit(format!("position of axis {idx} unknown: relative jog cannot be checked")));
                };
                let target = pos + words[5] as i32 as i64;
                if let Some(&(lo, hi)) = self.config.safety.soft_limits_word.get(idx as usize)
                    && (target < lo || target > hi)
                {
                    return Err(GateRefusal::SoftLimit(format!("jog target {target} on axis {idx} outside [{lo}, {hi}]")));
                }
            }
        }
        let streaming = w.register() == REG_FIFO_DATA || (w.register() == REG_FIFO_CONTROL && words == [2]);
        if streaming && !self.comm_ok(now_ms) {
            if self.fifo_running || self.arming.job_token().is_some() {
                self.give_up_job();
            }
            return Err(GateRefusal::CommLoss);
        }
        // Everything that moves, fires or switches an output on needs a status the gate has
        // actually seen, and seen recently: an alarm it never read is an alarm it cannot honour.
        if !matches!(policy, Policy::Always | Policy::Connect) && !self.comm_ok(now_ms) {
            return Err(GateRefusal::CommLoss);
        }
        if w.register() == REG_FIFO_CONTROL && words == [2] && self.card_fifo_has_laser && self.arming.state() != ArmState::LaserArmed {
            return Err(GateRefusal::LaserFramesQueued);
        }
        if w.register() == REG_FIFO_CONTROL && words == [2] && self.queue_held {
            return Err(GateRefusal::QueueHeld);
        }
        Ok(())
    }

    /// Update tracking after the card acknowledged `w`.
    pub fn after_send(&mut self, w: &SafeWrite, now_ms: u64) {
        let words = w.words();
        // A position read before a move says nothing about where the head is after it.
        match (w.register(), words.first()) {
            (REG_COMMAND, Some(&crate::command::CMD_JOG)) => {
                self.positions_word.remove(&(words.get(1).copied().unwrap_or(0) & 0x7FFF_FFFF));
            }
            (REG_COMMAND, Some(&(crate::command::CMD_HOME | crate::command::CMD_GOTO))) | (REG_FIFO_CONTROL, Some(&crate::command::FIFO_START)) => {
                self.positions_word.clear();
            }
            _ => {}
        }
        match w.register() {
            REG_FIFO_DATA => {
                self.frames_since_clear = true;
                if crate::fifo::parse_fifo_words(words).is_ok_and(|f| f.items.iter().any(|i| i.is_laser_for(&self.config.safety))) {
                    self.card_fifo_has_laser = true;
                }
            }
            REG_FIFO_CONTROL => match words.first() {
                Some(2) => self.fifo_running = true,
                Some(3) => {
                    self.fifo_running = false;
                    self.note_stop();
                }
                Some(1) => {
                    self.fifo_running = false;
                    self.card_fifo_has_laser = false;
                    self.frames_since_clear = false;
                    self.queue_held = false;
                }
                _ => {}
            },
            REG_COMMAND => {
                if words.len() == 6 && words[0] == 3 && words[1] & 0x8000_0000 == 0 {
                    // relative jog: needs a keep-alive if it outlasts the deadman timeout
                    let (v, dist) = (words[2] as u64, (words[5] as i32 as i64).unsigned_abs());
                    if dist * 1000 > v * self.config.deadman_timeout_ms {
                        self.leases.insert(words[1], now_ms + self.config.deadman_timeout_ms);
                    } else {
                        self.leases.remove(&words[1]);
                    }
                } else if words.len() >= 2 && words[0] == 1 {
                    self.leases.retain(|idx, _| words[1] & (1 << idx) == 0);
                }
            }
            _ => {}
        }
    }

    /// Track a write that went out but was not acknowledged (no answer, or one that could not be
    /// read). The card may have acted on it, so the gate assumes the dangerous half: a held jog
    /// gets its deadman lease, a start counts as running, a frame counts as queued. Nothing is
    /// assumed done that would relax the gate: a stop or a clear that was not acknowledged leaves
    /// the tracking as it was.
    pub fn after_unconfirmed(&mut self, w: &SafeWrite, now_ms: u64) {
        let words = w.words();
        match (w.register(), words.first()) {
            (REG_COMMAND, Some(&crate::command::CMD_JOG)) => {
                self.positions_word.remove(&(words.get(1).copied().unwrap_or(0) & 0x7FFF_FFFF));
                if words.len() == 6 && words[1] & 0x8000_0000 == 0 {
                    let (v, dist) = (words[2] as u64, (words[5] as i32 as i64).unsigned_abs());
                    if dist * 1000 > v * self.config.deadman_timeout_ms {
                        self.leases.insert(words[1], now_ms + self.config.deadman_timeout_ms);
                    }
                }
            }
            (REG_COMMAND, Some(&(crate::command::CMD_HOME | crate::command::CMD_GOTO))) => self.positions_word.clear(),
            (REG_FIFO_CONTROL, Some(&crate::command::FIFO_START)) => {
                self.positions_word.clear();
                self.fifo_running = true;
            }
            (REG_FIFO_CONTROL, Some(&FIFO_STOP)) => self.note_stop(),
            (REG_FIFO_DATA, _) => {
                self.frames_since_clear = true;
                if crate::fifo::parse_fifo_words(words).is_ok_and(|f| f.items.iter().any(|i| i.is_laser_for(&self.config.safety))) {
                    self.card_fifo_has_laser = true;
                }
            }
            _ => {}
        }
    }

    /// Forget what was read from the card (status age, output word, positions, homed). After a
    /// disconnect or a card restart nothing that needs a fresh status passes until the next poll.
    pub fn forget_card_state(&mut self) {
        self.last_status_ms = None;
        self.last_do_word = None;
        self.positions_word.clear();
        self.homed = false;
        self.leases.clear();
    }

    /// Key still held: extend the lease of `axis_index`. False if there is no live lease.
    pub fn jog_keepalive(&mut self, axis_index: u32, now_ms: u64) -> bool {
        match self.leases.get_mut(&axis_index) {
            Some(deadline) => {
                *deadline = now_ms + self.config.deadman_timeout_ms;
                true
            }
            None => false,
        }
    }

    /// The release stop for `axis_index` that [`service`](Self::service) asked for did not reach the
    /// card (no answer, or one that could not be read). The lease comes back, already due, so the
    /// next call of [`service`](Self::service) asks for the stop again. A card that answered, even
    /// with an exception, has the stop and needs no second one.
    pub fn release_stop_not_confirmed(&mut self, axis_index: u32, now_ms: u64) {
        self.leases.entry(axis_index).or_insert(now_ms);
    }

    /// Axis indices whose deadman lease ran out; each needs a release stop. The leases are removed.
    pub fn service(&mut self, now_ms: u64) -> Vec<u32> {
        let expired: Vec<u32> = self.leases.iter().filter(|&(_, &d)| now_ms >= d).map(|(&i, _)| i).collect();
        for i in &expired {
            self.leases.remove(i);
        }
        expired
    }

    /// Comm-loss watchdog: true (and the gate disarms) if a job or the FIFO is active and the
    /// status poll is stale.
    pub fn watchdog(&mut self, now_ms: u64) -> bool {
        if (self.fifo_running || self.arming.job_token().is_some()) && !self.comm_ok(now_ms) {
            self.give_up_job();
            return true;
        }
        false
    }

    /// Contact was lost during a job: disarm, and treat the job as stopped. Whatever the card
    /// still holds of it is not run on by a bare start ([`GateRefusal::QueueHeld`]).
    fn give_up_job(&mut self) {
        self.arming.disarm("comm loss");
        self.note_stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{self, MachineParams};
    use crate::fifo::FifoItem;

    fn status(a1: u32, a2: u32, proc: u32, param: u32) -> StatusBlock {
        let mut w = vec![0u32; 36];
        w[Status::Alarm1 as usize] = a1;
        w[Status::Alarm2 as usize] = a2;
        w[Status::ProcessingStatus as usize] = proc;
        w[Status::ParamStatus as usize] = param;
        StatusBlock::from_words(&w).unwrap()
    }

    fn jog() -> SafeWrite {
        command::jog_step(0, 5.0, &MachineParams::default()).unwrap()
    }

    #[test]
    fn arming_rules() {
        let mut a = ArmingStateMachine::new();
        assert!(a.allows(Policy::Always) && a.allows(Policy::Connect));
        assert!(!a.allows(Policy::Motion));
        assert!(a.begin_job().is_err());
        a.arm_motion().unwrap();
        assert!(a.allows(Policy::Motion) && a.allows(Policy::MotionHomed) && !a.allows(Policy::Laser));
        let tok = a.begin_job().unwrap();
        assert!(a.begin_job().is_err());
        assert!(a.arm_laser(false, Some(&tok)).is_err());
        assert!(a.arm_laser(true, None).is_err());
        assert!(a.arm_laser(true, Some("job-9")).is_err());
        a.arm_laser(true, Some(&tok)).unwrap();
        assert!(a.allows(Policy::Laser));
        a.on_stop();
        assert_eq!(a.state(), ArmState::MotionArmed);
        assert!(a.job_token().is_none());
        // laser needs a fresh job
        assert!(a.arm_laser(true, Some(&tok)).is_err());
        let tok = a.begin_job().unwrap();
        a.arm_laser(true, Some(&tok)).unwrap();
        a.end_job(Some(&tok)).unwrap();
        assert_eq!(a.state(), ArmState::MotionArmed);
        assert!(a.end_job(Some("nope")).is_err());
    }

    #[test]
    fn estop_latches_until_acknowledged() {
        let mut a = ArmingStateMachine::new();
        a.arm_motion().unwrap();
        a.estop();
        assert_eq!(a.state(), ArmState::Disarmed);
        assert!(a.estop_latched());
        assert!(a.arm_motion().is_err());
        assert!(!a.allows(Policy::Motion));
        assert!(a.allows(Policy::Always), "stop primitives stay available");
        a.acknowledge_estop();
        assert_eq!(a.state(), ArmState::Disarmed, "stays disarmed after the acknowledge");
        a.arm_motion().unwrap();
        assert!(a.history.iter().any(|t| t.event == "estop"));
    }

    #[test]
    fn writes_are_gated_by_policy() {
        let mut g = SafetyGate::default();
        assert!(matches!(g.check_write(&jog(), 0), Err(GateRefusal::NeedsArming { .. })));
        assert!(g.check_write(&command::stop_all(0, &MachineParams::default()).unwrap(), 0).is_ok());
        assert!(g.check_write(&command::connect_prologue().unwrap(), 0).is_ok());
        g.arming.arm_motion().unwrap();
        assert_eq!(g.check_write(&jog(), 0), Err(GateRefusal::CommLoss), "no status seen yet");
        g.observe_status(0, &status(0, 0, 0, 0));
        assert!(g.check_write(&jog(), 0).is_ok());
        let goto = command::go_to(10.0, 10.0, &MachineParams::default()).unwrap();
        assert_eq!(g.check_write(&goto, 0), Err(GateRefusal::NotHomed));
        g.homed = true;
        assert!(g.check_write(&goto, 0).is_ok());
    }

    #[test]
    fn laser_class_writes_need_the_laser_level() {
        let laser = SafeWrite::from_raw_for_simulator(0x65, &[9999, 2, 0x100, 0x100], &SafetyConfig::default(), None).unwrap();
        let mut g = SafetyGate::default();
        g.arming.arm_motion().unwrap();
        g.observe_status(0, &status(0, 0, 0, 0));
        assert!(matches!(g.check_write(&laser, 0), Err(GateRefusal::NeedsArming { policy: Policy::Laser, .. })));
        let t = g.arming.begin_job().unwrap();
        g.arming.arm_laser(true, Some(&t)).unwrap();
        assert!(g.check_write(&laser, 0).is_ok());
        g.arming.estop();
        assert!(g.check_write(&laser, 0).is_err());
    }

    #[test]
    fn alarms_stop_and_disarm() {
        let mut g = SafetyGate::default();
        g.arming.arm_motion().unwrap();
        let acts = g.observe_status(0, &status(0, 1 << 5, 0, 0));
        assert!(matches!(acts.as_slice(), [GateAction::Stop(_)]));
        assert_eq!(g.arming.state(), ArmState::Disarmed);
        assert!(g.machine_fault.is_some());
        // non-safety writes are refused while the fault stays
        g.arming.arm_motion().unwrap();
        assert!(matches!(g.check_write(&jog(), 0), Err(GateRefusal::MachineFault(_))));
        assert!(g.check_write(&command::stop_all_no_decel().unwrap(), 0).is_ok());
        // cleared
        assert!(g.observe_status(10, &status(0, 0, 0, 0)).is_empty());
        assert!(g.machine_fault.is_none());
        assert!(g.check_write(&jog(), 10).is_ok());
    }

    #[test]
    fn card_estop_latches() {
        let mut g = SafetyGate::default();
        g.arming.arm_motion().unwrap();
        let acts = g.observe_status(0, &status(1 << 30, 0, 0, 0));
        assert_eq!(acts, vec![GateAction::Estop]);
        assert!(g.arming.estop_latched());
        // the second poll does not repeat the action
        assert!(g.observe_status(1, &status(1 << 30, 0, 0, 0)).is_empty());
    }

    #[test]
    fn restart_required_blocks_motion() {
        let mut g = SafetyGate::default();
        g.arming.arm_motion().unwrap();
        g.observe_status(0, &status(0, 0, 0, 1));
        assert!(matches!(g.check_write(&jog(), 0), Err(GateRefusal::MachineFault(m)) if m.contains("restart")));
    }

    #[test]
    fn bit24_alone_counts_only_while_cutting() {
        let mut g = SafetyGate::default();
        g.arming.arm_motion().unwrap();
        assert!(g.observe_status(0, &status(0x0100_0000, 0, 0, 0)).is_empty());
        assert!(g.machine_fault.is_none() || g.machine_fault.as_deref() == Some(""));
        let acts = g.observe_status(1, &status(0x0100_0000, 0, 1, 0));
        assert!(matches!(acts.as_slice(), [GateAction::Stop(_)]));
    }

    #[test]
    fn streaming_needs_a_fresh_status_poll() {
        let mut g = SafetyGate::default();
        g.arming.arm_motion().unwrap();
        let frame = command::fifo_frame(1, &[FifoItem::tick(1, 1)]).unwrap();
        assert_eq!(g.check_write(&frame, 0), Err(GateRefusal::CommLoss));
        g.observe_status(100, &status(0, 0, 0, 0));
        assert!(g.check_write(&frame, 600).is_ok());
        assert_eq!(g.check_write(&frame, 1200), Err(GateRefusal::CommLoss));
        // start is a streaming write too
        assert_eq!(g.check_write(&command::fifo_start().unwrap(), 1200), Err(GateRefusal::CommLoss));
        // stop never needs a poll
        assert!(g.check_write(&command::fifo_stop().unwrap(), 1200).is_ok());
    }

    #[test]
    fn watchdog_disarms_when_a_job_runs_blind() {
        let mut g = SafetyGate::default();
        g.arming.arm_motion().unwrap();
        g.observe_status(0, &status(0, 0, 0, 0));
        g.after_send(&command::fifo_start().unwrap(), 0);
        assert!(g.fifo_running);
        assert!(!g.watchdog(500));
        assert!(g.watchdog(1500));
        assert_eq!(g.arming.state(), ArmState::Disarmed);
        g.after_send(&command::fifo_stop().unwrap(), 1500);
        assert!(!g.fifo_running);
    }

    #[test]
    fn relative_jogs_are_checked_against_the_soft_limits() {
        let mut g = SafetyGate::default();
        g.arming.arm_motion().unwrap();
        g.observe_status(0, &status(0, 0, 0, 0));
        g.homed = true;
        g.positions_word.insert(0, 1_370_000);
        let far = command::jog_step(0, 5.0, &MachineParams::default()).unwrap();
        assert!(matches!(g.check_write(&far, 0), Err(GateRefusal::SoftLimit(_))));
        let back = command::jog_step(0, -5.0, &MachineParams::default()).unwrap();
        assert!(g.check_write(&back, 0).is_ok());
        g.positions_word.insert(0, 2_000);
        assert!(matches!(g.check_write(&back, 0), Err(GateRefusal::SoftLimit(_))));
    }

    #[test]
    fn deadman_lease_expires_and_keepalive_extends() {
        let mut g = SafetyGate::default();
        g.arming.arm_motion().unwrap();
        let held = command::jog_continuous(0, true, &MachineParams::default(), None).unwrap();
        g.after_send(&held, 0);
        assert!(g.service(100).is_empty());
        assert!(g.jog_keepalive(0, 150));
        assert!(g.service(300).is_empty());
        assert_eq!(g.service(400), vec![0]);
        assert!(!g.jog_keepalive(0, 400), "lease is gone");
        // a short step never needs a lease
        g.after_send(&jog(), 1000);
        assert!(g.service(5000).is_empty());
        // an explicit stop clears the lease
        g.after_send(&held, 2000);
        g.after_send(&command::jog_release_stop(&[0], 200_000, &MachineParams::default()).unwrap(), 2010);
        assert!(g.service(9000).is_empty());
    }

    #[test]
    fn laser_frames_queued_block_start() {
        let mut g = SafetyGate::default();
        g.arming.arm_motion().unwrap();
        g.observe_status(0, &status(0, 0, 0, 0));
        let laser_frame = SafeWrite::from_raw_for_simulator(0x66, &[1, 0x0008_0BB8, 0, (5000 << 16) | 4], &SafetyConfig::default(), None).unwrap();
        g.after_send(&laser_frame, 0);
        assert!(g.card_fifo_has_laser);
        assert_eq!(g.check_write(&command::fifo_start().unwrap(), 0), Err(GateRefusal::LaserFramesQueued));
        g.after_send(&command::fifo_clear().unwrap(), 0);
        assert!(g.check_write(&command::fifo_start().unwrap(), 0).is_ok());
    }

    #[test]
    fn reads_use_the_allow_list() {
        let g = SafetyGate::default();
        assert!(g.check_read(1000, 36).is_ok());
        assert!(g.check_read(59500, 1).is_err());
    }

    #[test]
    fn the_gate_applies_its_own_limits_not_only_the_defaults() {
        let mut cfg = GateConfig::default();
        cfg.safety.soft_limits_word = [(0, 500_000), (0, 300_000)];
        cfg.safety.max_speed_word = 100_000;
        cfg.safety.laser_do_ports = vec![2, 5, 6, 9];
        cfg.safety.motion_do_ports = vec![1, 3, 7];
        cfg.safety.home_slots = vec![0];
        let mut g = SafetyGate::new(cfg);
        g.arming.arm_motion().unwrap();
        g.observe_status(0, &status(0, 0, 0, 0));
        g.homed = true;
        let p = MachineParams::default();
        // a go-to and an absolute move past this gate's bed (inside the default one)
        assert!(g.check_write(&command::go_to(1000.0, 900.0, &p).unwrap(), 0).is_err());
        assert!(g.check_write(&command::move_axis_absolute(0, 1000.0, &p).unwrap(), 0).is_err());
        // a jog faster than this gate's ceiling
        assert!(g.check_write(&command::jog_step(0, -1.0, &p).unwrap(), 0).is_err());
        // a slot this gate does not home
        assert!(g.check_write(&command::home_axis(1).unwrap(), 0).is_err());
        assert!(g.check_write(&command::home_axis(0).unwrap(), 0).is_ok());
        // port 2 is a laser port here: switching it on needs the laser level
        let on = SafeWrite::try_from_raw(0x65, &[9999, 2, 0b10, 0b10]).unwrap();
        assert!(matches!(g.check_write(&on, 0), Err(GateRefusal::NeedsArming { policy: Policy::Laser, .. })));
        // stops and switch-offs pass whatever the limits are
        g.config.safety.stop_vd_range = (50_000, 60_000);
        assert!(g.check_write(&command::stop_all(0, &p).unwrap(), 0).is_ok());
        assert!(g.check_write(&command::do_off(2).unwrap(), 0).is_ok());
        assert!(g.check_write(&command::fifo_stop().unwrap(), 0).is_ok());
    }

    #[test]
    fn an_output_record_in_the_stream_is_judged_by_this_gates_ports() {
        // Port 2 is a laser port on this machine. A stream record that switches it on is laser
        // data here, although the default port list calls it gas.
        let mut cfg = GateConfig::default();
        cfg.safety.laser_do_ports = vec![2, 5, 6, 9];
        cfg.safety.motion_do_ports = vec![1, 3, 7];
        let mut g = SafetyGate::new(cfg);
        g.arming.arm_motion().unwrap();
        g.observe_status(0, &status(0, 0, 0, 0));
        let mut words = vec![1u32];
        words.extend(FifoItem::tick(1, 0).words());
        words.extend([0x000C_270F, 2, 0b10, 0b10]);
        let frame = SafeWrite::try_from_raw(0x66, &words).unwrap();
        assert!(matches!(g.check_write(&frame, 0), Err(GateRefusal::NeedsArming { policy: Policy::Laser, .. })));
        // and once such a frame is on the card, a start needs the laser level too
        g.after_send(&frame, 0);
        assert!(g.card_fifo_has_laser);
        // port 3 stays a gas port: fine with motion armed
        let mut words = vec![2u32];
        words.extend([0x000C_270F, 2, 0b100, 0b100]);
        let frame = SafeWrite::try_from_raw(0x66, &words).unwrap();
        assert!(g.check_write(&frame, 0).is_ok());
    }

    #[test]
    fn every_limit_of_the_configuration_is_honoured_by_the_gate() {
        // One field tightened at a time; each write passes a gate with the default limits.
        let p = MachineParams::default();
        let ready = |change: &dyn Fn(&mut SafetyConfig)| {
            let mut cfg = GateConfig::default();
            change(&mut cfg.safety);
            let mut g = SafetyGate::new(cfg);
            g.arming.arm_motion().unwrap();
            g.observe_status(0, &status(0, 0, 0, 0));
            g.homed = true;
            g.positions_word.insert(0, 100_000);
            g.positions_word.insert(1, 100_000);
            g
        };
        let gas_on = SafeWrite::try_from_raw(0x65, &[9999, 2, 0b100, 0b100]).unwrap();
        let cases: Vec<(&str, Box<dyn Fn(&mut SafetyConfig)>, SafeWrite)> = vec![
            ("laser_do_ports", Box::new(|c| c.laser_do_ports.push(3)), gas_on.clone()),
            ("motion_do_ports", Box::new(|c| c.motion_do_ports.retain(|&p| p != 3)), gas_on),
            ("jog_axis_indices", Box::new(|c| c.jog_axis_indices = vec![0]), command::jog_step(1, 1.0, &p).unwrap()),
            ("home_slots", Box::new(|c| c.home_slots = vec![1]), command::home_axis(0).unwrap()),
            ("goto_axis_mask", Box::new(|c| c.goto_axis_mask = 0x1), command::go_to(10.0, 10.0, &p).unwrap()),
            ("max_speed_word", Box::new(|c| c.max_speed_word = 150_000), command::jog_step(0, 1.0, &p).unwrap()),
            ("max_acc", Box::new(|c| c.max_acc = 1000), command::jog_step(0, 1.0, &p).unwrap()),
            ("max_jog_distance_word", Box::new(|c| c.max_jog_distance_word = 1000), command::jog_step(0, 5.0, &p).unwrap()),
            ("soft_limits_word", Box::new(|c| c.soft_limits_word = [(0, 50_000), (0, 50_000)]), command::go_to(60.0, 10.0, &p).unwrap()),
        ];
        for (field, change, w) in &cases {
            assert!(ready(&|_| {}).check_write(w, 0).is_ok(), "{field}: passes the default gate");
            assert!(ready(change.as_ref()).check_write(w, 0).is_err(), "{field} is not honoured");
        }
        // The tenth field bounds the stop deceleration. A stop is the one write no limit holds back.
        let mut g = ready(&|c| c.stop_vd_range = (90_000, 95_000));
        assert!(g.check_write(&command::stop_all(0, &p).unwrap(), 0).is_ok());
        assert!(g.check_write(&command::jog_release_stop(&[0], 0, &p).unwrap(), 0).is_ok());
    }
}
